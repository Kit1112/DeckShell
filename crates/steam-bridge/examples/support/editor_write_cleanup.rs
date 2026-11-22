//! Conservative policy for the destructive live example; also exercised by offline tests.
use steam_bridge::controller_list::ActiveLayout;

pub fn same_selection(a: &ActiveLayout, b: &ActiveLayout) -> bool {
    a.url == b.url && a.selection_type == b.selection_type
}

/// Refuse saves that could overwrite an existing personal autosave, including an inactive one.
/// `None` means the inventory could not be proved complete, never an empty inventory.
pub fn can_write(before: &ActiveLayout, editing_url: &str, inventory: Option<&[String]>) -> bool {
    !before.url.is_empty()
        && before.url == editing_url
        && !before.url.starts_with("autosave://")
        && inventory.is_some_and(|urls| !urls.iter().any(|u| u.starts_with("autosave://")))
}

/// A final active URL alone is not evidence of ownership. Require a complete baseline,
/// the copy observed immediately after our save, and an unchanged final selection.
pub fn created_copy<'a>(
    before: &ActiveLayout,
    editing_url: &str,
    inventory: Option<&[String]>,
    saved: Option<&'a ActiveLayout>,
    final_active: Option<&ActiveLayout>,
    writes_verified: bool,
) -> Option<&'a ActiveLayout> {
    if !writes_verified || !can_write(before, editing_url, inventory) {
        return None;
    }
    let saved = saved?;
    let final_active = final_active?;
    (saved.url.starts_with("autosave://")
        && saved.url.len() > "autosave://".len()
        && saved.url != before.url
        && same_selection(saved, final_active))
    .then_some(saved)
}

/// Deletion must also be withheld when reselection failed or its readback is unavailable.
pub fn restoration_verified(before: &ActiveLayout, after: Option<&ActiveLayout>, select_ok: bool) -> bool {
    select_ok && after.is_some_and(|a| same_selection(a, before))
}
