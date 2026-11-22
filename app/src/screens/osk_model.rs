//! Модель виртуальной клавиатуры Steam Deck (docs/keyboard-analysis-2026-11-18.md): раскладки, геометрия,
//! состояния Shift/Caps, фокус, эмодзи. Чистая логика без Slint — проверяется headless-тестами.
//! Раскладки и ширины клавиш — из sp.js:410195-412307 и CSS (доли ряда); размеры панели — со скриншотов Деки 1280x800.

use steam_assets::i18n;
use std::borrow::Cow;
use std::sync::LazyLock;

/// Масштаб css→px на скриншотах Деки (панель 357 px = 239 css).
pub const SCALE: f32 = 1.494;
pub const PANEL_H: f32 = 357.0;
/// В режиме эмодзи панель ниже: верх на 455 px (скриншот 011601), клавиши остаются на своих местах.
pub const EMOJI_PANEL_H: f32 = 345.0;
const PAD_L: f32 = 1.0 * SCALE;
const PAD_R: f32 = 3.0 * SCALE;
const PAD_T: f32 = 3.0 * SCALE;
const ROW_H: f32 = 47.0 * SCALE;
/// Отступы области нажатия вокруг клавиши: сверху 1, справа 1, снизу 2, слева 2 css.
const HIT: (f32, f32, f32, f32) = (1.0 * SCALE, 1.0 * SCALE, 2.0 * SCALE, 2.0 * SCALE);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Char,
    Half,
    Tab,
    Meta,
    Close,
    Caps,
    LShift,
    RShift,
    Enter,
    Backspace,
    Space,
    Spacer,
    /// Эмодзи и вкладки категорий.
    Emoji,
    Tab2,
}

