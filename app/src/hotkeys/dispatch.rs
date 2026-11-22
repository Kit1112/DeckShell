//! Hotkey outputs -> existing app actions (UI thread).
use steam_assets::i18n::tr_ref;
use super::{Chord, Out};
use crate::actions::Act;
use crate::app::App;
use crate::assets::t_or;
use crate::data::PowerAction;
use crate::input::Action;
use crate::screens::steam_menu::MenuAct;
use crate::sound::{self, Snd};
use crate::{HintRow, HotkeysState, PowerRow};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

impl App {
    pub fn hotkey_out(&mut self, o: Out) {
        match o {
            // STEAM menu / QAM go through input.rs as ordinary actions
            Out::SteamMenu => self.handle_pad(Action::Steam),
            Out::Qam => self.handle_pad(Action::Qam),
            Out::PowerMenu => self.hk_power_menu(),
            Out::HintShow => self.hk_hint(true),
            Out::HintHide => self.hk_hint(false),
            Out::Chord(c) => self.hk_chord(c),
            Out::Brightness { up, elapsed_ms } => self.hk_brightness(up, elapsed_ms),
        }
    }

    /// One brightness step of the chord, on the stock slider's curve and hold (`level::hold_step`).
    fn hk_brightness(&mut self, up: bool, elapsed_ms: u64) {
        // step from the value being written (a repeat during a hold must count on top of it), else the shown scale
        let inflight = crate::actions::level_in_flight(true);
        let Some(cur) = inflight.or(self.qm.st.brightness) else {
            self.toast(tr_ref("hotkeys.brightness_unavailable_current_level_is_unconfirmed"));
            return;
        };
        let step = crate::level::hold_step(elapsed_ms as f32);
        let next = crate::level::chord_next(cur, up, step);
        crate::diag::log("bright.step", serde_json::json!({
            "dir": if up { "up" } else { "down" }, "src": if inflight.is_some() { "inflight" } else { "shown" },
            "cur": cur, "step": step, "elapsed_ms": elapsed_ms, "next": next
        }));
        crate::actions::set_brightness(&self.ds, next);
        self.level_changed(crate::level::Kind::Brightness, next, false, "chord");
    }

