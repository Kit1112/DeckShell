//! Every side-effecting action goes through `run()`. `DECKSHELL_DRYRUN=1` logs instead of calling anything
//! (use it for ALL testing: never launch / install / uninstall / power for real while developing).
//! Primary executor is steam-bridge (same SteamClient calls as the native handlers in docs/cdp/behavior/handlers.json);
//! when the bridge is down the steam:// URL / deck-system fallbacks of `DataSource` are used.
use std::sync::{Arc, Mutex, OnceLock};

use steam_bridge::Bridge;
use steam_control::SteamPage;

use crate::data::{DataSource, PowerAction};

#[derive(Debug, Clone)]
pub enum Act {
    /// Play / Resume. `rungame` = steam://rungameid id for the URL fallback.
    Launch { app_id: u32, rungame: u64 },
    Stop(u32),
    /// Open the native DeckShell install modal.
    Install(u32),
    /// Confirmed local removal: only enqueue after the named-game confirmation.
    Uninstall(u32),
    OpenStore(u32),
    Achievements(u32),
    Properties(u32),
    Controller(u32),
    PauseDownload(u32),
    ResumeDownload(u32),
    PauseAllDownloads,
    ResumeAllDownloads,
    Power(PowerAction),
    SetWifi(bool),
    SetBluetooth(bool),
    SetProfile(String),
    NotificationsViewed,
    Open(SteamPage),
    /// SystemPerfStore setter (QAM Performance): method name + JS argument list, e.g. `SetFPSLimit`, `"40"`. Needs the gamescope Perf channel.
    Perf { method: &'static str, args: String },
    /// gamescope root CARDINAL written directly (QAM Performance without the Perf channel, e.g. GAMESCOPE_FPS_LIMIT); readback in gamescope_atoms.
    GsAtom { name: &'static str, value: u32 },
    /// settingsStore client setting write (`steamos_tdp_limit`, `steamos_manual_gpu_clock_hz`, ...), `value` = JSON literal.
    ClientSetting { name: &'static str, value: String },
    SetAirplane(bool),
    UiAutoScale(bool),
    UiScaleFactor(f32),
    OpenChat(u32),
    FriendProfile(u32),
    /// QAM Friends: open a chat room group by id (Favorites / Groups / Recent rows)
    OpenChatGroup(String),
    /// QAM Friends header "Invites" / "Add friend": NavigateToInvites
    FriendsInvites,
    /// QAM Friends header "New group": CreateChatDialog
    CreateChatGroup,
    NotifDismiss(u64),
    OpenAccount,
    StoreSearch(String),
}

pub fn dryrun() -> bool {
    static D: OnceLock<bool> = OnceLock::new();
    *D.get_or_init(|| std::env::var("DECKSHELL_DRYRUN").map(|v| v != "0" && !v.is_empty()).unwrap_or(false) || deck_system::guard::active())
}

static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);
static STEAM_WINDOW_OP: Mutex<()> = Mutex::new(());

/// Called by the backend worker when the bridge (dis)connects.
pub fn set_bridge(b: Option<Bridge>) {
    *BRIDGE.lock().unwrap() = b;
}
pub fn bridge() -> Option<Bridge> {
    BRIDGE.lock().unwrap().clone()
}

/// Fire-and-forget on a short-lived thread (Steam calls may take up to 2 s; the UI thread never waits).
pub fn run(act: Act, ds: &Arc<DataSource>) {
    crate::diag::log("action.request", serde_json::json!({"action":format!("{act:?}").split(['(', '{', ' ']).next().unwrap_or("unknown"),"delegates_ui":delegates_steam_ui(&act),"dryrun":dryrun()}));
    // suspend / shutdown / restart: the animation plays first, then the real action (src/anim)
    if let Act::Power(p @ (PowerAction::Sleep | PowerAction::Restart | PowerAction::Shutdown)) = &act {
        let (ds2, act2) = (ds.clone(), act.clone());
        if crate::anim::power(*p, Box::new(move || run_now(act2, &ds2))) {
            return;
        }
    }
    run_now(act, ds)
}

fn run_now(act: Act, ds: &Arc<DataSource>) {
    if dryrun() {
        eprintln!("[deckshell] DRYRUN {act:?}");
        return;
    }
    // Steam windows may already exist, so detecting only a newly seen appid cannot hand over focus.
    // This is after DRYRUN and only consumed by gamescope in our shell session.
    if delegates_steam_ui(&act) {
        crate::gamescope::request_steam_ui();
    } else if let Act::Launch { app_id, .. } = &act {
        // Play/Resume is an explicit focus request even if the game window is
        // already known to gamescope. This remains after the DRYRUN guard.
        crate::gamescope::request_game(*app_id);
    }
    let ds = ds.clone();
    std::thread::spawn(move || exec(act, &ds));
}

#[derive(Default)]
struct LevelQueue { active: bool, latest: Option<f32>, generation: u64, last: Option<f32> }
impl LevelQueue {
    fn submit(&mut self, value: f32) -> bool {
        self.latest = Some(value);
        self.last = Some(value);
        self.generation += 1;
        if self.active { false } else { self.active = true; true }
    }

    fn for_snapshot(&self, generation: u64, observed: Option<f32>) -> Option<f32> {
        if self.active || self.generation != generation { self.last } else { observed }
    }
}
static BRIGHTNESS_QUEUE: Mutex<LevelQueue> = Mutex::new(LevelQueue { active: false, latest: None, generation: 0, last: None });
static VOLUME_QUEUE: Mutex<LevelQueue> = Mutex::new(LevelQueue { active: false, latest: None, generation: 0, last: None });

/// Version requests so a slow status read cannot replace a newer slider value.
pub fn brightness_generation() -> u64 {
    BRIGHTNESS_QUEUE.lock().unwrap().generation
}

pub fn brightness_for_snapshot(generation: u64, observed: Option<f32>) -> Option<f32> {
    let state = BRIGHTNESS_QUEUE.lock().unwrap();
    state.for_snapshot(generation, observed)
}

pub fn level_in_flight(brightness: bool) -> Option<f32> {
    let queue = if brightness { &BRIGHTNESS_QUEUE } else { &VOLUME_QUEUE };
    let state = queue.lock().unwrap();
    if state.active { state.last } else { None }
}

/// Writes the newest pending value until no newer one arrived during the write. The queue lock is
/// released before `write` runs: UI-thread readers (`level_in_flight`, `brightness_generation`, the
/// result hand-off) must never wait for a sysfs or bridge call. `active` is cleared under the same
/// lock that observed `latest == None`, so a submit racing with the exit always starts a new writer.
fn drain_level_queue(queue: &Mutex<LevelQueue>, mut write: impl FnMut(f32) -> Result<(), String>) -> (f32, u64, Result<(), String>) {
    loop {
        let (value, generation) = {
            let mut state = queue.lock().unwrap();
            (state.latest.take().unwrap(), state.generation)
        };
        let result = write(value);
        let mut state = queue.lock().unwrap();
        if state.latest.is_some() { continue; }
        state.active = false;
        return (value, generation, result);
    }
}

/// Brightness has one writer and one reader: Steam's display API (`display.rs`), the call the stock slider makes.
/// Steam keeps the requested scale position and reports it back, so a confirmed write is its own readback.
/// The raw backlight value is logged only, for diagnosis.
pub fn set_brightness(ds: &Arc<DataSource>, value: f32) {
    let value = crate::display::clamp(value);
    let queue = &BRIGHTNESS_QUEUE;
    if !queue.lock().unwrap().submit(value) { return; }
    let ds = ds.clone();
    std::thread::spawn(move || {
        let (value, generation, result) = drain_level_queue(queue, |value| {
            let result = if dryrun() { Err("пробный режим".to_string()) } else { ds.set_brightness(value).map_err(|e| e.to_string()) };
            crate::diag::log("level.write", serde_json::json!({"brightness": true, "value": value, "path": "steam", "ok": result.is_ok()}));
            result
        });
        let observed = result.is_ok().then_some((value, false));
        let raw = deck_system::backlight_raw();
        crate::diag::log("bright.write", serde_json::json!({"wrote": value, "ok": result.is_ok(), "raw": raw.map(|r| r.0), "max": raw.map(|r| r.1)}));
        crate::screens::defer(move |app| {
            if queue.lock().unwrap().generation == generation {
                app.qam_level_result(true, observed, result);
            }
        });
    });
}

/// Use the same system sink for writes and readback, avoiding asynchronous Steam updates.
fn write_volume(ds: &DataSource, value: f32) -> Result<(), String> {
    let result = ds.set_volume(value).map_err(|e| e.to_string());
    crate::diag::log("level.write", serde_json::json!({"brightness": false, "value": value, "path": "system", "ok": result.is_ok()}));
    result
}

/// Volume from the QAM slider. Serialized like brightness: a drag keeps only the latest value.
pub fn set_volume(ds: &Arc<DataSource>, value: f32) {
    let queue = &VOLUME_QUEUE;
    if !queue.lock().unwrap().submit(value) { return; }
    let ds = ds.clone();
    std::thread::spawn(move || {
        let (_, generation, result) = drain_level_queue(queue, |value| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if dryrun() { return Err("пробный режим".to_string()); }
                write_volume(&ds, value)
            })).unwrap_or_else(|_| Err("ошибка системного вызова".to_string()))
        });
        let observed = ds.volume();
        crate::screens::defer(move |app| {
            if queue.lock().unwrap().generation == generation {
                app.qam_level_result(false, observed, result);
            }
        });
    });
}

