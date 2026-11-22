//! gamescope client protocol for the DeckShell window (X11 on gamescope's Xwayland) + ready file.
//!
//! Verified on gamescope 3.16.30 (`-e`, nested + headless, context/archive/docs/gamescope-integration.md section 10):
//! in Steam mode (`-e`) a mapped window is NOT shown until it is focusable (`STEAM_GAME` on the window)
//! AND the root atom `GAMESCOPECTRL_BASELAYER_APPID` names that appid.  `STEAM_GAME`/`STEAM_BIGPICTURE`
//! alone leave `GAMESCOPE_FOCUSED_WINDOW` empty = black screen.  Native Wayland is not usable: the real
//! session starts gamescope without `--expose-wayland`, and Wayland windows carry no appid.
//!
//! Activation (env, set by session/deckshell-session.sh):
//!   DECKSHELL_GS_BASE=1       claim the base layer (STEAM_GAME=0x0DEC5E11, BASELAYER_APPID=0x0DEC5E11)
//!                             and keep it claimed while nothing else is focused.  Never set when DeckShell is
//!                             started as a normal Steam game (stage 1): then Steam/gamescope own focus.
//!   DECKSHELL_READY_FILE=PATH written (atomically, "pid xid unix_ms") once a frame was rendered and, under
//!                             gamescope, gamescope reports our window as GAMESCOPE_FOCUSED_WINDOW.
//!                             The session watchdog treats a missing file after 10 s as "black screen".
#[path = "gamescope_events.rs"]
mod events;

use slint::{ComponentHandle, RenderingState};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, MapState, PropMode, Window};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// Independent from Steam's own Big Picture windows (769).
const BP_APPID: u32 = 0x0DEC_5E11;
const STEAM_APPID: u32 = 769;
// Last user intent wins, including when a store request races a delayed Guide handler.
const HOME: u8 = 1;
const STEAM_UI: u8 = 2;
const GAME: u8 = 3;
const OVERLAY: u8 = 4;
// Low three bits = action, next 32 = appid, remaining 29 = generation. One snapshot keeps the
// worker's request and the asynchronous Steam-window operation consistent.
static INTENT: AtomicU64 = AtomicU64::new(0);
static MANAGE_PAD_FOCUS: AtomicBool = AtomicBool::new(false);
static PAD_FOCUSED: AtomicBool = AtomicBool::new(true);
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);
static PAD_GATE_REQUESTED: AtomicBool = AtomicBool::new(false);
static FOCUS_INTENT: AtomicU64 = AtomicU64::new(0);
static FOCUSED_GAME: AtomicU32 = AtomicU32::new(0);
static OVERLAY_WINDOW_FLAGGED: AtomicBool = AtomicBool::new(false);
static OVERLAY_REJECTED_INTENT: AtomicU64 = AtomicU64::new(0);
static OVERLAY_REQUEST_INTENT: AtomicU64 = AtomicU64::new(0);
static OVERLAY_REQUEST_FRAME: AtomicU64 = AtomicU64::new(0);
static OVERLAY_REQUEST_LOCK: Mutex<()> = Mutex::new(());
/// True when the current overlay request could not safely be applied.
pub fn overlay_rejected() -> bool {
    let intent = INTENT.load(Ordering::Acquire);
    (intent & 7) as u8 == OVERLAY && OVERLAY_REJECTED_INTENT.load(Ordering::Acquire) == intent
}
/// A currently observed game graphics app, never a guessed process identity.
pub fn focused_game() -> Option<u32> {
    let game = FOCUSED_GAME.load(Ordering::Acquire);
    (is_game_app(game) && FOCUS_INTENT.load(Ordering::Acquire) == INTENT.load(Ordering::Acquire)).then_some(game)
}
/// Remains true through the clear transition until normal focus is observed.
pub fn overlay_window_flagged() -> bool { OVERLAY_WINDOW_FLAGGED.load(Ordering::Acquire) }
/// Close is complete only after the worker has observed the current non-overlay intent.
pub fn overlay_close_complete() -> bool {
    let intent = INTENT.load(Ordering::Acquire);
    overlay_close_acknowledged(intent, FOCUS_INTENT.load(Ordering::Acquire),
        OVERLAY_WINDOW_FLAGGED.load(Ordering::Acquire))
        && INTENT.load(Ordering::Acquire) == intent
}
/// The compositor is observed in the state the current request asked for, so the overlay window flag (which keeps
/// the page transparent) can be dropped. A GAME request whose game has no window left is acknowledged once our own
/// window is the shown and focused one: nothing is under a transparent page then.
fn overlay_flag_cleared(action: u8, appid: u32, apps: &[u32], gfx: Option<u32>, input: Option<u32>, own_focused: bool) -> bool {
    match action {
        GAME => (gfx == Some(appid) && input == Some(appid))
            || (!apps.contains(&appid) && gfx == Some(BP_APPID) && own_focused),
        HOME => gfx == Some(BP_APPID) && own_focused,
        STEAM_UI => input == Some(STEAM_APPID),
        _ => false,
    }
}
/// The UI gave up waiting for the worker's acknowledgement (see `App::sync_game_overlay`): drop the flag so the page
/// is drawn again. The X properties were cleared by the worker when the overlay request ended.
pub fn drop_overlay_window_flag(reason: &str) {
    if OVERLAY_WINDOW_FLAGGED.swap(false, Ordering::AcqRel) {
        crate::diag::log("overlay.flag", serde_json::json!({"flagged":false,"reason":reason,"intent":INTENT.load(Ordering::Acquire)}));
    }
}
fn overlay_close_acknowledged(intent: u64, focus_intent: u64, flagged: bool) -> bool {
    (intent & 7) as u8 != OVERLAY && intent == focus_intent && !flagged
}
/// Whether the matching native Steam yield should navigate to its running-game route.
pub fn yield_resumes_game(expected: u64) -> bool {
    INTENT.load(Ordering::Acquire) == expected && (expected & 7) as u8 == GAME
}
/// True only while the current overlay intent has observed game graphics and our input focus.
pub fn overlay_active() -> bool {
    OVERLAY_ACTIVE.load(Ordering::Acquire) && FOCUS_INTENT.load(Ordering::Acquire) == INTENT.load(Ordering::Acquire)
}
pub fn accepts_pad_input() -> bool {
    !MANAGE_PAD_FOCUS.load(Ordering::Acquire) || (PAD_FOCUSED.load(Ordering::Acquire)
        && FOCUS_INTENT.load(Ordering::Acquire) == INTENT.load(Ordering::Acquire))
}
fn next_intent(old: u64, action: u8, appid: u32) -> u64 {
    let generation = ((old >> 35).wrapping_add(1)) & 0x1fff_ffff;
    (generation << 35) | (u64::from(appid) << 3) | u64::from(action)
}
fn pad_owner(action: u8) -> crate::input::padgate::Owner {
    use crate::input::padgate::Owner;
    match action {
        GAME => Owner::Game,
        STEAM_UI => Owner::Steam,
        // No request yet means the shell has just claimed the base layer.
        _ => Owner::Shell,
    }
}
/// A delegation that ended (Store closed) leaves the STEAM_UI intent behind; the shell owns again.
fn pad_owner_now(action: u8, steam_delegated: bool) -> crate::input::padgate::Owner {
    if action == STEAM_UI && !steam_delegated { crate::input::padgate::Owner::Shell } else { pad_owner(action) }
}
fn pad_focus(input_app: Option<u32>, apps: &[u32]) -> crate::input::padgate::Focus {
    use crate::input::padgate::Focus;
    match input_app {
        Some(BP_APPID) => Focus::Shell,
        Some(id) if is_game_app(id) && apps.contains(&id) => Focus::Game,
        _ => Focus::Other,
    }
}
fn request(action: u8, appid: u32) -> u64 {
    OVERLAY_ACTIVE.store(false, Ordering::Release);
    if MANAGE_PAD_FOCUS.load(Ordering::Acquire) {
        PAD_FOCUSED.store(false, Ordering::Release);
        // The game's pad follows the request at once, not the 200 ms focus poll: a press
        // right after STEAM/QAM must not reach the game, and Resume must not wait.
        PAD_GATE_REQUESTED.store(true, Ordering::Release);
        crate::input::padgate::set(pad_owner(action) == crate::input::padgate::Owner::Shell);
    }
    let old = INTENT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
        Some(next_intent(old, action, appid))
    }).expect("intent update cannot fail");
    if let Some(t) = WORKER.get() { t.unpark(); }
    // Resume the UI now: a shell-visible request must not wait for the UI's next poll.
    wake_ui();
    next_intent(old, action, appid)
}
pub fn request_reclaim() { request(HOME, 0); }
pub fn request_game(appid: u32) {
    if is_game_app(appid) {
        request(GAME, appid);
    }
}
/// Ask to overlay an already-focused game. Invalid app IDs are ignored.
pub fn request_overlay(appid: u32) {
    if is_game_app(appid) {
        let _guard = OVERLAY_REQUEST_LOCK.lock().unwrap();
        let intent = request(OVERLAY, appid);
        // UI must request a redraw; only a later completed frame is safe to expose.
        let frame = FRAME.load(Ordering::Acquire);
        OVERLAY_REQUEST_FRAME.store(frame, Ordering::Relaxed);
        OVERLAY_REQUEST_INTENT.store(intent, Ordering::Release);
    }
}
pub fn request_steam_ui() {
    // Block queued A/navigation immediately, before the 200 ms gamescope poll.
    if MANAGE_PAD_FOCUS.load(Ordering::Acquire) {
        PAD_FOCUSED.store(false, Ordering::Release);
    }
    request(STEAM_UI, 0);
}
pub fn steam_ui_requested() -> bool { INTENT.load(Ordering::Acquire) & 7 == u64::from(STEAM_UI) }