impl Kind {
    /// Доля ширины ряда; None — делит остаток поровну (flex-grow 1).
    fn frac(self) -> Option<f32> {
        Some(match self {
            Kind::Tab => 0.07,
            Kind::Caps => 0.094,
            Kind::LShift | Kind::RShift => 0.14,
            Kind::Enter | Kind::Backspace => 0.117,
            Kind::Meta | Kind::Close => 0.082,
            Kind::Half => 0.033,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    Char(String),
    Backspace,
    Tab,
    Enter,
    Shift,
    Caps,
    AltGr,
    Space,
    ToEmoji,
    ToLayout,
    ToAbc,
    Left,
    Right,
    Up,
    Down,
    Paste,
    Hide,
    /// Вкладка категории эмодзи (0 Недавние … 8 Флаги) либо предметы Steam (9..12).
    Cat(usize),
    Emoji(usize),
    None,
}

/// Описание клавиши до размещения.
#[derive(Clone, Debug)]
pub struct KeyDef {
    pub act: Act,
    pub kind: Kind,
    /// Основной символ / подпись; для спецклавиш — токен локализации в `tok`.
    pub base: String,
    /// Символ при Shift (мелкая надпись сверху).
    pub shift: Option<String>,
    /// AltGr-символ (мелкая надпись справа внизу).
    pub alt: Option<String>,
    pub tok: Option<(&'static str, &'static str)>,
    pub icon: Option<&'static str>,
    /// Вторая стрелка пары [←,↑]: показывается мелко, а под Shift становится основной.
    pub icon2: Option<&'static str>,
    pub act2: Option<Act>,
    pub glyph_l: Option<&'static str>,
    pub glyph_r: Option<&'static str>,
}

impl KeyDef {
    fn new(act: Act, kind: Kind) -> Self {
        KeyDef { act, kind, base: String::new(), shift: None, alt: None, tok: None, icon: None, icon2: None, act2: None, glyph_l: None, glyph_r: None }
    }
    fn ch(c: &str) -> Self {
        let mut k = Self::new(Act::Char(c.into()), Kind::Char);
        k.base = c.into();
        k
    }
    /// `[основной, shift, altgr]` как в массивах Steam.
    fn pair(a: &str, s: &str) -> Self {
        let mut k = Self::ch(a);
        k.shift = Some(s.into());
        k
    }
    fn tok(act: Act, kind: Kind, t: &'static str, fallback: &'static str) -> Self {
        let mut k = Self::new(act, kind);
        k.tok = Some((t, fallback));
        k
    }
}

fn backspace() -> KeyDef {
    let mut k = KeyDef::tok(Act::Backspace, Kind::Backspace, "#Key_Backspace", "Backspace");
    k.glyph_l = Some("x");
    k
}
fn tab() -> KeyDef {
    KeyDef::tok(Act::Tab, Kind::Tab, "#Key_Tab", "Tab")
}
fn caps() -> KeyDef {
    let mut k = KeyDef::tok(Act::Caps, Kind::Caps, "#Key_Caps", "Caps");
    k.glyph_r = Some("l3");
    k
}
fn enter() -> KeyDef {
    let mut k = KeyDef::tok(Act::Enter, Kind::Enter, "#MainTabsSearch", i18n::tr_ref("osk_model.search"));
    k.glyph_l = Some("r2");
    k
}
fn lshift() -> KeyDef {
    let mut k = KeyDef::tok(Act::Shift, Kind::LShift, "#Key_Shift", "Shift");
    k.glyph_r = Some("l2");
    k
}
fn rshift() -> KeyDef {
    let mut k = KeyDef::tok(Act::Shift, Kind::RShift, "#Key_Shift", "Shift");
    k.glyph_l = Some("l2");
    k
}
fn space() -> KeyDef {
    let mut k = KeyDef::new(Act::Space, Kind::Space);
    k.glyph_l = Some("y");
    k
}
fn half(a: &str, s: &str) -> KeyDef {
    let mut k = KeyDef::pair(a, s);
    k.kind = Kind::Half;
    k
}
fn arrows(a: (Act, &'static str), b: (Act, &'static str)) -> KeyDef {
    let mut k = KeyDef::new(a.0, Kind::Meta);
    k.icon = Some(a.1);
    k.icon2 = Some(b.1);
    k.act2 = Some(b.0);
    k
}

fn number_row(ru: bool) -> Vec<KeyDef> {
    let mut v = vec![];
    if ru {
        v.push({
            let mut k = KeyDef::ch(&'ё'.to_string());
            k.kind = Kind::Half;
            k
        });
        for (a, s) in [("1", "!"), ("2", "\""), ("3", "№"), ("4", ";"), ("5", "%"), ("6", ":"), ("7", "?"), ("8", "*"), ("9", "("), ("0", ")"), ("-", "_"), ("=", "+")] {
            let mut k = KeyDef::pair(a, s);
            if a == "8" {
                k.alt = Some("₽".into());
            }
            v.push(k);
        }
    } else {
        v.push(half("`", "~"));
        for (a, s) in [("1", "!"), ("2", "@"), ("3", "#"), ("4", "$"), ("5", "%"), ("6", "^"), ("7", "&"), ("8", "*"), ("9", "("), ("0", ")"), ("-", "_"), ("=", "+")] {
            v.push(KeyDef::pair(a, s));
        }
    }
    v.push(backspace());
    v
}

/// Нижний ряд стандартной клавиатуры (`v` / `B` в sp.js; у русской добавляется AltGr). Клавиша «Сместить» в Gaming Mode скрыта.
/// Клавиша «язык» скрыта, если выбрана одна раскладка (sp.js `BIsKeyEnabled("SwitchKeys_Layout")`).
fn bottom_row(altgr: bool, layout_key: bool) -> Vec<KeyDef> {
    let mut v = vec![];
    let mut e = KeyDef::new(Act::ToEmoji, Kind::Meta);
    e.icon = Some("Emoji");
    v.push(e);
    if layout_key {
        let mut l = KeyDef::new(Act::ToLayout, Kind::Meta);
        l.icon = Some("Network");
        v.push(l);
    }
    v.push(space());
    if altgr {
        v.push(KeyDef::tok(Act::AltGr, Kind::Meta, "#Key_AltGr", "AltGr"));
    }
    v.push(arrows((Act::Left, "KaratLeft"), (Act::Up, "KaratUp")));
    v.push(arrows((Act::Right, "KaratRight"), (Act::Down, "KaratDown")));
    v.push(KeyDef::tok(Act::Paste, Kind::Meta, "#Key_Paste", i18n::tr_ref("osk_model.paste")));
    let mut c = KeyDef::new(Act::Hide, Kind::Close);
    c.icon = Some("HideKeyboard");
    v.push(c);
    v
}

fn letters(rows: &[&[&str]]) -> Vec<Vec<KeyDef>> {
    rows.iter().map(|r| r.iter().map(|c| KeyDef::ch(c)).collect()).collect()
}

pub const LAYOUT_NAMES: [(&str, &str, &str); 2] =
    [("qwerty", "#KeyboardLayout_QWERTY", "osk_model.english_qwerty"), ("russian", "#KeyboardLayout_Russian", "osk_model.russian")];

/// Раскладка Steam (`ESteamDeckKeyboardLayout`, sp.js:117551: QWERTY 0, Russian 18) → индекс в `LAYOUT_NAMES`.
/// Остальные раскладки Steam пока не перенесены — пропускаются.
pub fn layout_from_steam(id: i64) -> Option<usize> {
    match id {
        0 => Some(0),
        18 => Some(1),
        _ => None,
    }
}

/// Пять рядов раскладки. 0 — QWERTY, 1 — русская.
fn layout_rows(idx: usize, layout_key: bool) -> Vec<Vec<KeyDef>> {
    let ru = idx == 1;
    let mut r1: Vec<KeyDef>;
    let mut r2: Vec<KeyDef>;
    let mut r3: Vec<KeyDef>;
    if !ru {
        let l = letters(&[&["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"], &["a", "s", "d", "f", "g", "h", "j", "k", "l"], &["z", "x", "c", "v", "b", "n", "m"]]);
        r1 = vec![tab()];
        r1.extend(l[0].clone());
        r1.extend([KeyDef::pair("[", "{"), KeyDef::pair("]", "}"), KeyDef::pair("\\", "|")]);
        r2 = vec![caps()];
        r2.extend(l[1].clone());
        r2.extend([KeyDef::pair(";", ":"), KeyDef::pair("'", "\""), enter()]);
        r3 = vec![lshift()];
        r3.extend(l[2].clone());
        r3.extend([KeyDef::pair(",", "<"), KeyDef::pair(".", ">"), KeyDef::pair("/", "?"), rshift()]);
    } else {
        let l = [
            ['й', 'ц', 'у', 'к', 'е', 'н', 'г', 'ш', 'щ', 'з', 'х', 'ъ'].as_slice(),
            &['ф', 'ы', 'в', 'а', 'п', 'р', 'о', 'л', 'д', 'ж', 'э'],
            &['я', 'ч', 'с', 'м', 'и', 'т', 'ь', 'б', 'ю'],
        ].map(|row| row.iter().map(|ch| KeyDef::ch(&ch.to_string())).collect::<Vec<_>>());
        r1 = vec![tab()];
        r1.extend(l[0].clone());
        r1.push(KeyDef::pair("\\", "/"));
        r2 = vec![caps()];
        r2.extend(l[1].clone());
        r2.push(enter());
        r3 = vec![lshift()];
        r3.extend(l[2].clone());
        r3.extend([KeyDef::pair(".", ","), rshift()]);
    }
    vec![number_row(ru), r1, r2, r3, bottom_row(ru, layout_key)]
}

/// Вкладки категорий эмодзи: (ключ, иконка, токен локализации, запасной текст).
pub const EMOJI_TABS: [(&str, &str, &str); 13] = [
    ("RecentSteamItems", "ClockOutline", "#Emoji_RecentSteamItems"),
    ("Emoticons", "@emoticons", "#Emoji_Emoticons"),
    ("Stickers", "@stickers", "#Emoji_Stickers"),
    ("ChatFX", "@chatfx", "#Emoji_ChatFX"),
    ("", "", ""),
    ("Recent", "Recent", "#Emoji_Recent"),
    ("People", "Emoji", "#Emoji_People"),
    ("Activity", "Activities", "#Emoji_Activity"),
    ("Animals", "Nature", "#Emoji_Animals"),
    ("Food", "Food", "#Emoji_Food"),
    ("Travel", "Travel", "#Emoji_Travel"),
    ("Objects", "Objects", "#Emoji_Objects"),
    ("Symbols", "Symbols", "#Emoji_Symbols"),
];
// В списке 13 строк, а вкладок 14 (+Flags, добавляется в `emoji_keys`). Иконки «@…» — оригинальные SVG Steam из app/ui/assets/osk.
pub const TAB_SLOTS: usize = 14;
/// Индексы вкладок в `Act::Cat`: 0..3 предметы Steam, 4..12 Unicode (Недавние, Люди … Флаги).
pub const CAT_STEAM: usize = 4;

/// Список эмодзи Steam (sp.js `FullEmojiList`, 1714 штук) и начала категорий People..Flags.
pub fn emoji_list() -> &'static (Vec<(String, u8)>, [usize; 8]) {
    use std::sync::OnceLock;
    static L: OnceLock<(Vec<(String, u8)>, [usize; 8])> = OnceLock::new();
    L.get_or_init(|| {
        let src = include_str!("osk_emoji.txt");
        let v: Vec<(String, u8)> = src
            .lines()
            .skip(1) // заголовок; сами эмодзи могут начинаться с «#» (keycap)
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once('\t').map(|(e, t)| (e.to_string(), t.parse().unwrap_or(0))))
            .collect();
        let find = |s: &str| v.iter().position(|(e, _)| e == s).unwrap_or(0);
        let starts = [find("😀"), find("💆"), find("🐵"), find("🍇"), find("🌍"), find("👓"), find("💋"), find("🏁")];
        (v, starts)
    })
}

/// Unicode-категории: диапазон индексов в `emoji_list` (People, Activity, Animals, Food, Travel, Objects, Symbols, Flags).
pub fn emoji_range(cat: usize) -> (usize, usize) {
    let (v, s) = emoji_list();
    let i = cat.saturating_sub(CAT_STEAM + 1).min(7);
    (s[i], if i == 7 { v.len() } else { s[i + 1] })
}

/// Оттенок эмодзи (sp.js `zt`, `h.C`): 0 по умолчанию, 1 светлый … 5 тёмный. Ключи FullEmojiList содержат метки
/// `*DEFAULT_PLACEHOLDER*` (вариант-селектор FE0F только у оттенка по умолчанию) и `%TINT_PLACEHOLDER%` (куда встаёт модификатор).
pub fn tinted(key: &str, tint: u8) -> String {
    if tint == 0 || tint > 5 {
        return key.replace("*DEFAULT_PLACEHOLDER*", "\u{FE0F}").replace("%TINT_PLACEHOLDER%", "");
    }
    let m = ["", "\u{1F3FB}", "\u{1F3FC}", "\u{1F3FD}", "\u{1F3FE}", "\u{1F3FF}"][tint as usize];
    let v = key.replace("*DEFAULT_PLACEHOLDER*", "");
    if v.contains("%TINT_PLACEHOLDER%") { v.replace("%TINT_PLACEHOLDER%", m) } else { v + m }
}

/// Расширенные символы долгого нажатия (sp.js:409673 `s_rgExtendedKeys`).
fn extended_table(c: &str) -> &'static str {
    match c {
        "a" => "áàâãäåæāą",
        "c" => "ćçč",
        "e" => "éèêëēėę",
        "i" => "íìîïįī",
        "l" => "ł",
        "n" => "ńñ",
        "o" => "óòôõöøōœ",
        "s" => "śßš",
        "u" => "úùûüū",
        "y" => "ÿ",
        "z" => "źžż",
        "0" => "°",
        "-" => "–—·",
        "=" => "≈≠",
        "." => "…",
        "!" => "¡",
        "?" => "¿",
        "'" => "‘’",
        "\"" => "“”„»«",
        "$" => "¢€£¥₱₩",
        "%" => "‰",
        "&" => "§",
        _ => "",
    }
}

