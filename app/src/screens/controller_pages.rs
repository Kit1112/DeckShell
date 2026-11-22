//! Controller editor pages other than «Кнопки», built from the native editing copy (task 501).
//! Source keys are `k_EControllerSource` (decoded from a real Deck layout, see docs/controller-gaps-2026-11-19.md):
//! 1 left trackpad, 2 right trackpad, 3 left stick, 6/7 left/right trigger, 10 gyro, 12 right stick, 13 D-pad.
//! Rows follow the stock captures `editor-dpad/15b`, `editor-triggers/17`, `editor-trackpads/19`, `editor-joysticks/18`,
//! `editor-gyro/20`, `editor-virtual-menu/21`, `editor-action-sets/22`. Nothing is written from here.
use super::controller_live::{msg, number, string, BindingRow};
use super::controller_sticks as sticks;
use crate::assets::t_or;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Group title, no panel, not focusable.
    Header,
    /// «Функции»: the source mode (dropdown) with optional gear.
    Function,
    /// A binding row (value + gear); `binding` is the editing-copy row when the layout holds a command.
    Input,
    /// Read-only dropdown parameter.
    Param,
    /// Bare button without a row panel (calibrate, add virtual menu, add action set …).
    Button,
    /// Action set name with a gear only.
    SetRow,
}
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub label: String,
    /// Plain name for dialog titles / accessibility (`A`, `Левый бампер`, `Нажатие`).
    pub name: String,
    pub value: String,
    /// Steam controller glyph (`knockout`), empty = none.
    pub glyph: &'static str,
    pub binding: Option<usize>,
    /// Index into `sticks::sticks` for a joystick «Функции» row.
    pub stick: Option<usize>,
    pub source: Option<i64>,
    /// `CMsgControllerInput.key` of an Input row and the source key that owns it (None: found by the layout).
    pub input: Option<i64>,
    pub input_source: Option<i64>,
    pub gear: bool,
    /// Empty command (stock greys the button and gear).
    pub dim: bool,
    /// Bare buttons: fixed width (device px) and absolute x (`None` = right aligned).
    pub width: f32,
    pub x: Option<f32>,
    pub id: Id,
}
/// What activating a non-binding row means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Id { None, Mode, Calibrate, AddVirtualMenu, AddActionSet, GlobalSettings, ActionSet, Analog }

pub const SECTION_TITLES: [&str; 8] = ["Кнопки", "Крестовина", "Триггеры", "Джойстики", "Трекпады", "Гироскопы", "Виртуальное меню", "Наборы действий"];

fn item(kind: Kind, label: &str, value: &str) -> Item {
    Item { kind, label: label.into(), name: label.into(), value: value.into(), glyph: "", binding: None, stick: None, source: None, input: None, input_source: None, gear: false, dim: false, width: 0., x: None, id: Id::None }
}
/// ABXY belong to the face-button source (key 4); bumpers, grips and menu buttons to whichever mode lists them.
fn input_is_face(input: i64) -> bool { matches!(input, 9..=12) }
fn header(label: &str) -> Item { item(Kind::Header, label, "") }

