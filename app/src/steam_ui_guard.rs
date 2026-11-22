//! Steam's own Gamepad UI (Big Picture) must never be on screen while DeckShell runs
//! (docs/steam-ui-never-2026-11-20.md).
//!
//! Evidence from the Deck (read-only, 2026-11-20):
//! * Steam opens the Deck controller's hidraw node itself (`Added HIDAPI device 'Steam Deck' … /dev/hidraw2`),
//!   so every STEAM press reaches the headless (`-silent`) client too: `controller_ui.txt` logs
//!   `Guide button sent to JS`, and `steamui_html.txt` logs `CreateMainWindow` right after it.
//!   hidraw has no exclusive grab, and Steam needs that node for Steam Input in games.
//! * The headless client sometimes asks for the Gamepad UI window itself: `CreateMainWindow: UIMode: 4`
//!   / `ensure gamepadui window exists` (18.11 14:04:27, 17:57:56, 18:43:34; 19.11 19:48:41; 20.11 19:13:38),
//!   with Steam's own JS complaining "DesktopUI windows should only ever be present when the main
//!   instance's UI mode is desktop". Two of them came with a STEAM press while Steam was offline.
//!
//! Two parts here:
//! 1. `controller_guide_button_focus_steam` ("Guide button focuses Steam", a desktop-mode client setting) is
//!    switched off while DeckShell runs, with the previous value kept for `--restore-steam-guide-focus`.
//! 2. A hard guard, independent of the cause: the gamescope worker hands every scan of the X windows to
//!    `Guard::step`. A mapped Steam window marked `STEAM_BIGPICTURE`, or a Steam-process window marked
//!    `STEAM_OVERLAY`, that DeckShell did not ask for is reported with its reason and closed with
//!    `steam://close/bigpicture` over steam.pipe (never starts Steam, never touches a game), and the
//!    worker re-asserts DeckShell's base layer.
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const STEAM_APPID: u32 = 769;
pub const GUIDE_FOCUS_SETTING: &str = "controller_guide_button_focus_steam";
pub const CLOSE_URL: &str = "steam://close/bigpicture";

/// What the gamescope worker read for one top-level X window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamWindow {
    pub xid: u32,
    pub pid: u32,
    /// `STEAM_GAME`
    pub appid: u32,
    /// `STEAM_BIGPICTURE`
    pub bigpicture: u32,
    /// `STEAM_OVERLAY`
    pub overlay: u32,
    pub mapped: bool,
    /// `/proc/<pid>/comm`
    pub comm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The Gamepad UI main window (`STEAM_BIGPICTURE`).
    GamepadUi,
    /// A Steam UI window drawn by gamescope over everything (`STEAM_OVERLAY` on a Steam process window).
    SteamOverlay,
}

/// Steam's client and its browser process. The in-game overlay (`gameoverlayui`) belongs to the game.
pub fn is_steam_comm(comm: &str) -> bool {
    matches!(comm, "steam" | "steamwebhelper") || comm.starts_with("steamwebhelper")
}

/// The Steam UI window on screen that nobody in DeckShell asked for, if any.
/// `delegated`: DeckShell itself handed the screen to Steam (store/install wizard), so Steam windows are wanted.
pub fn foreign_steam_ui<'a>(windows: &'a [SteamWindow], own_pid: u32, delegated: bool) -> Option<(&'a SteamWindow, Kind)> {
    if delegated {
        return None;
    }
    let steam = |w: &SteamWindow| w.pid != own_pid && (w.appid == STEAM_APPID || is_steam_comm(&w.comm));
    windows.iter().filter(|w| w.mapped && steam(w)).find_map(|w| {
        if w.bigpicture != 0 {
            Some((w, Kind::GamepadUi))
        } else if w.overlay != 0 && is_steam_comm(&w.comm) {
            Some((w, Kind::SteamOverlay))
        } else {
            None
        }
    })
}

/// First close request at once, then every `RETRY` while the window stays, at most `MAX_CLOSE` per episode.
pub const RETRY: Duration = Duration::from_secs(3);
pub const MAX_CLOSE: u32 = 5;
const STILL_LOG: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Log the line; send `CLOSE_URL` when `close`; re-assert DeckShell's base layer.
    Act { line: String, close: bool },
    /// Log only.
    Log(String),
}

