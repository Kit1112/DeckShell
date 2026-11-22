//! Страница «Настройки Как джойстик» правого трекпада (кадры владельца 21.11, controls/05–11).
//! Таблица настроек — режим 12 (`mouse_joystick`) из таблицы режимов стока (chunk~2dcc5aaf7.js, ключ `12:[…]`):
//! ключи, порядок (`sortOrder`), разделы, зависимости. Значения и пределы читаются из редактируемой копии раскладки
//! (`group.settings`: ключ, значение, min, max — знаковый zigzag), запись — тем же `SetEditingControllerConfigurationSourceMode`.
//! Геометрия строк — устройство 1280×800 (кадры 1:1), выведена из измерений кадров.
use super::controller_live::{msg, number};
use super::controller_sticks as sticks;
use crate::assets::t_or;
use steam_bridge::controller_edit::BindingTarget;

/// Режим группы, для которого есть страница (`mouse_joystick`, «Как джойстик»).
pub const MODE: i64 = 12;
pub const SECTIONS: [(&str, &str); 5] = [
    ("#ControllerConfigurator_Section_Basic", "Основное"),
    ("#ControllerConfigurator_Section_Output", "Вывод"),
    ("#ControllerConfigurator_Section_Trackpad", "Трекпад"),
    ("#ControllerConfigurator_Section_TriggerDampening", "Замедление триггером"),
    ("#ControllerConfigurator_Section_Haptic", "Тактильная отдача"),
];
fn section_name(i: usize) -> String { let (t, f) = SECTIONS[i.min(4)]; t_or(t, f) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Slider, Toggle, Choice }
/// (значение, `ControllerSettingValue_<id>`, запасной текст)
type Opt = (i64, &'static str, &'static str);
const FRICTION: &[Opt] = &[(0, "Off", "Выкл."), (1, "Low", "Низк."), (2, "Medium", "Средн."), (3, "High", "Высок."), (4, "None", "Отсутствует")];
const HAPTIC: &[Opt] = &[(0, "Off", "Выкл."), (1, "Low", "Низк."), (2, "Medium", "Средн."), (3, "High", "Высок.")];
const OUTPUT: &[Opt] = &[(0, "LeftJoystick", "Левый джойстик"), (1, "RightJoystick", "Правый джойстик")];
const CLAMP: &[Opt] = &[
    (0, "Off", "Выкл."), (1, "TriggerDampeningRightSoft", "Замедление при плавном нажатии правого триггера"),
    (2, "TriggerDampeningLeftSoft", "Замедление при плавном нажатии левого триггера"), (3, "TriggerDampeningBothSoft", "Замедление при плавном нажатии любого триггера"),
    (4, "TriggerDampeningRightAlways", "Замедление при нажатии правого триггера"), (5, "TriggerDampeningLeftAlways", "Замедление при нажатии левого триггера"),
    (6, "TriggerDampeningBothAlways", "Замедление при нажатии любого триггера"),
];
const GYRO_HAPTIC: &[Opt] = &[(0, "Off", "Выкл."), (1, "OnEnable", "При включении гироскопа"), (2, "OnDisable", "При выключении гироскопа"), (3, "OnEnableAndDisable", "При включении/выключении гироскопа")];

