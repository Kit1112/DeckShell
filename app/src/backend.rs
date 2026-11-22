//! steam-bridge worker: connects to Steam's SharedJSContext on a background thread (never on the UI thread),
//! loads everything once, then follows the bridge's change events and posts typed updates to the UI thread.
//! When Steam is absent the worker keeps retrying every few seconds; the app runs on steam-data meanwhile.
use std::time::{Duration, Instant};

use steam_bridge::{Bridge, BridgeEvent};

pub use steam_bridge::{App as BApp, ArtSet, Collection as BCollection, Downloads, Notifications, SystemInfo};

/// Everything that makes up the "library" view of Steam (apps incl. not installed, dynamic collections, recents).
#[derive(Debug, Clone, Default)]
pub struct LibData {
    pub apps: Vec<BApp>,
    pub collections: Vec<BCollection>,
    pub recent: Vec<u32>,
    /// Art exactly as Steam resolves it (Steam's URL lists -> local files of the bridge's ACTIVE account).
    /// Empty until the first art pass finished; callers fall back to file-based lookup for missing apps.
    pub art: std::collections::HashMap<u32, ArtSet>,
    /// Active account id from the bridge (0 = unknown).
    pub account: u32,
}

pub enum Update {
    /// Connection state changed; `true` = bridge is the live source.
    Connected(bool),
    Library(Box<LibData>),
    Running(Vec<u32>),
    Downloads(Box<Downloads>),
    Notifications(Box<Notifications>),
    System(Box<SystemInfo>),
    Profile { steamid: u64, name: String, sampled_at: Instant },
}

fn post(u: Update) -> bool {
    slint::invoke_from_event_loop(move || {
        crate::with_app(|a| a.on_backend(u));
    })
    .is_ok()
}

/// Read profile independently of system stores and library artwork.
pub fn refresh_profile(b: &Bridge, target: Option<&str>) -> bool {
    let sampled_at = Instant::now();
    let Ok(snapshot) = b.login_accounts() else { return false };
    let Some((steamid, name)) = snapshot.ready_profile(target) else { return false };
    post(Update::Profile { steamid, name: name.to_owned(), sampled_at })
}

fn load_library(b: &Bridge) -> Option<LibData> {
    let account = b.active_account().ok()?;
    let apps = b.apps().ok()?;
    let collections = b.collections().unwrap_or_default();
    let recent = b.recent_apps().unwrap_or_default();
    // Identity must not depend on a later, optional artwork-resolution pass.
    if b.active_account().ok()? != account { return None; }
    Some(LibData { apps, collections, recent, account, ..Default::default() })
}

/// Ask Steam for the art URL lists of every app and map them to local files (see steam_bridge::art).
fn load_art(b: &Bridge, lib: &LibData) -> Option<(u32, std::collections::HashMap<u32, ArtSet>)> {
    let root = steam_data::default_root()?;
    let ids: Vec<u32> = lib.apps.iter().map(|a| a.appid).collect();
    let t = Instant::now();
    match b.art_all(&root, &ids) {
        Ok(r) => {
            eprintln!("[deckshell] bridge: art for {} apps resolved by Steam (account {}) in {:?}", r.1.len(), r.0, t.elapsed());
            Some(r)
        }
        Err(e) => {
            eprintln!("[deckshell] bridge: art resolution failed: {e}");
            None
        }
    }
}

/// Library without art first (fast), then the same library with Steam-resolved art (first pass waits for AppDetails).
fn post_library_with_art(b: &Bridge, mut lib: LibData, cached: &mut Option<(u32, std::collections::HashMap<u32, ArtSet>)>) -> bool {
    if cached.as_ref().is_some_and(|(account, _)| *account != lib.account) { *cached = None; }
    if let Some((acct, art)) = cached.clone() {
        lib.account = acct;
        lib.art = art;
    }
    let first = cached.is_none();
    if !post(Update::Library(Box::new(lib.clone()))) {
        return false;
    }
    if let Some((acct, art)) = load_art(b, &lib) {
        // Identity may change while Steam resolves AppDetails on this worker.
        if acct != lib.account { return true; }
        let changed = cached.as_ref().map(|(a, m)| *a != acct || *m != art).unwrap_or(true);
        if changed || first {
            lib.account = acct;
            lib.art = art.clone();
            *cached = Some((acct, art));
            return post(Update::Library(Box::new(lib)));
        }
    }
    true
}