/// Ширина и высота ячейки ряда расширенных символов (CSS `KeyboardExtendedKey` 48×44, ряд над клавишей `top:-44px`).
pub const EXT_W: f32 = 48.0 * SCALE;
pub const EXT_H: f32 = 44.0 * SCALE;

/// Открытый ряд расширенных символов / оттенков эмодзи (долгое нажатие 450 мс).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtRow {
    /// Индекс клавиши, над которой открыт ряд.
    pub parent: usize,
    /// (текст, оттенок для эмодзи).
    pub items: Vec<(String, Option<u8>)>,
    /// Выбранный элемент: геймпад — всегда (автофокус у клавиши); касание/тачпад — элемент под пальцем.
    pub sel: Option<usize>,
    pub x: f32,
    pub y: f32,
}

impl ExtRow {
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if y < self.y || y >= self.y + EXT_H || x < self.x {
            return None;
        }
        let i = ((x - self.x) / EXT_W) as usize;
        (i < self.items.len()).then_some(i)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Standard,
    Emoji,
}

/// Размещённая клавиша.
#[derive(Clone, Debug)]
pub struct PKey {
    pub def: KeyDef,
    pub row: usize,
    pub col: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Внутренний прямоугольник (видимая клавиша) внутри области нажатия.
    pub kx: f32,
    pub ky: f32,
    pub kw: f32,
    pub kh: f32,
}

