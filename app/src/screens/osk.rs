//! Виртуальная клавиатура Steam Deck: связка модели (`osk_model`) с UI и полями ввода.
//! Разбор оригинала: docs/keyboard-analysis-2026-11-18.md. Управление: сенсор (набор на отпускание), A, D-pad/стики,
//! тачпады (позиционный ввод по половинам), X Backspace (удержание — повтор 450/200 мс), Y пробел, R2 «Поиск»/Enter,
//! L2 Shift (удержание), L3 Caps, L1/R1 вкладки эмодзи, B закрыть. Отклик: звук Typing + хаптика Deck (см. `haptic`).
use steam_assets::i18n;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::osk_emoji_img;
use super::osk_model::{pad_point, Act, Kb, Kind, Mode, Out, PKey, CAT_STEAM, EMOJI_COL_W, EXT_W, SCALE};
use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::{AppWindow, OskExt, OskKey, OskState, SearchState};

const LONG_PRESS: Duration = Duration::from_millis(450);
const REPEAT: Duration = Duration::from_millis(200);
const NAME_SHOWN: Duration = Duration::from_secs(2);

/// Хаптика клавиатуры (sp.js:419009 `k_Presets`): пресет 2 по умолчанию — Click, интенсивность 4, −5 dB.
pub const HAPTIC_TOUCH: (u8, u8, i8) = (2, 4, -5);

/// Настройки клавиатуры Steam (sp.js:419000, `SteamClient.Storage`, только чтение): хаптика нажатия
/// (None — пресет 0 «выкл.»), масштаб позиционного ввода тачпадом, раскладки.
#[derive(Clone, Debug, PartialEq)]
pub struct KbSettings {
    pub haptic: Option<(u8, u8, i8)>,
    pub pad_scale: f32,
    pub layouts: Vec<usize>,
    pub current: usize,
}

impl Default for KbSettings {
    fn default() -> Self {
        KbSettings { haptic: Some(HAPTIC_TOUCH), pad_scale: 1.0, layouts: vec![0, 1], current: 0 }
    }
}

/// Пресеты `k_Presets` и свой (`10` + `keyboard_haptic_custom`).
fn haptic_preset(preset: i64, custom: &serde_json::Value) -> Option<(u8, u8, i8)> {
    match preset {
        0 => None,
        1 => Some((1, 2, -2)),
        3 => Some((2, 3, -3)),
        10 => {
            let t = custom.get("eHapticType")?.as_u64()? as u8;
            let i = custom.get("unIntensity")?.as_u64()? as u8;
            let g = custom.get("ndBGain")?.as_i64()? as i8;
            (t != 0 && i != 0).then_some((t, i, g))
        }
        _ => Some(HAPTIC_TOUCH),
    }
}

/// Разбор ответа чтения хранилища Steam: JSON-строки ключей `keyboard_*` (null — значение по умолчанию).
pub fn parse_settings(v: &serde_json::Value) -> KbSettings {
    let mut s = KbSettings::default();
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok());
    if let Some(p) = get("keyboard_haptic_preset").and_then(|p| p.as_i64()) {
        s.haptic = haptic_preset(p, &get("keyboard_haptic_custom").unwrap_or_default());
    }
    if let Some(sc) = get("keyboard_trackpad_typing_scale").and_then(|x| x.as_f64()) {
        if sc > 0.1 && sc < 10.0 {
            s.pad_scale = sc as f32;
        }
    }
    if let Some(l) = get("keyboard_layout") {
        let sel: Vec<usize> = l.get("selectedLayouts").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_i64()).filter_map(super::osk_model::layout_from_steam).collect()).unwrap_or_default();
        let mut sel2: Vec<usize> = vec![];
        for x in sel {
            if !sel2.contains(&x) {
                sel2.push(x);
            }
        }
        if !sel2.is_empty() {
            s.layouts = sel2;
        }
        let cur = l.get("currentLayout").and_then(|x| x.as_i64()).and_then(super::osk_model::layout_from_steam);
        s.current = cur.filter(|c| s.layouts.contains(c)).unwrap_or(s.layouts[0]);
    }
    if let Some(t) = get("keyboard_trackpding_typing_trigger_as_click").and_then(|x| x.as_bool()) {
        crate::input::set_osk_trigger_click(t);
    }
    s
}

const SETTINGS_JS: &str = r#"(async()=>{const r={};for(const k of ["keyboard_haptic_preset","keyboard_haptic_custom","keyboard_trackpad_typing_scale","keyboard_trackpding_typing_trigger_as_click","keyboard_layout"]){try{r[k]=await SteamClient.Storage.GetString(k)}catch(e){r[k]=null}}return r})()"#;

thread_local! { static SETTINGS: RefCell<KbSettings> = RefCell::new(KbSettings::default()); }

fn settings() -> KbSettings {
    SETTINGS.with(|s| s.borrow().clone())
}
/// Наведение тачпадом на другую клавишу: Tick, интенсивность 1, 0 dB.
pub const HAPTIC_HOVER: (u8, u8, i8) = (1, 1, 0);

#[derive(Default, Clone, Copy)]
struct Pad {
    active: bool,
    x: f32,
    y: f32,
    hover: Option<usize>,
    down: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InputSource {
    #[default]
    FocusNav,
    PadCursor,
}

impl InputSource {
    fn update(&mut self, action: Action, is_repeat: bool) {
        match action {
            Action::KbPad { on: true, .. } | Action::KbClick { down: true, .. } => {
                *self = Self::PadCursor;
            }
            // Releases and repeats from a direction already held when the pad
            // took over do not reclaim focus.
            Action::Up | Action::Down | Action::Left | Action::Right
            | Action::A | Action::KbA(true) | Action::X | Action::KbBack(true)
            | Action::Y | Action::L2 | Action::R2 | Action::KbShift(true)
            | Action::KbCaps | Action::LB | Action::RB if !is_repeat => {
                *self = Self::FocusNav;
            }
            _ => {}
        }
    }

    fn show_focus(self, pad_mode: bool) -> bool {
        pad_mode && self == Self::FocusNav
    }
}

/// Источник долгого нажатия (sp.js `m_rgLongPress[3]`: касание, левый и правый тачпады; геймпад — кнопка A).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Src {
    Touch,
    Pad(usize),
    A,
}

#[derive(Clone, Copy, Debug)]
struct Hold {
    src: Src,
    key: usize,
    since: Instant,
}

