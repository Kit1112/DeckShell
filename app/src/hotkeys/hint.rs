//! Chord hint table (context/archive/docs/hotkeys/SPEC.md, order and labels as in Big Picture; Steam Deck glyphs only).

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub section: &'static str,
    /// Russian label (Steam token text)
    pub label: &'static str,
    /// glyph file name without `.svg` in app/assets/hotkeys/
    pub glyph: &'static str,
    /// activator caption (uppercased by the UI), empty when none
    pub activator: &'static str,
}

const fn r(section: &'static str, label: &'static str, glyph: &'static str, activator: &'static str) -> Row {
    Row { section, label, glyph, activator }
}

pub const ROWS: [Row; 18] = [
    r("GENERAL", "Принудительное закрытие игры", "shared_button_b", "Долгое нажатие"),
    r("GENERAL", "Повысить яркость экрана", "shared_lstick_up", ""),
    r("GENERAL", "Понизить яркость экрана", "shared_lstick_down", ""),
    r("ACCESSIBILITY", "Включить программу чтения с экрана", "sd_button_view", ""),
    r("ACCESSIBILITY", "Вкл./выкл. приближение", "sd_l1", ""),
    r("ACCESSIBILITY", "Показать клавиатуру", "shared_button_x", "Завершение нажатия"),
    r("KEYS", "Клавиша Enter", "shared_dpad_right", ""),
    r("KEYS", "Клавиша Tab", "shared_dpad_down", ""),
    r("KEYS", "Клавиша Escape", "shared_dpad_left", ""),
    r("MOUSE", "Как мышь", "sd_rtrackpad_swipe", ""),
    r("MOUSE", "Джойстик-мышь", "shared_rstick", ""),
    r("MOUSE", "Нажатие левой кнопки мыши", "sd_rtrackpad_click", ""),
    r("MOUSE", "Нажатие правой кнопки мыши", "sd_l2_half", ""),
    r("MOUSE", "Нажатие левой кнопки мыши", "sd_r2_half", ""),
    r("GAME RECORDING", "Сделать скриншот", "sd_r1", ""),
    r("GAME RECORDING", "Добавить отметку на временной шкале", "shared_button_y", ""),
    r("GAME RECORDING", "Вкл./выкл. запись вручную", "shared_button_a", ""),
    r("GAME RECORDING", "Создать ролик", "shared_dpad_up", ""),
];

/// Hint column split as on the Deck captures: left = GENERAL, ACCESSIBILITY, KEYS; right = MOUSE, GAME RECORDING.
pub fn is_left(section: &str) -> bool {
    matches!(section, "GENERAL" | "ACCESSIBILITY" | "KEYS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighteen_rows_split_9_9() {
        assert_eq!(ROWS.len(), 18);
        assert_eq!(ROWS.iter().filter(|r| is_left(r.section)).count(), 9);
        assert_eq!(ROWS.iter().filter(|r| !is_left(r.section)).count(), 9);
    }
}