#[derive(Default)]
pub struct Guard {
    since: Option<Instant>,
    last_close: Option<Instant>,
    closes: u32,
    last_still: Option<Instant>,
    xid: u32,
}

impl Guard {
    pub fn step(&mut self, found: Option<(&SteamWindow, Kind)>, game: Option<u32>, now: Instant) -> Step {
        let Some((w, kind)) = found else {
            if let Some(since) = self.since.take() {
                let ms = now.duration_since(since).as_millis();
                let closes = self.closes;
                *self = Guard::default();
                return Step::Log(format!("[steam-ui-guard] Steam UI window gone after {ms} ms ({closes} close request(s))"));
            }
            return Step::Nothing;
        };
        let what = format!(
            "{kind:?} xid={:#x} pid={} comm={} appid={} bigpicture={} overlay={} game={}",
            w.xid, w.pid, if w.comm.is_empty() { "?" } else { &w.comm }, w.appid, w.bigpicture, w.overlay,
            game.map_or("none".to_string(), |g| g.to_string())
        );
        if self.since.is_none() || (self.xid != w.xid && self.closes < MAX_CLOSE) {
            let first = self.since.is_none();
            if first {
                self.since = Some(now);
                self.closes = 0;
            }
            self.xid = w.xid;
            self.last_close = Some(now);
            self.closes += 1;
            let under = if game.is_some() { "; the game keeps running, only Steam's UI is closed" } else { "" };
            return Step::Act { line: format!("[steam-ui-guard] FOREIGN STEAM UI on screen: {what} -> {CLOSE_URL} over steam.pipe{under}"), close: true };
        }
        if self.closes < MAX_CLOSE && self.last_close.is_some_and(|t| now.duration_since(t) >= RETRY) {
            self.last_close = Some(now);
            self.closes += 1;
            return Step::Act { line: format!("[steam-ui-guard] still on screen ({}/{MAX_CLOSE}): {what} -> {CLOSE_URL} again", self.closes), close: true };
        }
        if self.closes >= MAX_CLOSE && self.last_still.is_none_or(|t| now.duration_since(t) >= STILL_LOG) {
            self.last_still = Some(now);
            let secs = self.since.map_or(0, |s| now.duration_since(s).as_secs());
            return Step::Log(format!("[steam-ui-guard] STILL ON SCREEN after {secs} s and {MAX_CLOSE} close requests: {what}; base layer kept on DeckShell"));
        }
        Step::Nothing
    }
}

/// Send the close request to the running Steam. Never starts Steam; nothing under DECKSHELL_DRYRUN.
pub fn request_close() {
    if crate::actions::dryrun() {
        eprintln!("[steam-ui-guard] DRYRUN: {CLOSE_URL} not sent");
        return;
    }
    if std::env::var("DECKSHELL_STEAM_UI_GUARD").is_ok_and(|v| v == "log") {
        eprintln!("[steam-ui-guard] {CLOSE_URL} not sent (DECKSHELL_STEAM_UI_GUARD=log)");
        return;
    }
    let _ = steam_control::open_url(CLOSE_URL);
}

// ---------------------------------------------------------------- Guide button setting

fn backup_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/state"))
        .join("deckshell/steam-guide-focus.orig")
}

pub const READ_GUIDE_FOCUS: &str =
    "(()=>{const s=window.settingsStore;if(!s)return {store:false};const c=s.m_ClientSettings||s.clientSettings||{};const v=c.controller_guide_button_focus_steam;return {store:true,value:v===undefined?null:v}})()";

/// What to do with the stored setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuidePlan {
    /// The store is not ready or the key is unknown: try again on the next connection.
    Unknown,
    AlreadyOff,
    /// Switch off; `backup` = the value to keep unless a backup already exists.
    TurnOff { backup: Option<bool> },
}

pub fn guide_plan(read: &serde_json::Value, backup_exists: bool) -> GuidePlan {
    if read.get("store").and_then(|v| v.as_bool()) != Some(true) {
        return GuidePlan::Unknown;
    }
    match read.get("value").and_then(|v| v.as_bool()) {
        None => GuidePlan::Unknown,
        Some(false) => GuidePlan::AlreadyOff,
        Some(true) => GuidePlan::TurnOff { backup: (!backup_exists).then_some(true) },
    }
}