fn load_all(b: &Bridge, art: &mut Option<(u32, std::collections::HashMap<u32, ArtSet>)>) -> bool {
    let t = Instant::now();
    let Some(lib) = load_library(b) else { return false };
    let n = lib.apps.len();
    if !post_library_with_art(b, lib, art) {
        return false;
    }
    if let Ok(r) = b.running_apps() {
        post(Update::Running(r));
    }
    if let Ok(d) = b.downloads() {
        post(Update::Downloads(Box::new(d)));
    }
    if let Ok(n) = b.notifications() {
        post(Update::Notifications(Box::new(n)));
    }
    if let Ok(s) = b.system() {
        post(Update::System(Box::new(s)));
    }
    eprintln!("[deckshell] bridge: {n} apps loaded in {:?}", t.elapsed());
    true
}

/// 0 hidden, 1 shown; stock default `bShowBatteryPercentage: false` until Steam says otherwise.
static PERCENT_PREF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Page toggle path: the header follows at once instead of waiting for the next poll.
pub fn set_percent_pref(v: bool) { PERCENT_PREF.store(v, std::sync::atomic::Ordering::Relaxed); }

/// Page writes of the percentage preference that have not been read back yet; the poll leaves the flag alone meanwhile
/// (a read that started before the write landed used to flip the header and the page back).
pub static PERCENT_WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub const READ_PERCENT_PREF: &str = "(()=>{try{const p=settingsStore.GetBatteryPreferences();return JSON.stringify(p&&p.Value!==undefined?p.Value:p)}catch(e){return null}})()";

fn poll_percent_pref() {
    use std::sync::atomic::Ordering::Relaxed;
    if PERCENT_WRITES.load(Relaxed) > 0 { return; }
    let Some(b) = crate::actions::bridge() else { return };
    if let Ok(v) = b.eval_json(READ_PERCENT_PREF) {
        if let Some(flag) = battery_logic::pref_show(&v) {
            // a write that started while this read ran owns the flag
            if PERCENT_WRITES.load(Relaxed) > 0 || PERCENT_PREF.swap(flag, Relaxed) == flag { return; }
            // changed in Steam (or elsewhere): an open power page follows
            crate::screens::defer(move |a| a.power_percent_external(flag));
        }
    }
}

/// Stock header `Tr`: region 1 (never region 2), `neverShowBatteryPercent` false, the full-battery hint is not drawn here.
pub fn header_percent_shown(full: bool) -> bool {
    battery_logic::show_header_percent(PERCENT_PREF.load(std::sync::atomic::Ordering::Relaxed), false, 1, full, false)
}

#[cfg(test)]
#[test]
fn header_percent_follows_pref() {
    set_percent_pref(false);
    assert!(!header_percent_shown(false));
    set_percent_pref(true);
    assert!(header_percent_shown(false));
    assert!(!header_percent_shown(true), "full battery hides it without the hint");
    set_percent_pref(false);
}

