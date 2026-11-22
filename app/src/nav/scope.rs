//! Who owns pad input: one scope at a time.
//!
//! Screens here still keep their own focus state, so "which surface gets this press" used to be
//! decided separately in several places, and the header was entered whenever an Up press left the
//! tracked page state unchanged. A modal whose focus was not tracked (the power menu) therefore
//! threw every Up into the header, which then swallowed the following presses.
//!
//! The rule now: exactly one scope owns input, the topmost open surface. A press never falls
//! through to a scope below it, and the header belongs to a plain page: it can be entered only
//! from `Page` with no surface above it, and it is left as soon as any surface opens.
use crate::app::App;
use crate::input::Action;
use crate::router::Route;

/// Input owners, topmost first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Controller,
    Properties,
    Artwork,
    /// Confirmation dialog (exit game, shutdown, ...).
    Confirm,
    /// Power menu and the controller-chords modal opened from it.
    Power,
    /// Sort / tile / download context menu.
    ContextMenu,
    Qam,
    SideMenu,
    Search,
    /// A popup that belongs to the current page: game gear menu, media viewer, Settings → Анимации modal/keyboard.
    PagePopup,
    /// The top bar, reached from a plain page.
    Header,
    Page,
}

/// The header may take focus only when Up hit the top of a plain page and nothing opened meanwhile.
pub fn header_entry_allowed(a: Action, before: Scope, after: Scope, state_unchanged: bool) -> bool {
    a == Action::Up && state_unchanged && before == Scope::Page && after == Scope::Page
}

impl App {
    /// The topmost open surface, not counting header focus.
    pub fn surface_scope(&self) -> Scope {
        if self.account.open || self.install.wizard.view != crate::install_flow::View::Closed || self.sm.confirm.is_some() {
            Scope::Confirm
        } else if crate::screens::hotkeys::dispatch::power_open() {
            Scope::Power
        } else if crate::screens::controller::is_open() {
            Scope::Controller
        } else if crate::screens::properties::is_open() {
            Scope::Properties
        } else if crate::sgdb::is_open() {
            Scope::Artwork
        } else if self.lb.sort_open || (matches!(self.router.current(), Route::Settings(_)) && (self.se.storage_sort_menu || self.se.recording_menu.is_some())) {
            // `sort_open` alone says the popup is shown: `lb.menu` keeps its last content after
            // STEAM, QAM or a route change closed the popup and must not hold the pad.
            Scope::ContextMenu
        } else if self.qm.open {
            Scope::Qam
        } else if self.sm.open {
            Scope::SideMenu
        } else if self.lb.search {
            Scope::Search
        } else if (matches!(self.router.current(), Route::App(_)) && self.gm.mn.open)
            || (matches!(self.router.current(), Route::Media) && (self.media_viewer_open() || self.media_selecting()))
            || (matches!(self.router.current(), Route::Settings(_)) && crate::screens::anim_page::modal_or_keyboard_open())
            || crate::screens::settings::controller_devices::owns_scope(self)
        {
            Scope::PagePopup
        } else {
            Scope::Page
        }
    }

    /// The scope that receives the next press.
    pub fn focus_scope(&self) -> Scope {
        match self.surface_scope() {
            Scope::Page if crate::screens::chrome::hf() >= 0 => Scope::Header,
            s => s,
        }
    }

    /// A menu, dialog or modal that must stay on screen over a running game.
    pub fn overlay_surface_open(&self) -> bool {
        matches!(self.surface_scope(), Scope::Confirm | Scope::Power | Scope::Qam | Scope::SideMenu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Scope; 12] = [
        Scope::Controller, Scope::Properties, Scope::Artwork, Scope::Confirm, Scope::Power, Scope::ContextMenu,
        Scope::Qam, Scope::SideMenu, Scope::Search, Scope::PagePopup, Scope::Header, Scope::Page,
    ];

    #[test]
    fn header_is_entered_only_by_up_at_the_top_of_a_plain_page() {
        assert!(header_entry_allowed(Action::Up, Scope::Page, Scope::Page, true));
        // The page moved its own focus: not at the top.
        assert!(!header_entry_allowed(Action::Up, Scope::Page, Scope::Page, false));
        for a in [Action::Down, Action::Left, Action::Right, Action::A, Action::B] {
            assert!(!header_entry_allowed(a, Scope::Page, Scope::Page, true));
        }
    }

    #[test]
    fn no_surface_above_the_page_can_send_focus_to_the_header() {
        // The owner's case: Up inside the power menu jumped to the top bar.
        for s in ALL.into_iter().filter(|s| *s != Scope::Page) {
            assert!(!header_entry_allowed(Action::Up, s, s, true), "{s:?}");
            assert!(!header_entry_allowed(Action::Up, s, Scope::Page, true), "closing {s:?}");
            assert!(!header_entry_allowed(Action::Up, Scope::Page, s, true), "opening {s:?}");
        }
    }
}