fn keep_guide() -> bool {
    std::env::var("DECKSHELL_KEEP_STEAM_GUIDE_FOCUS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Called by the backend worker on each bridge connection. Runs on its own thread.
pub fn on_bridge_connected(b: &steam_bridge::Bridge) {
    // Only in the DeckShell session: a desktop test run must not change the PC's Steam.
    if !crate::env_flag("DECKSHELL_GS_BASE") {
        return;
    }
    if keep_guide() {
        eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING} left as is (DECKSHELL_KEEP_STEAM_GUIDE_FOCUS)");
        return;
    }
    let b = b.clone();
    let _ = std::thread::Builder::new().name("steam-ui-guard".into()).spawn(move || {
        let read = match b.eval_json(READ_GUIDE_FOCUS) {
            Ok(v) => v,
            Err(e) => return eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING}: read failed: {e}"),
        };
        let path = backup_path();
        match guide_plan(&read, path.exists()) {
            GuidePlan::Unknown => eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING}: not readable yet ({read})"),
            GuidePlan::AlreadyOff => eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING}=false (STEAM press does not focus Steam)"),
            GuidePlan::TurnOff { backup } => {
                if crate::actions::dryrun() {
                    return eprintln!("[steam-ui-guard] DRYRUN: {GUIDE_FOCUS_SETTING} is true; would set false");
                }
                if let Some(v) = backup {
                    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
                    if let Err(e) = std::fs::write(&path, format!("{v}\n")) {
                        // Without a backup the owner's value could not be restored: leave Steam unchanged.
                        return eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING}: backup {} failed: {e}; setting left unchanged", path.display());
                    }
                }
                let wrote = b.set_client_setting(GUIDE_FOCUS_SETTING, "false");
                std::thread::sleep(Duration::from_millis(300));
                let after = b.eval_json(READ_GUIDE_FOCUS).ok().and_then(|v| v.get("value").and_then(|v| v.as_bool()));
                eprintln!("[steam-ui-guard] {GUIDE_FOCUS_SETTING} true -> false: write {wrote:?}, read back {after:?} (backup {})", path.display());
            }
        }
    });
}

