//! What a button does on a chooser card (task 505). Pure decision table: the UI executes the returned `Plan`
//! through the bridge (writes only outside DECKSHELL_DRYRUN), tests pin the table.
//!
//! Buttons on a focused card: A preview, X apply from preview, Y own copy from a template,
//! Menu delete a personal layout (second press confirms). Publish and export are not offered here (see docs/community-layouts-todo.md).

use super::controller_browser::Tab;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button { A, X, Y, Menu }

/// Facts about the focused card that decide the outcome.
#[derive(Clone, Debug, Default)]
pub struct Card { pub url: String, pub personal: bool, pub template: bool, pub community: bool, pub active: bool, pub selection_type: u32 }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Plan {
    Preview,
    /// SetSelectedConfigForApp(url); a community item downloads on selection.
    Apply { url: String, selection_type: u32 },
    /// Template copy: select, edit, ConvertConfigToSelectionType(1).
    CreateFromTemplate { url: String },
    /// First Y on an own layout: ask again.
    ConfirmDelete { url: String },
    /// Second Y on the same layout.
    Delete { url: String, selected_type: Option<u32> },
    Info(&'static str),
    Nothing,
}

pub const ALREADY: &str = "controller.this_layout_is_already_selected";
pub const NO_COPY: &str = "controller.you_can_only_create_a_copy_from_a";
pub const NO_DELETE: &str = "controller.you_can_only_delete_your_own_layouts";

/// `pending`: the url awaiting delete confirmation, if any.
pub fn decide(tab: Tab, card: &Card, button: Button, pending: Option<&str>) -> Plan {
    match button {
        Button::A if card.active => Plan::Info(ALREADY),
        Button::A => Plan::Preview,
        Button::X if card.active => Plan::Info(ALREADY),
        Button::X => Plan::Apply { url: card.url.clone(), selection_type: if card.personal { 1 } else { 0 } },
        Button::Y if card.template && tab != Tab::User => Plan::CreateFromTemplate { url: card.url.clone() },
        Button::Menu if card.personal && pending == Some(card.url.as_str()) => Plan::Delete { url: card.url.clone(), selected_type: card.active.then_some(card.selection_type) },
        Button::Menu if card.personal => Plan::ConfirmDelete { url: card.url.clone() },
        Button::Y if tab == Tab::User => Plan::Info(NO_DELETE),
        Button::Y => Plan::Info(NO_COPY),
        Button::Menu => Plan::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn card(url: &str) -> Card { Card { url: url.into(), ..Default::default() } }
    fn tpl() -> Card { Card { template: true, ..card("template://controller_neptune_gamepad_joystick.vdf") } }
    fn own() -> Card { Card { personal: true, selection_type: 1, ..card("autosave://a/2379780/X.vdf") } }
    fn com() -> Card { Card { community: true, ..card("workshop://3177015367") } }

    /// The table of the docs (element/state/button -> result), one assertion per row.
    #[test]
    fn button_table() {
        use Button::*;
        let apply = |u: &str| Plan::Apply { url: u.into(), selection_type: u.starts_with("autosave://") as u32 };
        assert_eq!(decide(Tab::Community, &com(), A, None), Plan::Preview);
        assert_eq!(decide(Tab::Community, &com(), X, None), apply("workshop://3177015367"));
        assert_eq!(decide(Tab::Templates, &tpl(), X, None), apply("template://controller_neptune_gamepad_joystick.vdf"));
        assert_eq!(decide(Tab::User, &own(), X, None), apply("autosave://a/2379780/X.vdf"));
        assert_eq!(decide(Tab::Community, &Card { active: true, ..com() }, A, None), Plan::Info(ALREADY));
        assert_eq!(decide(Tab::Templates, &tpl(), Y, None), Plan::CreateFromTemplate { url: tpl().url });
        assert_eq!(decide(Tab::Recommended, &tpl(), Y, None), Plan::CreateFromTemplate { url: tpl().url });
        assert_eq!(decide(Tab::Community, &com(), Y, None), Plan::Info(NO_COPY));
        assert_eq!(decide(Tab::User, &own(), Menu, None), Plan::ConfirmDelete { url: own().url });
        assert_eq!(decide(Tab::User, &own(), Menu, Some("autosave://other.vdf")), Plan::ConfirmDelete { url: own().url });
        assert_eq!(decide(Tab::User, &own(), Menu, Some(&own().url)), Plan::Delete { url: own().url, selected_type: None });
        assert_eq!(decide(Tab::User, &Card { active: true, ..own() }, Menu, Some(&own().url)), Plan::Delete { url: own().url, selected_type: Some(1) });
        assert_eq!(decide(Tab::User, &com(), Y, None), Plan::Info(NO_DELETE));
        assert_eq!(decide(Tab::Community, &com(), Menu, None), Plan::Nothing);
    }

    #[test]
    fn community_and_template_are_never_deleted() {
        for c in [com(), tpl()] {
            for pending in [None, Some(c.url.as_str())] {
                assert!(!matches!(decide(Tab::User, &c, Button::Menu, pending), Plan::Delete { .. } | Plan::ConfirmDelete { .. }));
            }
        }
    }
}