/// Group.mode → (token, fallback) of the stock `B` table (`SourceMode_*`).
pub fn mode_label(mode: i64) -> String {
    let (token, fallback) = match mode {
        0 => ("None", "Не выбрано"), 1 => ("Dpad", "Крестовина"), 2 => ("Buttons", "Кнопки"), 3 => ("FourButtons", "Блок кнопок"),
        4 => ("AbsoluteMouse", "Абсолютная мышь"), 5 => ("RelativeMouse", "Относительная мышь"), 6 => ("JoystickMove", "Джойстик"),
        7 => ("JoystickMouse", "Джойстик-мышь"), 8 => ("JoystickCamera", "Джойстик-камера"), 9 => ("ScrollWheel", "Колесо прокрутки"),
        10 => ("Trigger", "Триггер"), 11 => ("TouchMenu", "Сенсорное меню"), 12 => ("MouseJoystick", "Как джойстик"),
        13 => ("MouseRegion", "Область действия мыши"), 14 => ("RadialMenu", "Радиальное меню"), 15 => ("SingleButton", "Одна кнопка"),
        16 => ("Switches", "Переключатели"), 17 => ("FlickStick", "Flick Stick"),
        18 => ("2DScrollwheel", "Свайп в сторону"), 19 => ("Hotbar", "Панель быстрого доступа"),
        20 => ("Disabled", "Не выбрано"), 22 => ("Gyro_To_Mouse_Title", "Гироскоп как мышь"),
        23 => ("Gyro_To_JoystickCamera_Title", "Гироскоп как джойстик: камера"),
        24 => ("Gyro_To_JoystickDeflection_Title", "Гироскоп как джойстик: отклонение"),
        25 => ("GyroToCameraAngles_Title", "Гироскоп: углы камеры"),
        26 => ("JoystickToCameraAngles_Title", "Джойстик: углы камеры"),
        27 => ("FlickStickToCameraAngles_Title", "Флик-стик: углы камеры"),
        28 => ("TrackPadToCameraAngles_Title", "Трекпад: углы камеры"), _ => return format!("Режим {mode}"),
    };
    t_or(&format!("#ControllerConfigurator_SourceMode_{token}"), fallback)
}
fn mode_token(mode: i64) -> &'static str {
    match mode { 1 => "Dpad", 6 => "JoystickMove", 7 => "JoystickMouse", 10 => "Trigger", 12 => "MouseJoystick", 15 => "SingleButton", _ => "JoystickMove" }
}
fn input_label(mode: i64, input: i64) -> String {
    let (suffix, fallback) = match (mode, input) {
        (10, 13) => ("Click", "Полное нажатие на триггер"), (10, 15) => ("Edge", "Неполное нажатие на триггер"),
        (_, 13) => ("Click", "Нажатие"), (_, 50) => ("Touch", "Касание"), (_, 15) => ("Edge", "Внешнее кольцо"), _ => return String::new(),
    };
    t_or(&format!("#ControllerConfigurator_SourceMode_{}_{suffix}", mode_token(mode)), fallback)
}