/// Размещение клавиш ряда: проценты для фиксированных, остаток поровну.
fn place_row(defs: Vec<KeyDef>, row: usize, y: f32, inner_x: f32, inner_w: f32) -> Vec<PKey> {
    // ширина в CSS не включает padding области нажатия (content-box): 7 % + 4.5 px и т.д.
    let hp = HIT.1 + HIT.3;
    let fixed: f32 = defs.iter().filter_map(|d| d.kind.frac()).map(|f| f * inner_w + hp).sum();
    let n_flex = defs.iter().filter(|d| d.kind.frac().is_none()).count().max(1) as f32;
    let flex = ((inner_w - fixed) / n_flex).max(0.0);
    let mut x = inner_x;
    let mut out = vec![];
    for (col, d) in defs.into_iter().enumerate() {
        let w = d.kind.frac().map(|f| f * inner_w + hp).unwrap_or(flex);
        out.push(PKey { def: d, row, col, x, y, w, h: ROW_H, kx: x + HIT.3, ky: y + HIT.0, kw: w - HIT.1 - HIT.3, kh: ROW_H - HIT.0 - HIT.2 });
        x += w;
    }
    out
}

fn standard_keys(layout: usize, layout_key: bool, width: f32, height: f32) -> Vec<PKey> {
    let top = height - PANEL_H + PAD_T;
    layout_rows(layout, layout_key).into_iter().enumerate()
        .flat_map(|(r, defs)| place_row(defs, r, top + r as f32 * ROW_H, PAD_L, width - PAD_L - PAD_R))
        .collect()
}

static DECK_LAYOUTS: LazyLock<[Vec<PKey>; 4]> = LazyLock::new(|| {
    std::array::from_fn(|i| standard_keys(i / 2, i % 2 == 1, 1280.0, 800.0))
});

pub(super) fn warm_layouts() {
    LazyLock::force(&DECK_LAYOUTS);
}

/// Полное состояние клавиатуры.
#[derive(Clone, Debug)]
pub struct Kb {
    pub layout: usize,
    pub layouts: Vec<usize>,
    pub mode: Mode,
    /// 0 выкл, 1 однократно, 2 закреплён/удерживается.
    pub shift: u8,
    pub caps: u8,
    pub altgr: u8,
    /// Индекс в списке размещённых клавиш (для фокуса геймпада).
    pub focus: usize,
    /// Выбранная категория эмодзи (Act::Cat).
    pub cat: usize,
    /// Первая видимая колонка сетки эмодзи.
    pub scroll: usize,
    pub recent: Vec<usize>,
    pub win_w: f32,
    pub win_h: f32,
    /// Куда вернуть фокус при возврате «вверх» из ряда (MAINTAIN_X).
    pub keep_x: f32,
    /// Выбранные оттенки эмодзи (`EmojiStore.m_emojiTints`): индекс → 0..5.
    pub tints: std::collections::HashMap<usize, u8>,
    pub ext: Option<ExtRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Text(String),
    Backspace,
    Enter,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Paste,
    Hide,
    None,
}