/// The game owns the screen and our window is under it: nothing of the shell is visible, so the UI
/// need not animate. Any shell-visible request (Guide, Home, Steam) makes this false at once.
pub fn shell_covered() -> bool {
    covered_by_game(INTENT.load(Ordering::Acquire), focused_game())
}
fn covered_by_game(intent: u64, focused: Option<u32>) -> bool {
    (intent & 7) as u8 == GAME && focused.is_some()
}

/// Wake the UI thread so it re-reads what it must run (animation clock, clock text, one fresh frame) now.
/// Safe from any thread; the UI handles it on its own event loop.
fn wake_ui() {
    let _ = slint::invoke_from_event_loop(|| {
        crate::with_app(|a| a.shell_signal());
    });
}

/// Longest wait for the UI to render one new frame before a shell that was hidden under a game is revealed.
const REVEAL_TIMEOUT: Duration = Duration::from_millis(300);
/// Longest wait for a UI frame before an overlay request is refused (never wait forever on a stalled UI).
const OVERLAY_FRAME_TIMEOUT: Duration = Duration::from_millis(300);
fn overlay_wait_expired(since: Instant, now: Instant) -> bool {
    now.duration_since(since) >= OVERLAY_FRAME_TIMEOUT
}
/// Keep waiting while HOME reveals a shell that a game covered and no new frame has completed since the wait began.
/// Without this gamescope would show the last buffer, which may be from before the game started.
fn reveal_waiting(request: u8, have_notifier: bool, hidden_before: bool, wait: Option<(u64, Instant)>, frame: u64, now: Instant) -> bool {
    if request != HOME || !have_notifier || !hidden_before { return false; }
    match wait {
        Some((from, deadline)) => frame == from && now < deadline,
        None => true,
    }
}

// A queued yield is valid only for the exact HOME/Play generation that created it.
fn owned_intent_layer(intent: u64) -> Option<Vec<u32>> {
    match (intent & 7) as u8 {
        HOME => Some(vec![BP_APPID]),
        GAME | OVERLAY if is_game_app((intent >> 3) as u32) => Some(vec![(intent >> 3) as u32, BP_APPID]),
        _ => None,
    }
}
fn dismiss_matches(current: u64, expected: u64) -> bool {
    current == expected && owned_intent_layer(expected).is_some()
}
pub fn current_yield_layer(expected: u64) -> Option<Vec<u32>> {
    dismiss_matches(INTENT.load(Ordering::Acquire), expected)
        .then(|| owned_intent_layer(expected)).flatten()
}

fn is_game_app(appid: u32) -> bool {
    appid != 0 && appid != BP_APPID && appid != STEAM_APPID
}

// Retire only a selected game that was observed and has now disappeared. A later
// poll processes the new generation through the ordinary HOME/GAME handler,
// including its one native Steam yield. Do not re-run the old GAME callback.
fn game_exit_intent(intent: u64, before: Option<u32>, after: Option<u32>, apps: &[u32], gfx: Option<u32>) -> Option<u64> {
    let exited = before?;
    if after.is_some() || apps.contains(&exited) || (intent >> 3) as u32 != exited
        || !matches!((intent & 7) as u8, GAME | OVERLAY) {
        return None;
    }
    // Another game may already own graphics when the selected game disappears.
    // Preserve that observed foreground game, not an arbitrary background app.
    let other = gfx.filter(|id| *id != exited && is_game_app(*id) && apps.contains(id));
    Some(match other {
        Some(id) => next_intent(intent, GAME, id),
        None => next_intent(intent, HOME, 0),
    })
}