/// `deckshell --restore-steam-guide-focus`: put back the value saved before DeckShell switched it off.
pub fn restore_cli() -> i32 {
    let path = backup_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        println!("no backup at {}: nothing to restore", path.display());
        return 0;
    };
    let value = match text.trim() { "true" => "true", "false" => "false", other => {
        eprintln!("backup {} holds {other:?}, not true/false", path.display());
        return 2;
    } };
    let b = match steam_bridge::Bridge::connect() {
        Ok(b) => b,
        Err(e) => { eprintln!("Steam bridge unavailable: {e}"); return 1; }
    };
    match b.set_client_setting(GUIDE_FOCUS_SETTING, value) {
        Ok(_) => {
            let _ = std::fs::remove_file(&path);
            println!("{GUIDE_FOCUS_SETTING} = {value} restored");
            0
        }
        Err(e) => { eprintln!("restore failed: {e}"); 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win(xid: u32, pid: u32, comm: &str, appid: u32, bp: u32, overlay: u32, mapped: bool) -> SteamWindow {
        SteamWindow { xid, pid, appid, bigpicture: bp, overlay, mapped, comm: comm.into() }
    }

    /// The window list read on the Deck on 2026-11-20 19:56 (DeckShell on screen): nothing foreign.
    fn deck_idle() -> Vec<SteamWindow> {
        vec![
            win(0x80000b, 1817, "deckshell", 233594385, 0, 0, true),
            win(0x260003f, 4363, "steamwebhelper", 769, 0, 0, false),
            win(0x2600097, 4363, "steamwebhelper", 769, 0, 0, false),
            win(0x1e00003, 4363, "steamwebhelper", 769, 0, 0, false),
            win(0x1200041, 4309, "steam", 0, 0, 0, false),
            win(0x800003, 1817, "deckshell", 0, 0, 0, true),
            win(0x400007, 1677, "mangoapp", 0, 0, 0, true),
        ]
    }

    #[test]
    fn idle_deck_has_no_foreign_ui() {
        assert_eq!(foreign_steam_ui(&deck_idle(), 1817, false), None);
    }

    #[test]
    fn mapped_bigpicture_window_is_foreign() {
        let mut w = deck_idle();
        w[1].mapped = true;
        w[1].bigpicture = 1;
        let (f, k) = foreign_steam_ui(&w, 1817, false).unwrap();
        assert_eq!((f.xid, k), (0x260003f, Kind::GamepadUi));
        // Unmapped Gamepad UI is not on screen.
        w[1].mapped = false;
        assert_eq!(foreign_steam_ui(&w, 1817, false), None);
    }

    #[test]
    fn steam_overlay_is_foreign_but_game_overlay_and_own_windows_are_not() {
        let mut w = deck_idle();
        w.push(win(0x3000001, 4363, "steamwebhelper", 769, 0, 1, true));
        assert_eq!(foreign_steam_ui(&w, 1817, false).map(|(f, k)| (f.xid, k)), Some((0x3000001, Kind::SteamOverlay)));
        let mut g = deck_idle();
        g.push(win(0x3100001, 7000, "gameoverlayui", 367520, 0, 1, true));
        g.push(win(0x80000c, 1817, "deckshell", 769, 1, 1, true)); // own window, whatever its marks
        assert_eq!(foreign_steam_ui(&g, 1817, false), None);
    }

    #[test]
    fn explicit_delegation_to_steam_is_respected() {
        let mut w = deck_idle();
        w[1].mapped = true;
        w[1].bigpicture = 1;
        assert_eq!(foreign_steam_ui(&w, 1817, true), None);
    }

    #[test]
    fn plain_desktop_steam_window_is_left_to_the_base_layer() {
        // A desktop window (store page opened on purpose) has no BIGPICTURE/OVERLAY mark.
        let mut w = deck_idle();
        w[1].mapped = true;
        assert_eq!(foreign_steam_ui(&w, 1817, false), None);
    }

    #[test]
    fn guard_closes_at_once_retries_and_stops_after_budget() {
        let t = Instant::now();
        let w = win(0x260003f, 4363, "steamwebhelper", 769, 1, 0, true);
        let mut g = Guard::default();
        let s = g.step(Some((&w, Kind::GamepadUi)), Some(367520), t);
        match s {
            Step::Act { line, close } => {
                assert!(close);
                assert!(line.contains("FOREIGN STEAM UI") && line.contains("game=367520") && line.contains("game keeps running"), "{line}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(g.step(Some((&w, Kind::GamepadUi)), None, t + Duration::from_secs(1)), Step::Nothing);
        let mut closes = 1;
        let mut now = t;
        for _ in 0..20 {
            now += RETRY;
            if let Step::Act { close: true, .. } = g.step(Some((&w, Kind::GamepadUi)), None, now) { closes += 1; }
        }
        assert_eq!(closes, MAX_CLOSE);
        match g.step(Some((&w, Kind::GamepadUi)), None, now + STILL_LOG) {
            Step::Log(l) => assert!(l.contains("STILL ON SCREEN"), "{l}"),
            Step::Nothing => {}
            other => panic!("{other:?}"),
        }
        match g.step(None, None, now + STILL_LOG + Duration::from_secs(1)) {
            Step::Log(l) => assert!(l.contains("gone after"), "{l}"),
            other => panic!("{other:?}"),
        }
        // A new episode starts fresh.
        assert!(matches!(g.step(Some((&w, Kind::GamepadUi)), None, now + STILL_LOG * 2), Step::Act { close: true, .. }));
    }

    #[test]
    fn guide_setting_plan() {
        assert_eq!(guide_plan(&json!({"store":false}), false), GuidePlan::Unknown);
        assert_eq!(guide_plan(&json!({"store":true,"value":null}), false), GuidePlan::Unknown);
        assert_eq!(guide_plan(&json!({"store":true,"value":false}), false), GuidePlan::AlreadyOff);
        assert_eq!(guide_plan(&json!({"store":true,"value":true}), false), GuidePlan::TurnOff { backup: Some(true) });
        // A backup from an earlier run holds the owner's value; never overwrite it with ours.
        assert_eq!(guide_plan(&json!({"store":true,"value":true}), true), GuidePlan::TurnOff { backup: None });
    }

    #[test]
    fn close_url_never_opens_the_gamepad_ui() {
        assert!(CLOSE_URL.starts_with("steam://close/"));
        assert!(!CLOSE_URL.contains("open"));
    }
}