#[cfg(test)]
mod brightness_queue_tests {
    use super::LevelQueue;

    #[test]
    fn burst_keeps_latest_request_and_invalidates_old_snapshots() {
        let mut queue = LevelQueue::default();
        let old_generation = queue.generation;
        for value in [0.21, 0.22, 0.23, 0.24] { queue.submit(value); }
        assert_ne!(queue.generation, old_generation);
        assert_eq!(queue.last, Some(0.24));
        assert_eq!(queue.latest, Some(0.24));
    }

    #[test]
    fn stale_and_inflight_polls_cannot_overwrite_a_new_brightness() {
        let mut queue = LevelQueue::default();
        let old = queue.generation;
        queue.submit(0.8);
        assert_eq!(queue.for_snapshot(old, Some(0.2)), Some(0.8));
        assert_eq!(queue.for_snapshot(queue.generation, Some(0.2)), Some(0.8));
        queue.active = false;
        assert_eq!(queue.for_snapshot(old, Some(0.2)), Some(0.8));
        assert_eq!(queue.for_snapshot(queue.generation, Some(0.75)), Some(0.75));
    }
}

#[cfg(test)]
mod level_queue_tests {
    use super::LevelQueue;

    #[test]
    fn coalesces_in_flight_writes_and_invalidates_old_results() {
        let mut queue = LevelQueue::default();
        assert!(queue.submit(0.2));
        let first_generation = queue.generation;
        assert_eq!(queue.latest.take(), Some(0.2));
        assert!(!queue.submit(0.4));
        assert!(!queue.submit(0.8));
        assert_eq!(queue.latest.take(), Some(0.8));
        assert_ne!(queue.generation, first_generation);
        queue.active = false;
        assert!(queue.submit(0.5));
    }
}

