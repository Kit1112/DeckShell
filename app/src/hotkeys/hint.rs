//! Chord hint table (context/archive/docs/hotkeys/SPEC.md, order and labels as in Big Picture; Steam Deck glyphs only).

use crate::assets::t_or;
use steam_assets::i18n::tr_ref;

#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub section: &'static str,
    label_token: &'static str,
    label_key: &'static str,
    /// glyph file name without `.svg` in app/assets/hotkeys/
    pub glyph: &'static str,
    /// catalog key for the activator caption, empty when none
    activator: &'static str,
}

impl Row {
    pub fn label(self) -> String {
        t_or(self.label_token, tr_ref(self.label_key))
    }

    pub fn activator(self) -> String {
        match self.activator {
            "hotkeys.long_press" => t_or("#ControllerBinding_ActivatorDropDown_LongPress", tr_ref(self.activator)),
            "hotkeys.release_press" => t_or("#ControllerBinding_ActivatorDropDown_Release", tr_ref(self.activator)),
            _ => String::new(),
        }
    }
}

const fn r(section: &'static str, label_token: &'static str, label_key: &'static str, glyph: &'static str, activator: &'static str) -> Row {
    Row { section, label_token, label_key, glyph, activator }
}

pub const ROWS: [Row; 18] = [
    r("GENERAL", "#ControllerActionKey_Quit_Application", "hotkeys.force_game_shutdown", "shared_button_b", "hotkeys.long_press"),
    r("GENERAL", "#ControllerActionKey_Screen_Brightness_Up", "hotkeys.screen_brightness_up", "shared_lstick_up", ""),
    r("GENERAL", "#ControllerActionKey_Screen_Brightness_Down", "hotkeys.screen_brightness_down", "shared_lstick_down", ""),
    r("ACCESSIBILITY", "#ControllerActionKey_ScreenReader_Enable", "hotkeys.screen_reader_enable", "sd_button_view", ""),
    r("ACCESSIBILITY", "#ControllerActionKey_Toggle_Magnifier", "hotkeys.toggle_magnifier", "sd_l1", ""),
    r("ACCESSIBILITY", "#ControllerActionKey_Show_Keyboard", "hotkeys.show_keyboard", "shared_button_x", "hotkeys.release_press"),
    r("KEYS", "#KeyboardKey_Return", "hotkeys.enter_key", "shared_dpad_right", ""),
    r("KEYS", "#KeyboardKey_Tab", "hotkeys.tab_key", "shared_dpad_down", ""),
    r("KEYS", "#KeyboardKey_Escape", "hotkeys.escape_key", "shared_dpad_left", ""),
    r("MOUSE", "#ControllerConfigurator_SourceMode_AbsoluteMouse_Title", "hotkeys.as_mouse", "sd_rtrackpad_swipe", ""),
    r("MOUSE", "#ControllerConfigurator_SourceMode_JoystickMouse_Title", "hotkeys.joystick_mouse", "shared_rstick", ""),
    r("MOUSE", "#MouseKey_Button_Left", "hotkeys.left_mouse_click", "sd_rtrackpad_click", ""),
    r("MOUSE", "#MouseKey_Button_Right", "hotkeys.right_mouse_click", "sd_l2_half", ""),
    r("MOUSE", "#MouseKey_Button_Left", "hotkeys.left_mouse_click", "sd_r2_half", ""),
    r("GAME RECORDING", "#ControllerActionKey_Screenshot", "hotkeys.take_screenshot", "sd_r1", ""),
    r("GAME RECORDING", "#ControllerActionKey_GR_Marker", "hotkeys.add_timeline_marker", "shared_button_y", ""),
    r("GAME RECORDING", "#ControllerActionKey_GR_Toggle", "hotkeys.toggle_manual_recording", "shared_button_a", ""),
    r("GAME RECORDING", "#ControllerActionKey_GR_Clip", "hotkeys.create_clip", "shared_dpad_up", ""),
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