pub const EMOJI_ROWS: usize = 3;
pub const EMOJI_COL_W: f32 = 58.0 * SCALE;
pub const MAX_RECENT: usize = 30;

impl Kb {
    pub fn new(win_w: f32, win_h: f32) -> Kb {
        let mut k = Kb { layout: 0, layouts: vec![0, 1], mode: Mode::Standard, shift: 0, caps: 0, altgr: 0, focus: 0, cat: CAT_STEAM + 1, scroll: 0, recent: vec![], win_w, win_h, keep_x: 0.0, tints: Default::default(), ext: None };
        k.focus_initial();
        k
    }

    pub fn panel_top(&self) -> f32 {
        self.win_h - self.panel_h()
    }

    pub fn panel_h(&self) -> f32 {
        if self.mode == Mode::Emoji { EMOJI_PANEL_H } else { PANEL_H }
    }

    /// Начальный фокус: ряд 2, колонка 5 (`s_initialFocusRow/Column`) — клавиша «g».
    pub fn focus_initial(&mut self) {
        let keys = self.keys();
        self.focus = keys.iter().position(|k| k.row == 2 && k.col == 5).unwrap_or(0);
        if self.mode == Mode::Emoji {
            self.focus = keys.iter().position(|k| matches!(k.def.act, Act::Emoji(_))).unwrap_or(0);
        }
        self.keep_x = keys.get(self.focus).map(|k| k.x + k.w / 2.0).unwrap_or(0.0);
    }

    pub fn shift_on(&self) -> bool {
        self.shift != 0
    }
    pub fn upper(&self) -> bool {
        (self.shift != 0) ^ (self.caps != 0)
    }