#[cfg(test)]
mod drain_tests {
    use super::{drain_level_queue, LevelQueue};
    use std::sync::Mutex;

    fn leaked_queue() -> &'static Mutex<LevelQueue> {
        Box::leak(Box::new(Mutex::new(LevelQueue::default())))
    }

    #[test]
    fn queue_lock_is_free_while_a_write_is_in_progress() {
        // A UI-thread reader (level_in_flight, brightness_generation, the result hand-off) must not wait for the write.
        let queue = leaked_queue();
        assert!(queue.lock().unwrap().submit(0.3));
        let mut written = vec![];
        let (value, _, result) = drain_level_queue(queue, |v| {
            assert!(queue.try_lock().is_ok(), "queue lock held during the write");
            written.push(v);
            Ok(())
        });
        assert_eq!((value, result), (0.3, Ok(())));
        assert_eq!(written, vec![0.3]);
        assert!(!queue.lock().unwrap().active);
    }

    #[test]
    fn value_submitted_during_a_write_is_written_next_and_last_one_wins() {
        let queue = leaked_queue();
        assert!(queue.lock().unwrap().submit(0.3));
        let mut written = vec![];
        let (value, generation, result) = drain_level_queue(queue, |v| {
            written.push(v);
            if v == 0.3 {
                // a drag arrives while the first write is in progress: no new writer, latest wins
                assert!(!queue.lock().unwrap().submit(0.5));
                assert!(!queue.lock().unwrap().submit(0.6));
            }
            Ok(())
        });
        assert_eq!(written, vec![0.3, 0.6]);
        assert_eq!(value, 0.6);
        assert_eq!(generation, queue.lock().unwrap().generation);
        assert!(result.is_ok());
        assert!(!queue.lock().unwrap().active);
    }

    #[test]
    fn failed_write_is_reported_with_its_generation() {
        let queue = leaked_queue();
        assert!(queue.lock().unwrap().submit(0.4));
        let (_, generation, result) = drain_level_queue(queue, |_| Err("sysfs".to_string()));
        assert_eq!(result, Err("sysfs".to_string()));
        assert_eq!(generation, queue.lock().unwrap().generation);
        assert!(!queue.lock().unwrap().active);
    }
}