/// Палец на эмодзи-сетке: сдвиг больше `DRAG_START` по горизонтали — прокрутка вместо нажатия.
#[derive(Clone, Copy, Debug)]
struct Drag {
    x0: f32,
    scroll0: usize,
    active: bool,
}

const DRAG_START: f32 = 20.0;

struct Osk {
    open: bool,
    drag: Option<Drag>,
    /// Удерживаемая клавиша с вариантами: через 450 мс открывается ряд расширенных символов (`kb.ext`).
    hold: Option<Hold>,
    kb: Kb,
    /// Клавиша под пальцем (сенсор) — набирается на отпускание.
    touched: Option<usize>,
    a_pressed: Option<usize>,
    input_source: InputSource,
    pads: [Pad; 2],
    /// Удержание Backspace: когда нажат и когда последний раз отправлено.
    back: Option<(Instant, Instant, bool)>,
    name_until: Option<Instant>,
    imgs: HashMap<String, Image>,
}

thread_local! {
    static OSK: RefCell<Option<Osk>> = const { RefCell::new(None) };
    static TICK: Timer = Timer::default();
    static NAME_TIMER: Timer = Timer::default();
}

fn with<R>(f: impl FnOnce(&mut Osk) -> R) -> R {
    OSK.with(|o| {
        let mut o = o.borrow_mut();
        f(o.get_or_insert_with(|| Osk { open: false, drag: None, hold: None, kb: Kb::new(1280.0, 800.0), touched: None, a_pressed: None, input_source: InputSource::default(), pads: [Pad::default(); 2], back: None, name_until: None, imgs: HashMap::new() }))
    })
}

/// Хаптика Deck через Steam: `SteamClient.Input.TriggerSimpleHapticEvent(controller, pad, type, intensity, gain)`.
/// pad: 0 левый тачпад, 1 правый, 2 оба (`N.ek/SN/t3`). Живая проверка на Деке не выполнялась; в DRYRUN только печать.
pub fn haptic(pad: u8, h: (u8, u8, i8)) {
    let js = format!("SteamClient.Input.TriggerSimpleHapticEvent(window.SteamUIStore?.MostRecentlyActiveControllerIndex ?? 0, {pad}, {}, {}, {})", h.0, h.1, h.2);
    crate::setters::write("osk-haptic", js);
}

/// Режим поля ввода: влияет на фильтр символов и вид вывода (пароль — скрытый, числа — только цифры).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InputMode {
    #[default]
    Normal,
    /// R2 «Поиск» / Enter выполняет поиск.
    Search,
    /// Вывод скрыт (`mask`); в поле попадает настоящий текст.
    Password,
    /// Только цифры и `. , - +`.
    Numeric,
}

impl InputMode {
    /// Пропускает ли режим символ `c`.
    pub fn accepts(self, c: char) -> bool {
        !c.is_control() && (self != InputMode::Numeric || c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+'))
    }
}

/// Текст для показа в поле: пароль заменяется точками, остальное без изменений.
pub fn mask(mode: InputMode, text: &str) -> String {
    if mode == InputMode::Password { "•".repeat(text.chars().count()) } else { text.to_string() }
}

/// Единый приёмник текста «поле → клавиатура»: символы, каретка/вставка (необязательно), Поиск/Enter, закрытие.
/// Экран регистрирует его через `attach`; клавиатура сама закрывается и вызывает `cancel` при B / «Свернуть» / свайпе вверх.
#[derive(Clone, Copy)]
pub struct ExtSink {
    pub id: u8,
    pub mode: InputMode,
    pub typed: fn(&mut App, Option<char>),
    pub enter: fn(&mut App),
    pub cancel: fn(&mut App),
}

thread_local! { static SINK: std::cell::Cell<Option<ExtSink>> = const { std::cell::Cell::new(None) }; }

/// Открыть клавиатуру для приёмника (идемпотентно; смена приёмника переключает цель, клавиатура остаётся открытой).
pub fn attach(a: &mut App, s: ExtSink) {
    if SINK.with(|c| c.get().map(|x| x.id)) != Some(s.id) {
        SINK.with(|c| c.set(Some(s)));
    }
    if !a.osk_is_open() {
        a.osk_set_open(true);
    }
}

/// Закрыть клавиатуру, если она привязана к этому приёмнику (без вызова `cancel`).
pub fn detach(a: &mut App, s: ExtSink) {
    if SINK.with(|c| c.get().map(|x| x.id)) == Some(s.id) {
        SINK.with(|c| c.set(None));
        if a.osk_is_open() {
            a.osk_set_open(false);
        }
    }
}

fn sink() -> Option<ExtSink> {
    SINK.with(|c| c.get())
}

/// Кто сейчас владеет введённым текстом.
#[derive(Clone, Copy)]
enum Target {
    Sink(ExtSink),
    /// поиск «Анимаций» (экран другого исполнителя, на `ExtSink` не переведён)
    Anim,
    /// глобальный поиск (верхняя строка / экран поиска)
    Search,
}

fn target(a: &App) -> Option<Target> {
    if let Some(s) = sink() {
        Some(Target::Sink(s))
    } else if super::anim_page::keyboard_target() {
        Some(Target::Anim)
    } else if a.lb.search {
        Some(Target::Search)
    } else {
        None
    }
}

/// Режим текущей цели (для фильтра символов, подписи клавиши Enter и физической клавиатуры).
pub fn current_mode(a: &App) -> Option<InputMode> {
    target(a).map(|t| match t {
        Target::Sink(s) => s.mode,
        _ => InputMode::Search,
    })
}

/// Физическая клавиатура должна передавать символы (есть поле ввода, в которое они попадут).
pub fn wants_text(a: &App) -> bool {
    target(a).is_some() || super::chrome::hf() == 1
}

fn ext_type(a: &mut App, t: Target, c: Option<char>) {
    match t {
        Target::Sink(s) => (s.typed)(a, c),
        Target::Anim => super::anim_page::type_char(a, c),
        Target::Search => match c {
            Some(c) => a.search_char(c),
            None => a.search_backspace(),
        },
    }
}

/// Клавиатуру закрыли (B / «скрыть» / касание вне / свайп вверх): у приёмника это отмена, фокус возвращается полю.
fn ext_closed(a: &mut App, t: Target) {
    match t {
        Target::Sink(s) => {
            SINK.with(|c| c.set(None));
            (s.cancel)(a);
        }
        Target::Anim => super::anim_page::close_keyboard(a),
        Target::Search => a.search_focus_field(),
    }
}

fn decode_icon(name: &str) -> Image {
    match name {
        "@emoticons" => Image::load_from_svg_data(include_bytes!("../../ui/assets/osk/emoticons.svg")).unwrap_or_default(),
        "@stickers" => Image::load_from_svg_data(include_bytes!("../../ui/assets/osk/stickers.svg")).unwrap_or_default(),
        "@chatfx" => Image::load_from_svg_data(include_bytes!("../../ui/assets/osk/chatfx.svg")).unwrap_or_default(),
        n if n.starts_with("glyph:") => {
            let file = match &n[6..] {
                "x" => "shared_button_x",
                "y" => "shared_button_y",
                "l2" => "sd_l2",
                "r2" => "sd_r2",
                "l1" => "sd_l1",
                "r1" => "sd_r1",
                "l3" => "shared_l3",
                o => o,
            };
            steam_assets::glyph(file, "knockout").and_then(|p| Image::load_from_path(&p).ok()).unwrap_or_default()
        }
        // оригинальные SVG Steam из кэша иконок; ~13 маленьких файлов, один раз за сеанс (кэш `imgs`)
        n => steam_assets::icon(n)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| Image::load_from_svg_data(t.replace("currentColor", "#ffffff").as_bytes()).ok())
            .unwrap_or_default(),
    }
}