fn publish_exit_intent(current: &AtomicU64, expected: u64, next: u64) -> bool {
    current.compare_exchange(expected, next, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

fn overlay_frame_ready(intent: u64, published: u64, requested_frame: u64, frame: u64) -> bool {
    intent == published && frame != requested_frame
}

fn overlay_observed(game: u32, apps: &[u32], gfx: Option<u32>, input: Option<u32>, mapped: bool) -> bool {
    mapped && apps.contains(&game) && gfx == Some(game) && input == Some(BP_APPID)
}

#[derive(Default)]
struct LayerPolicy {
    known_apps: Option<Vec<u32>>,
    pending_app: Option<(u32, Instant)>,
    steam_delegated: bool,
    steam_seen: bool,
    steam_requested_at: Option<Instant>,
    steam_missing_since: Option<Instant>,
    requested_game: Option<u32>,
    requested_game_seen: bool,
    cancelled_game: Option<u32>,
    /// Last re-assertion of an explicit game layer, and how many were made for this request.
    handover_last: Option<Instant>,
    handover_attempts: u8,
}
/// Gamescope may keep a base layer written before the game window existed. Re-assert it while the
/// requested game is focusable but not shown, at most this often and this many times per request.
const HANDOVER_INTERVAL: Duration = Duration::from_millis(500);
const HANDOVER_ATTEMPTS: u8 = 20;
impl LayerPolicy {
    fn request(&mut self, request: u8, appid: u32, apps: &[u32], now: Instant) -> Option<Vec<u32>> {
        match request {
            HOME | STEAM_UI => {
                // Cancel the 1.5 s auto-promotion: it must not undo a newer STEAM/QAM press.
                self.pending_app = None;
                // A cancelled launch may map its first window after Guide/QAM.
                // Keep that late window from undoing the newer menu request.
                self.cancelled_game = self.requested_game.or(self.cancelled_game);
                self.requested_game = None;
                self.requested_game_seen = false;
                self.known_apps = Some(apps.to_vec());
                self.steam_delegated = request == STEAM_UI;
                self.steam_seen = self.steam_delegated && apps.contains(&STEAM_APPID);
                self.steam_requested_at = self.steam_delegated.then_some(now);
                self.steam_missing_since = None;
                Some(if self.steam_delegated { vec![STEAM_APPID, BP_APPID] } else { vec![BP_APPID] })
            }
            GAME | OVERLAY if is_game_app(appid) => {
                // Play and Resume must work for already-known windows too. An absent
                // game window falls through to shell until it becomes focusable.
                self.pending_app = None;
                self.known_apps = Some(apps.to_vec());
                self.steam_delegated = false;
                self.steam_seen = false;
                self.steam_requested_at = None;
                self.steam_missing_since = None;
                // A game already observed under this selection stays observed across the menu's own requests
                // (OVERLAY -> GAME on close). If it died meanwhile, `update` must retire it; treating it as a
                // launch that has not mapped yet kept the GAME intent on a dead game and the page hidden.
                let seen_before = self.requested_game == Some(appid) && self.requested_game_seen;
                self.requested_game = Some(appid);
                self.requested_game_seen = seen_before || apps.contains(&appid);
                self.cancelled_game = None;
                self.handover_last = None;
                self.handover_attempts = 0;
                Some(vec![appid, BP_APPID])
            }
            _ => None,
        }
    }
    /// The requested game is focusable but gamescope does not show it: the layer must be written again.
    /// Returns the layer to re-assert, or `None` when nothing is pending, the game is already shown,
    /// the game is not focusable yet, or the retry budget or interval is used up.
    fn handover_retry(&mut self, apps: &[u32], shown_app: Option<u32>, now: Instant) -> Option<Vec<u32>> {
        let appid = self.requested_game?;
        if !apps.contains(&appid) || shown_app == Some(appid) || self.handover_attempts >= HANDOVER_ATTEMPTS {
            return None;
        }
        if self.handover_last.is_some_and(|t| now.duration_since(t) < HANDOVER_INTERVAL) {
            return None;
        }
        self.handover_last = Some(now);
        self.handover_attempts += 1;
        Some(vec![appid, BP_APPID])
    }
    fn preferred_layer(&self) -> Vec<u32> {
        match self.requested_game {
            Some(appid) => vec![appid, BP_APPID],
            None => vec![BP_APPID],
        }
    }
    fn repair_layer(&self, layer: &[u32], apps: &[u32]) -> Option<Vec<u32>> {
        if self.steam_delegated {
            return None;
        }
        if self.requested_game.is_some() {
            let desired = self.preferred_layer();
            return (layer != desired).then_some(desired);
        }
        // Gamescope skips unavailable queue entries. A missing prefix followed
        // by Steam can still select Steam even though layer.first() is not 769.
        // Use this poll's apps, not known_apps (which has early-return paths).
        let selected = layer.iter().find(|id| apps.contains(id)).or_else(|| layer.first());
        (selected == Some(&STEAM_APPID)).then(|| vec![BP_APPID])
    }
    fn update(&mut self, apps: &[u32], now: Instant) -> Option<Vec<u32>> {
        if let Some(appid) = self.requested_game {
            if apps.contains(&appid) {
                self.requested_game_seen = true;
            } else if self.requested_game_seen {
                self.requested_game = None;
                self.requested_game_seen = false;
                self.known_apps = Some(apps.to_vec());
                return Some(vec![BP_APPID]);
            }
            // Explicit selection wins over unrelated newly mapped windows.
            return None;
        }
        if self.steam_delegated {
            if apps.contains(&STEAM_APPID) {
                self.steam_seen = true;
                self.steam_missing_since = None;
            } else {
                // CEF may briefly recreate a browser. A failed explicit UI request must also expire.
                let missing = *self.steam_missing_since.get_or_insert(now);
                let closed = self.steam_seen && now.duration_since(missing) >= Duration::from_millis(400);
                let never_opened = !self.steam_seen && self.steam_requested_at
                    .is_some_and(|t| now.duration_since(t) >= Duration::from_secs(5));
                if closed || never_opened {
                    self.steam_delegated = false;
                    self.steam_seen = false;
                    return Some(vec![BP_APPID]);
                }
            }
        }
        match self.known_apps.as_mut() {
            None => self.known_apps = Some(apps.to_vec()),
            Some(known) => {
                known.retain(|k| apps.contains(k));
                for &ap in apps {
                    if !known.contains(&ap) {
                        known.push(ap);
                        // Guide/QAM may create Steam windows; only an explicit action delegates to them.
                        if is_game_app(ap) && self.cancelled_game != Some(ap) {
                            self.pending_app = Some((ap, now));
                        }
                    }
                }
            }
        }
        if let Some((ap, since)) = self.pending_app {
            if !apps.contains(&ap) {
                self.pending_app = None;
            } else if now.duration_since(since) >= Duration::from_millis(1500) {
                self.pending_app = None;
                self.steam_delegated = false;
                return Some(vec![ap, BP_APPID]);
            }
        }
        None
    }
}

static FRAME: AtomicU64 = AtomicU64::new(0);
/// Frames completed while the shell page was on screen. A HOME reveal waits on this one only: a frame drawn while a
/// game menu hides the page shows nothing of the shell, and releasing the reveal on it showed an empty window.
static PRESENT_FRAME: AtomicU64 = AtomicU64::new(0);
/// The UI hides its page (`Shell.game-overlay`). Written on the UI thread, read by the render notifier.
static PAGE_SUPPRESSED: AtomicBool = AtomicBool::new(false);

pub fn set_page_suppressed(suppressed: bool) {
    if PAGE_SUPPRESSED.swap(suppressed, Ordering::AcqRel) != suppressed {
        crate::diag::log("page.suppressed", serde_json::json!({"suppressed":suppressed,"intent":INTENT.load(Ordering::Acquire),
            "flagged":OVERLAY_WINDOW_FLAGGED.load(Ordering::Acquire),"frame":FRAME.load(Ordering::Acquire),
            "present_frame":PRESENT_FRAME.load(Ordering::Acquire)}));
    }
}

/// Count one completed frame: `frame` always (overlay requests wait on it), `present` only when the page was shown.
fn count_frame(frame: &AtomicU64, present: &AtomicU64, suppressed: bool) {
    frame.fetch_add(1, Ordering::Release);
    if !suppressed {
        present.fetch_add(1, Ordering::Release);
    }
}

pub fn init(ui: &crate::AppWindow) {
    let ready = std::env::var_os("DECKSHELL_READY_FILE").map(PathBuf::from);
    let base = crate::env_flag("DECKSHELL_GS_BASE");
    if let Some(p) = &ready {
        let _ = std::fs::remove_file(p);
    }
    if ready.is_none() && !base {
        return;
    }
    // One rendering notifier per window; DECKSHELL_FRAMESTATS owns it when enabled -> time-based fallback.
    let have_notifier = !crate::env_flag("DECKSHELL_FRAMESTATS")
        && ui
            .window()
            .set_rendering_notifier(|state, _| {
                if matches!(state, RenderingState::AfterRendering) {
                    count_frame(&FRAME, &PRESENT_FRAME, PAGE_SUPPRESSED.load(Ordering::Acquire));
                }
            })
            .is_ok();
    if let Ok(h) = std::thread::Builder::new()
        .name("gamescope".into())
        .spawn(move || run(ready, base, have_notifier))
    {
        let _ = WORKER.set(h.thread().clone());
    }
}

/// The worker thread, woken by `request` so an intent is applied at once, not on the next poll.
static WORKER: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
/// Title of the REC indicator window (app/ui/rec_badge.slint). `find_window` never takes it for our main window.
pub const REC_BADGE_TITLE: &str = "DeckShell REC";
static REC_BADGE_WANTED: AtomicBool = AtomicBool::new(false);

/// Whether the REC indicator is a gamescope external overlay (a separate window above the game) rather than part of
/// the main window. Only the gamescope session (DECKSHELL_GS_BASE) has the compositor for it.
pub fn rec_badge_external() -> bool { crate::env_flag("DECKSHELL_GS_BASE") }

/// Asks the worker to mark the REC window as an external overlay once it is mapped. `false` clears the mark.
pub fn set_rec_badge(on: bool) {
    REC_BADGE_WANTED.store(on, Ordering::Release);
    if let Some(t) = WORKER.get() { t.unpark(); }
}
/// After an intent change the worker polls this often, to see the compositor's answer quickly.
const SETTLE_POLL: Duration = Duration::from_millis(25);
const SETTLE_WINDOW: Duration = Duration::from_millis(600);

struct Atoms {
    steam_game: u32,
    base_appid: u32,
    focused_win: u32,
    focusable_win: u32,
    focusable_apps: u32,
    pid: u32,
    steam_bp: u32,
    overlay: u32,
    input_focus: u32,
    focused_app: u32,
    focused_app_gfx: u32,
    base_window: u32,
    net_wm_name: u32,
    external_overlay: u32,
}

fn atom(c: &RustConnection, name: &str, only_if_exists: bool) -> u32 {
    c.intern_atom(only_if_exists, name.as_bytes()).ok().and_then(|r| r.reply().ok()).map(|r| r.atom).unwrap_or(0)
}

fn get32(c: &RustConnection, win: Window, prop: u32) -> Vec<u32> {
    if prop == 0 {
        return vec![];
    }
    c.get_property(false, win, prop, AtomEnum::ANY, 0, 4096)
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().map(|i| i.collect()))
        .unwrap_or_default()
}

fn set32(c: &RustConnection, win: Window, prop: u32, v: &[u32]) {
    if prop == 0 {
        return;
    }
    if crate::diag::enabled() {
        let name = c.get_atom_name(prop).ok().and_then(|r|r.reply().ok()).map(|r|String::from_utf8_lossy(&r.name).into_owned());
        crate::diag::log("gamescope.write", serde_json::json!({"xid":win,"atom":prop,"name":name,"values":v}));
    }
    let _ = c.change_property32(PropMode::REPLACE, win, prop, AtomEnum::CARDINAL, v);
    let _ = c.flush();
}

fn is_mapped(c: &RustConnection, w: Window) -> bool {
    c.get_window_attributes(w)
        .ok()
        .and_then(|r| r.reply().ok())
        .map(|r| r.map_state == MapState::VIEWABLE)
        .unwrap_or(false)
}

fn window_title(c: &RustConnection, win: Window, net_wm_name: u32) -> String {
    if net_wm_name == 0 { return String::new(); }
    c.get_property(false, win, net_wm_name, AtomEnum::ANY, 0, 256)
        .ok()
        .and_then(|r| r.reply().ok())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default()
}

/// Passive windows must never be claimed as the shell/base layer.
fn is_passive_overlay_title(title: &str) -> bool {
    matches!(title, REC_BADGE_TITLE | crate::level::POPUP_TITLE)
}

#[cfg(test)]
pub(crate) fn popup_focus_snapshot() -> (u64, u64, bool, bool, bool) {
    (INTENT.load(Ordering::Acquire), FOCUS_INTENT.load(Ordering::Acquire),
     PAD_FOCUSED.load(Ordering::Acquire), PAD_GATE_REQUESTED.load(Ordering::Acquire),
     OVERLAY_WINDOW_FLAGGED.load(Ordering::Acquire))
}

/// Mark only this passive window; no base-layer, Steam overlay or input-focus writes.
pub fn mark_level_popup(window: &slint::Window) -> Result<(), String> {
    use slint::winit_030::{WinitWindowAccessor, winit::raw_window_handle::{HasWindowHandle, RawWindowHandle}};
    window.with_winit_window(|w| {
        // Pointer pass-through is best effort: gamescope routes input by focus, never to an external overlay, so a
        // refusal here must not keep the window from getting its compositor role (task 526).
        if let Err(e) = w.set_cursor_hittest(false) { eprintln!("[deckshell] overlay: cursor hit test: {e}"); }
        let RawWindowHandle::Xlib(handle) = w.window_handle().map_err(|e| format!("window handle: {e}"))?.as_raw() else {
            return Err("level overlay requires gamescope X11".into());
        };
        let (c, _) = RustConnection::connect(None).map_err(|e| format!("x11 connect: {e}"))?;
        let mut hints = x11rb::properties::WmHints::new();
        hints.input = Some(false);
        hints.set(&c, handle.window as u32).map_err(|e| format!("WM_HINTS: {e}"))?
            .check().map_err(|e| format!("WM_HINTS: {e}"))?;
        let overlay = atom(&c, "GAMESCOPE_EXTERNAL_OVERLAY", false);
        c.change_property32(PropMode::REPLACE, handle.window as u32, overlay, AtomEnum::CARDINAL, &[1])
            .map_err(|e| format!("GAMESCOPE_EXTERNAL_OVERLAY: {e}"))?.check().map_err(|e| format!("GAMESCOPE_EXTERNAL_OVERLAY: {e}"))?;
        c.flush().map_err(|e| e.to_string())
    }).unwrap_or_else(|| {
        // The software test backend has no native X11 window.
        #[cfg(test)] { Ok(()) }
        #[cfg(not(test))] { Err("overlay has no native winit window yet".into()) }
    })
}

/// Our top-level (mapped, _NET_WM_PID == us) window on gamescope Xwayland, excluding passive notifications.
fn find_window(c: &RustConnection, root: Window, a: &Atoms) -> Option<Window> {
    let kids = c.query_tree(root).ok()?.reply().ok()?.children;
    kids.into_iter().find(|&w| {
        is_mapped(c, w) && get32(c, w, a.pid).first() == Some(&std::process::id())
            && !is_passive_overlay_title(&window_title(c, w, a.net_wm_name))
    })
}

/// While a recording is shown, marks the mapped REC window as an external overlay: composited above the game, not
/// focusable, so it takes no focus or pad input. Done once per mapping; `applied` is reset when the badge is off.
fn rec_badge_step(c: &RustConnection, root: Window, a: &Atoms, applied: &mut Option<Window>) {
    if !REC_BADGE_WANTED.load(Ordering::Acquire) {
        if let Some(w) = applied.take() {
            set32(c, w, a.external_overlay, &[0]);
            crate::diag::log("gamescope.rec_badge", serde_json::json!({"xid": w, "external_overlay": false}));
        }
        return;
    }
    if applied.is_some() || a.external_overlay == 0 { return; }
    let Some(kids) = c.query_tree(root).ok().and_then(|r| r.reply().ok()).map(|r| r.children) else { return };
    let found = kids.into_iter().find(|&w| {
        is_mapped(c, w) && get32(c, w, a.pid).first() == Some(&std::process::id())
            && window_title(c, w, a.net_wm_name) == REC_BADGE_TITLE
    });
    if let Some(w) = found {
        set32(c, w, a.external_overlay, &[1]);
        eprintln!("[deckshell] gamescope: REC window {w:#x} marked as external overlay");
        crate::diag::log("gamescope.rec_badge", serde_json::json!({"xid": w, "external_overlay": true}));
        *applied = Some(w);
    }
}

fn write_ready(path: &PathBuf, xid: Window) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, format!("{} {} {}\n", std::process::id(), xid, ms)).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        eprintln!("[deckshell] gamescope: ready file {} (xid {xid:#x})", path.display());
    }
}