fn launch_url(rungame: u64) -> String {
    format!("steam://rungameid/{rungame}")
}

#[cfg(test)]
mod launch_tests {
    #[test]
    fn preserves_full_game_id_in_steam_url() {
        assert_eq!(super::launch_url(12_345_678_901), "steam://rungameid/12345678901");
    }
}

fn delegates_steam_ui(act: &Act) -> bool {
    matches!(act, Act::Open(_) | Act::OpenStore(_) | Act::Achievements(_) | Act::Properties(_)
        | Act::Controller(_) | Act::OpenChat(_) | Act::FriendProfile(_) | Act::OpenChatGroup(_) | Act::FriendsInvites | Act::CreateChatGroup
        | Act::OpenAccount | Act::StoreSearch(_))

}

fn exec(act: Act, ds: &DataSource) {
    if let Act::Install(id) = act {
        crate::screens::defer(move |app| crate::screens::install::open(app, id));
        return;
    }
    if let Act::Uninstall(id) = act {
        uninstall_confirmed(id);
        return;
    }
    // OS power belongs to our existing logind/systemctl backend. A successful
    // CDP evaluation of Steam's void SuspendPC is not a successful suspend,
    // and enabling native Gamepad UI to make it work also handles our input.
    if let Act::Power(p @ (PowerAction::Sleep | PowerAction::Restart | PowerAction::Shutdown)) = &act {
        ds.power(*p);
        return;
    }
    // gamescope atom write: no Steam bridge involved, the readback decides the log line
    if let Act::GsAtom { name, value } = &act {
        match crate::gamescope_atoms::write_cardinal(name, *value) {
            Ok(v) => eprintln!("[deckshell] {name}={v} (gamescope atom, readback matched)"),
            Err(e) => eprintln!("[deckshell] {act:?} failed: {e}"),
        }
        return;
    }
    let _window_op = delegates_steam_ui(&act).then(|| STEAM_WINDOW_OP.lock().unwrap());
    let b = bridge();
    let via = |r: Option<steam_bridge::Result<bool>>| -> bool { matches!(r, Some(Ok(_))) };
    let ok = match (&act, b.as_ref()) {
        // Launch through Steam's URL handler: the native JS handler can decline a
        // request (or return before the asynchronous launch completes).
        (Act::Launch { .. }, _) => false,
        (Act::Stop(id), Some(b)) => via(Some(b.stop(*id))),
        (Act::OpenStore(id), Some(b)) => via(Some(b.open_store(*id))),
        (Act::PauseDownload(id), Some(b)) => via(Some(b.pause_download(*id))),
        (Act::ResumeDownload(id), Some(b)) => via(Some(b.resume_download(*id))),
        (Act::PauseAllDownloads, Some(b)) => via(Some(b.pause_downloads())),
        (Act::ResumeAllDownloads, Some(b)) => via(Some(b.resume_downloads())),
        (Act::SetWifi(on), Some(b)) => matches!(b.set_wifi(*on), Ok(true)),
        (Act::Perf { method, args }, Some(b)) => matches!(b.perf_call(method, args), Ok(true)),
        (Act::ClientSetting { name, value }, Some(b)) => via(Some(b.set_client_setting(name, value))),
        (Act::SetAirplane(on), Some(b)) => via(Some(b.set_airplane(*on))),
        (Act::UiAutoScale(on), Some(b)) => via(Some(b.set_ui_auto_scale(*on))),
        (Act::UiScaleFactor(f), Some(b)) => via(Some(b.set_ui_scale_factor(*f as f64))),
        (Act::OpenChat(acc), Some(b)) => via(Some(b.friend_open_chat(*acc))),
        (Act::FriendProfile(acc), Some(b)) => via(Some(b.friend_profile(*acc))),
        (Act::OpenChatGroup(gid), Some(b)) => via(Some(b.open_chat_group(gid))),
        (Act::FriendsInvites, Some(b)) => via(Some(b.friends_navigate_to_invites())),
        (Act::CreateChatGroup, Some(b)) => via(Some(b.friends_create_group())),
        (Act::NotifDismiss(id), Some(b)) => via(Some(b.notification_dismiss(*id))),
        (Act::OpenAccount, Some(b)) => via(Some(b.open_account())),
        (Act::StoreSearch(t), Some(b)) => via(Some(b.open_store_search(t))),
        (Act::NotificationsViewed, Some(b)) => via(Some(b.notifications_viewed())),
        _ => false,
    };
    if ok {
        eprintln!("[deckshell] {act:?} (bridge)");
        return;
    }
    // fallbacks: steam:// URLs and deck-system
    match act {
        Act::Launch { rungame, app_id } => {
            let r = steam_control::open_url(&launch_url(rungame));
            // A refused URL means no game will come: take the launch cover down (task 330).
            if r.is_err() { crate::launch_splash::fail(app_id); }
            log(&act, r)
        }
        Act::Stop(_) => log(&act, steam_control::kill_running_game()),
        Act::Install(_) => unreachable!("native install gate handles this before all backends"),
        Act::Uninstall(_) => unreachable!("confirmed uninstall has no URL fallback"),
        Act::OpenStore(id) => ds.open(SteamPage::StoreApp(id)),
        Act::Achievements(id) => ds.open(SteamPage::Achievements(id)),
        Act::Properties(id) => ds.open(SteamPage::Properties(id)),
        Act::Controller(id) => ds.open(SteamPage::Controller(id)),
        Act::Open(p) => ds.open(p),
        Act::Power(p) => ds.power(p),
        Act::SetWifi(on) => ds.set_wifi(on),
        Act::SetBluetooth(on) => ds.set_bluetooth(on),
        Act::SetProfile(p) => ds.set_profile(&p),
        Act::OpenAccount => log(&act, steam_control::open_url("steam://openurl/https://store.steampowered.com/account/")),
        Act::StoreSearch(ref t) => log(&act, steam_control::open_url(&format!("steam://openurl/https://store.steampowered.com/search/?term={}", urlenc(t)))),
        Act::OpenChat(_) | Act::OpenChatGroup(_) | Act::FriendsInvites | Act::CreateChatGroup => ds.open(SteamPage::Friends),
        Act::FriendProfile(acc) => log(&act, steam_control::open_url(&format!("steam://url/SteamIDPage/{}", 76561197960265728u64 + acc as u64))),
        Act::Perf { .. } | Act::ClientSetting { .. } | Act::SetAirplane(_) | Act::UiAutoScale(_) | Act::UiScaleFactor(_) | Act::NotifDismiss(_) => {
            eprintln!("[deckshell] {act:?}: needs the Steam bridge (unavailable)");
        }
        Act::GsAtom { .. } => unreachable!("handled before the bridge path"),
        Act::PauseDownload(_) | Act::ResumeDownload(_) | Act::PauseAllDownloads | Act::ResumeAllDownloads | Act::NotificationsViewed => {
            eprintln!("[deckshell] {act:?}: needs the Steam bridge (unavailable)");
        }
    }
}

