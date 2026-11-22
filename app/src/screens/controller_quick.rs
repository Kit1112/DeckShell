//! «Быстрые настройки» корня раскладки: строки читаются из редактируемой копии раскладки Steam (тот же ответ
//! `StartEditing…`, что у редактора), как их строит sp.js `ControllerConfigurationQuickSettings`:
//! тумблер кнопок хвата, «Функции: Гироскоп», «Функции: Правый трекпад» (+ шестерёнка), инверсии оси Y.
//! Запись идёт теми же вызовами, что и в редакторе (`SetEditingControllerConfigurationSourceMode`).
use steam_assets::i18n::{tr_args, tr_ref};
use super::controller_live::{msg, number, string};
use super::controller_pages as pages;
use super::controller_pad as pad;
use super::controller_sticks as sticks;
use steam_bridge::controller_edit::BindingTarget;
use steam_bridge::{Error, Result};

/// `k_EControllerSource`: гироскоп, правый трекпад, правый джойстик.
pub const GYRO: i64 = 10;
pub const RPAD: i64 = 2;
pub const RSTICK: i64 = 12;
/// Остальные источники редактора: левый трекпад, левый стик, лицевые кнопки (ABXY), левый/правый триггер.
pub const LPAD: i64 = 1;
pub const LSTICK: i64 = 3;
pub const FACE: i64 = 4;
pub const DPAD: i64 = 13;
pub const LTRIGGER: i64 = 6;
pub const RTRIGGER: i64 = 7;
/// Псевдо-источник списка выбора стиля при создании виртуального меню (не `k_EControllerSource`).
pub const VMENU: i64 = 1000;
/// Режим «джойстик» (`joystick_move`), см. `controller_sticks`.
pub const MODE_JOYSTICK: i64 = 6;
/// `Me.R0` в быстрых настройках sp.js, экспорт `R0` модуля 46145.
pub const INVERT_Y_KEY: i64 = 42;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind { Toggle, Dropdown, Slider(Box<pad::PRow>) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Кнопки хвата (L4/R4/L5/R5 имеют привязки).
    Grips,
    /// «Функции: …» источника.
    Mode(i64),
    Shift(i64),
    /// Инверсия оси Y источника.
    InvertY(i64),
    Adjust(i64),
}
#[derive(Clone, Debug, PartialEq)]
pub struct QRow { pub action: Action, pub kind: Kind, pub label: String, pub value: String, pub on: bool, pub gear: bool }

/// Режимы выпадающего списка источника в порядке стока (кадры владельца controls/03, 04): после списка идут
/// «Создать сменный режим» и «Отмена». Номера — `k_EControllerMode`: 4 «Как мышь» (трекпад), 12 `mouse_joystick`,
/// 22 `gyro_to_mouse`, 23 камера и 24 отклонение гироскопа (таблица режимов chunk~2dcc5aaf7.js, `Gyro_To_*` в sp.js).
pub fn mode_choices(source: i64) -> &'static [i64] {
    match source {
        GYRO => &[0, 22, 23, 24],
        // `trackpad-mode-options`: Как мышь, Flick Stick, Область действия мыши, Джойстик, Как джойстик, Крестовина,
        // Блок кнопок, Одна кнопка, Свайп в сторону (2DScrollwheel), Колесо прокрутки.
        RPAD | LPAD => &[0, 4, 17, 13, 6, 12, 1, 3, 15, 18, 9],
        // `stick-mode-options`: Джойстик-мышь, Flick Stick, Область действия мыши, Джойстик, Крестовина, Колесо прокрутки.
        RSTICK | LSTICK => &[0, 7, 17, 13, 6, 1, 9],
        // `trigger-mode-options`: только «Триггер».
        LTRIGGER | RTRIGGER => &[0, 10],
        // `buttons-mode`: Джойстик, Крестовина, Блок кнопок.
        FACE => &[0, 6, 1, 3],
        DPAD => &[0, 6, 1, 3],
        // Стоковое меню «Создать» страницы «Виртуальное меню»: те же стили, что в sp.js (`$p`, `zy`, `bv`).
        VMENU => &steam_bridge::controller_edit::VIRTUAL_MENU_MODES,
        _ => &[],
    }
}