fn run(ready: Option<PathBuf>, base: bool, have_notifier: bool) {
    let t_start = Instant::now();
    let conn = RustConnection::connect(None).ok();
    let (c, root, a, gs) = match &conn {
        Some((c, screen)) => {
            let root = c.setup().roots[*screen].root;
            let a = Atoms {
                steam_game: atom(c, "STEAM_GAME", false),
                base_appid: atom(c, "GAMESCOPECTRL_BASELAYER_APPID", false),
                focused_win: atom(c, "GAMESCOPE_FOCUSED_WINDOW", true),
                focusable_win: atom(c, "GAMESCOPE_FOCUSABLE_WINDOWS", true),
                focusable_apps: atom(c, "GAMESCOPE_FOCUSABLE_APPS", true),
                pid: atom(c, "_NET_WM_PID", false),
                steam_bp: atom(c, "STEAM_BIGPICTURE", false),
                overlay: atom(c, "STEAM_OVERLAY", false),
                input_focus: atom(c, "STEAM_INPUT_FOCUS", false),
                focused_app: atom(c, "GAMESCOPE_FOCUSED_APP", true),
                focused_app_gfx: atom(c, "GAMESCOPE_FOCUSED_APP_GFX", true),
                base_window: atom(c, "GAMESCOPECTRL_BASELAYER_WINDOW", true),
                net_wm_name: atom(c, "_NET_WM_NAME", true),
                external_overlay: atom(c, "GAMESCOPE_EXTERNAL_OVERLAY", false),
            };
            let gs = a.focused_win != 0; // atom exists only on gamescope's Xwayland
            (Some(c), root, a, gs)
        }
        None => (None, 0, Atoms { steam_game: 0, base_appid: 0, focused_win: 0, focusable_win: 0, focusable_apps: 0, pid: 0, steam_bp: 0, overlay: 0, input_focus: 0, focused_app: 0, focused_app_gfx: 0, base_window: 0, net_wm_name: 0, external_overlay: 0 }, false),
    };
    eprintln!("[deckshell] gamescope: x11={} gamescope={gs} base={base}", c.is_some());
    MANAGE_PAD_FOCUS.store(gs && base, Ordering::Release);

    let layer_events = if gs && base {
        match events::watch(std::thread::current()) {
            Ok(events) => Some(events),
            Err(error) => {
                eprintln!("[deckshell] gamescope: cannot subscribe to layer events: {error}; falling back to polling");
                None
            }
        }
    } else { None };

    let mut xid: Option<Window> = None;
    let mut rec_badge_xid: Option<Window> = None;
    let mut unfocused_since: Option<Instant> = None;
    let mut last_assert = Instant::now();
    let mut ready_done = false;
    let mut last_log = Instant::now();
    let mut policy = LayerPolicy::default();
    let mut last_intent = 0;
    let mut last_focus_snapshot = None;
    let mut last_focus_log = Instant::now();
    // True only after we set both properties on our mapped window.
    let mut overlay_set = false;
    let mut overlay_game: Option<u32> = None;
    let mut pad_gate = crate::input::padgate::Policy::default();
    let mut settle_until = Instant::now();
    // The shell was covered by a game at some point since its last reveal (sticky until a HOME reveal).
    let mut hidden_before = false;
    // Frame counter and deadline of an in-progress wait for one fresh frame before a HOME reveal.
    let mut reveal: Option<(u64, Instant)> = None;
    // Intent and start of an in-progress wait for a UI frame before an overlay is shown.
    let mut overlay_wait: Option<(u64, Instant)> = None;
    let mut covered_seen = false;
    // Hard guard against Steam's own Gamepad UI on screen (steam_ui_guard).
    let mut ui_guard = crate::steam_ui_guard::Guard::default();
    let mut last_ui_scan = Instant::now();
    loop {
        let frame = FRAME.load(Ordering::Acquire) != 0 || (!have_notifier && t_start.elapsed() > Duration::from_millis(2500));
        if let (Some(c), true) = (c, gs) {
            rec_badge_step(c, root, &a, &mut rec_badge_xid);
            if xid.is_none() {
                xid = find_window(c, root, &a);
                if let Some(w) = xid {
                    eprintln!("[deckshell] gamescope: our window {w:#x} after {:?}", t_start.elapsed());
                    if base {
                        claim(c, w, root, &a);
                    }
                }
            }
            if let (Some(w), true) = (xid, base) {
                let apps = get32(c, root, a.focusable_apps);
                let intent = INTENT.load(Ordering::Acquire);
                // Reconcile a competing Steam write before focus diagnostics/round trips.
                // Events only wake us: the current policy and intent remain the sole writer.
                // This is recovery AFTER XChangeProperty, not a zero-frame prevention barrier.
                if intent == last_intent && layer_events.as_ref().is_none_or(|events| events.take()) {
                    if let Some(layer) = policy.repair_layer(&get32(c, root, a.base_appid), &apps) {
                        if INTENT.load(Ordering::Acquire) == intent {
                            eprintln!("[deckshell] gamescope: restore selected BASELAYER_APPID={layer:?}");
                            set32(c, root, a.base_appid, &layer);
                        }
                    }
                }
                if intent != last_intent {
                    let request = (intent & 7) as u8;
                    let appid = (intent >> 3) as u32;
                    if request == OVERLAY && have_notifier && !overlay_frame_ready(
                        intent, OVERLAY_REQUEST_INTENT.load(Ordering::Acquire),
                        OVERLAY_REQUEST_FRAME.load(Ordering::Relaxed), FRAME.load(Ordering::Acquire),
                    ) {
                        // Force one UI frame at the start of the wait; a stalled UI must not hold this worker.
                        let since = match overlay_wait {
                            Some((waited, since)) if waited == intent => since,
                            _ => {
                                wake_ui();
                                overlay_wait = Some((intent, Instant::now()));
                                Instant::now()
                            }
                        };
                        if !overlay_wait_expired(since, Instant::now()) {
                            // Re-read intent on the next iteration so a close cancels this wait.
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        // No new frame in time: refuse this overlay and go on with later requests.
                        overlay_wait = None;
                        OVERLAY_REJECTED_INTENT.store(intent, Ordering::Release);
                        last_intent = intent;
                        eprintln!("[deckshell] gamescope: overlay refused: no UI frame within {OVERLAY_FRAME_TIMEOUT:?}");
                        continue;
                    }
                    overlay_wait = None;
                    if INTENT.load(Ordering::Acquire) != intent { continue; }
                    // HOME over a game: our window still shows its last buffer, so reveal only after one new frame.
                    if reveal_waiting(request, have_notifier, hidden_before, reveal, PRESENT_FRAME.load(Ordering::Acquire), Instant::now()) {
                        if reveal.is_none() {
                            reveal = Some((PRESENT_FRAME.load(Ordering::Acquire), Instant::now() + REVEAL_TIMEOUT));
                            crate::diag::log("reveal.wait", serde_json::json!({"intent":intent,"present_frame":PRESENT_FRAME.load(Ordering::Acquire),
                                "page_suppressed":PAGE_SUPPRESSED.load(Ordering::Acquire)}));
                            wake_ui();
                        }
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    if let Some((from, deadline)) = reveal.take() {
                        let present = PRESENT_FRAME.load(Ordering::Acquire);
                        crate::diag::log("reveal.done", serde_json::json!({"intent":intent,"new_frame":present != from,
                            "timed_out":present == from && Instant::now() >= deadline,"page_suppressed":PAGE_SUPPRESSED.load(Ordering::Acquire)}));
                    }
                    if request == HOME { hidden_before = false; }
                    last_intent = intent;
                    settle_until = Instant::now() + SETTLE_WINDOW;
                    // Clear while still mapped: gamescope can retain an unmapped overlay.
                    if overlay_set {
                        clear_overlay(c, w, &a);
                        overlay_set = false;
                    }
                    overlay_game = None;
                    OVERLAY_ACTIVE.store(false, Ordering::Release);
                    // Do not promote an arbitrary or stale game to base for an overlay.
                    let gfx = get32(c, root, a.focused_app_gfx).first().copied();
                    let valid_overlay = request == OVERLAY && have_notifier && is_game_app(appid)
                        && apps.contains(&appid) && gfx == Some(appid)
                        && c.get_window_attributes(w).ok().and_then(|r| r.reply().ok())
                            .is_some_and(|r| r.map_state == MapState::VIEWABLE);
                    if request == OVERLAY && !valid_overlay {
                        OVERLAY_REJECTED_INTENT.store(intent, Ordering::Release);
                        eprintln!("[deckshell] gamescope: overlay refused: render notifier, game graphics or own mapped window unavailable");
                    } else if let Some(layer) = policy.request(request, appid, &apps, Instant::now()) {
                        eprintln!("[deckshell] gamescope: user request -> BASELAYER_APPID={layer:?}");
                        set32(c, root, a.base_appid, &layer);
                        if request == OVERLAY {
                            // A frame completed after the transparent UI request.
                            OVERLAY_WINDOW_FLAGGED.store(true, Ordering::Release);
                            crate::diag::log("overlay.flag", serde_json::json!({"flagged":true,"reason":"overlay shown","intent":intent}));
                            set32(c, w, a.overlay, &[1]);
                            set32(c, w, a.input_focus, &[1]);
                            overlay_set = true;
                            overlay_game = Some(appid);
                        }
                        if matches!(request, HOME | GAME | OVERLAY) {
                            crate::actions::yield_steam_ui(intent);
                        }
                    }
                }
                let focused = get32(c, root, a.focused_win).first().copied().unwrap_or(0);
                if crate::diag::enabled() {
                    let focusable = get32(c, root, a.focusable_win);
                    let layer = get32(c, root, a.base_appid);
                    let focused_app = get32(c, root, a.focused_app);
                    let focused_app_gfx = get32(c, root, a.focused_app_gfx);
                    let window_pin = get32(c, root, a.base_window);
                    let snapshot = (focused, focused_app.clone(), focused_app_gfx.clone(), window_pin.clone(), apps.clone(), focusable.clone(), layer.clone(), policy.steam_delegated);
                    // Overlay flags/mapping can change without changing the base focus tuple.
                    // A sixty-second diagnostic heartbeat captures those transitions without flooding the idle session log.
                    if last_focus_snapshot.as_ref() != Some(&snapshot) || last_focus_log.elapsed() >= Duration::from_secs(60) {
                        let windows = c.query_tree(root).ok().and_then(|r|r.reply().ok()).map(|r|r.children).unwrap_or_default()
                            .into_iter().map(|xid| serde_json::json!({"xid":xid,"pid":get32(c,xid,a.pid),"appid":get32(c,xid,a.steam_game),
                                "bp":get32(c,xid,a.steam_bp),"overlay":get32(c,xid,a.overlay),"input_focus":get32(c,xid,a.input_focus),
                                "mapped":c.get_window_attributes(xid).ok().and_then(|r|r.reply().ok()).is_some_and(|r|r.map_state==MapState::VIEWABLE)})).collect::<Vec<_>>();
                        crate::diag::log("gamescope.state", serde_json::json!({"our_xid":w,"focused_xid":focused,"focused_app":focused_app,"focused_app_gfx":focused_app_gfx,"window_pin":window_pin,"apps":apps,"focusable":focusable,"layer":layer,"steam_delegated":policy.steam_delegated,"intent":intent,"windows":windows}));
                        last_focus_snapshot = Some(snapshot);
                        last_focus_log = Instant::now();
                    }
                }
                let live_intent = INTENT.load(Ordering::Acquire);
                let input_app = get32(c, root, a.focused_app).first().copied();
                let gfx_app = get32(c, root, a.focused_app_gfx).first().copied();
                let mapped = c.get_window_attributes(w).ok().and_then(|r| r.reply().ok())
                    .is_some_and(|r| r.map_state == MapState::VIEWABLE);
                let overlay_focused = overlay_set && overlay_game.is_some_and(|game|
                    overlay_observed(game, &apps, gfx_app, input_app, mapped))
                    && get32(c, w, a.overlay).first() == Some(&1)
                    && get32(c, w, a.input_focus).first() == Some(&1)
                    && live_intent == last_intent;
                let observed_game = gfx_app.filter(|game| is_game_app(*game) && apps.contains(game))
                    .filter(|_| !policy.steam_delegated && live_intent == last_intent).unwrap_or(0);
                FOCUSED_GAME.store(observed_game, Ordering::Release);
                if observed_game != 0 { hidden_before = true; }
                // The UI stops or restarts its animation clock on this edge, not on its next poll.
                let covered_now = covered_by_game(last_intent, (observed_game != 0).then_some(observed_game));
                if covered_now != covered_seen {
                    covered_seen = covered_now;
                    crate::diag::log("shell.covered", serde_json::json!({"covered":covered_now,"game":observed_game,"intent":last_intent}));
                    wake_ui();
                }
                // A property flush alone is not compositor acknowledgement. Hold alpha until
                // focus is observed back at the selected game, Steam, or our HOME base.
                if !overlay_set && OVERLAY_WINDOW_FLAGGED.load(Ordering::Acquire) && live_intent == last_intent {
                    let action = (last_intent & 7) as u8;
                    let appid = (last_intent >> 3) as u32;
                    if overlay_flag_cleared(action, appid, &apps, gfx_app, input_app, focused == w) {
                        OVERLAY_WINDOW_FLAGGED.store(false, Ordering::Release);
                        crate::diag::log("overlay.flag", serde_json::json!({"flagged":false,"reason":"focus observed","intent":last_intent,
                            "game_alive":apps.contains(&appid),"gfx":gfx_app,"input":input_app}));
                    }
                }
                FOCUS_INTENT.store(last_intent, Ordering::Release);
                OVERLAY_ACTIVE.store(overlay_focused, Ordering::Release);
                PAD_FOCUSED.store(if overlay_set { overlay_focused } else {
                    live_intent == last_intent && (last_intent & 7) as u8 != OVERLAY
                        && focused == w && !policy.steam_delegated
                }, Ordering::Release);
                // Withhold Steam's virtual pads from the game while the shell owns input.
                if PAD_GATE_REQUESTED.swap(false, Ordering::AcqRel) { pad_gate.requested(); }
                if INTENT.load(Ordering::Acquire) == live_intent {
                    let hold = pad_gate.gate(pad_owner_now((live_intent & 7) as u8, policy.steam_delegated), pad_focus(input_app, &apps), Instant::now());
                    // A request published meanwhile has already set the gate itself.
                    if !PAD_GATE_REQUESTED.load(Ordering::Acquire) { crate::input::padgate::set(hold); }
                    crate::input::padgate::tick();
                }
                let game_before = policy.requested_game;
                let lifecycle_layer = policy.update(&apps, Instant::now());
                let exit_intent = game_exit_intent(last_intent, game_before, policy.requested_game, &apps, gfx_app);
                if let Some(next) = exit_intent {
                    let published = publish_exit_intent(&INTENT, last_intent, next);
                    crate::diag::log("gamescope.game_exit", serde_json::json!({"from":last_intent,"to":next,"published":published}));
                }
                // Play writes the game layer before its window exists; re-assert it once the window is
                // focusable but not shown. The [shell] step before it is invisible only while the shell is the shown layer.
                if INTENT.load(Ordering::Acquire) == last_intent {
                    if let Some(layer) = policy.handover_retry(&apps, gfx_app, Instant::now()) {
                        eprintln!("[deckshell] gamescope: game {} focusable but not shown -> re-assert BASELAYER_APPID={layer:?}", layer[0]);
                        set32(c, root, a.base_appid, &[BP_APPID]);
                        set32(c, root, a.base_appid, &layer);
                    }
                }
                // A lifecycle transition or a newer UI request must be reconciled
                // by the next normal request handler before any fallback writes.
                if focused == 0 && !policy.steam_delegated && INTENT.load(Ordering::Acquire) == last_intent {
                    let since = *unfocused_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_millis(400) {
                        let layer = policy.preferred_layer();
                        eprintln!("[deckshell] gamescope: nothing focused -> BASELAYER_APPID={layer:?}");
                        set32(c, w, a.steam_game, &[BP_APPID]);
                        set32(c, root, a.base_appid, &layer);
                        unfocused_since = None;
                    }
                } else {
                    unfocused_since = None;
                }
                if let Some(layer) = lifecycle_layer {
                    if overlay_set && overlay_game.is_some_and(|game| !apps.contains(&game)) {
                        clear_overlay(c, w, &a);
                        overlay_set = false;
                        overlay_game = None;
                        OVERLAY_ACTIVE.store(false, Ordering::Release);
                        PAD_FOCUSED.store(false, Ordering::Release);
                    }
                    if exit_intent.is_none() && INTENT.load(Ordering::Acquire) == last_intent {
                        eprintln!("[deckshell] gamescope: app lifecycle -> BASELAYER_APPID={layer:?}");
                        set32(c, root, a.base_appid, &layer);
                    }
                }
                if last_ui_scan.elapsed() >= UI_SCAN {
                    last_ui_scan = Instant::now();
                    let windows = steam_windows(c, root, &a);
                    let delegated = policy.steam_delegated || (last_intent & 7) as u8 == STEAM_UI;
                    let found = crate::steam_ui_guard::foreign_steam_ui(&windows, std::process::id(), delegated);
                    let game = found.and_then(|_| steam_control::running_game());
                    match ui_guard.step(found, game, Instant::now()) {
                        crate::steam_ui_guard::Step::Act { line, close } => {
                            eprintln!("{line}");
                            crate::diag::log("steam_ui_guard", serde_json::json!({"line":line,"close":close}));
                            if close { std::thread::spawn(crate::steam_ui_guard::request_close); }
                            // Keep gamescope's base layer on DeckShell (or the selected game) meanwhile.
                            if INTENT.load(Ordering::Acquire) == last_intent {
                                let layer = policy.preferred_layer();
                                if get32(c, root, a.base_appid) != layer {
                                    eprintln!("[steam-ui-guard] base layer -> {layer:?}");
                                    set32(c, root, a.base_appid, &layer);
                                }
                            }
                        }
                        crate::steam_ui_guard::Step::Log(line) => eprintln!("{line}"),
                        crate::steam_ui_guard::Step::Nothing => {}
                    }
                }
                // somebody dropped our appid (window recreated / Steam rewrote): re-assert slowly
                if last_assert.elapsed() > Duration::from_secs(3) {
                    last_assert = Instant::now();
                    let focusable = get32(c, root, a.focusable_win);
                    if !focusable.chunks(3).any(|t| t.first() == Some(&w)) {
                        // Restore our identity without undoing an explicitly delegated Steam/game layer.
                        set32(c, w, a.steam_game, &[BP_APPID]);
                    }
                }
            }
        }
        if !ready_done && frame {
            let focused_ok = !gs
                || xid.is_some_and(|w| c.is_some_and(|c| get32(c, root, a.focused_win).first() == Some(&w)));
            if focused_ok {
                ready_done = true;
                match &ready {
                    Some(p) => write_ready(p, xid.unwrap_or(0)),
                    None => {}
                }
                if !base {
                    return;
                }
            } else if t_start.elapsed() > Duration::from_secs(5) && last_log.elapsed() > Duration::from_secs(5) {
                // visibility is decided by gamescope; keep trying (and logging) until the watchdog gives up
                last_log = Instant::now();
                eprintln!("[deckshell] gamescope: frame rendered but window not focused by gamescope yet");
            }
        }
        // `park_timeout` instead of `sleep`: `request` unparks this thread.
        std::thread::park_timeout(if xid.is_none() { Duration::from_millis(50) } else if Instant::now() < settle_until { SETTLE_POLL } else { Duration::from_millis(200) });
    }
}

/// How often the top-level windows are checked for Steam's own UI.
const UI_SCAN: Duration = Duration::from_secs(1);

/// Top-level windows with the marks `steam_ui_guard` needs.
fn steam_windows(c: &RustConnection, root: Window, a: &Atoms) -> Vec<crate::steam_ui_guard::SteamWindow> {
    let children = c.query_tree(root).ok().and_then(|r| r.reply().ok()).map(|r| r.children).unwrap_or_default();
    children.into_iter().filter_map(|xid| {
        let mapped = c.get_window_attributes(xid).ok().and_then(|r| r.reply().ok()).is_some_and(|r| r.map_state == MapState::VIEWABLE);
        if !mapped { return None; }
        let first = |prop: u32| get32(c, xid, prop).first().copied().unwrap_or(0);
        let pid = first(a.pid);
        let comm = if pid != 0 { std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default() } else { String::new() };
        Some(crate::steam_ui_guard::SteamWindow { xid, pid, appid: first(a.steam_game), bigpicture: first(a.steam_bp), overlay: first(a.overlay), mapped, comm })
    }).collect()
}

fn clear_overlay(c: &RustConnection, w: Window, a: &Atoms) {
    set32(c, w, a.input_focus, &[0]);
    set32(c, w, a.overlay, &[0]);
}

fn claim(c: &RustConnection, w: Window, root: Window, a: &Atoms) {
    set32(c, w, a.steam_game, &[BP_APPID]);
    set32(c, root, a.base_appid, &[BP_APPID]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn passive_notifications_are_never_main_window_candidates() {
        assert!(is_passive_overlay_title(REC_BADGE_TITLE));
        assert!(is_passive_overlay_title(crate::level::POPUP_TITLE));
        assert!(!is_passive_overlay_title("DeckShell"));
    }

    #[test]
    fn observed_game_exit_retires_generation_once_for_home_yield() {
        let now = Instant::now();
        for action in [GAME, OVERLAY] {
            let old = next_intent(0, action, 42);
            let current = AtomicU64::new(old);
            let mut p = LayerPolicy::default();
            p.request(action, 42, &[BP_APPID, 42], now);
            let before = p.requested_game;
            assert_eq!(p.update(&[BP_APPID, STEAM_APPID], now), Some(vec![BP_APPID]));
            let home = game_exit_intent(old, before, p.requested_game, &[BP_APPID, STEAM_APPID], Some(STEAM_APPID)).unwrap();
            assert!(publish_exit_intent(&current, old, home));
            assert_eq!(home & 7, u64::from(HOME));
            assert_eq!(owned_intent_layer(home), Some(vec![BP_APPID]));
            assert!(!dismiss_matches(home, old));
            assert!(dismiss_matches(home, home));
            assert!(!publish_exit_intent(&current, old, home));
            assert_eq!(p.update(&[BP_APPID, STEAM_APPID], now + Duration::from_millis(200)), None);
        }
    }

    #[test]
    fn exit_cannot_replace_a_newer_home_game_or_steam_request() {
        let old = next_intent(0, GAME, 42);
        let exit = game_exit_intent(old, Some(42), None, &[BP_APPID], None).unwrap();
        for (action, id) in [(HOME, 0), (GAME, 69), (STEAM_UI, 0), (OVERLAY, 69)] {
            let newer = next_intent(old, action, id);
            let current = AtomicU64::new(newer);
            assert!(!publish_exit_intent(&current, old, exit));
            assert_eq!(current.load(Ordering::Acquire), newer);
        }
    }

    #[test]
    fn pending_unseen_launch_is_not_retired() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        let intent = next_intent(0, GAME, 42);
        p.request(GAME, 42, &[BP_APPID], now);
        let before = p.requested_game;
        assert_eq!(p.update(&[BP_APPID], now + Duration::from_secs(5)), None);
        assert_eq!(game_exit_intent(intent, before, p.requested_game, &[BP_APPID], None), None);
        assert_eq!(p.requested_game, Some(42));
    }

    #[test]
    fn exit_preserves_another_observed_foreground_game_only() {
        let old = next_intent(0, GAME, 42);
        let apps = [BP_APPID, STEAM_APPID, 69];
        let next = game_exit_intent(old, Some(42), None, &apps, Some(69)).unwrap();
        assert_eq!(next & 7, u64::from(GAME));
        assert_eq!((next >> 3) as u32, 69);
        assert_eq!(owned_intent_layer(next), Some(vec![69, BP_APPID]));
        for gfx in [None, Some(42), Some(70), Some(STEAM_APPID), Some(BP_APPID)] {
            assert_eq!(game_exit_intent(old, Some(42), None, &apps, gfx).unwrap() & 7, u64::from(HOME));
        }
        assert_eq!(game_exit_intent(old, Some(42), None, &[BP_APPID, 42], Some(42)), None);
        let delegated = next_intent(old, STEAM_UI, 0);
        assert_eq!(game_exit_intent(delegated, Some(42), None, &apps, Some(69)), None);
    }

    #[test]
    fn repair_uses_current_apps_even_when_delegation_expiry_keeps_old_known_apps() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(STEAM_UI, 0, &[BP_APPID, STEAM_APPID], now);
        p.update(&[BP_APPID], now);
        assert_eq!(p.update(&[BP_APPID, 69], now + Duration::from_millis(500)), Some(vec![BP_APPID]));
        assert!(!p.known_apps.as_ref().unwrap().contains(&69));
        assert_eq!(p.repair_layer(&[69, STEAM_APPID], &[BP_APPID, 69]), None);
        assert_eq!(p.repair_layer(&[9001, STEAM_APPID], &[]), None);
    }

    #[test]
    fn home_repairs_steam_behind_unavailable_layer_prefix() {
        let mut p = LayerPolicy::default();
        let now = Instant::now();
        p.request(HOME, 0, &[BP_APPID, STEAM_APPID], now);
        // Synthetic missing target models the captured post-exit queue. Its numeric
        // identity is irrelevant: Gamescope skips it and selects Steam next.
        assert_eq!(p.repair_layer(&[9001, STEAM_APPID], &[BP_APPID, STEAM_APPID]), Some(vec![BP_APPID]));
        assert_eq!(p.repair_layer(&[9001, 9002, STEAM_APPID, BP_APPID], &[BP_APPID, STEAM_APPID]), Some(vec![BP_APPID]));
        assert_eq!(p.repair_layer(&[9001, BP_APPID, STEAM_APPID], &[BP_APPID, STEAM_APPID]), None);
        assert_eq!(p.repair_layer(&[9001], &[BP_APPID, STEAM_APPID]), None);
        p.update(&[BP_APPID, STEAM_APPID, 9001], now);
        assert_eq!(p.repair_layer(&[9001, STEAM_APPID], &[BP_APPID, STEAM_APPID, 9001]), None);
        p.request(STEAM_UI, 0, &[BP_APPID, STEAM_APPID], now);
        assert_eq!(p.repair_layer(&[9001, STEAM_APPID], &[BP_APPID, STEAM_APPID]), None);
    }

    #[test]
    fn queued_home_dismissal_requires_the_same_generation() {
        let home = next_intent(0, HOME, 0);
        assert!(dismiss_matches(home, home));
        for (action, appid) in [(HOME, 0), (STEAM_UI, 0), (GAME, 42)] {
            assert!(!dismiss_matches(next_intent(home, action, appid), home));
        }
        let game = next_intent(home, GAME, 42);
        let newer_home = next_intent(game, HOME, 0);
        assert!(!dismiss_matches(newer_home, home));
        assert!(dismiss_matches(newer_home, newer_home));
        assert!(dismiss_matches(game, game));
        assert_eq!(owned_intent_layer(game), Some(vec![42, BP_APPID]));
        assert_eq!(owned_intent_layer(home), Some(vec![BP_APPID]));
        assert_eq!(owned_intent_layer(next_intent(home, GAME, STEAM_APPID)), None);
        assert_eq!(owned_intent_layer(next_intent(home, STEAM_UI, 0)), None);
        assert!(!dismiss_matches(0, 0));
    }

    use super::*;

    #[test]
    fn overlay_close_waits_for_current_intent_and_cleared_flag() {
        let overlay = next_intent(0, OVERLAY, 42);
        let close = next_intent(overlay, GAME, 42);
        let newer_close = next_intent(close, HOME, 0);
        assert!(!overlay_close_acknowledged(overlay, overlay, false));
        assert!(!overlay_close_acknowledged(close, overlay, false));
        assert!(!overlay_close_acknowledged(close, close, true));
        assert!(overlay_close_acknowledged(close, close, false));
        assert!(!overlay_close_acknowledged(newer_close, close, false));
        assert!(overlay_close_acknowledged(newer_close, newer_close, false));
    }

    #[test]
    fn overlay_waits_for_new_frame_after_its_request() {
        let intent = next_intent(0, OVERLAY, 42);
        assert!(!overlay_frame_ready(intent, intent, 5, 5));
        assert!(overlay_frame_ready(intent, intent, 5, 6));
    }

    #[test]
    fn newer_overlay_cannot_consume_an_old_requests_frame() {
        let old = next_intent(0, OVERLAY, 42);
        let new = next_intent(old, OVERLAY, 42);
        assert!(!overlay_frame_ready(new, old, 5, 6));
        assert!(!overlay_frame_ready(new, new, 6, 6));
        assert!(overlay_frame_ready(new, new, 6, 7));
        assert!(!dismiss_matches(next_intent(old, HOME, 0), old));
    }

    #[test]
    fn overlay_yield_and_focus_require_exact_game_and_current_generation() {
        let overlay = next_intent(0, OVERLAY, u32::MAX);
        assert_eq!(owned_intent_layer(overlay), Some(vec![u32::MAX, BP_APPID]));
        assert!(dismiss_matches(overlay, overlay));
        assert!(!dismiss_matches(next_intent(overlay, HOME, 0), overlay));
        assert!(!dismiss_matches(next_intent(overlay, OVERLAY, u32::MAX), overlay));
        let apps = [BP_APPID, 42];
        assert!(overlay_observed(42, &apps, Some(42), Some(BP_APPID), true));
        assert!(!overlay_observed(42, &apps, Some(42), Some(42), true));
        assert!(!overlay_observed(42, &apps, Some(BP_APPID), Some(BP_APPID), true));
        assert!(!overlay_observed(42, &apps, Some(42), Some(BP_APPID), false));
        assert!(!overlay_observed(42, &[BP_APPID], Some(42), Some(BP_APPID), true));
    }

    #[test]
    fn overlay_policy_keeps_game_and_game_exit_restores_home() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        assert_eq!(p.request(OVERLAY, 42, &[BP_APPID, 42], now), Some(vec![42, BP_APPID]));
        assert_eq!(p.repair_layer(&[42, BP_APPID], &[BP_APPID, STEAM_APPID, 42]), None);
        assert_eq!(p.update(&[BP_APPID, 42], now), None);
        assert_eq!(p.update(&[BP_APPID], now), Some(vec![BP_APPID]));
        assert_eq!(p.preferred_layer(), vec![BP_APPID]);
    }

    /// Owner scenario of 2026-11-15 21:03 (task 342): game menu -> Library -> back to the game -> game menu ->
    /// «Выйти из игры». The game's window was gone when the closing menu published GAME; the selection was then
    /// treated as a launch not mapped yet, the GAME intent stayed on a dead game, the overlay flag was never
    /// acknowledged and the page stayed hidden under a drawn header and footer.
    #[test]
    fn game_that_died_under_its_menu_is_retired_and_the_page_flag_drops() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        let alive = [BP_APPID, 42];
        let dead = [BP_APPID];
        // Launch and play.
        let mut intent = next_intent(0, GAME, 42);
        p.request(GAME, 42, &dead, now);
        assert_eq!(p.update(&alive, now), None);
        // Menu over the game -> Library page (HOME) -> back to the game (GAME) -> menu again (OVERLAY).
        for (action, id) in [(OVERLAY, 42), (HOME, 0), (GAME, 42), (OVERLAY, 42)] {
            intent = next_intent(intent, action, id);
            p.request(action, id, &alive, now);
            assert_eq!(p.update(&alive, now), None);
        }
        // «Выйти из игры»: the game dies while the menu slides out, then the menu's close publishes GAME.
        intent = next_intent(intent, GAME, 42);
        p.request(GAME, 42, &dead, now);
        let before = p.requested_game;
        assert_eq!(p.update(&dead, now), Some(vec![BP_APPID]), "a game seen before and gone now is retired, not awaited");
        let home = game_exit_intent(intent, before, p.requested_game, &dead, Some(BP_APPID)).expect("exit publishes HOME");
        assert_eq!(home & 7, u64::from(HOME));
        // Even before HOME is processed, the stale GAME intent no longer keeps the page transparent.
        assert!(overlay_flag_cleared(GAME, 42, &dead, Some(BP_APPID), Some(BP_APPID), true));
        assert!(overlay_flag_cleared(HOME, 0, &dead, Some(BP_APPID), Some(BP_APPID), true));
    }

    #[test]
    fn overlay_flag_holds_until_the_requested_focus_is_observed() {
        let alive = [BP_APPID, 42];
        // Closing the menu back to a live game: transparent until the game owns graphics and input.
        assert!(!overlay_flag_cleared(GAME, 42, &alive, Some(42), Some(BP_APPID), true));
        assert!(!overlay_flag_cleared(GAME, 42, &alive, Some(BP_APPID), Some(BP_APPID), true));
        assert!(overlay_flag_cleared(GAME, 42, &alive, Some(42), Some(42), false));
        // Dead game: only once our own window is the shown and focused one.
        assert!(!overlay_flag_cleared(GAME, 42, &[BP_APPID], Some(BP_APPID), Some(BP_APPID), false));
        assert!(!overlay_flag_cleared(GAME, 42, &[BP_APPID, STEAM_APPID], Some(STEAM_APPID), Some(STEAM_APPID), false));
        // HOME and Steam keep their rules; an overlay request never clears.
        assert!(!overlay_flag_cleared(HOME, 0, &alive, Some(42), Some(BP_APPID), true));
        assert!(overlay_flag_cleared(STEAM_UI, 0, &alive, Some(STEAM_APPID), Some(STEAM_APPID), false));
        assert!(!overlay_flag_cleared(OVERLAY, 42, &alive, Some(42), Some(BP_APPID), true));
    }

    #[test]
    fn a_launch_not_mapped_yet_is_still_awaited() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        // Fresh launch: never seen, so absence is not an exit.
        p.request(GAME, 42, &[BP_APPID], now);
        assert_eq!(p.update(&[BP_APPID], now + Duration::from_secs(30)), None);
        // A different game selected after one was seen starts unseen again.
        p.update(&[BP_APPID, 42], now);
        p.request(GAME, 43, &[BP_APPID, 42], now);
        assert_eq!(p.update(&[BP_APPID, 42], now + Duration::from_secs(30)), None);
    }

    #[test]
    fn resume_selects_an_existing_game_after_returning_home() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.update(&[BP_APPID, 42], now);
        p.request(HOME, 0, &[BP_APPID, 42], now);
        assert_eq!(p.update(&[BP_APPID, 42], now + Duration::from_secs(2)), None);
        assert_eq!(p.request(GAME, 42, &[BP_APPID, 42], now), Some(vec![42, BP_APPID]));
        assert_eq!(p.repair_layer(&[BP_APPID], &[BP_APPID, STEAM_APPID, 42]), Some(vec![42, BP_APPID]));
        assert_eq!(p.repair_layer(&[42, BP_APPID], &[BP_APPID, STEAM_APPID, 42]), None);
    }

    #[test]
    fn launch_selects_game_before_window_maps_and_survives_steam_rewrite() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        assert_eq!(p.request(GAME, 42, &[BP_APPID], now), Some(vec![42, BP_APPID]));
        assert_eq!(p.preferred_layer(), vec![42, BP_APPID]);
        assert_eq!(p.update(&[BP_APPID], now + Duration::from_secs(30)), None);
        assert_eq!(p.repair_layer(&[STEAM_APPID], &[BP_APPID, STEAM_APPID, 42]), Some(vec![42, BP_APPID]));
        assert_eq!(p.update(&[BP_APPID, 42], now + Duration::from_secs(31)), None);
        assert_eq!(p.repair_layer(&[BP_APPID], &[BP_APPID, STEAM_APPID, 42]), Some(vec![42, BP_APPID]));
    }

    #[test]
    fn guide_cancels_explicit_launch_even_if_window_maps_later() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(GAME, 42, &[BP_APPID], now);
        assert_eq!(p.request(HOME, 0, &[BP_APPID], now), Some(vec![BP_APPID]));
        p.update(&[BP_APPID, 42], now);
        assert_eq!(p.update(&[BP_APPID, 42], now + Duration::from_secs(2)), None);
        assert_eq!(p.repair_layer(&[STEAM_APPID], &[BP_APPID, STEAM_APPID, 42]), Some(vec![BP_APPID]));
        assert_eq!(p.request(GAME, 42, &[BP_APPID, 42], now), Some(vec![42, BP_APPID]));
    }

    #[test]
    fn explicit_steam_ui_cancels_game_layer_repair() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(GAME, 42, &[BP_APPID, 42], now);
        assert_eq!(p.request(STEAM_UI, 0, &[BP_APPID, 42], now), Some(vec![STEAM_APPID, BP_APPID]));
        assert_eq!(p.repair_layer(&[STEAM_APPID, BP_APPID], &[BP_APPID, STEAM_APPID, 42]), None);
        assert_eq!(p.update(&[BP_APPID, 42, STEAM_APPID], now), None);
    }

    #[test]
    fn closing_requested_game_returns_to_shell() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(GAME, 42, &[BP_APPID], now);
        p.update(&[BP_APPID, 42], now);
        assert_eq!(p.update(&[BP_APPID], now), Some(vec![BP_APPID]));
        assert_eq!(p.repair_layer(&[BP_APPID], &[BP_APPID, STEAM_APPID, 42]), None);
    }

    #[test]
    fn unrelated_windows_do_not_replace_explicit_game_selection() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(GAME, 42, &[BP_APPID, 42], now);
        p.update(&[BP_APPID, 42, 43], now);
        assert_eq!(p.update(&[BP_APPID, 42, 43], now + Duration::from_secs(2)), None);
        assert_eq!(p.preferred_layer(), vec![42, BP_APPID]);
    }

    #[test]
    fn intent_packs_full_appid_and_changes_for_repeated_resume_and_home() {
        let first = next_intent(0, GAME, u32::MAX);
        assert_eq!(first & 7, u64::from(GAME));
        assert_eq!((first >> 3) as u32, u32::MAX);
        let second = next_intent(first, GAME, u32::MAX);
        assert_ne!(first, second);
        let home = next_intent(second, HOME, 0);
        assert_eq!(home & 7, u64::from(HOME));
        assert_eq!((home >> 3) as u32, 0);
        assert_ne!(second, home);
        let wrapped = next_intent((0x1fff_ffffu64 << 35) | 3, GAME, 42);
        assert_eq!(wrapped >> 35, 0);
        assert_eq!((wrapped >> 3) as u32, 42);
    }

    #[test]
    fn invalid_game_ids_do_not_select_shell_or_steam_as_game() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        for appid in [0, BP_APPID, STEAM_APPID] {
            assert_eq!(p.request(GAME, appid, &[BP_APPID], now), None);
            assert_eq!(p.requested_game, None);
        }
    }

    #[test]
    fn guide_cancels_pending_promotion_and_ignores_native_steam() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.update(&[BP_APPID], now);
        p.update(&[BP_APPID, STEAM_APPID, 42], now);
        assert_eq!(p.request(HOME, 0, &[BP_APPID, STEAM_APPID, 42], now), Some(vec![BP_APPID]));
        assert_eq!(p.update(&[BP_APPID, STEAM_APPID, 42], now + Duration::from_secs(2)), None);
        assert!(!p.steam_delegated);
    }

    #[test]
    fn store_can_reopen_an_existing_steam_window_and_return_on_close() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.update(&[BP_APPID, STEAM_APPID], now);
        assert_eq!(p.request(STEAM_UI, 0, &[BP_APPID, STEAM_APPID], now), Some(vec![STEAM_APPID, BP_APPID]));
        assert_eq!(p.update(&[BP_APPID], now), None);
        assert_eq!(p.update(&[BP_APPID], now + Duration::from_millis(500)), Some(vec![BP_APPID]));
        assert!(!p.steam_delegated);
    }

    #[test]
    fn missing_store_expires_and_browser_recreation_is_tolerated() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(STEAM_UI, 0, &[BP_APPID], now);
        p.update(&[BP_APPID], now);
        assert_eq!(p.update(&[BP_APPID], now + Duration::from_secs(5)), Some(vec![BP_APPID]));
        p.request(STEAM_UI, 0, &[BP_APPID, STEAM_APPID], now);
        assert_eq!(p.update(&[BP_APPID], now), None);
        assert_eq!(p.update(&[BP_APPID, STEAM_APPID], now + Duration::from_millis(200)), None);
        assert!(p.steam_delegated);
    }

    #[test]
    fn delayed_store_mapping_and_game_fallback() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(STEAM_UI, 0, &[BP_APPID], now);
        assert_eq!(p.update(&[BP_APPID], now), None);
        assert!(p.steam_delegated);
        p.update(&[BP_APPID, STEAM_APPID], now);
        assert!(p.steam_seen);
        p.request(HOME, 0, &[BP_APPID, STEAM_APPID], now);
        p.update(&[BP_APPID, STEAM_APPID, 42], now);
        assert_eq!(p.update(&[BP_APPID, STEAM_APPID, 42], now + Duration::from_secs(2)), Some(vec![42, BP_APPID]));
        assert_eq!(p.update(&[BP_APPID, STEAM_APPID], now + Duration::from_secs(3)), None);
    }

    #[test]
    fn play_reasserts_the_game_layer_once_its_window_is_focusable() {
        let now = Instant::now();
        let mut p = LayerPolicy::default();
        p.request(GAME, 42, &[BP_APPID], now);
        // Play: the window is not focusable yet, so there is nothing to re-assert.
        assert_eq!(p.handover_retry(&[BP_APPID], Some(BP_APPID), now), None);
        // The window appears while the shell is still shown: re-assert at once.
        let apps = [BP_APPID, 42];
        assert_eq!(p.handover_retry(&apps, Some(BP_APPID), now), Some(vec![42, BP_APPID]));
        // Within the interval nothing is written again.
        assert_eq!(p.handover_retry(&apps, Some(BP_APPID), now + Duration::from_millis(200)), None);
        assert_eq!(p.handover_retry(&apps, Some(BP_APPID), now + HANDOVER_INTERVAL), Some(vec![42, BP_APPID]));
        // Shown: stop.
        assert_eq!(p.handover_retry(&apps, Some(42), now + Duration::from_secs(5)), None);
    }

    #[test]
    fn resume_of_a_running_game_is_reasserted_and_budget_is_per_request() {
        let now = Instant::now();
        let apps = [BP_APPID, 42];
        let mut p = LayerPolicy::default();
        // Resume: the window already exists, the shell is shown.
        p.request(GAME, 42, &apps, now);
        let mut t = now;
        let mut writes = 0;
        for _ in 0..100 {
            if p.handover_retry(&apps, Some(BP_APPID), t).is_some() { writes += 1; }
            t += HANDOVER_INTERVAL;
        }
        assert_eq!(writes, usize::from(HANDOVER_ATTEMPTS));
        // A new request starts a new budget.
        p.request(GAME, 42, &apps, t);
        assert_eq!(p.handover_retry(&apps, Some(BP_APPID), t), Some(vec![42, BP_APPID]));
        // Going Home ends the retries for that game.
        p.request(HOME, 0, &apps, t);
        assert_eq!(p.handover_retry(&apps, Some(BP_APPID), t + HANDOVER_INTERVAL), None);
    }

    #[test]
    fn covered_only_by_a_focused_game_under_game_intent() {
        let game = (42u64 << 3) | GAME as u64;
        assert!(covered_by_game(game, Some(42)));
        // Nothing observed yet: the shell still counts as visible, so the UI keeps animating.
        assert!(!covered_by_game(game, None));
        // Every shell-visible request uncovers at once, even while the game is still focused.
        for action in [HOME, STEAM_UI, OVERLAY] {
            assert!(!covered_by_game((42u64 << 3) | action as u64, Some(42)));
        }
    }

    #[test]
    fn overlay_frame_wait_is_bounded() {
        // A stalled UI (no new frame) must not hold the gamescope worker: the wait ends at the timeout.
        let since = Instant::now();
        assert!(!overlay_wait_expired(since, since));
        assert!(!overlay_wait_expired(since, since + OVERLAY_FRAME_TIMEOUT - Duration::from_millis(1)));
        assert!(overlay_wait_expired(since, since + OVERLAY_FRAME_TIMEOUT));
        // Frame ready: the gate opens without the timeout.
        assert!(overlay_frame_ready(7, 7, 3, 4));
        assert!(!overlay_frame_ready(7, 7, 3, 3));
    }

    #[test]
    fn home_reveal_waits_for_one_new_frame_and_only_after_a_covering_game() {
        let now = Instant::now();
        let deadline = now + REVEAL_TIMEOUT;
        // Never covered: no wait (a plain Home request keeps its old timing).
        assert!(!reveal_waiting(HOME, true, false, None, 5, now));
        // Covered: the wait starts.
        assert!(reveal_waiting(HOME, true, true, None, 5, now));
        // Still no new frame: keep waiting.
        assert!(reveal_waiting(HOME, true, true, Some((5, deadline)), 5, now));
        // A new frame completed: reveal.
        assert!(!reveal_waiting(HOME, true, true, Some((5, deadline)), 6, now));
        // No frames at all (UI stuck): reveal at the deadline, never hang the shell.
        assert!(!reveal_waiting(HOME, true, true, Some((5, deadline)), 5, deadline));
        // Other requests and the time-based fallback (no render notifier) never wait.
        assert!(!reveal_waiting(GAME, true, true, None, 5, now));
        assert!(!reveal_waiting(HOME, false, true, None, 5, now));
    }

    #[test]
    fn frame_under_a_game_menu_does_not_release_the_home_reveal() {
        use std::sync::atomic::AtomicU64;
        let frame = AtomicU64::new(0);
        let present = AtomicU64::new(0);
        let now = Instant::now();
        // Reveal starts while the game menu still hides the page: the wait records the presentable count.
        let from = present.load(Ordering::Acquire);
        let deadline = now + REVEAL_TIMEOUT;
        // Frames drawn with the page hidden advance FRAME (overlay requests still see them) but not PRESENT.
        count_frame(&frame, &present, true);
        count_frame(&frame, &present, true);
        assert_eq!(frame.load(Ordering::Acquire), 2);
        assert_eq!(present.load(Ordering::Acquire), from);
        assert!(reveal_waiting(HOME, true, true, Some((from, deadline)), present.load(Ordering::Acquire), now));
        // The first frame with the page on screen releases the reveal.
        count_frame(&frame, &present, false);
        assert_eq!(present.load(Ordering::Acquire), from + 1);
        assert!(!reveal_waiting(HOME, true, true, Some((from, deadline)), present.load(Ordering::Acquire), now));
    }
}