// A duplicate confirm while observation is pending must not dispatch again.
static UNINSTALL_PENDING: Mutex<Vec<u32>> = Mutex::new(Vec::new());
fn uninstall_confirmed(id: u32) {
    {
        let mut pending = UNINSTALL_PENDING.lock().unwrap();
        if pending.contains(&id) {
            crate::screens::defer(|a| a.toast("Удаление уже запрошено; дождитесь обновления Steam."));
            return;
        }
        pending.push(id);
    }
    let result = bridge().ok_or_else(|| "Steam bridge недоступен; запрос не отправлен.".to_string())
        .and_then(|b| crate::uninstall_flow::observe(&mut b.clone(), id));
    UNINSTALL_PENDING.lock().unwrap().retain(|&p| p != id);
    let message = match result {
        Ok(()) => "Steam подтвердил: игра удалена с устройства.".to_string(),
        Err(e) => format!("Удаление: {e}"),
    };
    eprintln!("[deckshell] uninstall {id}: {message}");
    crate::screens::defer(move |a| a.toast(&message));
}

fn urlenc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn log(act: &Act, r: std::io::Result<()>) {
    match r {
        Ok(()) => eprintln!("[deckshell] {act:?} (steam://)"),
        Err(e) => eprintln!("[deckshell] {act:?} failed: {e}"),
    }
}