    /// Все клавиши текущего режима с геометрией.
    pub fn keys(&self) -> Cow<'static, [PKey]> {
        let layout_key = self.layouts.len() > 1;
        match self.mode {
            Mode::Standard if (self.win_w, self.win_h) == (1280.0, 800.0) => {
                let index = 2 * usize::from(self.layout == 1) + usize::from(layout_key);
                Cow::Borrowed(&DECK_LAYOUTS[index])
            }
            Mode::Standard => Cow::Owned(standard_keys(self.layout, layout_key, self.win_w, self.win_h)),
            Mode::Emoji => Cow::Owned(self.emoji_keys(self.win_h - PANEL_H + PAD_T, PAD_L, self.win_w - PAD_L - PAD_R)),
        }
    }

    fn emoji_keys(&self, top: f32, inner_x: f32, inner_w: f32) -> Vec<PKey> {
        let mut out = vec![];
        // вкладки: слоты равной ширины между глифами L1/R1; активная подсвечивается в рендере.
        let tabs_x = 143.0 - HIT.3;
        let tabs_w = 1137.0 - 143.0 + HIT.3;
        let slot = tabs_w / TAB_SLOTS as f32;
        let ty = top + 14.0;
        for s in 0..TAB_SLOTS {
            let (act, icon) = match s {
                0..=3 => (Act::Cat(s), EMOJI_TABS[s].1),
                4 => (Act::None, ""),
                5..=13 => (Act::Cat(CAT_STEAM + (s - 5)), if s == 13 { "Flags" } else { EMOJI_TABS[s].1 }),
                _ => unreachable!(),
            };
            let mut d = KeyDef::new(act, if s == 4 { Kind::Spacer } else { Kind::Tab2 });
            if !icon.is_empty() {
                d.icon = Some(icon);
            }
            out.push(PKey { def: d, row: 0, col: s, x: tabs_x + slot * s as f32, y: ty, w: slot, h: ROW_H, kx: tabs_x + slot * s as f32 + HIT.3, ky: ty, kw: slot - HIT.3 - 1.0, kh: 44.0 * SCALE });
        }
        // сетка эмодзи: 3 строки, колонка 58 css
        let (a, b, total): (usize, usize, usize) = if self.cat <= 3 {
            (0, 0, 0)
        } else if self.cat == CAT_STEAM {
            (0, self.recent.len(), self.recent.len())
        } else {
            let (a, b) = emoji_range(self.cat);
            (a, b, b - a)
        };
        let _ = total;
        let vis_cols = ((self.win_w / EMOJI_COL_W).ceil() as usize) + 1;
        let gy = top + 14.0 + ROW_H;
        if self.cat > 3 {
            let n = b - a;
            let cols = n.div_ceil(EMOJI_ROWS);
            for c in self.scroll..(self.scroll + vis_cols).min(cols) {
                for r in 0..EMOJI_ROWS {
                    let i = c * EMOJI_ROWS + r;
                    if i >= n {
                        continue;
                    }
                    let idx = if self.cat == CAT_STEAM { self.recent[i] } else { a + i };
                    let x = 2.0 + (c - self.scroll) as f32 * EMOJI_COL_W;
                    let y = gy + r as f32 * (44.0 * SCALE + 0.0);
                    let mut d = KeyDef::new(Act::Emoji(idx), Kind::Emoji);
                    d.base = self.emoji_text(idx);
                    out.push(PKey { def: d, row: 1 + r, col: c, x, y, w: EMOJI_COL_W, h: 44.0 * SCALE - 0.6, kx: x, ky: y, kw: EMOJI_COL_W - 3.0, kh: 44.0 * SCALE - 0.6 });
                }
            }
        }
        // нижний ряд (s_rgSteamItemsBottomRow): ABC, пробел, Backspace, Поиск, стрелки, скрыть
        let by = top + 4.0 * ROW_H;
        let mut abc = KeyDef::tok(Act::ToAbc, Kind::Meta, "", "ABC");
        abc.base = "ABC".into();
        let mut bs = backspace();
        bs.kind = Kind::Enter; // 11.7 %
        let mut ent = enter();
        ent.kind = Kind::Enter;
        let mut close = KeyDef::new(Act::Hide, Kind::Close);
        close.icon = Some("HideKeyboard");
        let defs = vec![
            abc,
            space(),
            bs,
            ent,
            arrows((Act::Left, "KaratLeft"), (Act::Up, "KaratUp")),
            arrows((Act::Right, "KaratRight"), (Act::Down, "KaratDown")),
            close,
        ];
        for mut p in place_row(defs, 4, by, inner_x, inner_w) {
            // и Backspace, и Поиск — чёрные с глифами слева (в эмодзи-режиме у Backspace метка слева направо)
            if p.def.act == Act::Backspace {
                p.def.kind = Kind::Backspace;
            }
            if p.def.act == Act::Enter {
                p.def.kind = Kind::Enter;
            }
            out.push(p);
        }
        out
    }

    /// Текст эмодзи с сохранённым оттенком (оттенки только у `nNumTints == 1`).
    pub fn emoji_text(&self, idx: usize) -> String {
        let (key, n) = &emoji_list().0[idx];
        if *n == 1 { tinted(key, self.tints.get(&idx).copied().unwrap_or(0)) } else { key.clone() }
    }

    /// Варианты долгого нажатия для клавиши (sp.js:409130): активный символ, Shift/AltGr-символы клавиши и
    /// `s_rgExtendedKeys`; у эмодзи с оттенками — 5 оттенков и вариант по умолчанию. None — долгого нажатия нет.
    pub fn ext_items(&self, k: &PKey) -> Option<Vec<(String, Option<u8>)>> {
        let d = &k.def;
        if let Act::Emoji(i) = d.act {
            let (key, n) = &emoji_list().0[i];
            if *n != 1 {
                return None;
            }
            let mut v: Vec<(String, Option<u8>)> = (1..=5).map(|t| (tinted(key, t), Some(t))).collect();
            if self.ext_right(k) { v.insert(0, (tinted(key, 0), Some(0))) } else { v.push((tinted(key, 0), Some(0))) }
            return Some(v);
        }
        if !matches!(d.kind, Kind::Char | Kind::Half) {
            return None;
        }
        let Act::Char(base) = &d.act else { return None };
        let active = if self.shift_on() { d.shift.clone().unwrap_or_else(|| base.clone()) } else { base.clone() };
        if active.chars().count() != 1 || active == " " {
            return None;
        }
        let mut all = active.clone();
        for x in [&d.shift, &d.alt].into_iter().flatten() {
            all.push_str(x);
        }
        all.push_str(extended_table(&active));
        let mut v: Vec<(String, Option<u8>)> = vec![];
        for c in all.chars() {
            let c = if self.upper() && d.shift.is_none() { c.to_uppercase().to_string() } else { c.to_string() };
            if !v.iter().any(|(x, _)| *x == c) {
                v.push((c, None));
            }
        }
        if !self.ext_right(k) {
            v.reverse();
        }
        Some(v)
    }

    /// Ряд растёт вправо у клавиш левой половины ряда (`nKey < длина/2`), у эмодзи — левее середины экрана.
    fn ext_right(&self, k: &PKey) -> bool {
        if matches!(k.def.act, Act::Emoji(_)) {
            return k.kx < self.win_w / 2.0;
        }
        let n = self.keys().iter().filter(|x| x.row == k.row).count();
        (k.col as f32) < n as f32 / 2.0
    }

    /// Открыть ряд над клавишей `i` (истёк таймер долгого нажатия). Выбран символ у клавиши (`autoFocus`).
    /// `half` — для тачпада: ряд не выходит за его половину (`ExtendedRowStyleForSlot`).
    pub fn open_ext(&mut self, i: usize, half: Option<bool>) -> bool {
        let keys = self.keys();
        let Some(k) = keys.get(i) else { return false };
        let Some(items) = self.ext_items(k) else { return false };
        let right = self.ext_right(k);
        let w = items.len() as f32 * EXT_W;
        let mut x = if right { k.kx + 5.0 * SCALE } else { k.kx + k.kw - 5.0 * SCALE - w };
        let (lo, hi) = match half {
            Some(false) => (0.0, self.win_w / 2.0),
            Some(true) => (self.win_w / 2.0, self.win_w),
            None => (0.0, self.win_w),
        };
        x = x.min(hi - w).max(lo);
        // верхний ряд раскладки — ряд под клавишей (`TopRow`: top 44px)
        let y = if k.row == 0 || (self.mode == Mode::Emoji && k.row == 1) { k.ky + EXT_H } else { k.ky - EXT_H };
        let sel = Some(if right { 0 } else { items.len() - 1 });
        self.ext = Some(ExtRow { parent: i, items, sel, x, y });
        true
    }

    /// D-pad/стик в открытом ряду: только влево/вправо, остальное поглощается (`onMove* → true`).
    pub fn ext_nav(&mut self, dir: u8) {
        if let Some(r) = self.ext.as_mut() {
            let n = r.items.len();
            let cur = r.sel.unwrap_or(0);
            r.sel = Some(match dir {
                2 => cur.saturating_sub(1),
                3 => (cur + 1).min(n - 1),
                _ => cur,
            });
        }
    }

    /// Набор выбранного в ряду символа: эмодзи — запомнить оттенок и в «Недавние».
    pub fn press_ext(&mut self, j: usize) -> Out {
        let Some(row) = self.ext.take() else { return Out::None };
        let Some((text, tint)) = row.items.get(j).cloned() else { return Out::None };
        if let Some(t) = tint {
            if let Some(Act::Emoji(i)) = self.keys().get(row.parent).map(|k| k.def.act.clone()) {
                self.tints.insert(i, t);
                self.recent.retain(|x| *x != i);
                self.recent.insert(0, i);
                self.recent.truncate(MAX_RECENT);
            }
        }
        self.after_key();
        Out::Text(text)
    }

    pub fn layout_name(&self) -> (&'static str, &'static str) {
        let (_, tok, fb) = LAYOUT_NAMES[self.layout.min(LAYOUT_NAMES.len() - 1)];
        (tok, i18n::tr_ref(fb))
    }

    /// Текст, который будет набран клавишей `k` в текущем состоянии (None — не символ).
    pub fn typed(&self, k: &KeyDef) -> Option<String> {
        match &k.act {
            Act::Char(c) => {
                if self.shift_on() {
                    if let Some(s) = &k.shift {
                        return Some(s.clone());
                    }
                }
                if self.altgr != 0 {
                    if let Some(a) = &k.alt {
                        return Some(a.clone());
                    }
                }
                Some(if self.upper() { c.to_uppercase() } else { c.clone() })
            }
            Act::Emoji(i) => Some(self.emoji_text(*i)),
            Act::Space => Some(" ".into()),
            _ => None,
        }
    }

    /// После набранного символа: однократные Shift/AltGr сбрасываются (`Xe`).
    fn after_key(&mut self) {
        if self.shift == 1 {
            self.shift = 0;
        }
        if self.altgr == 1 {
            self.altgr = 0;
        }
    }

    /// Нажатие клавиши (touch-up / A / клик тачпада). Возвращает действие для поля ввода.
    pub fn press(&mut self, k: &KeyDef) -> Out {
        let act = if self.shift_on() && k.act2.is_some() { k.act2.clone().unwrap() } else { k.act.clone() };
        let out = match &act {
            Act::Char(_) | Act::Space | Act::Emoji(_) => {
                let t = self.typed(k).unwrap_or_default();
                if let Act::Emoji(i) = act {
                    self.recent.retain(|x| *x != i);
                    self.recent.insert(0, i);
                    self.recent.truncate(MAX_RECENT);
                }
                Out::Text(t)
            }
            Act::Backspace => Out::Backspace,
            Act::Tab => Out::Tab,
            Act::Enter => Out::Enter,
            Act::Left => Out::Left,
            Act::Right => Out::Right,
            Act::Up => Out::Up,
            Act::Down => Out::Down,
            Act::Paste => Out::Paste,
            Act::Hide => Out::Hide,
            Act::Shift => {
                self.shift = match self.shift {
                    0 => 1,
                    1 => 2,
                    _ => 0,
                };
                return Out::None;
            }
            Act::Caps => {
                self.caps = if self.caps == 0 { 2 } else { 0 };
                return Out::None;
            }
            Act::AltGr => {
                self.altgr = match self.altgr {
                    0 => 1,
                    1 => 2,
                    _ => 0,
                };
                return Out::None;
            }
            Act::ToEmoji => {
                self.mode = Mode::Emoji;
                self.cat = CAT_STEAM + if self.recent.is_empty() { 1 } else { 0 };
                self.scroll = 0;
                self.focus_initial();
                return Out::None;
            }
            Act::ToAbc => {
                self.mode = Mode::Standard;
                self.focus_initial();
                return Out::None;
            }
            Act::ToLayout => {
                if self.layouts.len() > 1 {
                    let p = self.layouts.iter().position(|l| *l == self.layout).unwrap_or(0);
                    self.layout = self.layouts[(p + 1) % self.layouts.len()];
                }
                return Out::None;
            }
            Act::Cat(c) => {
                self.cat = *c;
                self.scroll = 0;
                return Out::None;
            }
            Act::None => return Out::None,
        };
        self.after_key();
        out
    }

    /// L1/R1 в режиме эмодзи: соседняя категория (по кругу в пределах вкладок).
    pub fn cat_step(&mut self, d: i32) {
        if self.mode != Mode::Emoji {
            return;
        }
        let order: Vec<usize> = (0..4).chain(CAT_STEAM..CAT_STEAM + 9).collect();
        let p = order.iter().position(|c| *c == self.cat).unwrap_or(0) as i32;
        self.cat = order[(p + d).rem_euclid(order.len() as i32) as usize];
        self.scroll = 0;
        self.focus_initial();
    }

    /// Навигация фокуса D-pad/стиками (0 вверх, 1 вниз, 2 влево, 3 вправо). false — граница (выход вверх).
    pub fn nav(&mut self, dir: u8) -> bool {
        let keys = self.keys();
        if keys.is_empty() {
            return false;
        }
        let f = self.focus.min(keys.len() - 1);
        let cur = &keys[f];
        let (cx, cy) = (cur.kx + cur.kw / 2.0, cur.ky + cur.kh / 2.0);
        let focusable = |k: &PKey| !matches!(k.def.act, Act::None);
        let mut best: Option<(f32, usize)> = None;
        for (i, k) in keys.iter().enumerate() {
            if i == f || !focusable(k) {
                continue;
            }
            let (x, y) = (k.kx + k.kw / 2.0, k.ky + k.kh / 2.0);
            let ok = match dir {
                0 => y < cy - 2.0,
                1 => y > cy + 2.0,
                2 => x < cx - 2.0 && (y - cy).abs() < cur.kh * 0.6,
                _ => x > cx + 2.0 && (y - cy).abs() < cur.kh * 0.6,
            };
            if !ok {
                continue;
            }
            let ax = if dir < 2 { self.keep_x } else { cx };
            // вверх/вниз: ближайший ряд, затем ближайший по x (MAINTAIN_X: ключ, чья ширина накрывает keep_x)
            let d = if dir < 2 {
                let covers = (k.kx..=k.kx + k.kw).contains(&ax);
                (y - cy).abs() * 10.0 + if covers { 0.0 } else { (x - ax).abs() + 5.0 }
            } else {
                (x - cx).abs() * 10.0 + (y - cy).abs()
            };
            if best.map(|b| d < b.0).unwrap_or(true) {
                best = Some((d, i));
            }
        }
        if let Some((_, i)) = best {
            self.focus = i;
            if dir >= 2 {
                self.keep_x = keys[i].kx + keys[i].kw / 2.0;
            }
            return true;
        }
        // в режиме эмодзи упор в край сетки — прокрутка
        if self.mode == Mode::Emoji && dir >= 2 && matches!(cur.def.act, Act::Emoji(_)) {
            let total_cols = self.emoji_cols();
            if dir == 3 && self.scroll + 1 < total_cols {
                self.scroll += 1;
            } else if dir == 2 && self.scroll > 0 {
                self.scroll -= 1;
            }
            let keys2 = self.keys();
            if let Some(i) = keys2.iter().position(|k| k.row == cur.row && matches!(k.def.act, Act::Emoji(_)) && ((dir == 3 && k.col == cur.col + 1) || (dir == 2 && k.col + 1 == cur.col))) {
                self.focus = i;
            }
            return true;
        }
        false
    }

    /// Прокрутка эмодзи-сетки пальцем: первая видимая колонка в пределах сетки (последняя колонка остаётся у правого края).
    pub fn set_scroll(&mut self, col: i64) {
        let vis = (self.win_w / EMOJI_COL_W).floor() as i64;
        let max = (self.emoji_cols() as i64 - vis).max(0);
        self.scroll = col.clamp(0, max) as usize;
    }

    pub fn emoji_cols(&self) -> usize {
        if self.cat <= 3 {
            return 0;
        }
        let n = if self.cat == CAT_STEAM {
            self.recent.len()
        } else {
            let (a, b) = emoji_range(self.cat);
            b - a
        };
        n.div_ceil(EMOJI_ROWS)
    }

    /// Ключ под точкой (координаты окна): по области нажатия.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.keys().iter().position(|k| x >= k.x && x < k.x + k.w && y >= k.y && y < k.y + k.h && !matches!(k.def.act, Act::None))
    }
}

/// Тачпады: касание (-1..1, y вверх) → точка окна. Левый тачпад — левая половина панели, правый — правая.
pub fn pad_point(kb: &Kb, right: bool, x: f32, y: f32, scale: f32) -> (f32, f32) {
    let top = kb.panel_top();
    let (x0, x1) = if right { (kb.win_w / 2.0, kb.win_w) } else { (0.0, kb.win_w / 2.0) };
    let ux = 0.5 * (1.0 + (x * scale).clamp(-1.0, 1.0));
    let uy = 0.5 * (1.0 - (y * scale).clamp(-1.0, 1.0));
    // Keep saturated coordinates inside their half of the panel.
    ((x0 + (x1 - x0) * ux).min(x1 - 0.01), (top + kb.panel_h() * uy).min(kb.win_h - 0.01))
}

#[cfg(test)]
#[path = "osk_model_tests.rs"]
mod tests;