struct Def {
    key: i64, id: &'static str, label: &'static str, desc: &'static str, section: usize, order: i32, kind: Kind, opts: &'static [Opt],
    /// Показывать, только если настройка `.0` имеет одно из значений `.1` (`Ie([{eControllerSetting,rgValues}])` стока).
    when: Option<(i64, &'static [i64])>,
}
const DEADZONE_DESC: &str = "Определяет минимальное перемещение джойстика. Идеальная настройка полностью компенсирует встроенную в игру мёртвую зону. Увеличивайте это значение, пока мелкие движения не станут плавными и точными. Снизьте его, если движения станут слишком грубыми. Также попробуйте настройку «Увеличить точность малых движений».";
const fn d(key: i64, id: &'static str, label: &'static str, desc: &'static str, section: usize, order: i32, kind: Kind, opts: &'static [Opt], when: Option<(i64, &'static [i64])>) -> Def {
    Def { key, id, label, desc, section, order, kind, opts, when }
}
const DEFS: &[Def] = &[
    d(31, "VerticalSensitivityMouseJoystick", "Пропорции вертикальной чувствительности", "", 0, 1, Kind::Slider, &[], None),
    d(10, "CustomCurveExponent_mouse_joystick", "Пользовательская кривая", "В разных играх используются разные кривые чувствительности джойстика. Чтобы поведение совпадало с управлением мышью, лучше всего подойдёт линейная кривая. С помощью этой настройки можно компенсировать встроенную в игру кривую чувствительности. Обычно её стоит использовать лишь в крайнем случае, но если вы чувствуете нежелательное ускорение или слабую начальную реакцию на нажатие, передвиньте ползунок вправо. Если же вам необходимо избавиться от обратной акселерации, передвиньте ползунок влево.", 0, 25, Kind::Slider, &[], None),
    d(52, "DeadZonePrecisionJoystickMouse", "Увеличить точность малых движений", "Увеличивает точность небольших движений. Эта настройка должна сочетаться со значением минимального перемещения по горизонтали/вертикали. Если оно будет слишком высоким, то при включении этой настройки управление будет дрожащим даже в состоянии покоя. Если оно будет слишком низким, то небольшие движения будут считываться плохо или не будут работать вовсе. Сочетание этих настроек позволит увеличить точность движений и понизить частоту случайных срабатываний в состоянии покоя.", 0, 44, Kind::Slider, &[], None),
    d(8, "OutputJoystickMouseJoystick", "Отправка на джойстик", "Сигнал будет отправляться на выбранный джойстик.", 1, 0, Kind::Choice, OUTPUT, None),
    d(42, "InvertMouseYMouseJoystick", "Инверсия вертикальной оси", "", 1, 10, Kind::Toggle, &[], None),
    d(41, "InvertXMouseMouseJoystick", "Инверсия горизонтальной оси", "", 1, 11, Kind::Toggle, &[], None),
    d(45, "RotationMouseJoystick", "Компенсация движения по дуге", "Изменяет движение мыши по горизонтали. Проводя пальцем по трекпаду, вы совершаете дуговые движения. Чтобы они получались идеально прямыми, подстройте эту опцию под себя.", 1, 40, Kind::Slider, &[], None),
    d(44, "MouseMoveThresholdMouseJoystick", "Минимальное перемещение", "Курсор начнёт двигаться только после достижения указанной разницы между исходной и текущей позициями. Это полезно при работе с приложениями, фильтрующими или ограничивающими движения мыши, в результате чего мелкие движения могут не распознаваться. В идеале это значение должно быть достаточно маленьким, чтобы не подавлять небольшие движения.", 1, 41, Kind::Slider, &[], None),
    d(50, "DeadzoneXJoystickMouse", "Мин. перемещение по горизонтали", DEADZONE_DESC, 1, 42, Kind::Slider, &[], None),
    d(51, "DeadzoneYJoystickMouse", "Мин. перемещение по вертикали", DEADZONE_DESC, 1, 43, Kind::Slider, &[], None),
    d(33, "TrackballMouseJoystick", "Режим трекбола", "В этом режиме трекпад будет действовать как трекбол, а не как мышь. Появятся инерция и трение, характерные для трекбола.", 2, 30, Kind::Toggle, &[], None),
    d(34, "FrictionMouseJoystick", "Трение трекбола", "", 2, 31, Kind::Choice, FRICTION, Some((33, &[1]))),
    d(35, "VerticalFrictionMouseJoystick", "Чувствительность вертикального трения", "", 2, 32, Kind::Slider, &[], Some((33, &[1]))),
    d(48, "DoubleTapDurationMouseJoystick", "Длительность двойного касания", "", 2, 50, Kind::Slider, &[], None),
    d(38, "DoubleTapBeepMouseJoystick", "Сигнал двойного касания", "", 2, 51, Kind::Toggle, &[], None),
    d(40, "EdgeSpinScaleMouseJoystick", "Постоянная скорость", "Определяет, с какой скоростью будет двигаться указатель мыши при нажатии за пределами указанной зоны на трекпаде.", 2, 81, Kind::Slider, &[], None),
    d(39, "EdgeSpinRadiusMouseJoystick", "Радиус постоянной скорости", "Радиус зоны постоянной скорости. Радиус представлен слайдером, полное смещение которого влево означает отсутствие зоны, а полное смещение вправо — границы зоны на краях трекпада.", 2, 82, Kind::Slider, &[], None),
    d(78, "MouseTriggerClampMouseJoystick", "Замедление мыши при нажатии триггера", "Движения мыши замедляются во время нажатия триггера, что позволяет избежать случайных движений.", 3, 60, Kind::Choice, CLAMP, None),
    d(79, "MouseTriggerClampAmountMouseJoystick", "Сила замедления при нажатии триггера", "Определяет, насколько сильно замедляются движения мыши. Чем правее ползунок, тем сильнее замедление.", 3, 61, Kind::Slider, &[], Some((78, &[1, 2, 3, 4, 5, 6]))),
    d(3, "HapticIntensityMouseJoystick", "Уровень тактильной отдачи", "", 4, 100, Kind::Choice, HAPTIC, None),
    d(158, "GyroButtonHapticEffect", "Триггеры тактильной отдачи при активации гироскопа", "Выберите, когда должна срабатывать тактильная отдача: при включении гироскопа, его выключении или в обоих случаях. Сила отдачи определяется параметром «Уровень тактильной отдачи».", 4, 101, Kind::Choice, GYRO_HAPTIC, None),
];

/// Одна строка страницы: либо заголовок раздела (`heading`), либо настройка.
#[derive(Clone, Debug, PartialEq)]
pub struct PRow {
    pub key: i64, pub kind: Kind, pub heading: bool, pub section: usize,
    pub label: String, pub desc: String, pub value: i64, pub min: i64, pub max: i64, pub text: String,
    options: Vec<(i64, String)>,
    display_range: Option<[f64; 2]>,
    mask: bool,
    pub y: f32, pub h: f32, pub label_lines: usize, pub desc_lines: usize,
    /// Подпись и описание, уже разбитые на строки (шаг строк 30 / 24 px, как в стоке).
    pub label_text: Vec<String>, pub desc_text: Vec<String>,
}
impl PRow {
    pub fn focusable(&self) -> bool { !self.heading }
    pub fn fraction(&self) -> f32 { if self.max > self.min { ((self.value - self.min) as f32 / (self.max - self.min) as f32).clamp(0., 1.) } else { 0. } }
    pub fn on(&self) -> bool { self.value != 0 }
    /// Шаг ползунка: обычный — 1/40 диапазона, «плавный сдвиг» (Y) — единица.
    pub fn step(&self, dir: i64, fine: bool) -> i64 {
        let unit = if fine { 1 } else { ((self.max - self.min) / 40).max(1) };
        (self.value + dir * unit).clamp(self.min, self.max)
    }
    pub fn options(&self) -> &[(i64, String)] { &self.options }
    pub fn uses_mask(&self) -> bool { self.mask }

    fn display_value(&self) -> f64 {
        let Some([lo, hi]) = self.display_range else { return self.value as f64 };
        if self.min == self.max { return lo; }
        lo + (hi - lo) * (self.value - self.min) as f64 / (self.max - self.min) as f64
    }

    pub fn numeric_text(&self) -> String {
        if self.display_range.is_none() { return self.value.to_string(); }
        let text = format!("{:.3}", self.display_value());
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }

    pub fn slider_text_at(&self, value: i64) -> String {
        let numeric = self.numeric_text();
        let suffix = self.text.strip_prefix(&numeric).unwrap_or_default();
        let mut pending = self.clone();
        pending.value = value;
        format!("{}{suffix}", pending.numeric_text())
    }

    pub fn parse_numeric(&self, text: &str) -> Option<i64> {
        let Some([lo, hi]) = self.display_range else {
            let value: i64 = text.trim().parse().ok()?;
            return (self.min..=self.max).contains(&value).then_some(value);
        };
        let value: f64 = text.trim().replace(',', ".").parse().ok()?;
        if !(lo.min(hi)..=lo.max(hi)).contains(&value) { return None; }
        if lo == hi { return Some(self.value); }
        Some(self.min + ((value - lo) / (hi - lo) * (self.max - self.min) as f64).round() as i64)
    }
}
fn signed(raw: &[u8], field: u32) -> Option<i64> { let n = number(raw, field).ok()?; Some((n >> 1) ^ -(n & 1)) }

/// Ширина символа и межстрочный шаг, подобраны по кадрам (описание 18 px, подпись 24 px).
const DESC_CHAR: f32 = 10.0;
const LABEL_CHAR: f32 = 15.0;
const ROW_W: f32 = 849.0;
pub fn wrap(text: &str, width: f32, char_w: f32) -> Vec<String> {
    let max = (width / char_w).floor().max(1.) as usize;
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(cur) if cur.chars().count() + 1 + word.chars().count() <= max => { cur.push(' '); cur.push_str(word); }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}
pub fn wrap_lines(text: &str, width: f32, char_w: f32) -> usize { wrap(text, width, char_w).len() }

/// Ширина выпадающего поля: 375 px, у длинных значений — 406 (кадр 11: «При включении/выключении…»).
pub fn dropdown_width(text: &str) -> f32 { if text.chars().count() > 20 { 406. } else { 375. } }

fn locate(key: i64, id: &str, fallback_label: &str, fallback_desc: &str) -> (String, String) {
    let _ = key;
    // Описание показывается только там, где в таблице стока `showDesc` (у нас — непустой запасной текст).
    let desc = if fallback_desc.is_empty() { String::new() } else { t_or(&format!("#ControllerBinding_{id}_Description"), fallback_desc) };
    (t_or(&format!("#ControllerBinding_{id}"), fallback_label), desc)
}

#[derive(serde::Deserialize)]
struct StockRow {
    key: i64,
    id: String,
    label: String,
    #[serde(default)]
    description: String,
    section: String,
    section_label: String,
    order: i32,
    sources: Option<u32>,
    #[serde(default)]
    conditions: Vec<(i64, Vec<i64>)>,
    #[serde(default)]
    inverted: bool,
    #[serde(default)]
    toggle: bool,
    #[serde(default)]
    mask: bool,
    #[serde(default)]
    options: Vec<(i64, String, String)>,
    range: Option<[f64; 2]>,
    suffix: Option<(String, String)>,
}

static STOCK: std::sync::LazyLock<std::collections::BTreeMap<i64, Vec<StockRow>>> =
    std::sync::LazyLock::new(|| serde_json::from_str(include_str!("controller_mode_settings.json"))
        .expect("generated Steam controller settings"));

// Bit positions from Steam's k_eGamepadButtonBitMask for Steam Deck.
fn button_bits() -> impl Iterator<Item = (i64, &'static str)> {
    [(0, "R2"), (1, "L2"), (2, "R1"), (3, "L1"), (4, "Y"), (5, "B"), (6, "X"), (7, "A"),
     (8, "↑"), (9, "→"), (10, "←"), (11, "↓"), (12, "⧉"), (14, "≡"), (15, "L5"), (16, "R5"),
     (17, "Левый трекпад: нажатие"), (18, "Правый трекпад: нажатие"),
     (19, "Левый трекпад: касание"), (20, "Правый трекпад: касание"),
     (22, "L3"), (24, "R2 (мягкое)"), (25, "L2 (мягкое)"), (26, "R3"),
     (30, "Левый джойстик: отклонение"), (31, "Правый джойстик: отклонение"),
     (41, "L4"), (42, "R4"), (46, "Левый джойстик: касание"), (47, "Правый джойстик: касание")]
        .into_iter().map(|(bit, label)| (1_i64 << bit, label))
}

pub const SHIFT_BUTTONS: i64 = -2;

fn shift_buttons() -> impl Iterator<Item = (i64, &'static str)> {
    [(0, "R2"), (1, "L2"), (2, "Y"), (3, "B"), (4, "X"), (5, "A"), (6, "R1"), (7, "L1"),
     (8, "L3"), (9, "≡"), (11, "⧉"), (16, "↑"), (17, "→"), (18, "←"), (19, "↓"),
     (20, "L5"), (21, "R5"), (42, "Левый трекпад: нажатие"), (43, "Правый трекпад: нажатие"),
     (45, "L2 (мягкое)"), (46, "R2 (мягкое)"), (47, "L4"), (48, "R4"), (63, "R3"),
     (64, "Левый трекпад: касание"), (65, "Правый трекпад: касание")].into_iter()
}

pub fn read_shift_buttons(raw: &[u8], target: &BindingTarget) -> steam_bridge::Result<Vec<i64>> {
    for mode in msg(raw, 9)? {
        let reference = steam_bridge::controller_layout::fields(mode)?.into_iter().find(|field| field.number == 10);
        let id = if let Some(steam_bridge::controller_layout::Field { wire: steam_bridge::controller_layout::Wire::Varint(id), .. }) = reference {
            id as i64
        } else { number(mode, 2)? };
        let source = number(mode, 7)?;
        if id == target.modeid && (source == 0 || source == target.source_binding_key) {
            let buttons = steam_bridge::controller_layout::repeated_enums(mode, 11)?;
            if !buttons.is_empty() { return Ok(buttons); }
        }
    }
    Ok(Vec::new())
}

fn heading(section: usize, label: String) -> PRow {
    PRow { key: -1, kind: Kind::Toggle, heading: true, section, label, desc: String::new(),
        value: 0, min: 0, max: 0, text: String::new(), options: vec![], display_range: None,
        mask: false, y: 0., h: 0., label_lines: 1, desc_lines: 0, label_text: vec![], desc_text: vec![] }
}

fn parameter(section: usize, key: i64, label: String, desc: String, kind: Kind,
    (value, min, max): (i64, i64, i64), options: Vec<(i64, String)>) -> PRow {
    PRow { key, kind, heading: false, section, label, desc, value, min: min.min(max), max: min.max(max),
        text: String::new(), options, display_range: None, mask: false, y: 0., h: 0.,
        label_lines: 1, desc_lines: 0, label_text: vec![], desc_text: vec![] }
}

fn set_text(row: &mut PRow, suffix: &str) {
    row.text = match row.kind {
        Kind::Slider => format!("{}{suffix}", row.numeric_text()),
        Kind::Toggle => String::new(),
        Kind::Choice if row.mask => {
            let labels = button_bits().filter(|(bit, _)| row.value & bit != 0).map(|(_, label)| label).collect::<Vec<_>>();
            if row.value == 0 { "Не выбрано".into() }
            else if labels.is_empty() { format!("{:#x}", row.value) }
            else { labels.join(" + ") }
        }
        Kind::Choice => row.options.iter().find(|o| o.0 == row.value).map(|o| o.1.clone()).unwrap_or_else(|| row.value.to_string()),
    };
}

/// Stock metadata supplies labels, dependencies and options. Values and limits come from Steam.
pub fn rows(raw: &[u8], t: &BindingTarget) -> Vec<PRow> {
    let Ok(group) = sticks::group(raw, t) else { return vec![] };
    let Ok(list) = msg(group, 4) else { return vec![] };
    let setting = |key: i64| list.iter().copied().find(|s| number(s, 1).ok() == Some(key));
    let get = |key: i64| setting(key).and_then(|s| Some((signed(s, 2)?, signed(s, 3)?, signed(s, 4)?)));
    let mut out = Vec::new();
    if t.source_mode == MODE && matches!(t.source_binding_key, 1 | 2 | 11) {
        let mut defs: Vec<_> = DEFS.iter().collect();
        defs.sort_by_key(|d| (d.section, d.order));
        for def in defs {
            let Some(values) = get(def.key) else { continue };
            if def.when.is_some_and(|(dep, vals)| !get(dep).is_some_and(|v| vals.contains(&v.0))) { continue; }
            let (label, desc) = locate(def.key, def.id, def.label, def.desc);
            let options = def.opts.iter().map(|o| (o.0, t_or(&format!("#ControllerSettingValue_{}", o.1), o.2))).collect();
            let mut row = parameter(def.section, def.key, label, desc, def.kind, values, options);
            set_text(&mut row, "");
            if out.last().is_none_or(|p: &PRow| p.section != def.section) {
                out.push(heading(def.section, section_name(def.section)));
            }
            out.push(row);
        }
    } else if let Some(defs) = STOCK.get(&t.source_mode) {
        let mut defs: Vec<_> = defs.iter().collect();
        defs.sort_by_key(|d| d.order);
        let mut sections: Vec<(&str, Vec<PRow>)> = Vec::new();
        for def in defs {
            let Some(mut values) = get(def.key) else { continue };
            if def.sources.is_some_and(|mask| 1_u32.checked_shl(t.source_binding_key as u32).is_none_or(|bit| mask & bit == 0)) { continue; }
            let dependencies_met = def.conditions.iter().all(|(key, options)| get(*key).is_some_and(|v| options.contains(&v.0)));
            if dependencies_met == def.inverted { continue; }
            let options: Vec<_> = if def.mask {
                values.0 = setting(def.key).and_then(|raw| signed(raw, 7)).unwrap_or_default();
                button_bits().map(|(bit, label)| (values.0 ^ bit, format!("{} {label}", if values.0 & bit != 0 { "✓" } else { "○" }))).collect()
            } else {
                def.options.iter().map(|(value, id, label)| (*value, t_or(&format!("#ControllerSettingValue_{id}"), label))).collect()
            };
            let kind = if def.toggle { Kind::Toggle } else if options.is_empty() { Kind::Slider } else { Kind::Choice };
            let section = sections.iter().position(|(id, _)| *id == def.section).unwrap_or(sections.len());
            if section == sections.len() {
                let label = if def.section == "Basic" { section_name(0) } else {
                    t_or(&format!("#ControllerConfigurator_Setting_Section_{}", def.section), &def.section_label)
                };
                sections.push((&def.section, vec![heading(section, label)]));
            }
            let label = t_or(&format!("#ControllerBinding_{}", def.id), &def.label);
            let mut desc = if def.description.is_empty() { String::new() } else {
                t_or(&format!("#ControllerBinding_{}_Description", def.id), &def.description)
            };
            let mut row = parameter(section, def.key, label, String::new(), kind, values, options);
            row.display_range = def.range;
            row.mask = def.mask;
            desc = desc.replace("%1$s", &row.numeric_text()).replace("%1$d", &row.numeric_text());
            row.desc = desc;
            let suffix = def.suffix.as_ref().map(|(token, fallback)| t_or(token, fallback)).unwrap_or_default();
            set_text(&mut row, &suffix);
            sections[section].1.push(row);
        }
        out = sections.into_iter().flat_map(|(_, rows)| rows).collect();
    }
    if t.mode_shift {
        let selected = read_shift_buttons(raw, t).unwrap_or_default();
        let buttons = shift_buttons().map(|(key, label)| {
            (key, format!("{} {label}", if selected.contains(&key) { "✓" } else { "○" }))
        }).collect();
        let mut row = parameter(0, SHIFT_BUTTONS, "Кнопки сменного режима".into(), String::new(), Kind::Choice,
            (-1, 0, 0), buttons);
        let labels = shift_buttons().filter(|(key, _)| selected.contains(key)).map(|(_, label)| label).collect::<Vec<_>>();
        row.text = if labels.is_empty() { "Не выбрано".into() } else { labels.join(" + ") };
        for row in &mut out { row.section += 1; }
        out.splice(0..0, [heading(0, "Сменный режим".into()), row]);
    }
    layout(&mut out);
    out
}

pub fn read_value(raw: &[u8], target: &BindingTarget, key: i64, mask: bool) -> Option<i64> {
    let group = sticks::group(raw, target).ok()?;
    let setting = msg(group, 4).ok()?.into_iter().find(|s| number(s, 1).ok() == Some(key))?;
    signed(setting, if mask { 7 } else { 2 })
}

/// Высоты по кадрам: ползунок 137 (+описание), переключатель 68, список 96; нижний отступ — 29 px от центра последней строки описания,
/// шаг строк 24 (описание) / 30 (подпись списка); между строками 10 px, заголовок раздела занимает 82 px.
pub fn layout(rows: &mut [PRow]) -> f32 {
    let mut y = -1.0f32;
    let mut prev_heading = true;
    for r in rows.iter_mut() {
        if r.heading { r.y = y; r.h = 82.; y += 82.; prev_heading = true; continue; }
        if !prev_heading { y += 10.; }
        prev_heading = false;
        r.y = y;
        let drop_w = dropdown_width(&r.text);
        r.label_text = if r.kind == Kind::Choice { wrap(&r.label, drop_w_label(drop_w), LABEL_CHAR) } else { vec![r.label.clone()] };
        r.label_lines = r.label_text.len().max(1);
        r.desc_text = wrap(&r.desc, match r.kind { Kind::Slider => 800., Kind::Toggle => 735., Kind::Choice => drop_w_label(drop_w) }, DESC_CHAR);
        r.desc_lines = r.desc_text.len();
        let n = r.desc_lines as f32;
        r.h = match r.kind {
            Kind::Slider => if n > 0. { 133. + 24. * (n - 1.) + 29. } else { 137. },
            Kind::Toggle => if n > 0. { 67. + 24. * (n - 1.) + 29. } else { 68. },
            Kind::Choice => {
                let last_label = 34. + 30. * (r.label_lines as f32 - 1.);
                if n > 0. { (last_label + 32. + 24. * (n - 1.) + 29.).max(96.) } else { 96. }
            }
        };
        y += r.h;
    }
    y + 1.
}
fn drop_w_label(drop_w: f32) -> f32 { 853. + (375. - drop_w) - 397. - 18. - 20. }

/// Центр подписи (от верха строки) и первой строки описания.
pub fn label_center(r: &PRow) -> f32 {
    match r.kind { Kind::Slider => 40., Kind::Toggle => 34., Kind::Choice => if r.desc_lines > 0 || r.label_lines > 1 { 34. } else { 46. } }
}
pub fn desc_top(r: &PRow) -> f32 {
    let first = match r.kind { Kind::Slider => 133., Kind::Toggle => 67., Kind::Choice => 34. + 30. * (r.label_lines as f32 - 1.) + 32. };
    first - 12.
}

/// Индексы строк, на которые можно встать.
pub fn focusable(rows: &[PRow]) -> Vec<usize> { rows.iter().enumerate().filter(|(_, r)| r.focusable()).map(|(i, _)| i).collect() }

/// Куда прокрутить, чтобы строка `i` была видна: у первой строки раздела — вместе с заголовком (кадр 10).
pub fn scroll_for(rows: &[PRow], i: usize, current: f32, view_h: f32, content: f32) -> f32 {
    let Some(r) = rows.get(i) else { return 0. };
    let first_in_section = i > 0 && rows[i - 1].heading;
    let (top, bottom) = if first_in_section { (rows[i - 1].y, r.y + r.h) } else { (r.y, r.y + r.h) };
    let mut s = current;
    if first_in_section && (top < s || bottom > s + view_h) { s = top; }
    else if top < s { s = top; }
    else if bottom > s + view_h { s = bottom - view_h; }
    s.clamp(0., (content - view_h).max(0.))
}
/// Раздел, подсвеченный в левом списке: раздел строки в фокусе.
pub fn active_section(rows: &[PRow], focus: usize) -> usize { rows.get(focus).map_or(0, |r| r.section) }
/// Первая строка раздела (для перехода из левого списка).
pub fn first_row_of(rows: &[PRow], section: usize) -> Option<usize> { rows.iter().position(|r| r.section == section && !r.heading) }
/// Разделы, у которых есть строки, — левый список показывает только их.
pub fn present_sections(rows: &[PRow]) -> Vec<usize> { let mut v: Vec<usize> = Vec::new(); for r in rows { if r.heading && !v.contains(&r.section) { v.push(r.section); } } v }

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use steam_bridge::controller_edit::Writer;
    pub(in super::super) fn fixture(trackball: i64, clamp: i64) -> (Vec<u8>, BindingTarget) {
        let mut g = Writer::new();
        g.int(1, MODE); g.int(5, 2);
        for (key, v, min, max) in [(31, 100, 0, 200), (10, 200, 25, 375), (52, 75, 0, 100), (8, 1, 0, 1), (42, 0, 0, 1), (41, 0, 0, 1), (45, 0, -180, 180), (44, 0, 0, 500),
            (50, 10000, 0, 20000), (51, 10000, 0, 20000), (33, trackball, 0, 1), (34, 2, 0, 4), (35, 100, 0, 200), (48, 325, 0, 515), (38, 0, 0, 1), (40, 0, 0, 100), (39, 25000, 0, 32767),
            (78, clamp, 0, 6), (79, 50, 0, 100), (3, 1, 0, 3), (158, 3, 0, 3)] {
            let mut s = Writer::new();
            s.int(1, key);
            for (f, n) in [(2, v), (3, min), (4, max)] { s.int(f, ((n << 1) ^ (n >> 63)) as i64); }
            g.message(4, &s.finish());
        }
        let mut src = Writer::new(); src.int(1, 2); src.message(2, &g.finish());
        let mut set = Writer::new(); set.string(1, "Default"); set.message(6, &src.finish());
        let mut raw = Writer::new(); raw.message(8, &set.finish());
        let raw = raw.finish();
        let t = BindingTarget { action_set_key: "Default".into(), action_set_layer_key: None, source_binding_key: 2, mode_shift: false, input_key: 0, activator_index: 0, binding_index: 0, source_mode: MODE, modeid: 2 };
        (raw, t)
    }
    pub(in super::super) fn other_mode_fixture(source: i64, mode: i64, settings: &[(i64, i64, i64, i64)]) -> Vec<u8> {
        let mut group = Writer::new();
        group.int(1, mode);
        group.int(5, 2);
        for &(key, value, min, max) in settings {
            let mut setting = Writer::new();
            setting.int(1, key);
            for (field, n) in [(2, value), (3, min), (4, max)] { setting.int(field, (n << 1) ^ (n >> 63)); }
            group.message(4, &setting.finish());
        }
        let mut binding = Writer::new();
        binding.int(1, source);
        binding.message(2, &group.finish());
        let mut set = Writer::new();
        set.string(1, "Default");
        set.message(6, &binding.finish());
        let mut config = Writer::new();
        config.message(8, &set.finish());
        config.finish()
    }

    #[test]
    fn generated_modes_use_stock_kinds_and_native_units() {
        use super::super::controller_quick;
        let raw = other_mode_fixture(controller_quick::GYRO, 22, &[(140, 50, 0, 100), (33, 0, 0, 1), (34, 50, 0, 100)]);
        let target = controller_quick::target(&raw, controller_quick::GYRO).unwrap();
        let rows = rows(&raw, &target);
        let sensitivity = row(&rows, 140);
        assert_eq!(sensitivity.numeric_text(), "15");
        assert_eq!(sensitivity.parse_numeric("7,5"), Some(25));
        assert_eq!(sensitivity.parse_numeric("31"), None);
        assert!(!rows.iter().any(|r| r.key == 34));
        for (mode, source, key, kind) in [
            (1, 13, 24, Kind::Choice), (4, 2, 31, Kind::Slider), (9, 2, 55, Kind::Choice),
            (14, 2, 87, Kind::Choice), (23, 10, 42, Kind::Toggle), (24, 10, 154, Kind::Toggle),
        ] {
            let raw = other_mode_fixture(source, mode, &[(key, 0, 0, 100)]);
            let target = controller_quick::target(&raw, source).unwrap();
            assert_eq!(row(&super::rows(&raw, &target), key).kind, kind, "mode {mode}");
        }
    }

    fn row(rows: &[PRow], key: i64) -> &PRow { rows.iter().find(|r| r.key == key && !r.heading).unwrap() }
    #[test]
    fn sections_and_order_follow_the_stock_frames() {
        let (raw, t) = fixture(1, 0);
        let r = rows(&raw, &t);
        let names: Vec<_> = r.iter().filter(|r| r.heading).map(|r| r.label.clone()).collect();
        assert_eq!(names, ["Основное", "Вывод", "Трекпад", "Замедление триггером", "Тактильная отдача"]);
        let keys: Vec<_> = r.iter().filter(|r| !r.heading).map(|r| r.key).collect();
        assert_eq!(keys, [31, 10, 52, 8, 42, 41, 45, 44, 50, 51, 33, 34, 35, 48, 38, 40, 39, 78, 3, 158]);
        assert_eq!(row(&r, 8).text, "Правый джойстик");
        assert_eq!(row(&r, 3).text, "Низк.");
        assert_eq!(row(&r, 34).text, "Средн.");
    }
    #[test]
    fn dependent_rows_follow_trackball_and_trigger_clamp() {
        let (raw, t) = fixture(0, 3);
        let r = rows(&raw, &t);
        assert!(!r.iter().any(|r| r.key == 34 || r.key == 35));
        assert!(r.iter().any(|r| r.key == 79));
        let (raw, t) = fixture(1, 0);
        assert!(!rows(&raw, &t).iter().any(|r| r.key == 79));
    }
    #[test]
    fn heights_match_the_owner_frames() {
        let (raw, t) = fixture(1, 0);
        let r = rows(&raw, &t);
        let h = |k| row(&r, k).h;
        assert_eq!(h(31), 137.0);               // кадр 05: 141…278
        assert_eq!(h(10), 306.0);               // 288…594, 7 строк описания
        assert_eq!(h(33), 120.0);               // кадр 08: 369…489
        assert_eq!(h(34), 96.0);                // 498…594
        assert_eq!(h(38), 68.0);                // 354…422
        assert_eq!(h(8), 119.0);                // кадр 06: 543…662
        assert_eq!(h(78), 173.0);               // кадр 10: 390…563, подпись в две строки
        assert_eq!(h(3), 96.0);
        assert_eq!(row(&r, 10).desc_lines, 7);
        assert_eq!(row(&r, 52).desc_lines, 7);
        assert_eq!(row(&r, 44).desc_lines, 5);
        assert_eq!(row(&r, 78).label_lines, 2);
    }
    #[test]
    fn rows_are_placed_like_the_frames() {
        let (raw, t) = fixture(1, 0);
        let r = rows(&raw, &t);
        // кадр 05: заголовок «Основное» по центру 105, первая строка у 141 (шапка 60 + y)
        assert_eq!(60. + row(&r, 31).y, 141.0);
        assert_eq!(60. + row(&r, 10).y, 288.0);
        // «Вывод» (кадр 06): заголовок центром 508 при строке до 461
        let o = row(&r, 8);
        assert_eq!(o.y - (row(&r, 52).y + row(&r, 52).h), 82.0);
    }
    #[test]
    fn slider_steps_and_clamps() {
        let (raw, t) = fixture(1, 0);
        let r = rows(&raw, &t);
        let s = row(&r, 10);
        assert_eq!(s.step(1, false), 208);
        assert_eq!(s.step(-1, true), 199);
        assert_eq!(row(&r, 31).fraction(), 0.5);
        let z = PRow { value: 200, ..s.clone() };
        assert_eq!(PRow { value: 375, ..z.clone() }.step(1, false), 375);
    }
    #[test]
    fn scroll_keeps_row_visible_and_shows_heading_of_a_new_section() {
        let (raw, t) = fixture(1, 0);
        let r = rows(&raw, &t);
        let content = r.last().map(|l| l.y + l.h).unwrap();
        let i = r.iter().position(|x| x.key == 8).unwrap();
        let s = scroll_for(&r, i, 0., 680., content);
        assert_eq!(s, 0.0_f32.max(r[i - 1].y));
        let last = r.len() - 1;
        let s = scroll_for(&r, last, 0., 680., content);
        assert!(s + 680. >= r[last].y + r[last].h - 0.5);
    }
    #[test]
    fn unsupported_group_has_no_rows() {
        let (raw, mut t) = fixture(1, 0);
        t.modeid = 99;
        assert!(rows(&raw, &t).is_empty());
    }
}