/// TDP writes are complete only after the AMD power-cap sysfs value agrees.
/// The UI permits one pending request, avoiding out-of-order writes.
pub fn set_tdp_verified(w: u32) {
    std::thread::spawn(move || {
        let (observed, ok) = if dryrun() {
            eprintln!("[deckshell] DRYRUN TDP {w} W (not applied)");
            (None, false)
        } else {
            let result = deck_system::set_tdp_limit(w);
            let observed = deck_system::tdp_limit();
            let ok = result.is_ok() && observed.map(|v| v.0) == Some(w);
            eprintln!("[deckshell] TDP requested={w} W sysfs_readback={:?} result={:?}", observed.map(|v| v.0), result);
            (observed, ok)
        };
        let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| a.on_tdp_result(observed, ok)); });
    });
}

pub fn set_gpu_clock_verified(on: bool, mhz: u32) {
    std::thread::spawn(move || {
        let (observed, ok) = if dryrun() {
            eprintln!("[deckshell] DRYRUN GPU manual={on} clock={mhz} MHz (not applied)");
            (None, false)
        } else {
            // Persist first and wait for Steam's settings mirror before touching
            // hardware. Hybrid Steam does not apply these settings live; concurrent
            // independent writes used to race the manager and latch a failed row.
            let result = (|| -> Result<(), String> {
                let b = bridge().ok_or_else(|| "Steam bridge unavailable".to_string())?;
                if !b.set_manual_gpu_clock(on, mhz).map_err(|e| e.to_string())? {
                    return Err("Steam rejected GPU settings".into());
                }
                for _ in 0..30 {
                    let state = b.perf_state().map_err(|e| e.to_string())?;
                    if state["gpu"]["on"].as_bool() == Some(on) && state["gpu"]["mhz"].as_u64() == Some(u64::from(mhz)) {
                        return deck_system::set_gpu_clock(on, mhz).map_err(|e| e.to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err("Steam GPU settings were not acknowledged".into())
            })();
            let observed = deck_system::gpu_clock();
            let ok = result.is_ok() && observed.map(|v| (v.0, v.1)) == Some((on, on.then_some(mhz)));
            eprintln!("[deckshell] GPU manual={on} requested={mhz} MHz sysfs_readback={observed:?} result={result:?}");
            (observed, ok)
        };
        let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| a.on_gpu_clock_result(observed, ok)); });
    });
}

/// Steam's slider order is Off, 1, 2, 3, 4; its wire enum is 0, 4, 1, 2, 3.
const OVERLAY_LEVELS: [i32; 5] = [0, 4, 1, 2, 3];
pub(crate) fn observed_perf_overlay(v: &serde_json::Value) -> Option<i32> {
    let wire = v["perf"]["global"]["perf_overlay_level"].as_i64()?;
    OVERLAY_LEVELS.iter().position(|&n| i64::from(n) == wire).map(|n| n as i32)
}

/// The newest of `request` and everything already queued behind it: a slow write must not be followed by stale values.
fn latest_request<T>(rx: &std::sync::mpsc::Receiver<T>, mut request: T) -> T {
    while let Ok(next) = rx.try_recv() { request = next; }
    request
}

// A false setter result means the Perf channel was absent and no write was sent.
// An error may mean the request was sent but its reply was lost.
trait OverlayBackend {
    fn state(&mut self) -> Option<serde_json::Value>;
    fn write(&mut self, method: &str, arg: &str) -> Result<bool, ()>;
    fn wait(&mut self);
}

impl OverlayBackend for Bridge {
    fn state(&mut self) -> Option<serde_json::Value> { self.perf_state().ok() }
    fn write(&mut self, method: &str, arg: &str) -> Result<bool, ()> {
        self.perf_call(method, arg).map_err(|_| ())
    }
    fn wait(&mut self) { std::thread::sleep(std::time::Duration::from_millis(100)); }
}

// Returns (confirmed state, ambiguous write). Only observe a write after observing
// that its field differed beforehand: an already-matching value is not an ack.
fn apply_overlay(b: &mut impl OverlayBackend, level: i32) -> (Option<i32>, bool) {
    let Some(initial) = b.state() else { return (None, false); };
    let Some(old) = observed_perf_overlay(&initial) else { return (None, false); };
    let Some(&wire) = OVERLAY_LEVELS.get(level as usize) else { return (None, false); };
    if old == level { return (Some(level), false); }
    match b.write("SetPerfOverlayLevel", &wire.to_string()) {
        Ok(false) => return (None, false),
        Err(()) => return (None, true),
        Ok(true) => {}
    }
    for _ in 0..10 {
        b.wait();
        if b.state().and_then(|v| observed_perf_overlay(&v)) == Some(level) { return (Some(level), false); }
    }
    (None, true)
}