/// Пункты «Создать …» стока после списка режимов (`*-mode-options`): создают дочерние режимы, вызовы не
/// подтверждены на Деке, поэтому показываются как в стоке, а A даёт тост о недоступности.
pub fn create_entries(source: i64) -> &'static [(&'static str, &'static str)] {
    const RADIAL: (&str, &str) = ("#ControllerConfigurator_SourceMode_RadialMenu_Title", "controller.create_radial_menu");
    const TOUCH: (&str, &str) = ("#ControllerConfigurator_SourceMode_TouchMenu_Title", "controller.create_touch_menu");
    const HOTBAR: (&str, &str) = ("#ControllerConfigurator_SourceMode_Hotbar_Title", "controller.create_hotbar_menu");
    match source {
        RPAD | LPAD | RSTICK | LSTICK => &[RADIAL, TOUCH, HOTBAR],
        FACE => &[RADIAL],
        DPAD => &[HOTBAR],
        _ => &[],
    }
}

/// Строки списка режимов источника: (текст, значок) — режимы, затем пункты «Создать …».
pub fn list_modes(source: i64, simple_view: bool, current: i64) -> Vec<i64> {
    // Steam simpleView (sp.js module 38850) narrows the trackpad list on the root.
    let choices: &[i64] = match (source, simple_view) {
        (RPAD | LPAD, true) => &[0, 4, 12],
        _ => mode_choices(source),
    };
    let mut modes = choices.to_vec();
    if !modes.contains(&current) && !choices.is_empty() {
        modes.push(current);
    }
    modes
}

pub fn list_rows(source: i64, simple_view: bool, current: i64) -> Vec<(String, i32)> {
    let creates = if simple_view { &[][..] } else { create_entries(source) };
    list_modes(source, simple_view, current).into_iter()
        .map(|m| (mode_name(source, m), mode_icon(source, m)))
        .chain(creates.iter().map(|(token, fallback)| {
            let icon = if token.contains("Radial") { 6 } else if token.contains("Touch") { 7 } else { 8 };
            (crate::assets::t_or(token, tr_ref(fallback)), icon)
        }))
        .collect()
}

/// Название режима в строке и в списке источника (токены `SourceMode_*` стока).
pub fn mode_name(source: i64, mode: i64) -> String {
    use crate::assets::t_or;
    let (token, fallback) = match (source, mode) {
        (GYRO, 22) => ("Gyro_To_Mouse", tr_ref("controller.gyro_to_mouse")),
        (GYRO, 23) => ("Gyro_To_JoystickCamera", tr_ref("controller.gyro_to_joystick_camera")),
        (GYRO, 24) => ("Gyro_To_JoystickDeflection", tr_ref("controller.gyro_to_joystick_deflection")),
        (RPAD | LPAD, 4) => ("AbsoluteMouse", tr_ref("controller.as_mouse")),
        _ => return pages::mode_label(mode),
    };
    t_or(&format!("#ControllerConfigurator_SourceMode_{token}_Title"), fallback)
}

/// The icons selected by Steam's source-mode switch (sp.js:375383).
pub fn mode_icon(_source: i64, mode: i64) -> i32 {
    match mode {
        4 | 7 | 13 | 17 | 22 => 1,
        1 | 3 | 6 | 10 | 12 | 23 | 24 => 2,
        18 => 3,
        9 => 4,
        15 => 5,
        14 => 6,
        11 => 7,
        19 => 8,
        _ => 0,
    }
}

fn set0(raw: &[u8]) -> Result<&[u8]> {
    msg(raw, 8)?.into_iter().next().ok_or_else(|| Error::Protocol(tr_ref("controller.the_layout_has_no_action_sets").into()))
}

pub fn source_group(raw: &[u8], key: i64) -> Option<(i64, i64)> {
    group_at(set0(raw).ok()?, key, false).filter(|(mode, _)| !matches!(mode, 0 | 20))
}

/// Mode and group id of a source, normalizing Steam's two representations of a disabled source.
pub fn group_in_set(raw: &[u8], set_index: usize, source: i64) -> Option<(i64, i64)> {
    let sets = msg(raw, 8).ok()?;
    group_at(sets.get(set_index)?, source, false).filter(|(mode, _)| !matches!(mode, 0 | 20))
}

/// Режим источника в выбранном наборе действий редактора (0 — не выбрано).
pub fn mode_in_set(raw: &[u8], set_index: usize, key: i64) -> i64 {
    msg(raw, 8).ok().and_then(|sets| sets.get(set_index).copied()).and_then(|set| group_at(set, key, false))
        .map_or(0, |(mode, _)| if mode == 20 { 0 } else { mode })
}