/// The first action set (stock edits the active one); layers are not shown on these pages.
/// (mode, modeid) of the first group of source `key` in the displayed action set.
fn source_mode(set: &[u8], key: i64) -> Option<(i64, i64)> {
    for source in msg(set, 6).ok()? {
        if number(source, 1).ok()? != key { continue; }
        let group = msg(source, 2).ok()?.into_iter().next()?;
        return Some((number(group, 1).ok()?, number(group, 5).ok()?));
    }
    None
}
fn binding_for(set: &[u8], rows: &[BindingRow], key: i64, input: i64) -> Option<usize> {
    let set = string(set, 1).ok()?;
    rows.iter().position(|r| r.target.action_set_key == set && r.target.action_set_layer_key.is_none() && r.target.source_binding_key == key
        && r.target.input_key == input && r.target.activator_index == 0 && r.target.binding_index == 0)
}
fn input_row(raw: &[u8], rows: &[BindingRow], key: i64, mode: i64, input: i64, glyph: &'static str, label: Option<String>) -> Item {
    let mut it = item(Kind::Input, &label.unwrap_or_else(|| input_label(mode, input)), "");
    it.name = if it.label.is_empty() { input_label(mode, input) } else { it.label.clone() };
    it.glyph = glyph;
    it.gear = true;
    it.input = Some(input);
    it.input_source = Some(key);
    it.binding = binding_for(raw, rows, key, input);
    match it.binding {
        Some(i) => it.value = super::controller_editor::command_label(&rows[i].binding),
        None => { it.value = t_or("#ControllerConfigurator_AddCommand", "Добавить команду"); it.dim = true; }
    }
    it
}
fn function_row(source: i64, mode: Option<i64>, gear: bool) -> Item {
    // The stock token is "Функции: %1$s"; the capture shows only «Функции» (the mode is the button value).
    let label = t_or("#ControllerConfigurator_SourceMode_Group_Behavior", "Функции");
    let label = label.split(':').next().unwrap_or("Функции").trim().to_string();
    let mut it = item(Kind::Function, &label, &super::controller_quick::mode_name(source, mode.unwrap_or(0)));
    it.gear = gear;
    it.source = Some(source);
    if !gear { it.width = 375.; }
    it.id = Id::Mode;
    it
}
pub(super) fn source_title(key: i64) -> String {
    let (token, fallback) = match key {
        1 => ("LeftTrackpad", "Левый трекпад"), 2 => ("RightTrackpad", "Правый трекпад"), 3 => ("Joystick", "Левый джойстик"), 4 => ("FaceButtons", "Лицевые кнопки"), 6 => ("LeftTrigger", "Левый триггер"),
        7 => ("RightTrigger", "Правый триггер"), 10 => ("Gyro", "Гироскоп"), 12 => ("RightJoystick", "Правый джойстик"), 13 => ("DPad", "Крестовина"), _ => ("", "Источник"),
    };
    t_or(&format!("#ControllerConfigurator_Source_{token}"), fallback)
}
/// Direction glyphs of a D-pad-mode source: the left trackpad has its own `sd_ltrackpad_*` set (stock trackpads page, scroll frames).
fn dpad_glyphs(key: i64) -> [&'static str; 4] {
    if key == 1 { ["sd_ltrackpad_up", "sd_ltrackpad_down", "sd_ltrackpad_left", "sd_ltrackpad_right"] } else { ["shared_dpad_up", "shared_dpad_down", "shared_dpad_left", "shared_dpad_right"] }
}
fn dpad_rows(raw: &[u8], rows: &[BindingRow], key: i64, out: &mut Vec<Item>) {
    let g = dpad_glyphs(key);
    for (input, glyph) in [(1, g[0]), (5, g[1]), (7, g[2]), (3, g[3])] {
        let mut it = input_row(raw, rows, key, 1, input, glyph, Some(String::new()));
        it.name = match input { 1 => "Крестовина вверх", 5 => "Крестовина вниз", 7 => "Крестовина влево", _ => "Крестовина вправо" }.into();
        // Stock names the direction on the (empty) command button instead of a left label.
        if it.binding.is_none() { it.value = match input { 1 => "Крестовина вверх", 5 => "Крестовина вниз", 7 => "Крестовина влево", _ => "Крестовина вправо" }.into(); it.dim = false; }
        out.push(it);
    }
}

/// Rows of editor section `section` (1…7; 0 and the stick settings dialog stay with the caller).
pub fn items(raw: &[u8], rows: &[BindingRow], section: usize) -> Vec<Item> {
    items_for_set(raw, rows, section, 0)
}