#[cfg(test)]
mod overlay_queue_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn steam_overlay_slider_uses_wire_enum_order() {
        for (display, wire) in OVERLAY_LEVELS.iter().enumerate() {
            let state = json!({"perf":{"global":{"perf_overlay_level":wire}}});
            assert_eq!(observed_perf_overlay(&state), Some(display as i32));
        }
    }

    #[test]
    fn queued_requests_coalesce_to_latest() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send((2, 2)).unwrap(); tx.send((3, 0)).unwrap(); tx.send((4, 4)).unwrap();
        assert_eq!(latest_request(&rx, (1, 1)), (4, 4));
        assert!(rx.try_recv().is_err());
    }

    struct Fake { enabled: bool, level: i32, online: bool, delay: usize, pending: Vec<(usize, String, String)>, writes: Vec<(String, String)> }
    impl Fake {
        fn new() -> Self { Self { enabled: true, level: 4, online: true, delay: 2, pending: vec![], writes: vec![] } }
        fn tick(&mut self) {
            for p in &mut self.pending { p.0 -= 1; }
            let mut ready = vec![];
            self.pending.retain(|p| { if p.0 == 0 { ready.push((p.1.clone(), p.2.clone())); false } else { true } });
            // Reverse completions to model a backend that may reorder writes.
            for (method, arg) in ready.into_iter().rev() {
                if method == "SetPerfOverlayLevel" { self.level = arg.parse().unwrap(); }
                else { self.enabled = arg == "true"; }
            }
        }
    }
    impl OverlayBackend for Fake {
        fn state(&mut self) -> Option<serde_json::Value> {
            self.online.then(|| json!({"perf":{"global":{"perf_overlay_service_state": if self.enabled {2} else {1}, "perf_overlay_level":self.level}}}))
        }
        fn write(&mut self, method: &str, arg: &str) -> Result<bool, ()> {
            self.writes.push((method.into(), arg.into()));
            self.pending.push((self.delay, method.into(), arg.into()));
            Ok(true)
        }
        fn wait(&mut self) { self.tick(); }
    }
    #[test]
    fn old_matching_poll_could_accept_a_write_that_later_overwrites_newer() {
        let mut b = Fake::new();
        b.delay = 3;
        b.write("SetPerfOverlayEnabled", "true").unwrap();
        // The old worker accepted the pre-existing true state as confirmation.
        assert_eq!(b.state().and_then(|v| observed_perf_overlay(&v)), Some(1));
        b.delay = 1;
        b.write("SetPerfOverlayEnabled", "false").unwrap();
        b.tick();
        assert!(!b.enabled);
        b.tick(); b.tick();
        assert!(b.enabled); // The older write overtook the newer intent.
    }
    #[test]
    fn same_value_then_opposite_has_no_delayed_redundant_write() {
        let mut b = Fake::new();
        assert_eq!(apply_overlay(&mut b, 1), (Some(1), false));
        assert!(b.pending.is_empty());
        assert_eq!(apply_overlay(&mut b, 0), (Some(0), false));
        assert_eq!(b.writes, vec![("SetPerfOverlayLevel".into(), "0".into())]);
        assert!(b.pending.is_empty());
        assert_eq!(b.level, 0);
    }
    #[test]
    fn delayed_level_then_opposite_waits_for_each_write() {
        let mut b = Fake::new();
        assert_eq!(apply_overlay(&mut b, 4), (Some(4), false));
        assert_eq!(apply_overlay(&mut b, 1), (Some(1), false));
        assert!(b.pending.is_empty());
        assert_eq!(b.level, 4);
    }
    #[test]
    fn unavailable_before_write_can_recover_but_timeout_is_ambiguous() {
        let mut b = Fake::new(); b.online = false;
        assert_eq!(apply_overlay(&mut b, 0), (None, false));
        assert!(b.writes.is_empty());
        b.online = true;
        assert_eq!(apply_overlay(&mut b, 0), (Some(0), false));
        b.delay = 11;
        assert_eq!(apply_overlay(&mut b, 1), (None, true));
        assert!(!b.pending.is_empty());
    }
}