/// Адрес записи режима источника в наборе `set_index` (источник без группы получает режим 0 и modeid 0).
pub fn mode_target_at(raw: &[u8], set_index: usize, key: i64) -> Result<BindingTarget> {
    let sets = msg(raw, 8)?;
    let set = sets.get(set_index).ok_or_else(|| Error::Protocol(tr_ref("controller.action_set_unavailable").into()))?;
    let (mode, modeid) = group_at(set, key, false).unwrap_or((0, 0));
    target_address(set, key, false, mode, modeid)
}

/// Address for a command on an input that holds none yet (stock «Добавить команду»): the new activator is the next
/// free index and the binding index is -1 (append), with the (mode, modeid) of the owning source group.
/// `source` is the owning source key when the page knows it; otherwise the first top-level mode of this set that
/// lists the input and is reachable from a source group is used (grips, bumpers and menu buttons).
pub fn empty_input_target(raw: &[u8], set_index: usize, input: i64, source: Option<i64>) -> Result<BindingTarget> {
    let sets = msg(raw, 8)?;
    let set = *sets.get(set_index).ok_or_else(|| Error::Protocol(tr_ref("controller.action_set_unavailable").into()))?;
    let wanted = match source {
        Some(key) => group_at(set, key, false).filter(|(mode, _)| !matches!(mode, 0 | 20)),
        None => None,
    };
    let reachable = |mode: i64, modeid: i64| msg(set, 6).ok().into_iter().flatten()
        .flat_map(|src| msg(src, 2).ok().unwrap_or_default())
        .any(|group| number(group, 1).ok() == Some(mode) && number(group, 5).ok() == Some(modeid));
    for m in msg(raw, 9)? {
        let (mode, modeid) = (number(m, 1)?, number(m, 2)?);
        if source.is_some() && wanted != Some((mode, modeid)) { continue; }
        if source.is_none() && !reachable(mode, modeid) { continue; }
        let Some(existing) = msg(m, 4)?.into_iter().find(|i| number(i, 1).ok() == Some(input)) else { continue };
        let mut target = target_address(set, source.unwrap_or(0), false, mode, modeid)?;
        target.input_key = input;
        target.activator_index = msg(existing, 2)?.len() as i64;
        target.binding_index = -1;
        return Ok(target);
    }
    Err(Error::Protocol(tr_ref("controller.this_input_has_no_source_mode_in_the_2").into()))
}

fn group_at(set: &[u8], key: i64, shift: bool) -> Option<(i64, i64)> {
    let source = msg(set, 6).ok()?.into_iter().find(|source| number(source, 1).ok() == Some(key))?;
    let group = msg(source, 2).ok()?.into_iter().next()?;
    let group = if shift { msg(group, 7).ok()?.into_iter().next()? } else { group };
    Some((number(group, 1).ok()?, number(group, 5).ok()?))
}

pub fn mode_target(raw: &[u8], key: i64) -> Result<BindingTarget> {
    let set = set0(raw)?;
    let (mode, modeid) = group_at(set, key, false).unwrap_or((0, 0));
    target_address(set, key, false, mode, modeid)
}

pub fn target(raw: &[u8], key: i64) -> Result<BindingTarget> {
    target_at(raw, 0, key, false)
}

pub fn target_at(raw: &[u8], set_index: usize, key: i64, shift: bool) -> Result<BindingTarget> {
    let sets = msg(raw, 8)?;
    let set = sets.get(set_index).ok_or_else(|| Error::Protocol(tr_ref("controller.action_set_unavailable").into()))?;
    let (mode, modeid) = group_at(set, key, shift).filter(|(mode, _)| !matches!(mode, 0 | 20))
        .ok_or_else(|| Error::Protocol(tr_ref("controller.no_source_selected_in_the_layout").into()))?;
    target_address(set, key, shift, mode, modeid)
}

fn target_address(set: &[u8], key: i64, shift: bool, mode: i64, modeid: i64) -> Result<BindingTarget> {
    Ok(BindingTarget { action_set_key: string(set, 1)?, action_set_layer_key: None, source_binding_key: key,
        mode_shift: shift, input_key: 0, activator_index: 0, binding_index: 0, source_mode: mode, modeid })
}

pub fn can_create_shift(raw: &[u8], source: i64) -> bool {
    !mode_choices(source).is_empty() && target_at(raw, 0, source, true).is_err()
}

pub fn initial_shift_mode(mode: i64) -> i64 {
    if matches!(mode, 0 | 11 | 14 | 19) { 1 } else { mode }
}