pub fn prewarm() {
    osk_emoji_img::prewarm();
}

type IconPixels = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
static ICONS: std::sync::OnceLock<HashMap<String, IconPixels>> = std::sync::OnceLock::new();

pub fn prepare_icons() {
    ICONS.get_or_init(|| {
        let mut names = std::collections::BTreeSet::from(["glyph:l1".to_owned(), "glyph:r1".to_owned()]);
        let mut kb = Kb::new(1280.0, 800.0);
        kb.layouts = vec![0, 1];
        for (layout, mode) in [(0, Mode::Standard), (1, Mode::Standard), (0, Mode::Emoji)] {
            kb.layout = layout;
            kb.mode = mode;
            for key in kb.keys().iter() {
                names.extend([key.def.icon, key.def.icon2].into_iter().flatten().map(str::to_owned));
                names.extend([key.def.glyph_l, key.def.glyph_r].into_iter().flatten().map(|g| format!("glyph:{g}")));
            }
        }
        names.into_iter().filter_map(|name| decode_icon(&name).to_rgba8().map(|pixels| (name, pixels))).collect()
    });
}

fn load_icon(name: &str) -> Image {
    if let Some(icons) = ICONS.get() {
        return icons.get(name).cloned().map(Image::from_rgba8).unwrap_or_default();
    }
    // Фикстуры без главного цикла не запускают фоновую загрузку ассетов.
    #[cfg(test)]
    return decode_icon(name);
    #[cfg(not(test))]
    Image::default()
}

fn emoji_text(kb: &Kb, keys: &[PKey]) -> Vec<String> {
    let mut text = Vec::new();
    if kb.mode == Mode::Emoji {
        text.extend(keys.iter().filter(|key| key.def.kind == Kind::Emoji).map(|key| key.def.base.clone()));
        if let Some(ext) = &kb.ext {
            text.extend(ext.items.iter().filter(|(_, tint)| tint.is_some()).map(|(text, _)| text.clone()));
        }
    }
    text.sort_unstable();
    text.dedup();
    text
}

