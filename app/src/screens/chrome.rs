//! Wiring of the always-on chrome: top bar header targets (widgets/header_hits.slint), their D-pad cursor, and tile press-and-hold.
//! Header items (Steam comp nn, sp.js 401183-402100): 0 avatar (A -> /account), 1 search (A -> /routes/search + OSK), 2 SD card (A -> Settings/Storage),
//! 3 bell (A -> QAM Notifications), 4 Wi-Fi (A -> Settings/Internet), 5 battery (A -> Settings/Power), 6 downloads. Up from the content top focuses the
//! header (search first), Left/Right move between the drawn items, A activates, B/Down returns to the content.
use std::cell::Cell;
use std::path::Path;
use std::time::Instant;

use crate::actions::Act;
use crate::app::App;
use crate::input::Action;
use crate::router::Route;
use crate::sound::{play, Snd};
use crate::{AppWindow, LibraryState, TopBarState};
use slint::{ComponentHandle, Image};

thread_local! { static HF: Cell<i32> = const { Cell::new(-1) }; }

#[derive(Default)]
pub struct HeaderProfile {
    steamid: Option<u64>,
    sampled_at: Option<Instant>,
}

impl HeaderProfile {
    fn update(&mut self, top: &TopBarState<'_>, steam_root: &Path, steamid: u64, name: &str, sampled_at: Option<Instant>) {
        // An older poll can finish after the explicit refresh at the end of the flow.
        if sampled_at < self.sampled_at {
            return;
        }
        self.sampled_at = sampled_at;
        if self.steamid != Some(steamid) {
            self.steamid = Some(steamid);
            top.set_has_avatar(false);
            top.set_avatar(Image::default());
        }
        top.set_persona_name(name.into());
        // Steam can write the PNG after Ready. Retry whenever the avatar is still missing,
        // including after a failed decode of a partially written file.
        if !top.get_has_avatar() {
            let path = steam_root.join("config/avatarcache").join(format!("{steamid}.png"));
            if !path.is_file() {
                return;
            }
            if let Ok(image) = Image::load_from_path(&path) {
                top.set_avatar(image);
                top.set_has_avatar(true);
            }
        }
    }
}

/// Header item holding the D-pad cursor (-1: cursor is in the content).
pub fn hf() -> i32 {
    HF.with(|h| h.get())
}

/// Footer A label (loc token) for the focused header item (Steam: description of the item).
pub fn hint_token() -> &'static str {
    match hf() {
        0 => "#HeaderActions_AccountPage",
        2 => "#HeaderActions_StorageSettings",
        3 => "#ViewNotifications",
        4 => "#HeaderActions_Network",
        5 => "#HeaderActions_PowerSettings",
        6 => "#HeaderActions_Downloads",
        _ => "#ActionButtonLabelSelect",
    }
}

pub fn wire(ui: &AppWindow) {
    let tb = ui.global::<TopBarState>();
    tb.on_header_clicked(|i| super::defer(move |a| a.header_click(i)));
    ui.global::<LibraryState>().on_card_held(|ri, ci| super::defer(move |a| {
        let f = ri as usize;
        if let Some(r) = a.lb.card_rows.iter().position(|&x| x == f) {
            a.lb.zone = 1;
            a.lb.frow = r;
            a.lb.fcol = ci as usize;
            a.lib_focus_changed();
            a.handle(Action::Menu);
        }
    }));
}

impl App {
    pub(crate) fn init_user(&mut self) {
        let user = self.base.users.iter()
            .find(|user| Some(user.account_id) == self.base.active_user)
            .or_else(|| self.base.users.iter().find(|user| user.most_recent));
        if let Some(user) = user {
            let name = if user.persona_name.is_empty() { &user.account_name } else { &user.persona_name };
            let ui = self.ui();
            self.header_profile.update(&ui.global::<TopBarState>(), &self.base.steam_root, user.steam_id64, name, None);
        }
    }

    pub(crate) fn update_header_profile(&mut self, steamid: u64, name: &str, sampled_at: Instant) {
        let ui = self.ui();
        self.header_profile.update(&ui.global::<TopBarState>(), &self.base.steam_root, steamid, name, Some(sampled_at));
    }

    fn set_hf(&mut self, i: i32) {
        HF.with(|h| h.set(i));
        self.ui().global::<TopBarState>().set_hfocus(i);
        self.update_hint();
    }

    /// Indices of the header items that are drawn right now.
    fn header_items(&self) -> Vec<i32> {
        let ui = self.ui();
        let tb = ui.global::<TopBarState>();
        let mut v = vec![0, 1];
        if tb.get_dl_active() {
            v.push(6);
        }
        if tb.get_has_sdcard() {
            v.push(2);
        }
        if tb.get_unread() > 0 {
            v.push(3);
        }
        if tb.get_wifi() >= 0 {
            v.push(4);
        }
        if tb.get_bat_pct() >= 0 {
            v.push(5);
        }
        v
    }

    /// Tap / click / A on a header item.
    pub fn header_click(&mut self, idx: i32) {
        self.set_hf(-1);
        if idx == 1 {
            // При повторном касании поля сохраняем запрос и возвращаем фокус.
            if self.lb.search {
                self.search_focus_field();
                if !self.osk_is_open() {
                    self.osk_set_open(true);
                }
            } else {
                self.osk_set_open(false);
                self.open_search();
            }
            return;
        }
        self.osk_set_open(false);
        match idx {
            0 => {
                self.close_overlays();
                self.act(Act::OpenAccount);
            }
            2 => self.navigate(Route::Settings("storage".into())),
            3 => self.open_qam_tab(0),
            4 => self.navigate(Route::Settings("internet".into())),
            5 => self.navigate(Route::Settings("power".into())),
            6 if self.router.current() != &Route::Downloads => self.navigate(Route::Downloads),
            _ => {}
        }
    }

    /// Up pressed with nothing left to move to in the content: take the cursor to the header (called from `App::handle`).
    pub fn header_try_enter(&mut self) -> bool {
        if self.lb.search || self.sm.open || self.qm.open || self.lb.sort_open || hf() >= 0 {
            return false;
        }
        self.set_hf(1);
        true
    }

    /// Pad / keyboard input while the cursor is on the header. Returns true when consumed.
    pub fn handle_header(&mut self, a: Action) -> bool {
        let items = self.header_items();
        let cur = hf();
        let pos = items.iter().position(|&x| x == cur).unwrap_or(0);
        match a {
            Action::Left | Action::Right => {
                let n = if a == Action::Left { pos.checked_sub(1) } else { (pos + 1 < items.len()).then_some(pos + 1) };
                match n {
                    Some(n) => {
                        self.set_hf(items[n]);
                        play(Snd::BasicNav);
                    }
                    None => play(Snd::FailedNav),
                }
                true
            }
            // физическая клавиатура на строке поиска: открыть поиск и сразу набрать символ
            Action::Char(c) if cur == 1 => {
                self.header_click(1);
                self.search_char(c);
                true
            }
            Action::Down | Action::B | Action::Backspace => {
                self.set_hf(-1);
                play(Snd::BasicNav);
                true
            }
            Action::Up => {
                play(Snd::FailedNav);
                true
            }
            Action::A => {
                self.header_click(cur);
                true
            }
            // anything else (STEAM, QAM, search, options, bumpers...) leaves the header and is handled by the content / overlay
            _ => {
                self.set_hf(-1);
                false
            }
        }
    }

    pub fn header_clear(&mut self) {
        if hf() >= 0 {
            self.set_hf(-1);
        }
    }
}

#[cfg(test)]
#[path = "chrome_tests.rs"]
mod tests;