pub fn items_for_set(layout: &[u8], rows: &[BindingRow], section: usize, set_index: usize) -> Vec<Item> {
    let sets = msg(layout, 8).unwrap_or_default();
    let raw = sets.get(set_index).copied().unwrap_or_default();
    let mut out = Vec::new();
    match section {
        0 => {
            let find = |input: i64| string(raw, 1).ok().and_then(|set| rows.iter().position(|r| r.target.action_set_key == set
                && r.target.action_set_layer_key.is_none() && r.target.input_key == input && r.target.activator_index == 0 && r.target.binding_index == 0));
            let groups: [(&str, &str, &[(i64, &str, &str)]); 4] = [
                ("#ControllerConfigurator_FaceButtons", "Лицевые кнопки", &[(12, "A", "shared_button_a"), (10, "B", "shared_button_b"), (11, "X", "shared_button_x"), (9, "Y", "shared_button_y")]),
                ("#ControllerConfigurator_Bumpers", "Бамперы", &[(53, "Левый бампер", "sd_l1"), (54, "Правый бампер", "sd_r1")]),
                ("#ControllerConfigurator_ExtraButtons", "Дополнительные кнопки", &[(55, "L4", "sd_l4"), (56, "R4", "sd_r4"), (57, "L5", "sd_l5"), (58, "R5", "sd_r5")]),
                ("#ControllerConfigurator_MenuButtons", "Кнопки меню", &[(51, "Выбрать", "sd_button_view"), (52, "Начать", "sd_button_menu")]),
            ];
            for (n, (token, fallback, inputs)) in groups.into_iter().enumerate() {
                out.push(header(&t_or(token, fallback)));
                if n == 0 { out.push(function_row(4, source_mode(raw, 4).map(|m| m.0).or(Some(3)), true)); }
                for &(input, name, glyph) in inputs {
                    let mut it = item(Kind::Input, "", "");
                    it.name = name.into();
                    it.glyph = glyph;
                    it.gear = true;
                    it.input = Some(input);
                    it.input_source = (input_is_face(input)).then_some(4);
                    it.binding = find(input);
                    match it.binding {
                        Some(i) => it.value = super::controller_editor::command_label(&rows[i].binding),
                        None => { it.value = t_or("#ControllerConfigurator_AddCommand", "Добавить команду"); it.dim = true; }
                    }
                    out.push(it);
                }
            }
        }
        1 => {
            out.push(header(&source_title(13)));
            out.push(function_row(13, source_mode(raw, 13).map(|m| m.0), true));
            dpad_rows(raw, rows, 13, &mut out);
        }
        2 => for (key, glyph_full, glyph_soft, side) in [(7, "sd_r2", "sd_r2_half", "Правый триггер"), (6, "sd_l2", "sd_l2_half", "Левый триггер")] {
            let mode = source_mode(raw, key).map(|m| m.0);
            out.push(header(&source_title(key)));
            out.push(function_row(key, mode, true));
            out.push(input_row(raw, rows, key, 10, 13, glyph_full, None));
            out.push(input_row(raw, rows, key, 10, 15, glyph_soft, None));
            let mut analog = item(Kind::Param, &t_or("#ControllerConfigurator_SourceMode_Trigger_Analog", "Аналоговое нажатие триггера"), &t_or(&format!("#ControllerConfigurator_Source_{}", if key == 7 { "RightTrigger" } else { "LeftTrigger" }), side));
            analog.id = Id::Analog;
            analog.width = 375.;
            out.push(analog);
        },
        3 => {
            let all = sticks::sticks(layout).unwrap_or_default();
            // Stock lists the right stick first.
            let mut order: Vec<usize> = (0..all.len()).collect();
            order.sort_by_key(|&i| std::cmp::Reverse(all[i].target.source_binding_key));
            for i in order {
                let stick = &all[i];
                if string(raw, 1).ok().as_ref() != Some(&stick.target.action_set_key) || stick.target.action_set_layer_key.is_some() { continue; }
                let key = stick.target.source_binding_key;
                let (click, touch) = if key == 3 { ("shared_l3", "shared_lstick_touch") } else { ("shared_r3", "shared_rstick_touch") };
                out.push(header(&stick.label));
                let mut f = function_row(key, Some(stick.mode), true);
                f.stick = Some(i);
                out.push(f);
                for (input, glyph) in [(13, click), (50, touch), (15, "")] {
                    let mut it = input_row(raw, rows, key, stick.mode, input, glyph, None);
                    // Rows of other action sets/layers are addressed by the stick target.
                    it.binding = rows.iter().position(|r| r.target.action_set_key == stick.target.action_set_key && r.target.action_set_layer_key == stick.target.action_set_layer_key
                        && r.target.source_binding_key == key && r.target.input_key == input && r.target.activator_index == 0 && r.target.binding_index == 0);
                    if let Some(b) = it.binding { it.value = super::controller_editor::command_label(&rows[b].binding); it.dim = false; }
                    else { it.value = t_or("#ControllerConfigurator_AddCommand", "Добавить команду"); it.dim = true; }
                    out.push(it);
                }
            }
        }
        4 => for key in [2, 1] {
            let (mode, _) = source_mode(raw, key).unwrap_or((0, 0));
            let (click, touch, ring) = if key == 1 { ("sd_ltrackpad_click", "", "sd_ltrackpad_ring") } else { ("sd_rtrackpad_click", "", "sd_rtrackpad_ring") };
            out.push(header(&source_title(key)));
            out.push(function_row(key, source_mode(raw, key).map(|m| m.0), true));
            if mode == 1 { dpad_rows(raw, rows, key, &mut out); }
            for (input, glyph) in [(13, click), (50, touch), (15, ring)] { out.push(input_row(raw, rows, key, mode, input, glyph, None)); }
        },
        5 => {
            out.push(header(&source_title(10)));
            let mode = source_mode(raw, 10).map(|m| m.0);
            out.push(function_row(10, mode, mode.is_some_and(|value| !matches!(value, 0 | 20))));
            let mut calibrate = item(Kind::Button, &t_or("#Settings_ControllerCalibration_CalibrateIMU", "Калибровка гироскопа"), &t_or("#Settings_ControllerCalibration_Calibrate", "Калибровать"));
            calibrate.id = Id::Calibrate;
            calibrate.width = 240.;
            out.push(calibrate);
        }
        6 => {
            let mut add = item(Kind::Button, "", &t_or("#ControllerConfigurator_AddVirtualMenu", "Добавить виртуальное меню"));
            add.id = Id::AddVirtualMenu;
            add.width = 406.;
            out.push(add);
        }
        7 => {
            for set in sets {
                let name = string(set, 2).ok().filter(|n| !n.is_empty()).or_else(|| string(set, 1).ok()).unwrap_or_default();
                let mut it = item(Kind::SetRow, &name, "");
                it.gear = true;
                it.id = Id::ActionSet;
                out.push(it);
            }
            let mut add = item(Kind::Button, "", &t_or("#ControllerConfigurator_AddActionSet", "Добавить набор действий"));
            add.id = Id::AddActionSet; add.width = 410.; add.x = Some(0.);
            let mut global = item(Kind::Button, "", &t_or("#ControllerConfigurator_GlobalSettings", "Глобальные настройки набора"));
            global.id = Id::GlobalSettings; global.width = 409.; global.x = Some(440.);
            out.push(add);
            out.push(global);
        }
        _ => {}
    }
    out
}