fn update_model<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>, set: impl FnOnce(ModelRc<T>)) {
    if let Some(model) = current.as_any().downcast_ref::<VecModel<T>>() {
        if model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&row) {
                    model.set_row_data(i, row);
                }
            }
        } else {
            model.set_vec(rows);
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

fn okey(o: &mut Osk, i: usize, k: &PKey) -> OskKey {
    let kb = &o.kb;
    let d = &k.def;
    // пустую (ещё не готовую) иконку не запоминаем: набор растеризуется в фоне после старта
    let mut img = |name: &str| -> Image {
        if let Some(i) = o.imgs.get(name) {
            return i.clone();
        }
        let i = load_icon(name);
        if i.size().width > 0 {
            o.imgs.insert(name.to_string(), i.clone());
        }
        i
    };
    let hover = o.pads.iter().any(|p| p.active && p.hover == Some(i));
    let pressed = o.touched == Some(i) || o.a_pressed == Some(i)
        || o.pads.iter().any(|p| p.down && p.hover == Some(i));
    // у клавиши с открытым рядом вариантов фокус снят (sp.js `Ki = … && !_n`)
    let ext_parent = kb.ext.as_ref().map(|r| r.parent) == Some(i);
    let nav_focus = kb.focus == i && !ext_parent;
    let hover = hover && !ext_parent;
    let (mut on, mut oneshot) = (false, false);
    match d.act {
        Act::Shift => {
            on = kb.shift >= 2;
            oneshot = kb.shift == 1;
        }
        Act::Caps => on = kb.caps != 0,
        Act::AltGr => {
            on = kb.altgr >= 2;
            oneshot = kb.altgr == 1;
        }
        _ => {}
    }
    let hi = matches!(d.act, Act::Cat(c) if c == kb.cat) && kb.mode == Mode::Emoji;
    let tone = match d.kind {
        Kind::Tab | Kind::Caps | Kind::LShift | Kind::RShift | Kind::Enter | Kind::Backspace | Kind::Meta => 1,
        Kind::Spacer => 3,
        Kind::Emoji => 2,
        _ => 0,
    };
    let (mut main, mut small, mut mode, mut size) = (String::new(), String::new(), 0, 23.9f32);
    let mut eimg: Option<Image> = None;
    let up = kb.upper();
    match d.kind {
        Kind::Char | Kind::Half => {
            if kb.shift_on() && d.shift.is_some() {
                main = d.shift.clone().unwrap_or_default();
            } else {
                main = if up { d.base.to_uppercase() } else { d.base.clone() };
                if let Some(s) = &d.shift {
                    small = s.clone();
                    mode = 1;
                }
            }
        }
        Kind::Emoji => {
            eimg = osk_emoji_img::emoji_image(&d.base);
            main = if eimg.is_some() { String::new() } else { d.base.clone() };
            size = 20.0 * SCALE;
        }
        Kind::Tab | Kind::Caps | Kind::LShift => {
            main = d.tok.map(|(t, f)| t_or(t, f)).unwrap_or_default();
            mode = 2;
            size = 12.0 * SCALE;
        }
        Kind::RShift | Kind::Enter | Kind::Backspace => {
            main = d.tok.map(|(t, f)| if t.is_empty() { f.to_string() } else { t_or(t, f) }).unwrap_or_default();
            mode = 3;
            size = 12.0 * SCALE;
        }
        Kind::Space => {
            if o.name_until.is_some_and(|t| t > Instant::now()) {
                let (t, f) = kb.layout_name();
                main = t_or(t, f);
            }
        }
        Kind::Meta | Kind::Close => {
            size = 12.0 * SCALE;
            if !d.base.is_empty() {
                main = d.base.clone();
            } else if let Some((t, f)) = d.tok {
                main = t_or(t, f);
            }
        }
        _ => {}
    }
    let alt = if d.kind == Kind::Char || d.kind == Kind::Half { d.alt.clone().unwrap_or_default() } else { String::new() };
    // стрелки-пары: под Shift основной становится вторая
    let (icon_name, icon2_name) = match (d.icon, d.icon2) {
        (Some(a), Some(b)) => if kb.shift_on() { (Some(b), None) } else { (Some(a), Some(b)) },
        (a, _) => (a, None),
    };
    let ic = icon_name.map(&mut img);
    let ic2 = icon2_name.map(&mut img);
    let gl = d.glyph_l.map(|g| img(&format!("glyph:{g}")));
    let gr = d.glyph_r.map(|g| img(&format!("glyph:{g}")));
    OskKey {
        x: k.x, y: k.y, w: k.w, h: k.h, kx: k.kx, ky: k.ky, kw: k.kw, kh: k.kh,
        tone, nav_focus, hover, pressed, on, oneshot, hi,
        main: main.into(), main_size: size, bold: matches!(d.kind, Kind::Meta) && on, mode,
        small: small.into(), alt: alt.into(),
        has_icon: ic.is_some(), icon: ic.unwrap_or_default(),
        has_icon2: ic2.is_some(), icon2: ic2.unwrap_or_default(),
        has_gl: gl.is_some(), gl: gl.unwrap_or_default(),
        has_gr: gr.is_some(), gr: gr.unwrap_or_default(),
        emoji: d.kind == Kind::Emoji,
        has_eimg: eimg.is_some(), eimg: eimg.unwrap_or_default(),
    }
}

pub fn wire(ui: &AppWindow) {
    ui.global::<SearchState>().on_osk_dismiss(|| super::defer(|a| a.osk_dismiss()));
    let st = ui.global::<OskState>();
    st.on_dismiss(|| super::defer(|a| a.osk_dismiss()));
    st.on_touch_down(|x, y| super::defer(move |a| a.osk_touch_down(x, y)));
    st.on_touch_up(|_, _| super::defer(|a| a.osk_touch_up()));
    st.on_touch_cancel(|| super::defer(|a| a.osk_touch_cancel()));
    st.on_touch_move(|x, y| super::defer(move |a| a.osk_touch_move(x, y)));
    let l1 = load_icon("glyph:l1");
    let r1 = load_icon("glyph:r1");
    st.set_glyph_l1(l1);
    st.set_glyph_r1(r1);
}

/// Переносит состояние клавиатуры в `OskState` (общий путь для приложения и рендера-фикстур).
fn apply(ui: &AppWindow, pad_mode: bool) {
    let (keys, open, panel_y, emoji, message, pads, ext, show_focus, ext_nav) = with(|o| {
        let keys = o.kb.keys();
        if o.kb.mode == Mode::Emoji {
            osk_emoji_img::request_visible(if o.open { emoji_text(&o.kb, &keys) } else { vec![] });
        } else {
            osk_emoji_img::request_visible(Vec::new());
        }
        let model: Vec<OskKey> = keys.iter().enumerate().map(|(i, k)| okey(o, i, k)).collect();
        let ext = o.kb.ext.as_ref().map(|r| {
            let items: Vec<OskExt> = r
                .items
                .iter()
                .enumerate()
                .map(|(j, (t, tint))| {
                    let img = tint.and_then(|_| osk_emoji_img::emoji_image(t));
                    OskExt { text: if img.is_some() { SharedString::new() } else { t.as_str().into() }, sel: r.sel == Some(j), has_img: img.is_some(), img: img.unwrap_or_default() }
                })
                .collect();
            (r.x, r.y, items)
        });
        let emoji = o.kb.mode == Mode::Emoji;
        let message = if emoji && o.kb.cat < CAT_STEAM { t_or("#Keyboard_SteamItems_None", i18n::tr_ref("osk.no_objects")) } else { String::new() };
        (model, o.open, (o.kb.panel_top(), o.kb.panel_h()), emoji, message, o.pads, ext,
            o.input_source.show_focus(pad_mode), o.hold.is_some_and(|h| h.src == Src::A))
    });
    let st = ui.global::<OskState>();
    update_model(st.get_keys(), keys, |model| st.set_keys(model));
    st.set_show_focus(show_focus);
    st.set_panel_y(panel_y.0);
    st.set_panel_h(panel_y.1);
    st.set_emoji_mode(emoji);
    st.set_message(SharedString::from(message));
    apply_pad_cursors(&st, pads);
    st.set_ext_open(ext.is_some());
    st.set_ext_nav(ext_nav);
    if let Some((x, y, items)) = ext {
        st.set_ext_x(x);
        st.set_ext_y(y);
        st.set_ext_w(items.len() as f32 * EXT_W);
        update_model(st.get_ext(), items, |model| st.set_ext(model));
    }
    st.set_open(open);
}

fn apply_pad_cursors(st: &OskState, pads: [Pad; 2]) {
    st.set_pad_l(pads[0].active);
    st.set_pad_lx(pads[0].x);
    st.set_pad_ly(pads[0].y);
    st.set_pad_ld(pads[0].down);
    st.set_pad_r(pads[1].active);
    st.set_pad_rx(pads[1].x);
    st.set_pad_ry(pads[1].y);
    st.set_pad_rd(pads[1].down);
}

fn update_focus_source(ui: &AppWindow, action: Action, is_repeat: bool, pad_mode: bool) {
    let show_focus = with(|o| {
        o.input_source.update(action, is_repeat);
        o.input_source.show_focus(pad_mode)
    });
    // Moving within one key or using a text shortcut need not rebuild the keys.
    let state = ui.global::<OskState>();
    if state.get_show_focus() != show_focus {
        state.set_show_focus(show_focus);
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn either_pad_hides_focus_until_fresh_navigation() {
        for right in [false, true] {
            let mut source = InputSource::default();
            assert!(source.show_focus(true));
            assert!(!source.show_focus(false));

            source.update(Action::KbPad { right, x: 0, y: 0, on: true }, false);
            assert!(!source.show_focus(true));

            for action in [
                Action::KbClick { right, down: true },
                Action::KbClick { right, down: false },
                Action::KbPad { right, x: 0, y: 0, on: false },
                Action::Char('a'),
                Action::Backspace,
                Action::KbA(false),
                Action::KbBack(false),
                Action::KbShift(false),
            ] {
                source.update(action, false);
                assert!(!source.show_focus(true), "{action:?}");
            }
            source.update(Action::Right, true);
            assert!(!source.show_focus(true));
            source.update(Action::Right, false);
            assert!(source.show_focus(true));

            source.update(Action::KbPad { right, x: 800, y: 0, on: true }, false);
            assert!(!source.show_focus(true));
            source.update(Action::KbA(true), false);
            assert!(source.show_focus(true));
            source.update(Action::KbClick { right, down: true }, false);
            assert!(!source.show_focus(true));
            source.update(Action::Y, false);
            assert!(source.show_focus(true));
        }
    }
}

#[cfg(test)]
pub fn fixture_focus_input(ui: &AppWindow, action: Action) {
    update_focus_source(ui, action, false, true);
}

/// Тестовая подготовка состояния (рендер PNG ключевых состояний): `f` меняет модель, клавиатура считается открытой.
#[cfg(test)]
pub fn fixture_state(ui: &AppWindow, f: impl FnOnce(&mut Kb), pad_mode: bool, pad: Option<(bool, f32, f32)>, pressed: bool) {
    with(|o| {
        o.open = true;
        o.kb = Kb::new(1280.0, 800.0);
        f(&mut o.kb);
        o.name_until = None;
        o.pads = [Pad::default(); 2];
        o.input_source = InputSource::default();
        o.touched = None;
        o.a_pressed = None;
        if let Some((right, x, y)) = pad {
            o.input_source.update(Action::KbPad { right, x: 0, y: 0, on: true }, false);
            let (px, py) = pad_point(&o.kb, right, x, y, 1.0);
            o.pads[usize::from(right)] = Pad { active: true, x: px, y: py, hover: o.kb.hit(px, py), down: pressed };
        } else if pressed {
            o.a_pressed = Some(o.kb.focus);
        }
    });
    let st = ui.global::<OskState>();
    st.set_glyph_l1(load_icon("glyph:l1"));
    st.set_glyph_r1(load_icon("glyph:r1"));
    with(|o| osk_emoji_img::prepare_fixture(&emoji_text(&o.kb, &o.kb.keys())));
    apply(ui, pad_mode);
}

impl App {
    pub fn osk_is_open(&self) -> bool {
        with(|o| o.open)
    }

    fn osk_dismiss(&mut self) {
        let t = target(self);
        // закрытие клавиатуры само отменяет внешний приёмник (`osk_set_open(false)`); остальные цели уведомляются здесь
        self.osk_set_open(false);
        if let Some(t) = t.filter(|t| !matches!(t, Target::Sink(_))) {
            ext_closed(self, t);
        }
    }

    /// Показать клавиатуру по хоткею: если курсор на строке поиска в шапке — это поле и получает ввод.
    pub fn osk_show(&mut self) {
        if !self.osk_is_open() && target(self).is_none() && super::chrome::hf() == 1 {
            self.header_click(1);
            return;
        }
        self.osk_set_open(true);
    }

    pub fn osk_icons_ready(&mut self) {
        let ui = self.ui();
        let st = ui.global::<OskState>();
        st.set_glyph_l1(load_icon("glyph:l1"));
        st.set_glyph_r1(load_icon("glyph:r1"));
        if self.osk_is_open() {
            self.push_osk();
        }
    }

    pub fn osk_emoji_ready(&mut self) {
        if with(|o| o.open && o.kb.mode == Mode::Emoji) {
            self.push_osk();
        }
    }

    pub fn osk_set_open(&mut self, open: bool) {
        let was = with(|o| o.open);
        with(|o| {
            o.open = open;
            o.touched = None;
            o.a_pressed = None;
            o.pads = [Pad::default(); 2];
            o.back = None;
            o.hold = None;
            o.kb.ext = None;
            if open && !was {
                o.input_source = InputSource::default();
                let ui_size = (1280.0, 800.0);
                let st = settings();
                let layout = if st.layouts.contains(&o.kb.layout) { o.kb.layout } else { st.current };
                o.kb = Kb { recent: std::mem::take(&mut o.kb.recent), tints: std::mem::take(&mut o.kb.tints), layout, layouts: st.layouts, ..Kb::new(ui_size.0, ui_size.1) };
                o.kb.focus_initial();
                o.name_until = (o.kb.layouts.len() > 1).then(|| Instant::now() + NAME_SHOWN);
            }
        });
        crate::input::set_osk_open(open);
        if !open {
            // клавиатура закрыта любым путём (B, «Свернуть», смена экрана): приёмник не должен остаться привязанным
            if let Some(sk) = sink() {
                SINK.with(|c| c.set(None));
                (sk.cancel)(self);
            }
        }
        if open && !was {
            self.osk_schedule_name_refresh();
            self.osk_read_settings();
        }
        if open || was {
            self.push_osk();
        }
    }

    fn osk_schedule_name_refresh(&self) {
        NAME_TIMER.with(|t| {
            t.start(TimerMode::SingleShot, NAME_SHOWN + Duration::from_millis(30), || super::defer(|a| a.push_osk()));
        });
    }

    pub fn push_osk(&mut self) {
        apply(&self.ui(), self.pad_mode);
    }

    // ---------------------------------------------------------------- вывод в поле ввода

    /// Цель ввода; если поле не выбрано, а курсор стоит на строке поиска в шапке, ввод открывает поиск.
    fn osk_target(&mut self) -> Option<Target> {
        if target(self).is_none() && super::chrome::hf() == 1 {
            self.header_click(1);
        }
        target(self)
    }

    fn osk_text(&mut self, s: &str) {
        for c in s.chars() {
            if let Some(t) = self.osk_target() {
                let mode = current_mode(self).unwrap_or_default();
                if mode.accepts(c) {
                    ext_type(self, t, Some(c));
                }
            }
        }
    }

    fn osk_emit(&mut self, out: Out) {
        match out {
            Out::Text(t) => self.osk_text(&t),
            Out::Backspace => {
                if let Some(t) = self.osk_target() {
                    ext_type(self, t, None);
                }
            }
            Out::Left | Out::Right => {
                if let Some(Target::Search) = target(self) {
                    self.search_caret_move(if matches!(out, Out::Left) { -1 } else { 1 });
                }
            }
            Out::Paste => {
                if let Some(Target::Search) = target(self) {
                    self.search_paste();
                }
            }
            Out::Enter => {
                let t = self.osk_target();
                if let Some(Target::Sink(sk)) = t {
                    SINK.with(|c| c.set(None));
                    self.osk_set_open(false);
                    (sk.enter)(self);
                    return;
                }
                self.osk_set_open(false);
                match t {
                    Some(Target::Search) => self.search_to_results(),
                    Some(t @ Target::Anim) => ext_closed(self, t),
                    _ => {}
                }
            }
            Out::Hide => self.osk_dismiss(),
            Out::Tab | Out::Up | Out::Down | Out::None => {}
        }
    }

    /// Нажатие клавиши `i` (отпускание пальца / A / клик тачпада).
    fn osk_press_idx(&mut self, i: usize) {
        let def = with(|o| o.kb.keys().get(i).map(|k| k.def.clone()));
        let Some(def) = def else { return };
        let (out, layout_changed) = with(|o| {
            let before = (o.kb.layout, o.kb.mode);
            let out = o.kb.press(&def);
            let changed = o.kb.layout != before.0;
            if changed {
                o.name_until = Some(Instant::now() + NAME_SHOWN);
            }
            if o.kb.mode != before.1 {
                o.a_pressed = None;
                for pad in &mut o.pads {
                    pad.hover = if pad.active { o.kb.hit(pad.x, pad.y) } else { None };
                }
            }
            (out, changed)
        });
        if layout_changed {
            self.osk_schedule_name_refresh();
        }
        let hide = matches!(out, Out::Hide | Out::Enter);
        self.osk_emit(out);
        if !hide {
            self.push_osk();
        }
    }

    fn osk_feedback(pad: u8) {
        crate::sound::play(crate::sound::Snd::Typing);
        if let Some(h) = settings().haptic {
            haptic(pad, h);
        }
    }

    /// Настройки клавиатуры читаются из Steam при каждом открытии (их меняют в настройках Steam); до ответа — прежние.
    fn osk_read_settings(&self) {
        if (std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active()) {
            return;
        }
        crate::setters::read(SETTINGS_JS.into(), |a, v| {
            if v.is_null() {
                return;
            }
            let s = parse_settings(&v);
            let changed = SETTINGS.with(|c| std::mem::replace(&mut *c.borrow_mut(), s.clone()) != s);
            if changed && a.osk_is_open() {
                with(|o| {
                    o.kb.layouts = s.layouts.clone();
                    if !o.kb.layouts.contains(&o.kb.layout) {
                        o.kb.layout = s.current;
                    }
                    o.kb.focus_initial();
                });
                a.push_osk();
            }
        });
    }

    // ---------------------------------------------------------------- сенсор

    fn osk_touch_down(&mut self, x: f32, y: f32) {
        let hit = with(|o| {
            let i = o.kb.hit(x, y);
            o.drag = (o.kb.mode == Mode::Emoji && i.is_some_and(|i| o.kb.keys().get(i).is_some_and(|k| matches!(k.def.act, Act::Emoji(_)))))
                .then_some(Drag { x0: x, scroll0: o.kb.scroll, active: false });
            i
        });
        let Some(i) = hit else { return };
        self.set_pad(false);
        Self::osk_feedback(2);
        self.osk_flush_hold(Src::Touch);
        with(|o| o.touched = Some(i));
        self.osk_begin_hold(Src::Touch, i);
        self.push_osk();
    }

    /// Палец ведут по открытому ряду расширенных символов: выбран элемент под пальцем (`nExtendedKeyTouched`).
    fn osk_touch_move(&mut self, x: f32, y: f32) {
        // прокрутка эмодзи-сетки: нажатие отменяется, сетка идёт за пальцем по колонкам
        let scrolled = with(|o| {
            let d = o.drag.as_mut()?;
            if !d.active && (x - d.x0).abs() > DRAG_START && o.kb.ext.is_none() {
                d.active = true;
                o.touched = None;
                o.hold = None;
            }
            if !d.active {
                return None;
            }
            let (x0, s0) = (d.x0, d.scroll0);
            let before = o.kb.scroll;
            o.kb.set_scroll(s0 as i64 - ((x - x0) / EMOJI_COL_W).round() as i64);
            Some(o.kb.scroll != before)
        });
        if let Some(changed) = scrolled {
            if changed {
                self.push_osk();
            }
            return;
        }
        let changed = with(|o| {
            if !o.hold.is_some_and(|h| h.src == Src::Touch) {
                return false;
            }
            let Some(r) = o.kb.ext.as_mut() else { return false };
            let sel = r.hit(x, y);
            std::mem::replace(&mut r.sel, sel) != sel
        });
        if changed {
            self.push_osk();
        }
    }

    /// Начало нажатия клавиши `i` источником `src`: Backspace — повтор, клавиша с вариантами — таймер долгого нажатия.
    fn osk_begin_hold(&mut self, src: Src, i: usize) {
        let (is_back, has_ext) = with(|o| {
            let keys = o.kb.keys();
            let k = keys.get(i);
            (k.is_some_and(|k| k.def.act == Act::Backspace), k.is_some_and(|k| o.kb.ext_items(k).is_some()))
        });
        if is_back {
            with(|o| o.back = Some((Instant::now(), Instant::now(), false)));
            self.osk_start_tick();
        } else if has_ext {
            with(|o| o.hold = Some(Hold { src, key: i, since: Instant::now() }));
            self.osk_start_tick();
        }
    }

    /// Новое нажатие другим источником, пока удерживается клавиша, — сначала набрать удерживаемую (sp.js `KeyDown`).
    fn osk_flush_hold(&mut self, src: Src) {
        let prev = with(|o| match o.hold {
            Some(h) if h.src != src => {
                o.hold = None;
                o.kb.ext = None;
                Some(h.key)
            }
            _ => None,
        });
        if let Some(k) = prev {
            self.osk_press_idx(k);
        }
    }

    /// Конец удержания: открыт ряд — набрать выбранный вариант (или саму клавишу, если палец ушёл с ряда);
    /// иначе — обычное нажатие `fallback`. true — удержание этим источником было.
    fn osk_end_hold(&mut self, src: Src, fallback: Option<usize>) -> bool {
        let (held, ext) = with(|o| match o.hold {
            Some(h) if h.src == src => {
                o.hold = None;
                (Some(h.key), o.kb.ext.as_ref().map(|r| r.sel))
            }
            _ => (None, None),
        });
        let Some(key) = held else { return false };
        match ext {
            Some(Some(j)) => {
                let out = with(|o| o.kb.press_ext(j));
                self.osk_emit(out);
                self.push_osk();
            }
            Some(None) => {
                with(|o| o.kb.ext = None);
                self.osk_press_idx(key);
            }
            None => self.osk_press_idx(fallback.unwrap_or(key)),
        }
        true
    }

    /// Палец поднят: набирается клавиша, которой коснулись (sp.js `HandleTouchEnd` — `TypeKey(target)`).
    fn osk_touch_up(&mut self) {
        let (was, sent, dragged) = with(|o| {
            let was = o.touched.take();
            let sent = o.back.take().map(|b| b.2).unwrap_or(false);
            let dragged = o.drag.take().is_some_and(|d| d.active);
            (was, sent, dragged)
        });
        if dragged {
            self.push_osk();
            return;
        }
        if self.osk_end_hold(Src::Touch, was) {
            return;
        }
        match was {
            Some(i) if !sent => self.osk_press_idx(i),
            _ => self.push_osk(),
        }
    }

    fn osk_touch_cancel(&mut self) {
        with(|o| {
            o.drag = None;
            o.touched = None;
            o.back = None;
            if o.hold.is_some_and(|h| h.src == Src::Touch) {
                o.hold = None;
                o.kb.ext = None;
            }
        });
        self.push_osk();
    }

    // ---------------------------------------------------------------- повтор Backspace

    fn osk_start_tick(&self) {
        TICK.with(|t| {
            t.start(TimerMode::Repeated, Duration::from_millis(30), || super::defer(|a| a.osk_tick()));
        });
    }

    fn osk_tick(&mut self) {
        // долгое нажатие: открыть ряд вариантов у удерживаемой клавиши
        let opened = with(|o| match o.hold {
            Some(h) if o.kb.ext.is_none() && h.since.elapsed() >= LONG_PRESS => {
                let half = match h.src {
                    Src::Pad(i) => Some(i == 1),
                    _ => None,
                };
                let ok = o.kb.open_ext(h.key, half);
                if ok && h.src != Src::A {
                    // касание и тачпад выбирают элемент под пальцем, автофокус только у геймпада
                    if let Some(r) = o.kb.ext.as_mut() {
                        r.sel = None;
                    }
                }
                ok
            }
            _ => false,
        });
        if opened {
            crate::sound::play(crate::sound::Snd::Typing);
            self.push_osk();
        }
        let fire = with(|o| match o.back.as_mut() {
            Some((since, last, sent)) => {
                let due = if *sent { *last + REPEAT } else { *since + LONG_PRESS };
                if Instant::now() >= due {
                    *last = Instant::now();
                    *sent = true;
                    true
                } else {
                    false
                }
            }
            None => {
                if o.hold.is_none() || o.kb.ext.is_some() {
                    TICK.with(|t| t.stop());
                }
                false
            }
        });
        if fire {
            crate::sound::play(crate::sound::Snd::Typing);
            self.osk_emit(Out::Backspace);
        }
    }

    // ---------------------------------------------------------------- геймпад

    /// D-pad/стики. Вверх за верхний ряд — `HandleNavOut`: клавиатура скрывается, фокус остаётся в поле.
    fn osk_dir(&mut self, d: u8) {
        if !with(|o| o.kb.nav(d)) && d == 0 {
            self.osk_dismiss();
            return;
        }
        self.push_osk();
    }

    fn osk_pad_move(&mut self, right: bool, x: i16, y: i16, on: bool) {
        let idx = right as usize;
        // Steam's built-in Keyboard action set uses a raw outer radius of 32000.
        let (keys_changed, hover_tick, pads) = with(|o| {
            let (px, py) = pad_point(&o.kb, right, x as f32 / 32000.0, y as f32 / 32000.0, settings().pad_scale);
            let prev = o.pads[idx];
            if prev.active != on && crate::input::osk_pad_diag_enabled() {
                crate::diag::log("input.osk_pad.cursor", serde_json::json!({
                    "right": right, "contact": on, "x": px, "y": py,
                }));
            }
            // ряд вариантов открыт этим тачпадом: курсор выбирает в нём, клавиши под рядом не подсвечиваются
            if o.hold.is_some_and(|h| h.src == Src::Pad(idx)) {
                if let Some(r) = o.kb.ext.as_mut() {
                    let sel = if on { r.hit(px, py) } else { None };
                    let moved = std::mem::replace(&mut r.sel, sel) != sel;
                    o.pads[idx] = Pad { active: on, x: px, y: py, hover: prev.hover, down: prev.down && on };
                    return (moved, moved && sel.is_some(), o.pads);
                }
            }
            let hover = if on { o.kb.hit(px, py) } else { None };
            o.pads[idx] = Pad { active: on, x: px, y: py, hover, down: prev.down && on };
            (hover != prev.hover, hover.is_some() && hover != prev.hover, o.pads)
        });
        if hover_tick {
            haptic(idx as u8, HAPTIC_HOVER);
        }
        if keys_changed {
            self.push_osk();
        } else {
            apply_pad_cursors(&self.ui().global::<OskState>(), pads);
        }
    }

    fn osk_pad_click(&mut self, right: bool, down: bool) {
        let idx = right as usize;
        let hover = with(|o| o.pads[idx].hover);
        let Some(i) = hover else {
            let was_down = with(|o| std::mem::take(&mut o.pads[idx].down));
            if !down && !self.osk_end_hold(Src::Pad(idx), None) && was_down {
                apply_pad_cursors(&self.ui().global::<OskState>(), with(|o| o.pads));
            }
            return;
        };
        if down {
            Self::osk_feedback(idx as u8);
            self.osk_flush_hold(Src::Pad(idx));
            with(|o| o.pads[idx].down = true);
            self.osk_begin_hold(Src::Pad(idx), i);
            self.push_osk();
        } else {
            let (was_down, sent) = with(|o| {
                let w = o.pads[idx].down;
                o.pads[idx].down = false;
                (w, o.back.take().map(|b| b.2).unwrap_or(false))
            });
            if was_down && self.osk_end_hold(Src::Pad(idx), Some(i)) {
                return;
            }
            if was_down && !sent {
                self.osk_press_idx(i);
            } else {
                self.push_osk();
            }
        }
    }

    /// Ввод геймпада, пока клавиатура открыта. true — обработано.
    pub fn osk_handle(&mut self, a: Action) -> bool {
        self.osk_handle_repeat(a, false)
    }

    pub(crate) fn osk_handle_repeat(&mut self, a: Action, is_repeat: bool) -> bool {
        if !with(|o| o.open) {
            return false;
        }
        let pad_mode_actions = !matches!(a, Action::KbPad { .. } | Action::KbClick { .. } | Action::Char(_));
        if pad_mode_actions {
            self.set_pad(true);
        }
        update_focus_source(&self.ui(), a, is_repeat, self.pad_mode);
        // открыт ряд вариантов от кнопки A: D-pad/стики ходят только по нему, B закрывает ряд без набора
        if with(|o| o.kb.ext.is_some() && o.hold.is_some_and(|h| h.src == Src::A)) {
            match a {
                Action::Up | Action::Down | Action::Left | Action::Right => {
                    let d = match a { Action::Up => 0, Action::Down => 1, Action::Left => 2, _ => 3 };
                    with(|o| o.kb.ext_nav(d));
                    self.push_osk();
                    return true;
                }
                Action::B => {
                    with(|o| {
                        o.hold = None;
                        o.kb.ext = None;
                    });
                    self.push_osk();
                    return true;
                }
                _ => {}
            }
        }
        match a {
            Action::Up => self.osk_dir(0),
            Action::Down => self.osk_dir(1),
            Action::Left => self.osk_dir(2),
            Action::Right => self.osk_dir(3),
            // Deck: A с отпусканием — символы набираются на отпускание, удержание 450 мс открывает варианты,
            // Backspace повторяется; остальные клавиши срабатывают сразу (sp.js `KeyDown`/`KeyUp`).
            Action::KbA(true) => {
                Self::osk_feedback(2);
                self.osk_flush_hold(Src::A);
                let f = with(|o| o.kb.focus);
                let has_ext = with(|o| o.kb.keys().get(f).is_some_and(|k| o.kb.ext_items(k).is_some()));
                with(|o| o.a_pressed = Some(f));
                if has_ext {
                    self.osk_begin_hold(Src::A, f);
                    self.push_osk();
                } else {
                    self.osk_press_idx(f);
                }
            }
            Action::KbA(false) => {
                with(|o| o.a_pressed = None);
                if !self.osk_end_hold(Src::A, None) {
                    self.push_osk();
                }
            }
            Action::A => {
                Self::osk_feedback(2);
                let f = with(|o| o.kb.focus);
                self.osk_press_idx(f);
            }
            Action::B => self.osk_dismiss(),
            Action::X | Action::Backspace => {
                crate::sound::play(crate::sound::Snd::Typing);
                self.osk_emit(Out::Backspace);
            }
            Action::KbBack(down) => {
                if down {
                    Self::osk_feedback(2);
                    self.osk_emit(Out::Backspace);
                    with(|o| o.back = Some((Instant::now(), Instant::now(), false)));
                    self.osk_start_tick();
                } else {
                    with(|o| o.back = None);
                }
            }
            Action::Y => {
                Self::osk_feedback(2);
                self.osk_emit(Out::Text(" ".into()));
            }
            Action::R2 => {
                Self::osk_feedback(2);
                self.osk_emit(Out::Enter);
            }
            Action::L2 => {
                crate::sound::play(crate::sound::Snd::Typing);
                with(|o| o.kb.shift = if o.kb.shift == 0 { 1 } else { 0 });
                self.push_osk();
            }
            Action::KbShift(down) => {
                with(|o| o.kb.shift = if down { 2 } else { 0 });
                self.push_osk();
            }
            Action::KbCaps => {
                crate::sound::play(crate::sound::Snd::Typing);
                with(|o| o.kb.caps = if o.kb.caps == 0 { 2 } else { 0 });
                self.push_osk();
            }
            Action::LB => {
                with(|o| o.kb.cat_step(-1));
                self.push_osk();
            }
            Action::RB => {
                with(|o| o.kb.cat_step(1));
                self.push_osk();
            }
            Action::KbPad { right, x, y, on } => self.osk_pad_move(right, x, y, on),
            Action::KbClick { right, down } => self.osk_pad_click(right, down),
            Action::Char(c) => {
                let mut b = [0u8; 4];
                self.osk_text(c.encode_utf8(&mut b));
            }
            // STEAM / QAM / меню и прочее — не наши: пусть откроют свои окна поверх.
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn steam_keyboard_settings_are_parsed() {
        // ничего не сохранено — значения по умолчанию Steam
        assert_eq!(parse_settings(&json!({})), KbSettings::default());
        let v = json!({
            "keyboard_haptic_preset": "0",
            "keyboard_trackpad_typing_scale": "1.5",
            "keyboard_layout": "{\"currentLayout\":18,\"selectedLayouts\":[18,7]}",
        });
        let s = parse_settings(&v);
        assert_eq!(s.haptic, None, "Preset 0, haptic off.");
        assert_eq!(s.pad_scale, 1.5);
        assert_eq!(s.layouts, vec![1], "untransferred layouts skipped");
        assert_eq!(s.current, 1);
        let c = json!({"keyboard_haptic_preset": "10", "keyboard_haptic_custom": "{\"eHapticType\":1,\"unIntensity\":3,\"ndBGain\":-4}"});
        assert_eq!(parse_settings(&c).haptic, Some((1, 3, -4)));
        assert_eq!(parse_settings(&json!({"keyboard_haptic_preset": "1"})).haptic, Some((1, 2, -2)));
    }
}

#[cfg(test)]
#[path = "osk_field_tests.rs"]
pub(crate) mod field_tests;
