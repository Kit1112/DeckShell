//! Pure rules for the game-page gear "Управление" submenu (gamepad build: `bInGamepadUI = true`).
//! Source: sp.js `Mt` (498440, per-app action list), `BuildManageSubmenu` (498880). See context/archive/docs/manage/SPEC.md.

/// sp.js app_type constants (module 80954).
pub const AT_GAME: u32 = 1;
pub const AT_SOFTWARE: u32 = 2;
pub const AT_DLC_LIKE: u32 = 4; // `B7`
pub const AT_DEMO: u32 = 8;
pub const AT_TOOL: u32 = 8192; // `H4`: only "Uninstall" when installed
pub const AT_NONE: u32 = 2048; // `em`: no actions
pub const AT_P_: u32 = 65536; // `P_`
pub const AT_SHORTCUT: u32 = 1_073_741_824; // `Pe`

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Allowed {
    pub uninstall: bool,
    pub remove_shortcut: bool,
}

/// sp.js `Mt` reduced to the two actions that exist in the gamepad Manage menu (`a.ZX` Uninstall, `a.hA` RemoveShortcut).
/// `blocked` = `BIsAppBlocked()` (Family View). The Deck client is always the "local" per-client data.
pub fn allowed(app_type: u32, display_status: u32, installed: bool, blocked: bool) -> Allowed {
    let none = Allowed::default();
    if blocked {
        return none;
    }
    match app_type {
        AT_TOOL => Allowed { uninstall: installed, ..none },
        AT_NONE => none,
        AT_SHORTCUT => Allowed { remove_shortcut: true, ..none },
        AT_GAME | AT_SOFTWARE | AT_DLC_LIKE | AT_DEMO | AT_P_ => match display_status {
            // EAppDisplayStatus values from the first case group of `Mt`: [DesktopShortcut, Backup, Uninstall] when local
            1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 11 | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 24 | 25 | 29 | 31 | 34 | 35 | 37 | 38 | 39 => {
                Allowed { uninstall: true, ..none }
            }
            // second group: only when installed
            27 | 28 | 32 | 33 => Allowed { uninstall: installed, ..none },
            // 9, 10, 12, 13, 14, 30 and unknown: nothing
            _ => none,
        },
        _ => none,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ManageEntry {
    RemoveFromHidden,
    AddToFamilyGames,
    RemoveFromFamilyGames,
    CdKeys,
    AddToHidden,
    /// `Me` (sp.js:499184); `disabled` for mods / shortcuts (label gets `#GameAction_MarkAsPrivate_NoShortcuts`).
    MarkPrivate { unmark: bool, disabled: bool },
    RemoveLicense,
    RemoveShortcut,
    Uninstall,
}

/// Inputs of the gamepad Manage submenu for one app.
#[derive(Clone, Copy, Default, Debug)]
pub struct Flags {
    pub hidden: bool,
    pub cdkeys: bool,
    /// Family View: `Q.jR.isEnabled && !isParentalLocked`.
    pub family_active: bool,
    /// `BIsAppInBlockList()`
    pub in_block_list: bool,
    /// `BIsModOrShortcut()`
    pub mod_or_shortcut: bool,
    /// private-apps data loaded (`M.Pt()`) and feature flag `ze.Id()` on; `Some(app is already private)`.
    pub private: Option<bool>,
    /// `pt` (sp.js:499205): bFreeRemovableLicense (&& (not demo || setting)) || unMasterSubAppID
    pub removable_license: bool,
}

/// `pt` condition.
pub fn removable_license(free_removable: bool, master_sub_app: u32, is_demo: bool, show_uninstalled_demos: bool) -> bool {
    (free_removable && (!is_demo || show_uninstalled_demos)) || master_sub_app != 0
}

/// Item order of the gamepad Manage submenu (BuildManageSubmenu, sp.js:498880):
/// RemoveFromHidden, AddToFamilyGames, RemoveFromFamilyGames, CDKeys, AddToHidden, Private, RemoveLicense, RemoveShortcut, [sep] Uninstall.
pub fn entries(f: Flags, a: Allowed) -> Vec<ManageEntry> {
    let mut v = vec![];
    if f.hidden {
        v.push(ManageEntry::RemoveFromHidden);
    }
    if f.family_active && !f.mod_or_shortcut {
        v.push(if f.in_block_list { ManageEntry::AddToFamilyGames } else { ManageEntry::RemoveFromFamilyGames });
    }
    if f.cdkeys {
        v.push(ManageEntry::CdKeys);
    }
    if !f.hidden {
        v.push(ManageEntry::AddToHidden);
    }
    if let Some(p) = f.private {
        v.push(ManageEntry::MarkPrivate { unmark: p, disabled: f.mod_or_shortcut });
    }
    if f.removable_license {
        v.push(ManageEntry::RemoveLicense);
    }
    if a.remove_shortcut {
        v.push(ManageEntry::RemoveShortcut);
    }
    if a.uninstall {
        v.push(ManageEntry::Uninstall);
    }
    v
}

#[cfg(test)]
mod manage_tests {
    use super::*;
    use ManageEntry::*;

    #[test]
    fn installed_steam_game() {
        let a = allowed(AT_GAME, 11, true, false);
        assert_eq!(entries(Flags::default(), a), vec![AddToHidden, Uninstall]);
        assert_eq!(entries(Flags { hidden: true, cdkeys: true, ..Default::default() }, a), vec![RemoveFromHidden, CdKeys, Uninstall]);
    }
    #[test]
    fn not_installed_steam_game() {
        // display_status 1 (not installed in the first group) still offers Uninstall (sp.js Mt: `[ZX]` for remote-only)
        assert!(allowed(AT_GAME, 1, false, false).uninstall);
        assert!(!allowed(AT_GAME, 30, false, false).uninstall);
    }
    #[test]
    fn shortcut() {
        let a = allowed(AT_SHORTCUT, 11, true, false);
        assert_eq!(entries(Flags::default(), a), vec![AddToHidden, RemoveShortcut]);
    }
    #[test]
    fn updating_and_downloading_keep_uninstall() {
        for ds in [22, 23, 24, 25, 38] {
            assert!(allowed(AT_GAME, ds, true, false).uninstall, "ds {ds}");
        }
    }
    #[test]
    fn blocked_and_tool() {
        assert_eq!(allowed(AT_GAME, 11, true, true), Allowed::default());
        assert!(allowed(AT_TOOL, 0, true, false).uninstall);
        assert!(!allowed(AT_TOOL, 0, false, false).uninstall);
        assert_eq!(allowed(AT_NONE, 11, true, false), Allowed::default());
    }

    #[test]
    fn family_private_license() {
        let a = allowed(AT_GAME, 11, true, false);
        let f = Flags { family_active: true, in_block_list: true, private: Some(false), removable_license: true, ..Default::default() };
        assert_eq!(
            entries(f, a),
            vec![AddToFamilyGames, AddToHidden, MarkPrivate { unmark: false, disabled: false }, RemoveLicense, Uninstall]
        );
        let f = Flags { family_active: true, in_block_list: false, ..Default::default() };
        assert_eq!(entries(f, a), vec![RemoveFromFamilyGames, AddToHidden, Uninstall]);
        // shortcuts never get Family items; private is disabled for them
        let sc = allowed(AT_SHORTCUT, 11, true, false);
        let f = Flags { family_active: true, mod_or_shortcut: true, private: Some(true), ..Default::default() };
        assert_eq!(entries(f, sc), vec![AddToHidden, MarkPrivate { unmark: true, disabled: true }, RemoveShortcut]);
    }
    #[test]
    fn license_rule() {
        assert!(removable_license(true, 0, false, false));
        assert!(!removable_license(true, 0, true, false));
        assert!(removable_license(true, 0, true, true));
        assert!(removable_license(false, 7, false, false));
        assert!(!removable_license(false, 0, false, false));
    }
}