/// One worker serializes requests. An ambiguous write blocks later writes.
pub fn set_perf_overlay(generation: u64, level: i32, mango: bool) {
    use std::sync::{mpsc, OnceLock};
    static TX: OnceLock<mpsc::Sender<(u64, i32, bool)>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(u64, i32, bool)>();
        std::thread::spawn(move || {
            let mut poisoned = false;
            while let Ok(request) = rx.recv() {
                let (generation, level, mango) = latest_request(&rx, request);
                let (observed, ambiguous) = if !poisoned && !dryrun() {
                    if mango {
                        match crate::mango_config::write(level) {
                            Ok(()) => (crate::mango_config::read(), false),
                            Err(e) => { eprintln!("[deckshell] MangoApp config write failed: {e}"); (None, true) }
                        }
                    } else { bridge().map(|mut b| apply_overlay(&mut b, level)).unwrap_or((None, false)) }
                } else { (None, poisoned) };
                poisoned |= ambiguous;
                let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| a.on_perf_overlay_result(generation, observed, ambiguous)); });
            }
        });
        tx
    });
    let _ = tx.send((generation, level.clamp(0, 4), mango));
}

// Hidden=0, SetComposition(mode, ordered appids, windowID) matches Steam's
// CompositionStateStore.EnsureCorrectComposition.
// HideWindow can unmap before Gamescope sees overlay/input flags clear, retaining
// a black overlay commit. Native composition clears both flags while mapped.
const STEAM_YIELD_JS: &str = r#"(()=>{const s=window.SteamUIStore;if(!s)return {store:false};const g=s.WindowStore?.GamepadUIMainWindowInstance;const before=g?.MenuStore?.GetOpenSideMenu();s.CloseSideMenus();if({RESUME})s.NavigateToRunningApp();const w=g?.BrowserWindow;if(w)w.SteamClient.Window.SetComposition(0,{LAYER},0);return {store:true,gamepad:!!g,menu_before:before,menu_after:g?.MenuStore?.GetOpenSideMenu(),composition_requested:!!w}})()"#;

/// Yield only for explicit shell HOME/Play, never while delegating native Steam UI.
pub fn yield_steam_ui(intent: u64) {
    if dryrun() {
        eprintln!("[deckshell] DRYRUN hide native Steam UI");
        return;
    }
    let Some(b) = bridge() else {
        eprintln!("[deckshell] cannot hide native Steam UI: bridge unavailable");
        return;
    };
    std::thread::spawn(move || {
        let _window_op = STEAM_WINDOW_OP.lock().unwrap();
        // Recheck after the mutex wait: any newer user intent cancels this cleanup.
        let Some(layer) = crate::gamescope::current_yield_layer(intent) else {
            crate::diag::log("steam.dismiss.stale", serde_json::json!({"intent":intent}));
            return;
        };
        let js = STEAM_YIELD_JS.replace("{RESUME}", if crate::gamescope::yield_resumes_game(intent) { "true" } else { "false" })
            .replace("{LAYER}", &serde_json::to_string(&layer).unwrap());
        crate::diag::log("steam.dismiss.begin", serde_json::json!({}));
        let t = std::time::Instant::now();
        match b.eval_json(&js) {
            Ok(v) => crate::diag::log("steam.dismiss.result", serde_json::json!({"ms":t.elapsed().as_millis(),"result":v,"note":"Native Hidden composition requested; compositor must be checked independently"})),
            Err(e) => { crate::diag::log("steam.dismiss.error", serde_json::json!({"ms":t.elapsed().as_millis(),"error":e.to_string()})); eprintln!("[deckshell] native Steam UI hide failed: {e}"); },
        }
    });
}


#[cfg(test)]
mod install_safety_tests {
    use super::*;

    #[test]
    fn native_install_does_not_delegate_focus_to_steam() {
        assert!(!delegates_steam_ui(&Act::Install(42)));
        assert!(!delegates_steam_ui(&Act::Uninstall(42)));
    }

    #[test]
    fn install_gate_precedes_backends_and_has_no_url_fallback() {
        let source = include_str!("actions.rs");
        let exec = source.split("fn exec(act:").nth(1).unwrap();
        assert!(exec.find("if let Act::Install(id)").unwrap() < exec.find("let b = bridge()").unwrap());
        let backend = exec.split("fn uninstall_confirmed").next().unwrap();
        assert!(!backend.contains("b.install("));
        assert!(!backend.contains("steam_control::install("));
    }


}