/// Placement in device pixels (editor content space): rows follow the stock spacing measured from the DOM (CSS ×1.5):
/// header 54, 9 between rows, 36 before a following group, rows 96 (126 when a label or the command wraps).
#[derive(Clone, Debug)]
pub struct Placed { pub y: f32, pub h: f32, pub label: String, pub value: String }
/// Text width of a command in a row with glyph and gear (device px). Stock DOM editor-dpad/15b: «Крестовина вниз» (215 px in DejaVu) stays on one line,
/// «вверх» (229), «влево» (230), «вправо» (245) wrap to two, so the limit lies between 215 and 229.
const INPUT_VALUE_AVAIL: f32 = 222.;
pub fn place(items: &[Item]) -> (Vec<Placed>, f32) {
    use super::settings::text_wrap::wrapped;
    let mut y = 0f32;
    let mut out = Vec::new();
    for (n, it) in items.iter().enumerate() {
        let (h, label, value) = match it.kind {
            Kind::Header => { if n > 0 { y += 36. - 9.; } (54., it.label.clone(), String::new()) }
            Kind::Button if it.x.is_some() => (90., String::new(), wrapped(&it.value, 24., 276.)),
            Kind::Button if it.label.is_empty() => (126., String::new(), wrapped(&it.value, 24., it.width - 24.)),
            _ => {
                let label = wrapped(&it.label, 24., if it.glyph.is_empty() { 420. } else { 385. });
                let value = if it.kind == Kind::Input { wrapped(&it.value, 24., INPUT_VALUE_AVAIL) } else { it.value.clone() };
                let lines = label.lines().count().max(value.lines().count()).max(1);
                (if it.kind == Kind::Input && lines > 1 { 126. } else { 96. }, label, value)
            }
        };
        // two buttons share one line
        // two buttons share one line, 30 px below the action set rows
        match it.x { Some(0.) => y += 21., Some(_) => y -= 90. + 9., None => {} }
        out.push(Placed { y, h, label, value });
        y += h + if matches!(it.kind, Kind::Header) { 0. } else { 9. };
    }
    let content = out.last().map(|p| p.y + p.h).unwrap_or(0.);
    (out, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::controller_live::rows;

    // A real Deck layout (Balatro, task 264) is outside the repository: use it when present on the dev machine.
    fn real() -> Option<Vec<u8>> {
        std::fs::read(dirs_home().join(".cache/deckshell/task-264/20-start-raw.bin")).ok()
    }
    fn dirs_home() -> std::path::PathBuf { std::env::var_os("HOME").map(Into::into).unwrap_or_default() }

    #[test]
    fn button_page_follows_the_displayed_action_set() {
        use steam_bridge::controller_edit::{gamepad_binding, BindingTarget, Writer};
        let mut layout = Writer::new();
        let mut bindings = Vec::new();
        for (key, button) in [("Default", 3), ("Inventory", 4)] {
            let mut set = Writer::new();
            set.string(1, key);
            set.string(2, key);
            layout.message(8, &set.finish());
            bindings.push(BindingRow {
                target: BindingTarget { action_set_key: key.into(), action_set_layer_key: None, source_binding_key: 4,
                    mode_shift: false, input_key: 12, activator_index: 0, binding_index: 0, source_mode: 3, modeid: 0 },
                label: "A".into(), binding: gamepad_binding(button),
            });
        }
        let layout = layout.finish();
        for selected in 0..bindings.len() {
            let items = items_for_set(&layout, &bindings, 0, selected);
            assert_eq!(items.iter().find(|row| row.name == "A").unwrap().binding, Some(selected));
        }
    }

    #[test]
    fn dpad_triggers_trackpads_and_gyro_follow_stock_structure_on_a_real_layout() {
        let Some(raw) = real() else { return };
        let r = rows(&raw).unwrap();
        let dpad = items(&raw, &r, 1);
        assert_eq!(dpad.iter().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Header, Kind::Function, Kind::Input, Kind::Input, Kind::Input, Kind::Input]);
        assert_eq!(dpad[1].value, "Крестовина");
        let triggers = items(&raw, &r, 2);
        assert_eq!(triggers.iter().filter(|i| i.kind == Kind::Header).map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Правый триггер", "Левый триггер"]);
        assert_eq!(triggers[2].label, "Полное нажатие на триггер");
        assert_eq!(triggers[3].label, "Неполное нажатие на триггер");
        let pads = items(&raw, &r, 4);
        assert_eq!(pads[0].label, "Правый трекпад");
        assert_eq!(pads[1].value, "Джойстик");
        let left = pads.iter().position(|i| i.label == "Левый трекпад").unwrap();
        assert_eq!(pads[left + 1].value, "Крестовина");
        assert_eq!(pads[left + 2..left + 6].iter().filter(|i| i.kind == Kind::Input).count(), 4);
        let gyro = items(&raw, &r, 5);
        assert_eq!(gyro[1].value, "Не выбрано");
        assert!(!gyro[1].gear);
        assert_eq!(gyro[2].value, "Калибровать");
    }
    #[test]
    fn stick_page_lists_right_stick_first_with_click_touch_and_ring() {
        let Some(raw) = real() else { return };
        let r = rows(&raw).unwrap();
        let sticks = items(&raw, &r, 3);
        assert_eq!(sticks.iter().filter(|i| i.kind == Kind::Header).map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Правый джойстик", "Левый джойстик"]);
        assert_eq!(sticks[2].label, "Нажатие");
        assert_eq!(sticks[3].label, "Касание");
        assert_eq!(sticks[4].label, "Внешнее кольцо");
        assert!(sticks[3].dim && sticks[3].value == "Добавить команду");
    }
    #[test]
    fn action_sets_and_virtual_menu_pages_have_stock_buttons() {
        let raw = [];
        let vm = items(&raw, &[], 6);
        assert_eq!(vm.len(), 1);
        assert_eq!(vm[0].value, "Добавить виртуальное меню");
        assert_eq!(items(&raw, &[], 7).len(), 2);
    }
    #[test]
    fn stock_row_heights_follow_wrapping() {
        let mut one = item(Kind::Input, "", "Крестовина вниз");
        one.glyph = "shared_dpad_down";
        let mut two = item(Kind::Input, "", "Крестовина вверх");
        two.glyph = "shared_dpad_up";
        let mut empty = item(Kind::Input, "Полное нажатие на триггер", "Добавить команду");
        empty.glyph = "sd_r2";
        let (p, _) = place(&[one, two, empty]);
        assert_eq!(p[0].h, 96., "{:?}", p[0]);
        assert_eq!(p[1].h, 126.);
        assert_eq!(p[2].h, 126.);
        assert_eq!(p[1].y, 105.);
    }
    #[test]
    fn function_label_has_no_format_placeholder_and_labels_wrap_like_the_stock_capture() {
        assert_eq!(function_row(7, Some(10), true).label, "Функции");
        let mut full = item(Kind::Input, "Полное нажатие на триггер", "Добавить команду"); full.glyph = "sd_r2";
        let mut half = item(Kind::Input, "Неполное нажатие на триггер", "Добавить команду"); half.glyph = "sd_r2_half";
        let analog = item(Kind::Param, "Аналоговое нажатие триггера", "Правый триггер");
        let (p, _) = place(&[full, half, analog]);
        assert_eq!(p[0].label, "Полное нажатие на триггер");
        assert_eq!(p[1].label, "Неполное нажатие на\nтриггер");
        assert_eq!(p[2].label, "Аналоговое нажатие триггера");
    }
    #[test]
    fn stock_group_spacing_matches_the_dom() {
        // editor-triggers/17: header, «Функции», two rows, «Аналоговое», next header 36 device px after the previous panel.
        let items = vec![header("Правый триггер"), function_row(7, Some(10), true), { let mut i = item(Kind::Input, "Полное нажатие на триггер", "Добавить команду"); i.glyph = "sd_r2"; i }, { let mut i = item(Kind::Input, "Неполное нажатие на триггер", "Добавить команду"); i.glyph = "sd_r2_half"; i }, item(Kind::Param, "Аналоговое нажатие триггера", "Правый триггер"), header("Левый триггер")];
        let (p, _) = place(&items);
        assert_eq!((p[1].y, p[1].h), (54., 96.));
        assert_eq!(p[2].y, 54. + 96. + 9.);
        // stock CSS y 138 → 388 (Аналоговое) → header at 476: 36 device px between
        assert_eq!(p[5].y - (p[4].y + p[4].h), 36.);
    }
}

#[cfg(test)]
mod glyph_tests {
    /// Stock trackpads page (scroll frames): the left trackpad uses its own direction glyphs, a D-pad source the shared ones.
    #[test]
    fn left_trackpad_directions_use_trackpad_glyphs() {
        assert_eq!(super::dpad_glyphs(1), ["sd_ltrackpad_up", "sd_ltrackpad_down", "sd_ltrackpad_left", "sd_ltrackpad_right"]);
        assert_eq!(super::dpad_glyphs(13)[0], "shared_dpad_up");
    }
}