/// Кнопки хвата включены, если хотя бы у одного из L4/R4/L5/R5 есть привязка (`da` в sp.js).
pub fn grips_on(raw: &[u8]) -> bool {
    steam_bridge::controller_layout::parse_layout(raw).map(|l| {
        l.modes.iter().flat_map(|m| &m.inputs).filter(|i| (55..=58).contains(&i.key)).any(|i| i.activators.iter().any(|a| !a.bindings.is_empty()))
    }).unwrap_or(false)
}

fn invert_y(raw: &[u8], source: i64) -> Option<bool> {
    let target = target(raw, source).ok()?;
    // Non-gyro modes with R0 in the stock settings table. Joystick Move and
    // Joystick Mouse omit the row when Output Axis (mz = 21) omits Y.
    match target.source_mode {
        6 | 7 if sticks::value(raw, &target, 21).is_ok_and(|axis| axis == 1) => return None,
        4 | 6 | 7 | 8 | 12 | 13 | 22 | 23 | 24 | 26 | 28 => {}
        _ => return None,
    }
    sticks::value(raw, &target, INVERT_Y_KEY).ok().map(|value| value != 0)
}

fn slider(out: &mut Vec<QRow>, source: i64, settings: &[pad::PRow], key: i64, token: &str, fallback: &str) {
    if let Some(row) = settings.iter().find(|row| row.key == key && row.kind == pad::Kind::Slider) {
        out.push(QRow { action: Action::Adjust(source), kind: Kind::Slider(Box::new(row.clone())),
            label: crate::assets::t_or(token, fallback), value: row.text.clone(), on: false, gear: false });
    }
}

fn toggle(out: &mut Vec<QRow>, raw: &[u8], source: i64, token: &str, fallback: &str) {
    if let Some(on) = invert_y(raw, source) {
        out.push(QRow { action: Action::InvertY(source), kind: Kind::Toggle, label: crate::assets::t_or(token, fallback),
            value: String::new(), on, gear: false });
    }
}

fn mode_row(out: &mut Vec<QRow>, raw: &[u8], source: i64, label: String) {
    let mode = source_group(raw, source).map_or(0, |g| g.0);
    out.push(QRow { action: Action::Mode(source), kind: Kind::Dropdown, label: label.clone(),
        value: mode_name(source, mode), on: false, gear: mode != 0 });
    if let Ok(target) = target_at(raw, 0, source, true) {
        out.push(QRow { action: Action::Shift(source), kind: Kind::Dropdown,
            label: tr_args("controller.label_mode_shift", &[("label", &label.to_string())]), value: mode_name(source, target.source_mode),
            on: false, gear: true });
    }
}

fn mode_row_label(source: i64) -> String {
    crate::assets::t_or("#ControllerConfigurator_SourceMode_Group_Behavior", tr_ref("controller.1_s_behavior"))
        .replace("%1$s", &pages::source_title(source))
}

/// Stock QuickSettings order from sp.js:356426–356707. Read values and limits from the live layout.
pub fn rows(raw: &[u8], deck_controls: bool) -> Vec<QRow> {
    const SENSITIVITY: i64 = 30;
    const NATURAL: i64 = 140;
    const PIXELS: i64 = 137;
    const CAMERA_MIN: i64 = 149;
    let settings = |source| target(raw, source).ok().map(|t| pad::rows(raw, &t)).unwrap_or_default();
    let mode = |source| source_group(raw, source).map_or(0, |(mode, _)| mode);
    let mut out = Vec::new();
    if deck_controls {
        out.push(QRow { action: Action::Grips, kind: Kind::Toggle,
            label: crate::assets::t_or("#ControllerConfigurationQuickSettings_EnableGrips", tr_ref("controller.enable_back_grip_buttons")),
            value: String::new(), on: grips_on(raw), gear: false });
        mode_row(&mut out, raw, GYRO, mode_row_label(GYRO));
        let gyro = settings(GYRO);
        let key = if mode(GYRO) == 4 { NATURAL } else { SENSITIVITY };
        slider(&mut out, GYRO, &gyro, key, "#ControllerConfigurationQuickSettings_GyroSensitivity", tr_ref("controller.gyro_sensitivity_2"));
        if key != NATURAL {
            slider(&mut out, GYRO, &gyro, NATURAL, "#ControllerConfigurationQuickSettings_GyroNaturalSensitivity", tr_ref("controller.gyro_sensitivity"));
        }
        let pixels_source = if mode(GYRO) == 22 { Some(GYRO) }
            else { [RSTICK, LSTICK, RPAD, LPAD].into_iter().find(|&source| mode(source) == 17) };
        if let Some(source) = pixels_source {
            slider(&mut out, source, &settings(source), PIXELS, "#ControllerConfigurationQuickSettings_PixelsPerRevolution", tr_ref("controller.dots_per_360"));
        }
        toggle(&mut out, raw, GYRO, "#ControllerConfigurationQuickSettings_GyroInvert", tr_ref("controller.invert_gyro_y_axis"));
        slider(&mut out, GYRO, &gyro, CAMERA_MIN, "#ControllerConfigurationQuickSettings_GyroToJoystickCameraMinOutput", tr_ref("controller.minimum_gyro_output"));
        mode_row(&mut out, raw, RPAD, mode_row_label(RPAD));
        slider(&mut out, RPAD, &settings(RPAD), SENSITIVITY, "#ControllerConfigurationQuickSettings_RPadSensitivity", tr_ref("controller.right_trackpad_sensitivity"));
        toggle(&mut out, raw, RPAD, "#ControllerConfigurationQuickSettings_RPadInvert", tr_ref("controller.invert_right_trackpad_y_axis"));
    }
    slider(&mut out, RSTICK, &settings(RSTICK), SENSITIVITY, "#ControllerConfigurationQuickSettings_RStickSensitivity", tr_ref("controller.right_joystick_sensitivity"));
    toggle(&mut out, raw, RSTICK, "#ControllerConfigurationQuickSettings_RStickInvert", tr_ref("controller.invert_right_joystick_y_axis"));
    if deck_controls && source_group(raw, LPAD).is_some() {
        mode_row(&mut out, raw, LPAD, mode_row_label(LPAD));
    }
    out
}