    fn hk_chord(&mut self, c: Chord) {
        match c {
            Chord::ForceClose => {
                for id in self.live.running.clone() {
                    self.act(Act::Stop(id));
                }
            }
            Chord::ShowKeyboard => self.osk_show(),
            Chord::Enter => self.handle_pad(Action::A),
            Chord::Escape => self.handle_pad(Action::B),
            // STEAM+A: DeckShell's single native toggle, but only once Steam Input no longer binds it (record_chord checks).
            Chord::RecordToggle => self.record_chord(),
            // Steam Input owns SCREENSHOT / gr_marker / gr_clip.
            // Observe only: sending another operation can undo the native chord.
            Chord::Screenshot | Chord::Marker | Chord::Clip => {
                crate::diag::log("capture_chord", serde_json::json!({"chord":format!("{c:?}"),"owner":"steam_input","dispatch":"observe_only"}));
                self.observe_native_capture();
            }
            // logged no-ops: no such feature in DeckShell (see context/archive/docs/PC_CLAUDE_LOG.md)
            Chord::Tab | Chord::ScreenReader | Chord::Magnifier | Chord::MouseLeft | Chord::MouseRight => {
                eprintln!("[deckshell] hotkey {c:?}: not implemented");
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pk {
    Hint,
    Sleep,
    Shutdown,
    Restart,
    Account,
    Logout,
    Desktop,
    StockSteam,
    Cancel,
}

// sp.js:404906–404978: IN_GAMESCOPE excludes Minimize/ExitGamepadUI/ExitSteam.
// Logout remains unavailable. Account opens the in-shell saved-account surface.
const POWER: [(Pk, &str, &str, bool); 9] = [
    (Pk::Hint, "#PowerChordMenu_ChordScreen", "hotkeys.show_controller_shortcuts", false),
    (Pk::Sleep, "#Quit_Sleep", "hotkeys.suspend_system", true),
    (Pk::Shutdown, "#Quit_Shutdown", "hotkeys.shutdown", false),
    (Pk::Restart, "#Quit_Restart", "hotkeys.restart_system", false),
    (Pk::Account, "#ChangeUser", "hotkeys.change_account", true),
    (Pk::Logout, "#SignOut_Title", "hotkeys.sign_out", false),
    (Pk::Desktop, "#SwitchToDesktop", "hotkeys.switch_to_desktop", true),
    (Pk::StockSteam, "#DeckShell_StockSteam", "hotkeys.return_to_stock_steam", false),
    (Pk::Cancel, "#Button_Cancel", "hotkeys.cancel", true),
];
const POWER_ROW: f32 = 72.0; // contextMenuItem 48 CSS px at actual Gaming DPR1.5
const POWER_TOP: f32 = 0.0; // reference 05: first row meets the panel edge
const POWER_BOTTOM: f32 = 0.0;
fn power_enabled(k: Pk, stock_allowed: bool) -> bool {
    !matches!(k, Pk::Logout) && (k != Pk::StockSteam || stock_allowed)
}
fn power_y(i: usize) -> f32 {
    POWER_TOP + POWER_ROW * i as f32 + POWER.iter().take(i + 1).filter(|x| x.3).count() as f32 * 3.0
}
fn power_content_height() -> f32 { power_y(POWER.len() - 1) + POWER_ROW + POWER_BOTTOM }
fn power_next(from: usize, down: bool, stock_allowed: bool) -> usize {
    let mut next = from;
    loop {
        let n = if down { next.saturating_add(1).min(POWER.len() - 1) } else { next.saturating_sub(1) };
        if n == next { return from; }
        next = n;
        if power_enabled(POWER[next].0, stock_allowed) { return next; }
    }
}
// Rows in the power focus table use the same transition as the live handler.
fn power_focus_move(from: usize, action: Action, offset: usize, stock_allowed: bool) -> usize {
    match action {
        Action::Up => power_next(from, false, stock_allowed).max(offset),
        Action::Down => power_next(from, true, stock_allowed).max(offset),
        _ => from,
    }
}
fn power_scroll(scroll: f32, index: usize, viewport: f32) -> f32 {
    let top = power_y(index.min(POWER.len() - 1));
    let desired = if top < scroll { top } else if top + POWER_ROW > scroll + viewport { top + POWER_ROW - viewport } else { scroll };
    desired.clamp(0.0, (power_content_height() - viewport).max(0.0))
}

#[derive(Default)]
struct Hk {
    power: bool,
    modal: bool,
    focus: usize,
    offset: usize,
    return_side: bool,
}

thread_local! {
    static HK: RefCell<Hk> = RefCell::new(Hk::default());
}

fn hint_models() -> (Vec<HintRow>, Vec<HintRow>) {
    let (mut l, mut r) = (vec![], vec![]);
    let mut last = "";
    for (i, row) in super::hint::ROWS.iter().enumerate() {
        let v = if super::hint::is_left(row.section) { &mut l } else { &mut r };
        if row.section != last {
            last = row.section;
            v.push(HintRow { header: true, text: row.section.into(), glyph: 0, activator: "".into() });
        }
        v.push(HintRow { header: false, text: row.label().into(), glyph: i as i32, activator: row.activator().to_uppercase().into() });
    }
    (l, r)
}

/// The power menu or its chords modal is on screen.
pub fn power_open() -> bool { HK.with(|h| h.borrow().power) }
/// Focus state of the power menu for `App::sig`.
pub fn sig() -> i64 {
    HK.with(|h| { let h = h.borrow(); if h.power { 1 + h.focus as i64 + ((h.modal as i64) << 16) } else { 0 } })
}

impl App {
    fn hk_fill(&self, upper_title: bool) {
        let ui = self.ui();
        let st = ui.global::<HotkeysState>();
        let title = t_or("#ControllerChord_GuideButtonShortcuts", tr_ref("hotkeys.steam_button_shortcuts"));
        st.set_title(if upper_title { title.to_uppercase() } else { title }.into());
        let (l, r) = hint_models();
        st.set_left(ModelRc::new(VecModel::from(l)));
        st.set_right(ModelRc::new(VecModel::from(r)));
    }

    /// "..." held: overlay on top of the current screen; hidden on release. Takes no focus.
    pub fn hk_hint(&mut self, show: bool) {
        if show {
            self.hk_fill(true);
        }
        self.ui().global::<HotkeysState>().set_hold_open(show);
    }

    /// Long STEAM: power menu with "show controller chords" first.
    pub fn hk_power_menu(&mut self) {
        self.hk_power_open(false);
    }

    pub(crate) fn hk_power_from_menu(&mut self) {
        self.hk_power_open(true);
    }

    fn hk_power_open(&mut self, from_menu: bool) {
        let offset = usize::from(from_menu);
        // A confirmation left open (for example "Shut down?" from an earlier power menu) outranks
        // the power menu as an input scope and would keep the pad while this menu is drawn.
        self.dismiss_confirm();
        // Over a game the menu is an overlay on the game, like the side menus: the game keeps
        // the screen and gets it back on close. Only with no game in front does the shell
        // take the screen (for example when native Steam UI had it).
        if self.game_overlay.is_none() {
            self.game_overlay = crate::gamescope::focused_game().filter(|id| self.live.running.contains(id));
        }
        if self.game_overlay.is_none() { crate::gamescope::request_reclaim(); }
        let ui = self.ui();
        let st = ui.global::<HotkeysState>();
        st.set_power_title(t_or("#Power", tr_ref("hotkeys.power_off_system")).into());
        st.set_power_menu_label(t_or("#ActionButtonLabelMenu", tr_ref("hotkeys.menu")).to_uppercase().into());
        st.set_power_select_label(t_or("#ActionButtonLabelSelect", tr_ref("hotkeys.select")).to_uppercase().into());
        st.set_power_back_label(t_or("#ActionButtonLabelBack", tr_ref("hotkeys.previous")).to_uppercase().into());
        let rows: Vec<PowerRow> = POWER.iter().skip(offset).enumerate().map(|(i, (k, token, fallback, sep))| PowerRow { label: t_or(token, tr_ref(fallback)).into(), sep: *sep && i != 0, enabled: power_enabled(*k, self.live.running.is_empty()), y: power_y(i + offset) - (power_y(offset) - POWER_TOP) }).collect();
        st.set_power_content_height(power_content_height() - (power_y(offset) - POWER_TOP));
        st.set_power_scroll(0.0);
        st.set_power_items(ModelRc::new(VecModel::from(rows)));
        st.set_power_focus(0);
        st.set_power_open(true);
        HK.with(|h| *h.borrow_mut() = Hk { power: true, modal: false, focus: offset, offset, return_side: from_menu });
        // the side menu / QAM behind would otherwise take the pad
        if self.sm.open || self.qm.open {
            self.close_overlays();
        }
        self.sync_overlays();
        sound::play(Snd::ShowModal);
    }

    fn hk_close_with_return(&mut self, restore: bool) {
        let return_side = HK.with(|h| h.borrow().return_side);
        let ui = self.ui();
        let st = ui.global::<HotkeysState>();
        st.set_power_open(false);
        st.set_modal_open(false);
        HK.with(|h| *h.borrow_mut() = Hk::default());
        sound::play(Snd::HideModal);
        if restore && return_side {
            self.sm.open = true;
            self.build_menu();
            self.update_hint();
        }
        // Callers that open a follow-up dialog sync after it is up, so the game overlay
        // is not dropped and re-requested in between.
        if restore { self.sync_overlays(); }
    }

    fn hk_close(&mut self) { self.hk_close_with_return(true); }

    /// Input while the power menu / chord modal is open. Returns true when `a` was consumed.
    pub fn hk_handle(&mut self, a: Action) -> bool {
        // confirmation dialog started from the power menu (reuses the Steam menu dialog)
        if self.sm.confirm.is_some() && !self.sm.open {
            self.handle_side(a, false);
            return true;
        }
        let (power, modal, focus, offset) = HK.with(|h| {
            let h = h.borrow();
            (h.power, h.modal, h.focus, h.offset)
        });
        if !power {
            return false;
        }
        if matches!(a, Action::B | Action::Steam | Action::Qam) {
            self.hk_close();
            return true;
        }
        if modal {
            return true;
        }
        match a {
            Action::Up | Action::Down => {
                let n = power_focus_move(focus, a, offset, self.live.running.is_empty());
                HK.with(|h| h.borrow_mut().focus = n);
                let ui = self.ui();
                let st = ui.global::<HotkeysState>();
                st.set_power_focus((n - offset) as i32);
                let shift = power_y(offset) - POWER_TOP;
                st.set_power_scroll((power_scroll(st.get_power_scroll() + shift, n, st.get_power_viewport()) - shift).max(0.0));
            }
            Action::A => self.hk_power_pick_index(focus),
            _ => {}
        }
        true
    }

    pub fn hk_power_pick(&mut self, i: usize) {
        let offset = HK.with(|h| h.borrow().offset);
        self.hk_power_pick_index(i.saturating_add(offset));
    }

    fn hk_power_pick_index(&mut self, i: usize) {
        let Some(&(k, ..)) = POWER.get(i) else { return };
        if !power_enabled(k, self.live.running.is_empty()) { return; }
        match k {
            Pk::Hint => {
                self.hk_fill(false);
                self.ui().global::<HotkeysState>().set_modal_open(true);
                HK.with(|h| h.borrow_mut().modal = true);
            }
            Pk::Cancel => self.hk_close(),
            Pk::Sleep => {
                self.hk_close_with_return(false);
                self.act(crate::actions::Act::Power(PowerAction::Sleep));
                self.sync_overlays();
            }
            Pk::Shutdown | Pk::Restart | Pk::Desktop | Pk::StockSteam => {
                self.hk_close_with_return(false);
                self.menu_confirm_show(match k {
                    Pk::Shutdown => MenuAct::Shutdown,
                    Pk::Restart => MenuAct::Restart,
                    Pk::StockSteam => MenuAct::StockSteam,
                    _ => MenuAct::Desktop,
                });
                self.sync_overlays();
            }
            Pk::Account => {
                self.hk_close_with_return(false);
                crate::screens::account_switch::open(self);
            }
            Pk::Logout => {} // unavailable
        }
    }
}

impl App {
    pub fn hk_close_pub(&mut self) {
        self.hk_close();
    }
}

#[cfg(test)]
mod power_menu_tests {
    use super::*;
    #[test]
    fn power_modal_entry_regression() {
        // Menu and both hold routes share the same entries and selection logic.
        let menu_offset = 1;
        let hold_offset = 0;
        assert!(POWER[menu_offset].0 == Pk::Sleep);
        assert!(POWER[hold_offset].0 == Pk::Hint);
        assert!(POWER.iter().any(|row| row.0 == Pk::StockSteam && power_enabled(row.0, true)));
        assert!(POWER.last().unwrap().0 == Pk::Cancel);
        assert_eq!(power_next(menu_offset, false, true).max(menu_offset), menu_offset);
        assert_eq!(power_next(3, true, true), 4);
        let menu = include_str!("../screens/steam_menu.rs");
        let runtime = include_str!("../input/power/runtime.rs");
        assert!(menu.contains("MenuAct::Power => {\n                self.hk_power_from_menu();"));
        assert!(!menu.contains("sm.power_open"));
        assert!(runtime.contains("app.hk_power_menu()"));
        assert!(include_str!("dispatch.rs").contains("Out::PowerMenu => self.hk_power_menu()"));
        assert!(include_str!("dispatch.rs").contains("if restore && return_side"));
        let dispatch = include_str!("dispatch.rs");
        assert!(dispatch.contains("Pk::Cancel => self.hk_close()"));
        assert!(dispatch.contains("Action::B | Action::Steam | Action::Qam"));
        assert!(dispatch.contains("st.set_power_open(true)"));
        assert!(dispatch.contains("st.set_power_open(false)"));
        assert!(dispatch.contains("st.set_power_focus(0)"));
        assert!(dispatch.contains("self.menu_confirm_show(match k"));
    }
    #[test]
    fn power_focus_table() {
        use Action::{Up, Down, Left, Right, A, B};
        let buttons = [Up, Down, Left, Right, A, B];
        // Enabled rows, with unavailable Logout skipped.
        for (row, expected) in [
            (0, [0, 1, 0, 0, 0, 0]), (1, [0, 2, 1, 1, 1, 1]),
            (2, [1, 3, 2, 2, 2, 2]), (3, [2, 4, 3, 3, 3, 3]),
            (4, [3, 6, 4, 4, 4, 4]),
            (6, [4, 7, 6, 6, 6, 6]), (7, [6, 8, 7, 7, 7, 7]),
            (8, [7, 8, 8, 8, 8, 8]),
        ] {
            for (button, target) in buttons.into_iter().zip(expected) {
                assert_eq!(power_focus_move(row, button, 0, true), target, "row {row}, {button:?}");
            }
        }
        assert_eq!(power_focus_move(6, Down, 0, false), 8);
        assert_eq!(power_focus_move(1, Up, 1, true), 1);
    }
    #[test]
    fn power_navigation_reverses_from_last_item_for_both_entry_points() {
        for stock_allowed in [false, true] {
            for offset in [0, 1] {
                let mut path = vec![offset];
                loop {
                    let next = power_next(*path.last().unwrap(), true, stock_allowed);
                    if next == *path.last().unwrap() { break; }
                    path.push(next);
                }
                assert_eq!(*path.last().unwrap(), POWER.len() - 1);
                for pair in path.windows(2).rev() {
                    assert_eq!(power_next(pair[1], false, stock_allowed).max(offset), pair[0]);
                }
            }
        }
    }
    #[test]
    fn native_deck_entries_never_offer_desktop_only_exit_or_minimize() {
        let labels: Vec<_> = POWER.iter().map(|x| x.1).collect();
        assert!(!labels.iter().any(|s| matches!(*s, "#MinimizeSteam" | "#ExitGamepadUI" | "#ExitSteam")));
        assert!(labels.contains(&"#SwitchToDesktop"));
        assert!(power_enabled(Pk::Account, true));
        assert!(!power_enabled(Pk::Logout, true));
    }
    #[test]
    fn navigation_skips_unavailable_accounts_and_reveals_last_row() {
        let mut index = 0;
        let mut selected = Vec::new();
        loop {
            selected.push(POWER[index].0);
            let next = power_next(index, true, true);
            if next == index { break; }
            index = next;
        }
        assert!(selected == vec![Pk::Hint, Pk::Sleep, Pk::Shutdown, Pk::Restart, Pk::Account, Pk::Desktop, Pk::StockSteam, Pk::Cancel]);
        assert_eq!(power_next(6, false, true), 4);
        assert!(!power_enabled(Pk::StockSteam, false));
        assert_eq!(power_next(6, true, false), 8);
        let scroll = power_scroll(0.0, index, 200.0);
        assert!(power_y(index) >= scroll);
        assert!(power_y(index) + POWER_ROW <= scroll + 200.0);
        assert_eq!(power_scroll(scroll, 0, 200.0), POWER_TOP);
    }
}