/// Start the worker thread (no-op when `DECKSHELL_NOBRIDGE=1`).
pub fn spawn_battery() {
    std::thread::spawn(|| {
        let mut last = String::new();
        let mut tick = 0u32;
        loop {
            // stock BatteryPreferences, re-read every 2 s so a change made in Steam or on the Power page reaches the header
            if tick % 4 == 0 { poll_percent_pref(); }
            tick = tick.wrapping_add(1);
            let battery = deck_system::battery();
            let show = header_percent_shown(battery.as_ref().map(|b| b.full).unwrap_or(false));
            let state = format!("{:?}{show}", battery.as_ref().map(|b| (b.percent, b.online, b.charging, b.full)));
            if state != last {
                eprintln!("[deckshell] battery state {state}");
                last = state;
                let _ = slint::invoke_from_event_loop(move || {
                    use slint::ComponentHandle;
                    crate::with_app(|a| {
                        let ui = a.ui();
                        let tb = ui.global::<crate::TopBarState>();
                        if let Some(b) = battery {
                            let visual = crate::assets::battery_visual(Some(b.percent as f64 / 100.0), b.online.unwrap_or(b.charging), b.full);
                            tb.set_bat_pct(visual.percent);
                            tb.set_bat_full(visual.full);
                            tb.set_bat_show_pct(show);
                            tb.set_bat_text(battery_logic::percent_text(b.percent as f64 / 100.0).into());
                            tb.set_bat_charging(visual.bolt);
                            tb.set_battery_icon(crate::assets::battery_icon(b.percent as f64 / 100.0, b.online.unwrap_or(b.charging), b.full));
                            tb.set_battery_icon_dark(crate::assets::battery_icon_dark(b.percent as f64 / 100.0, b.online.unwrap_or(b.charging), b.full));
                        } else { tb.set_bat_pct(-1); }
                    });
                });
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

pub fn spawn() {
    if std::env::var("DECKSHELL_NOBRIDGE").map(|v| v != "0" && !v.is_empty()).unwrap_or(false) {
        eprintln!("[deckshell] bridge: disabled (DECKSHELL_NOBRIDGE)");
        return;
    }
    let _ = std::thread::Builder::new().name("steam-bridge".into()).spawn(|| loop {
        let b = match Bridge::connect() {
            Ok(b) => b,
            Err(_) => {
                std::thread::sleep(Duration::from_secs(4));
                continue;
            }
        };
        crate::actions::set_bridge(Some(b.clone()));
        let rx = b.subscribe();
        let mut art_cache = None;
        let mut last_sys = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(BridgeEvent::Connected) => {
                    art_cache = None;
                    // A websocket handshake can precede Steam store initialization.
                    // Publish the live source only after the first successful fingerprint.
                    if !post(Update::Connected(true)) {
                        return;
                    }
                    refresh_profile(&b, None);
                    crate::steam_ui_guard::on_bridge_connected(&b);
                    if !load_all(&b, &mut art_cache) {
                        break;
                    }
                }
                Ok(BridgeEvent::Disconnected) => break,
                Ok(BridgeEvent::LibraryChanged) => {
                    if let Some(l) = load_library(&b) {
                        if !post_library_with_art(&b, l, &mut art_cache) {
                            return;
                        }
                    }
                }
                Ok(BridgeEvent::DownloadsChanged) => {
                    if let Ok(d) = b.downloads() {
                        post(Update::Downloads(Box::new(d)));
                    }
                }
                Ok(BridgeEvent::NotificationsChanged) => {
                    if let Ok(n) = b.notifications() {
                        post(Update::Notifications(Box::new(n)));
                    }
                }
                Ok(BridgeEvent::RunningAppChanged(ids)) => {
                    post(Update::Running(ids));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            // Refresh system data and retry profiles whose cache files were not yet ready.
            if last_sys.elapsed() > Duration::from_secs(5) {
                last_sys = Instant::now();
                refresh_profile(&b, None);
                if let Ok(s) = b.system() {
                    post(Update::System(Box::new(s)));
                }
            }
        }
        crate::actions::set_bridge(None);
        post(Update::Connected(false));
        std::thread::sleep(Duration::from_secs(2));
    });
}

/// Reload all account-scoped data before dismissing the login cover.
pub fn reload_account(b: &Bridge) -> bool { load_all(b, &mut None) }