/// Что делает A на строке и какая запись для этого нужна.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// Открыть список режимов.
    OpenList(i64),
    Adjust,
    /// Записать значение настройки группы.
    Setting { source: i64, key: i64, value: i64 },
    /// Записать нельзя: причина для тоста.
    Refuse(&'static str),
}

pub fn activate(raw: &[u8], row: &QRow) -> Plan {
    match row.action {
        Action::Adjust(_) => Plan::Adjust,
        Action::Grips => Plan::Refuse(tr_ref("controller.grip_button_writes_are_unavailable_binding_writes_are")),
        Action::Mode(source) | Action::Shift(source) => Plan::OpenList(source),
        Action::InvertY(source) => match invert_y(raw, source) {
            Some(on) => Plan::Setting { source, key: INVERT_Y_KEY, value: if on { 0 } else { 1 } },
            None => Plan::Refuse(tr_ref("controller.axis_inversion_is_unavailable_in_the_current_mode")),
        },
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use steam_bridge::controller_edit::{set_source_mode_msg, set_source_setting_msg, Writer};

    fn group(mode: i64, modeid: i64, invert: Option<i64>) -> Writer {
        let mut g = Writer::new();
        g.int(1, mode);
        g.int(5, modeid);
        if let Some(v) = invert {
            let mut s = Writer::new();
            s.int(1, INVERT_Y_KEY);
            s.int(2, (v << 1) ^ (v >> 63));
            s.int(3, 0);
            s.int(4, 2);
            g.message(4, &s.finish());
        }
        g
    }
    pub(in super::super) fn layout(grip: bool, gyro: Option<i64>, rpad: Option<(i64, Option<i64>)>, rstick: Option<i64>) -> Vec<u8> {
        let mut set = Writer::new();
        set.string(1, "Default");
        for (key, g) in [(GYRO, gyro.map(|m| group(m, 1, None))), (RPAD, rpad.map(|(m, inv)| group(m, 2, inv))), (RSTICK, rstick.map(|inv| group(MODE_JOYSTICK, 3, Some(inv))))] {
            if let Some(g) = g {
                let mut s = Writer::new();
                s.int(1, key);
                s.message(2, &g.finish());
                set.message(6, &s.finish());
            }
        }
        let mut raw = Writer::new();
        raw.message(8, &set.finish());
        if grip {
            let mut binding = Writer::new();
            binding.int(1, 3);
            let mut act = Writer::new();
            act.int(1, 1);
            act.message(2, &binding.finish());
            let mut input = Writer::new();
            input.int(1, 55);
            input.message(2, &act.finish());
            let mut mode = Writer::new();
            mode.int(1, 16);
            mode.int(2, 9);
            mode.message(4, &input.finish());
            raw.message(9, &mode.finish());
        }
        raw.finish()
    }

    #[test]
    fn rows_follow_stock_order_and_live_values() {
        let raw = layout(true, None, Some((MODE_JOYSTICK, Some(0))), Some(1));
        let r = rows(&raw, true);
        assert_eq!(r.iter().map(|r| r.action).collect::<Vec<_>>(), vec![Action::Grips, Action::Mode(GYRO), Action::Mode(RPAD), Action::InvertY(RPAD), Action::InvertY(RSTICK)]);
        assert!(r[0].on);
        assert_eq!(r[1].value, tr_ref("controller.none"));
        assert!(!r[1].gear && r[2].gear);
        assert!(!r[3].on && r[4].on);
    }
    #[test]
    fn no_grip_bindings_means_toggle_off_and_other_modes_have_no_invert_row() {
        let raw = layout(false, Some(12), Some((5, None)), None);
        let r = rows(&raw, true);
        assert!(!r[0].on);
        assert_eq!(r.len(), 3);
        assert!(r[1].value != tr_ref("controller.none"));
    }
    #[test]
    fn invert_toggle_flips_the_live_value_and_matches_the_stock_setting_message() {
        let raw = layout(false, None, Some((MODE_JOYSTICK, Some(1))), None);
        let row = rows(&raw, true).into_iter().find(|r| r.action == Action::InvertY(RPAD)).unwrap();
        assert!(row.on);
        let plan = activate(&raw, &row);
        assert_eq!(plan, Plan::Setting { source: RPAD, key: 42, value: 0 });
        let t = target(&raw, RPAD).unwrap();
        assert_eq!((t.source_binding_key, t.modeid), (RPAD, 2));
        let wire = set_source_setting_msg(&t, 42, 0);
        assert_eq!(number(msg(&wire, 7).unwrap()[0], 1).unwrap(), 42);
    }
    #[test]
    fn invert_y_respects_source_mode_and_output_axis() {
        let raw = layout(false, None, Some((12, Some(1))), Some(0));
        let x360 = rows(&raw, false);
        assert_eq!(x360.len(), 1);
        assert_eq!(x360[0].action, Action::InvertY(RSTICK));
        let trackpad = rows(&raw, true).into_iter().find(|r| r.action == Action::InvertY(RPAD)).unwrap();
        assert_eq!(activate(&raw, &trackpad), Plan::Setting { source: RPAD, key: 42, value: 0 });

        let unsupported = layout(false, None, Some((17, Some(1))), None);
        assert!(invert_y(&unsupported, RPAD).is_none());
        let horizontal = super::super::controller_pad::tests::other_mode_fixture(
            RSTICK, 6, &[(21, 1, 0, 2), (42, 1, 0, 1)],
        );
        assert!(invert_y(&horizontal, RSTICK).is_none());
    }
    #[test]
    fn mode_change_message_uses_the_stock_fields() {
        let raw = layout(false, None, Some((MODE_JOYSTICK, None)), None);
        let t = target(&raw, RPAD).unwrap();
        let wire = set_source_mode_msg(&t, 1);
        assert_eq!(string(&wire, 1).unwrap(), "Default");
        assert_eq!((number(&wire, 3).unwrap(), number(&wire, 5).unwrap(), number(&wire, 9).unwrap(), number(&wire, 10).unwrap()), (RPAD, 1, 0, 1));
    }
    #[test]
    fn shift_request_and_nested_reply_keep_base_group_separate() {
        let mut base = group(12, 2, None);
        let raw = layout(false, None, Some((12, None)), None);
        assert!(can_create_shift(&raw, RPAD));
        let mut requested = mode_target(&raw, RPAD).unwrap();
        requested.mode_shift = true;
        let wire = set_source_mode_msg(&requested, 12);
        assert_eq!((number(&wire, 3).unwrap(), number(&wire, 4).unwrap(), number(&wire, 5).unwrap(), number(&wire, 12).unwrap()), (RPAD, 1, 12, 1));
        base.message(7, &group(12, 25, Some(0)).finish());
        let mut source = Writer::new();
        source.int(1, RPAD);
        source.message(2, &base.finish());
        let mut set = Writer::new();
        set.string(1, "Default");
        set.message(6, &source.finish());
        let mut reply = Writer::new();
        reply.message(8, &set.finish());
        let raw = reply.finish();
        assert!(!can_create_shift(&raw, RPAD));
        assert_eq!(source_group(&raw, RPAD), Some((12, 2)));
        let shift = target_at(&raw, 0, RPAD, true).unwrap();
        assert_eq!(shift.modeid, 25);
        assert!(shift.mode_shift);
        assert_eq!(sticks::value(&raw, &shift, 42).unwrap(), 0);
    }
    #[test]
    fn grips_and_unselected_sources_refuse_or_open_lists() {
        let raw = layout(true, None, None, None);
        let r = rows(&raw, true);
        assert!(matches!(activate(&raw, &r[0]), Plan::Refuse(_)));
        assert_eq!(activate(&raw, &r[2]), Plan::OpenList(RPAD));
        assert!(target(&raw, RPAD).is_err());
        assert!(mode_choices(GYRO).contains(&0) && mode_choices(99).is_empty());
    }
    #[test]
    fn mode_lists_match_the_stock_frames() {
        // кадры 03/04: гироскоп — мышь, джойстик-камера, джойстик-отклонение; трекпад — «Как мышь», «Как джойстик»
        assert_eq!(mode_choices(GYRO), &[0, 22, 23, 24]);
        assert_eq!(list_modes(RPAD, true, 12), [0, 4, 12]);
        assert_eq!(list_modes(RPAD, true, 6), [0, 4, 12, 6]);
        assert_eq!(mode_name(GYRO, 22), tr_ref("controller.gyro_to_mouse"));
        assert_eq!(mode_name(GYRO, 23), tr_ref("controller.gyro_to_joystick_camera"));
        assert_eq!(mode_name(GYRO, 24), tr_ref("controller.gyro_to_joystick_deflection"));
        assert_eq!(mode_name(RPAD, 4), tr_ref("controller.as_mouse"));
        assert_eq!(mode_name(RPAD, 12), tr_ref("controller.as_joystick"));
        assert_eq!((mode_icon(GYRO, 22), mode_icon(GYRO, 23), mode_icon(RPAD, 4), mode_icon(RPAD, 12), mode_icon(RPAD, 0)), (1, 2, 1, 2, 0));
    }
    #[test]
    fn editor_lists_follow_the_stock_frames_for_every_source() {
        let names = |source: i64| mode_choices(source).iter().map(|&m| mode_name(source, m)).collect::<Vec<_>>();
        assert_eq!(names(RSTICK), [tr_ref("controller.none"), tr_ref("controller.joystick_mouse"), "Flick Stick", tr_ref("controller.mouse_region"), tr_ref("controller.output_joystick"), tr_ref("controller.dpad"), tr_ref("controller.scroll_wheel")]);
        assert_eq!(names(LSTICK), names(RSTICK));
        assert_eq!(mode_icon(RPAD, 18), 3);
        assert_eq!(names(RTRIGGER), [tr_ref("controller.none"), tr_ref("controller.trigger")]);
        assert_eq!(names(LTRIGGER), names(RTRIGGER));
        assert_eq!(names(FACE), [tr_ref("controller.none"), tr_ref("controller.output_joystick"), tr_ref("controller.dpad"), tr_ref("controller.button_pad")]);
        assert_eq!(&names(LPAD)[..4], [tr_ref("controller.none"), tr_ref("controller.as_mouse"), "Flick Stick", tr_ref("controller.mouse_region")]);
        let rows = |source| list_rows(source, false, 0).into_iter().map(|r| r.0).collect::<Vec<_>>();
        assert_eq!(&rows(RSTICK)[7..], [tr_ref("controller.create_radial_menu"), tr_ref("controller.create_touch_menu"), tr_ref("controller.create_hotbar_menu")]);
        assert_eq!(&rows(FACE)[4..], [tr_ref("controller.create_radial_menu")]);
        assert_eq!(&names(RPAD)[9..], [tr_ref("controller.directional_swipe"), tr_ref("controller.scroll_wheel")]);
        assert_eq!(rows(RTRIGGER).len(), 2);
    }
    #[test]
    fn mode_in_set_reads_the_chosen_action_set_and_targets_it() {
        let raw = layout(false, None, Some((MODE_JOYSTICK, None)), None);
        assert_eq!(mode_in_set(&raw, 0, RPAD), MODE_JOYSTICK);
        assert_eq!(mode_in_set(&raw, 0, RSTICK), 0);
        assert_eq!(mode_in_set(&raw, 3, RPAD), 0);
        let t = mode_target_at(&raw, 0, RSTICK).unwrap();
        assert_eq!((t.source_binding_key, t.source_mode, t.modeid, t.action_set_key.as_str()), (RSTICK, 0, 0, "Default"));
        assert!(mode_target_at(&raw, 3, RSTICK).is_err());
    }

    fn input_layout() -> Vec<u8> {
        use steam_bridge::controller_edit::{gamepad_binding, Writer};
        let group = |mode: i64, id: i64| { let mut g = Writer::new(); g.int(1, mode); g.int(5, id); g.finish() };
        let source = |key: i64, mode: i64, id: i64| { let mut w = Writer::new(); w.int(1, key); w.message(2, &group(mode, id)); w.finish() };
        let mut set = Writer::new(); set.string(1, "Default");
        for (key, mode, id) in [(4, 3, 0), (5, 16, 7), (7, 10, 4)] { set.message(6, &source(key, mode, id)); }
        let input = |key: i64, with_binding: bool| { let mut i = Writer::new(); i.int(1, key);
            if with_binding { let mut a = Writer::new(); a.int(1, 1); a.message(2, &gamepad_binding(3)); i.message(2, &a.finish()); } i.finish() };
        let mode = |mode: i64, id: i64, inputs: &[(i64, bool)]| { let mut m = Writer::new(); m.int(1, mode); m.int(2, id); for (k, b) in inputs { m.message(4, &input(*k, *b)); } m.finish() };
        let mut out = Writer::new();
        out.message(8, &set.finish());
        // Like a real Deck layout: the Switches mode lists every button too, so ABXY need the face source as a hint.
        out.message(9, &mode(16, 7, &[(55, false), (53, true), (12, false)]));
        out.message(9, &mode(3, 0, &[(12, true), (10, false)]));
        out.message(9, &mode(10, 4, &[(13, false), (15, true)]));
        out.finish()
    }

    #[test]
    fn empty_input_targets_append_to_the_next_activator_of_the_owning_source() {
        let raw = input_layout();
        // Trigger: the source is known, the input has no activator yet.
        let t = empty_input_target(&raw, 0, 13, Some(7)).unwrap();
        assert_eq!((t.source_binding_key, t.source_mode, t.modeid, t.input_key, t.activator_index, t.binding_index), (7, 10, 4, 13, 0, -1));
        // A grip button: no hint, found through the Switches group.
        let t = empty_input_target(&raw, 0, 55, None).unwrap();
        assert_eq!((t.source_mode, t.modeid, t.activator_index, t.binding_index), (16, 7, 0, -1));
        // B on the face source (mode 3), not the Switches group that also lists it.
        let t = empty_input_target(&raw, 0, 10, Some(4)).unwrap();
        assert_eq!((t.source_mode, t.modeid), (3, 0));
        // A command already there: the next free activator index.
        let t = empty_input_target(&raw, 0, 15, Some(7)).unwrap();
        assert_eq!((t.activator_index, t.binding_index), (1, -1));
        // Unknown input / unknown set are refused rather than guessed.
        assert!(empty_input_target(&raw, 0, 99, Some(7)).is_err());
        assert!(empty_input_target(&raw, 2, 13, Some(7)).is_err());
        assert!(empty_input_target(&raw, 0, 13, Some(12)).is_err());
    }

    /// Real Deck layout (Balatro), when present on the dev machine: every empty row of every page gets a resolvable address.
    #[test]
    fn every_empty_row_of_a_real_layout_has_an_address() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(raw) = std::fs::read(std::path::Path::new(&home).join(".cache/deckshell/task-264/20-start-raw.bin")) else { return };
        let rows = crate::screens::controller_live::rows(&raw).unwrap();
        let mut checked = 0;
        for section in 0..=5 {
            for it in pages::items_for_set(&raw, &rows, section, 0) {
                if it.kind != pages::Kind::Input || it.binding.is_some() { continue; }
                let t = empty_input_target(&raw, 0, it.input.unwrap(), it.input_source).unwrap_or_else(|e| panic!("section {section} {:?} {:?}: {e}", it.name, it.input));
                let resolved = t.resolve_source_binding(&raw).unwrap_or_else(|e| panic!("section {section} {:?}: {e}", it.name));
                if let Some(owner) = it.input_source { assert_eq!(resolved.source_binding_key, owner, "{}", it.name); }
                assert_eq!(t.binding_index, -1);
                checked += 1;
            }
        }
        assert!(checked > 10, "only {checked} empty rows checked");
    }

    #[test]
    fn virtual_menu_style_list_matches_the_stock_create_menu() {
        // sp.js `pe`: three items in order $p (radial), zy (touch), bv (hotbar); no «Создать …» rows, no zero mode.
        assert_eq!(mode_choices(VMENU), &[14, 11, 19]);
        let names: Vec<String> = list_rows(VMENU, false, 14).into_iter().map(|r| r.0).collect();
        assert_eq!(names.len(), 3);
        assert!(names.iter().all(|n| !n.starts_with(tr_ref("controller.mode"))));
    }
}
