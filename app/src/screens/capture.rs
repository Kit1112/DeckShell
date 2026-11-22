//! Explicit screenshot API and native Steam recording UI. Physical capture chords
//! remain owned by Steam Input; no custom recorder or duplicate native toggle.
use steam_assets::i18n;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use steam_bridge::recording::ClipSummary;

use crate::app::App;
use crate::assets::t_or;

const DRY_ROOT: &str = "/tmp/deckshell-capture";
/// appid directory used when no game runs. UNVERIFIED: the Steam client itself uses its own id here (SPEC does not say).
const DESKTOP_APPID: u32 = 7;

/// `(screenshot, thumbnail, n)`: first free `ts_N.jpg` with N from 1.
pub fn shot_names(dir: &Path, ts: &str, exists: impl Fn(&Path) -> bool) -> (PathBuf, PathBuf, u32) {
    let mut n = 1;
    loop {
        let name = format!("{ts}_{n}.jpg");
        let p = dir.join("screenshots").join(&name);
        if !exists(&p) {
            return (p, dir.join("screenshots").join("thumbnails").join(&name), n);
        }
        n += 1;
    }
}

pub fn ffmpeg_bin() -> PathBuf {
    let p = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/deckshell/bin/ffmpeg");
    if p.is_file() { p } else { PathBuf::from("ffmpeg") }
}

/// ffmpeg args converting a grab to the full JPEG and the 200 px thumbnail in one run.
pub fn convert_args(src: &Path, jpg: &Path, thumb: &Path) -> Vec<String> {
    let s = |p: &Path| p.to_string_lossy().into_owned();
    vec![
        "-y".into(), "-loglevel".into(), "error".into(), "-i".into(), s(src),
        "-map".into(), "0:v".into(), "-q:v".into(), "2".into(), "-frames:v".into(), "1".into(), s(jpg),
        "-map".into(), "0:v".into(), "-vf".into(), "scale=200:-1".into(), "-q:v".into(), "4".into(), "-frames:v".into(), "1".into(), s(thumb),
    ]
}

#[derive(Debug, PartialEq, Eq)]
enum Backend {
    /// gamescope session: no capture subcommand is documented for `gamescopectl` (context/archive/docs/gamescope-integration.md lists only
    /// `screenshot_taken`, a notification) and the GAMESCOPECTRL_REQUEST_SCREENSHOT atom format/output path is not verified,
    /// so the X root of gamescope's Xwayland is grabbed. UNVERIFIED on the Deck: may show less than the whole composited frame.
    GamescopeX11,
    Spectacle,
    None,
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

fn backend() -> Backend {
    if std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some() {
        Backend::GamescopeX11
    } else if which("spectacle") {
        Backend::Spectacle
    } else {
        Backend::None
    }
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let o = cmd.stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

fn grab(raw: &Path) -> Result<(), String> {
    match backend() {
        Backend::Spectacle => run(Command::new("spectacle").args(["-b", "-n", "-f", "-o"]).arg(raw)),
        Backend::GamescopeX11 => {
            let d = std::env::var("DISPLAY").unwrap_or_default();
            run(Command::new(ffmpeg_bin()).args(["-y", "-loglevel", "error", "-f", "x11grab", "-i", &d, "-frames:v", "1"]).arg(raw))
        }
        Backend::None => Err("no capture backend (spectacle / gamescope X11)".into()),
    }
}

fn shot_root(steam_root: &Path, user: Option<u32>, appid: u32) -> PathBuf {
    if crate::actions::dryrun() {
        return PathBuf::from(DRY_ROOT).join("userdata").join("0").join("760/remote").join(appid.to_string());
    }
    steam_root.join("userdata").join(user.unwrap_or(0).to_string()).join("760/remote").join(appid.to_string())
}

impl App {
    /// Separate explicit screenshot API; never dispatched for physical STEAM+R1.
    pub fn take_screenshot(&mut self) {
        let appid = self.live.running.first().copied().unwrap_or(DESKTOP_APPID);
        let root = shot_root(&self.base.steam_root, self.base.active_user, appid);
        let ok_text = t_or("#MediaPage_TakeScreenshot", i18n::tr_ref("capture.screenshot_taken"));
        let fail_text = t_or("#DeckShell_ScreenshotFailed", i18n::tr_ref("capture.couldn_t_take_a_screenshot"));
        let _ = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
            let ts = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
            let (jpg, thumb, _) = shot_names(&root, &ts, |p| p.exists());
            let tmp = PathBuf::from(DRY_ROOT).join(format!("raw_{ts}.png"));
            let res = (|| {
                std::fs::create_dir_all(DRY_ROOT).map_err(|e| e.to_string())?;
                std::fs::create_dir_all(thumb.parent().unwrap()).map_err(|e| e.to_string())?;
                grab(&tmp)?;
                run(Command::new(ffmpeg_bin()).args(convert_args(&tmp, &jpg, &thumb)))
            })();
            let _ = std::fs::remove_file(&tmp);
            let succeeded = res.is_ok();
            let msg = match res {
                Ok(()) => {
                    eprintln!("[deckshell] screenshot {}", jpg.display());
                    ok_text
                }
                Err(e) => {
                    eprintln!("[deckshell] screenshot failed: {e}");
                    fail_text
                }
            };
            super::defer(move |a| { if succeeded {
                let event = crate::notifications::Event {
                    title: msg,
                    kind: "screenshot".into(),
                    image: jpg.to_string_lossy().into_owned(),
                    ..Default::default()
                };
                crate::notifications::enqueue(&a.ui(), event, !a.live.running.is_empty());
            } else { a.toast(&msg); } });
        });
    }

}

#[derive(Default)]
struct NativeCapture {
    reading: bool,
    sending: bool,
    view: Option<steam_bridge::recording::NativeView>,
    pending: Option<steam_bridge::recording::PendingCommand>,
    note: String,
    poll_scheduled: bool,
    /// A physical STEAM+A press waits for a native view read that started after it and decides start or stop.
    record_chord_at: Option<Instant>,
    /// Start of the newest native read; a press that arrives during a read is decided by the next one.
    read_started_at: Option<Instant>,
    /// When the pending Start or Stop was sent; bounds the confirmation re-reads.
    pending_since: Option<Instant>,
    pending_scheduled: bool,
    /// After an accepted Stop the clip is written later: watch the cache for a new permanent clip for a bounded time.
    clip_watch: Option<ClipWatch>,
    clip_watch_scheduled: bool,
    /// When the last Steam session ended: our own Stop, or a session end seen by a read (also a stock Stop).
    /// A new Start waits `START_COOLDOWN` after it.
    session_ended_at: Option<Instant>,
    /// Whether the last successful read showed an active session; a change to none marks a session end.
    last_active_seen: bool,
    gate_scheduled: bool,
    /// When DeckShell sent the last Stop. The REC indicator stays hidden for `STOP_BADGE_HOLD` after it, whatever a
    /// cached session snapshot still says (see `rec_badge_visible`).
    stop_sent_at: Option<Instant>,
    /// The first successful cached observation is a baseline, not a new event.
    notification_baseline: Option<bool>,
}

/// Clip ids known before a Stop, and when to give up waiting for the new clip.
#[derive(Clone)]
struct ClipWatch {
    before: Vec<String>,
    deadline: Instant,
}

enum WatchOutcome { Idle, Waiting, Found(u64), Expired }

const CLIP_WATCH_WINDOW: Duration = Duration::from_secs(60);
const CLIP_WATCH_STEP: Duration = Duration::from_secs(2);
/// Re-read on all routes, including while no recording is visible.
const RECORDING_POLL_STEP: Duration = Duration::from_secs(2);
/// After a Stop DeckShell sent, the REC indicator is hidden for this long even if the cached session still reads active.
pub const STOP_BADGE_HOLD: Duration = Duration::from_secs(30);
const PENDING_STEP: Duration = Duration::from_secs(1);
const PENDING_WINDOW: Duration = Duration::from_secs(60);
/// How long a press may wait for an unconfirmed previous command before it is dropped with a message.
const QUEUE_WINDOW: Duration = Duration::from_secs(10);
/// Minimum time between the end of a Steam recording session and the next Start.
/// Task 182: a Start 10.7 s after a Stop was followed by a GPU encoder (VCN) ring timeout and a Steam crash;
/// a Start 104 s after a Stop was fine. The cause is not proven; this margin is a mitigation, not a Steam fact.
pub const START_COOLDOWN: Duration = Duration::from_secs(60);

/// Why a new native Start must wait. `None` from `start_wait` means it may be dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartWait {
    /// Steam is still writing the previous clip: no new permanent clip has appeared yet and the watch is open.
    SaveOpen,
    /// The session ended too recently; the remaining time.
    Cooldown(Duration),
}

/// Start gate. A new Start needs no open clip watch and `START_COOLDOWN` since the last session end.
pub fn start_wait(now: Instant, session_ended_at: Option<Instant>, save_open: bool) -> Option<StartWait> {
    if save_open {
        return Some(StartWait::SaveOpen);
    }
    let ended = session_ended_at?;
    let elapsed = now.saturating_duration_since(ended);
    if elapsed >= START_COOLDOWN { None } else { Some(StartWait::Cooldown(START_COOLDOWN - elapsed)) }
}

pub fn wait_message(wait: StartWait) -> String {
    match wait {
        StartWait::SaveOpen => i18n::tr_ref("capture.steam_still_saves_the_previous_clip_recording").into(),
        StartWait::Cooldown(left) => i18n::tr_args("capture.new_recording_in_sec_steam_is_finishing", &[("seconds", &left.as_millis().div_ceil(1000).to_string())]),
    }
}

/// What one physical STEAM+A press does, decided from the newest read. Never more than one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressOutcome {
    /// A previous command is unconfirmed: wait for it, never send a second one.
    Queued,
    /// The previous command stayed unconfirmed too long: drop the press with a message.
    Expired,
    Stop,
    Start,
    Wait(StartWait),
    Unavailable,
}

pub fn press_outcome(press_age: Duration, pending: bool, can_stop: bool, can_start: bool, now: Instant, session_ended_at: Option<Instant>, save_open: bool) -> PressOutcome {
    if pending {
        return if press_age < QUEUE_WINDOW { PressOutcome::Queued } else { PressOutcome::Expired };
    }
    if can_stop {
        return PressOutcome::Stop;
    }
    if can_start {
        return match start_wait(now, session_ended_at, save_open) {
            Some(wait) => PressOutcome::Wait(wait),
            None => PressOutcome::Start,
        };
    }
    PressOutcome::Unavailable
}

/// True when a press arrived after the read in hand started, so that read cannot decide it.
pub fn read_is_stale(pressed_at: Option<Instant>, read_started: Option<Instant>) -> bool {
    matches!((pressed_at, read_started), (Some(at), Some(started)) if at > started)
}

/// Whether the REC indicator is shown. A pending Start shows it; an observed session shows it unless DeckShell's own Stop
/// was sent within `STOP_BADGE_HOLD` (Steam writes the clip after the session, and the cache can lag behind the Stop).
pub fn rec_badge_visible(start_pending: bool, session_active: bool, stop_sent_at: Option<Instant>, now: Instant) -> bool {
    if start_pending { return true; }
    if !session_active { return false; }
    !stop_sent_at.is_some_and(|t| now.saturating_duration_since(t) < STOP_BADGE_HOLD)
}

fn session_active(view: &steam_bridge::recording::NativeView) -> bool {
    matches!(&view.cache, steam_bridge::recording::Snapshot::Cached(c)
        if matches!(c.session, steam_bridge::recording::SessionObservation::ActiveGame(_)))
}

/// The first permanent clip that was not in the cache before the Stop, if any. Temporary clips never count.
pub fn new_permanent_clip<'a>(before: &[String], clips: &'a [ClipSummary]) -> Option<&'a ClipSummary> {
    clips.iter().find(|c| !c.temporary && !before.contains(&c.clip_id))
}

/// Steam's chord layout. DeckShell takes STEAM+A only when Steam Input no longer runs `gr_toggle` for it.
pub fn chord_layout_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/Steam/controller_base/chord_neptune.vdf")
}

/// Test-only opt-in: with the HID test socket the physical chord never reaches Steam Input, so the layout check
/// cannot prove anything there. `DECKSHELL_TEST_CHORD_OWNED=1` is honoured only together with the socket.
pub fn chord_owned_for_test() -> bool {
    test_chord_override(std::env::var_os("DECKSHELL_TEST_HID_SOCKET").is_some(), std::env::var("DECKSHELL_TEST_CHORD_OWNED").ok().as_deref())
}

fn test_chord_override(socket: bool, flag: Option<&str>) -> bool {
    socket && flag == Some("1")
}

/// True when the chord layout's `button_a` block is `empty_binding` and no longer runs `gr_toggle`.
/// Steam Input is then not a second toggle, so DeckShell's single native toggle is the only one.
pub fn chord_releases_record(vdf: &str) -> bool {
    let lines: Vec<&str> = vdf.lines().map(str::trim).collect();
    let Some(start) = lines.iter().position(|l| *l == "\"button_a\"") else { return false };
    let end = lines[start + 1..].iter().position(|l| l.starts_with("\"button_")).map_or(lines.len(), |p| start + 1 + p);
    let block = &lines[start..end];
    let bound_empty = block.iter().any(|l| l.starts_with("\"binding\"") && l.ends_with("\"controller_action empty_binding\""));
    bound_empty && !block.iter().any(|l| l.contains("gr_toggle"))
}

#[cfg(test)]
mod chord_layout_tests {
    use super::chord_releases_record;

    const STOCK: &str = "\"button_x\"\n{\n\t\"activators\"\n\t{\n\t\t\"release\"\n\t\t{\n\t\t\t\"bindings\"\n\t\t\t{\n\t\t\t\t\"binding\"\t\t\"controller_action SHOW_KEYBOARD\"\n\t\t\t}\n\t\t}\n\t}\n}\n\"button_a\"\n{\n\t\"activators\"\n\t{\n\t\t\"full_press\"\n\t\t{\n\t\t\t\"bindings\"\n\t\t\t{\n\t\t\t\t\"binding\"\t\t\"controller_action gr_toggle\"\n\t\t\t}\n\t\t}\n\t}\n}\n\"button_y\"\n{}\n";

    #[test]
    fn stock_layout_keeps_steam_input_recording() {
        assert!(!chord_releases_record(STOCK));
    }

    #[test]
    fn empty_binding_in_button_a_releases_the_chord() {
        let released = STOCK.replace("controller_action gr_toggle", "controller_action empty_binding");
        assert!(chord_releases_record(&released));
    }

    #[test]
    fn crlf_layout_is_read_the_same() {
        let released = STOCK.replace("controller_action gr_toggle", "controller_action empty_binding").replace('\n', "\r\n");
        assert!(chord_releases_record(&released));
    }

    #[test]
    fn only_button_a_decides() {
        // button_a released; a gr_toggle in another block must not hold DeckShell back
        let released_elsewhere = STOCK
            .replace("controller_action gr_toggle", "controller_action empty_binding")
            .replace("\"button_y\"\n{}", "\"button_y\"\n{\n\t\"binding\"\t\t\"controller_action gr_toggle\"\n}");
        assert!(chord_releases_record(&released_elsewhere));
        // button_a still stock; an empty binding elsewhere must not release it
        let stock_a = STOCK.replace("\"button_y\"\n{}", "\"button_y\"\n{\n\t\"binding\"\t\t\"controller_action empty_binding\"\n}");
        assert!(!chord_releases_record(&stock_a));
    }
}
thread_local! { static NATIVE: std::cell::RefCell<NativeCapture> = std::cell::RefCell::new(NativeCapture::default()); }
static EVENT_BRIDGE: std::sync::OnceLock<steam_bridge::Bridge> = std::sync::OnceLock::new();
thread_local! { static EVENT_RESET: std::cell::Cell<bool> = const { std::cell::Cell::new(true) }; }

impl App {
    pub fn poll_capture_events(&mut self) {
        let reset = EVENT_RESET.with(|r| r.get());
        let js = include_str!("capture_events.js").replace("__WAIT__", "true").replace("__RESET__", if reset { "true" } else { "false" }).replace("__IN_GAME__", if self.live.running.is_empty() { "false" } else { "true" });
        // Собственный CDP канал: ожидающий Promise не блокирует общий bridge.
        std::thread::spawn(move || {
            let value = EVENT_BRIDGE.get_or_init(steam_bridge::Bridge::lazy).eval_json(&js).unwrap_or(serde_json::Value::Null);
            super::defer(move |a| {
                let Some(events) = value.as_array() else {
                    EVENT_RESET.with(|r| r.set(true));
                    slint::Timer::single_shot(Duration::from_secs(2), || super::defer(|a| a.poll_capture_events()));
                    return;
                };
                EVENT_RESET.with(|r| r.set(false));
                for event_data in events {
                    let mut event = crate::notifications::Event::from_value(event_data);
                    match event.kind.as_str() {
                        "screenshot" => event.title = i18n::tr("capture.screenshot_taken"),
                        "achievement" if event.title.is_empty() => {
                            event.title = i18n::tr("capture.achievement_unlocked");
                        }
                        "friend" => {
                            if let Some(game) = event_data.get("game").and_then(|game| game.as_str()) {
                                event.body = i18n::tr_args("capture.friend_playing", &[("game", game)]);
                            }
                        }
                        _ => {}
                    }
                    event.resolve_screenshot(&a.base.steam_root, a.base.active_user);
                    crate::notifications::enqueue(&a.ui(), event, !a.live.running.is_empty());
                }
                a.poll_capture_events();
            });
        });
    }

    /// Physical capture chords belong to Steam Input. Only observe their result.
    pub fn observe_native_capture(&mut self) {
        self.refresh_native_recording();
        for secs in [1, 3] {
            slint::Timer::single_shot(std::time::Duration::from_secs(secs), || {
                super::defer(|a| a.refresh_native_recording());
            });
        }
    }

    /// Physical STEAM+A (task 148). Acts only when the chord layout no longer binds it to `gr_toggle`, so the
    /// native recorder gets exactly one toggle. A fresh native view decides start or stop; the press waits
    /// (queued, bounded) while a previous command is still unconfirmed.
    pub fn record_chord(&mut self) {
        let test_owned = chord_owned_for_test();
        let owned = test_owned || std::fs::read_to_string(chord_layout_path()).map(|t| chord_releases_record(&t)).unwrap_or(false);
        let (busy, bridge) = NATIVE.with(|n| (n.borrow().sending, crate::actions::bridge().is_some()));
        let dryrun = crate::actions::dryrun();
        if !owned || busy || !bridge || dryrun {
            crate::diag::log("record_chord", serde_json::json!({"owned": owned, "test_owned": test_owned, "busy": busy, "bridge": bridge, "dryrun": dryrun, "dispatch": "observe_only"}));
            self.observe_native_capture();
            return;
        }
        NATIVE.with(|n| n.borrow_mut().record_chord_at = Some(Instant::now()));
        self.refresh_native_recording();
    }

    /// Second half of `record_chord`, run once a read that started after the press has arrived. Stop wins when a session is observed.
    fn take_record_chord(&mut self) {
        enum Step { Idle, Reread, Act(PressOutcome) }
        let step = NATIVE.with(|n| {
            let n = n.borrow();
            let Some(at) = n.record_chord_at else { return Step::Idle };
            if read_is_stale(Some(at), n.read_started_at) { return Step::Reread; }
            let (can_stop, can_start) = match n.view.as_ref() {
                Some(v) => (v.stop_game().is_some(), v.start_game().is_some()),
                None => (false, false),
            };
            Step::Act(press_outcome(at.elapsed(), n.pending.is_some(), can_stop, can_start,
                Instant::now(), n.session_ended_at, n.clip_watch.is_some()))
        });
        match step {
            Step::Idle => {}
            Step::Reread => self.refresh_native_recording(),
            Step::Act(PressOutcome::Queued) => self.schedule_pending_poll(),
            Step::Act(outcome) => {
                NATIVE.with(|n| n.borrow_mut().record_chord_at = None);
                match outcome {
                    PressOutcome::Expired => {
                        crate::diag::log("record_chord", serde_json::json!({"action": "expired", "reason": "previous command unconfirmed"}));
                        self.toast(i18n::tr_ref("capture.steam_has_not_yet_confirmed_the_previous"));
                    }
                    PressOutcome::Stop | PressOutcome::Start => {
                        let start = outcome == PressOutcome::Start;
                        crate::diag::log("record_chord", serde_json::json!({"action": if start { "start" } else { "stop" }, "dispatch": "once"}));
                        self.native_recording_action(start);
                    }
                    PressOutcome::Wait(wait) => {
                        crate::diag::log("record_gate", serde_json::json!({"action": "start_refused", "reason": match wait {
                            StartWait::SaveOpen => "save_open".to_string(),
                            StartWait::Cooldown(left) => format!("cooldown_left_ms={}", left.as_millis()),
                        }}));
                        self.toast(&wait_message(wait));
                        self.schedule_gate_poll();
                    }
                    PressOutcome::Unavailable => {
                        crate::diag::log("record_chord", serde_json::json!({"action": "none", "reason": "no manual mode or game"}));
                        self.toast(i18n::tr_ref("capture.recording_not_available_you_need_manual_steam"));
                    }
                    PressOutcome::Queued => {}
                }
            }
        }
    }

    /// While a Start waits for the previous clip or the cooldown, re-read every `CLIP_WATCH_STEP`
    /// so the Start control enables itself at the right time.
    fn schedule_gate_poll(&mut self) {
        let want = NATIVE.with(|n| {
            let mut n = n.borrow_mut();
            let waiting = start_wait(Instant::now(), n.session_ended_at, n.clip_watch.is_some()).is_some();
            if waiting && !n.gate_scheduled { n.gate_scheduled = true; true } else { false }
        });
        if !want { return; }
        slint::Timer::single_shot(CLIP_WATCH_STEP, || super::defer(|a| {
            NATIVE.with(|n| n.borrow_mut().gate_scheduled = false);
            a.refresh_native_recording();
        }));
    }

    /// While a Start or Stop awaits Steam's confirmation, re-read about once a second, for a bounded time.
    /// Without this a session that started quietly would stay unconfirmed until the next read.
    fn schedule_pending_poll(&mut self) {
        let want = NATIVE.with(|n| {
            let mut n = n.borrow_mut();
            let live = n.pending.is_some() && n.pending_since.is_some_and(|t| t.elapsed() < PENDING_WINDOW);
            if live && !n.pending_scheduled { n.pending_scheduled = true; true } else { false }
        });
        if !want { return; }
        slint::Timer::single_shot(PENDING_STEP, || super::defer(|a| {
            NATIVE.with(|n| n.borrow_mut().pending_scheduled = false);
            a.refresh_native_recording();
        }));
    }

    pub fn native_recording_controls(&self) -> (bool, bool) {
        NATIVE.with(|n| {
            let n = n.borrow();
            let enabled = !n.sending && n.pending.is_none() && !crate::actions::dryrun();
            let gate_open = start_wait(Instant::now(), n.session_ended_at, n.clip_watch.is_some()).is_none();
            (enabled && gate_open && n.view.as_ref().and_then(|v| v.start_game()).is_some(),
             enabled && n.view.as_ref().and_then(|v| v.stop_game()).is_some())
        })
    }

    /// Present a short event over a game using the existing non-input external overlay.
    pub fn capture_notification(&mut self, active: bool) {
        crate::notifications::enqueue(&self.ui(), crate::notifications::Event::recording(active), !self.live.running.is_empty());
    }

    pub fn push_native_recording(&self) {
        // Recording state remains available to controls; persistent REC has been replaced by events.
    }

    pub fn refresh_native_recording(&mut self) {
        if NATIVE.with(|n| { let mut n=n.borrow_mut(); if n.reading || n.sending { true } else { n.reading=true; n.read_started_at=Some(Instant::now()); false } }) { return; }
        let Some(bridge) = crate::actions::bridge() else {
            NATIVE.with(|n| { let mut n=n.borrow_mut(); n.reading=false; n.view=None; n.notification_baseline=None; n.note=i18n::tr_ref("capture.steam_transport_is_not_available").into(); });
            self.push_native_recording(); self.schedule_native_recording_poll(); return;
        };
        let spawned = std::thread::Builder::new().name("native-recording-read".into()).spawn(move || {
            let result = bridge.native_recording_view();
            super::defer(move |a| {
                NATIVE.with(|n| n.borrow_mut().reading=false);
                match result {
                    Ok(view) => {
                        crate::diag::log("native_recording.observation", serde_json::json!({
                            "mode":view.mode, "cached_clips":view.clips.len(),
                            "active_session_observed":matches!(&view.cache, steam_bridge::recording::Snapshot::Cached(c) if matches!(c.session, steam_bridge::recording::SessionObservation::ActiveGame(_)))
                        }));
                        // Cache data is explicitly incomplete; never replace an error with []
                        // or infer a filesystem path from native clip IDs/thumbnail URLs.
                        a.media_merge_native_clips(&view.clips);
                        NATIVE.with(|n| {
                            let mut n=n.borrow_mut();
                            if let Some(p) = &n.pending {
                                use steam_bridge::recording::Confirmation;
                                match p.observe(&view) {
                                    Confirmation::SessionObserved => { n.pending=None; n.note=i18n::tr_ref("capture.steam_session_observed_after_launch_command").into(); },
                                    Confirmation::SessionEndObserved { additional_cached_clips } => { n.pending=None; n.note=i18n::tr_args("capture.the_recording_session_has_ended_there_are", &[("additional_cached_clips", &(additional_cached_clips).to_string())]); },
                                    Confirmation::Waiting => {},
                                }
                            }
                            // A session end seen by a read (ours or Steam Input's stock Stop) starts the Start gate
                            // and, unless a watch is already open, watches the cache for the clip it writes.
                            let now_active = session_active(&view);
                            if matches!(&view.cache, steam_bridge::recording::Snapshot::Cached(_)) {
                                let previous = n.notification_baseline.replace(now_active);
                                if previous.is_some_and(|previous| previous != now_active) {
                                    super::defer(move |a| a.capture_notification(now_active));
                                }
                            } else {
                                n.notification_baseline = None;
                            }
                            if n.last_active_seen && !now_active {
                                n.stop_sent_at = None;
                                n.session_ended_at = Some(Instant::now());
                                if n.clip_watch.is_none() {
                                    let before = n.view.as_ref().unwrap_or(&view).clips.iter().map(|c| c.clip_id.clone()).collect();
                                    n.clip_watch = Some(ClipWatch { before, deadline: Instant::now() + CLIP_WATCH_WINDOW });
                                }
                                crate::diag::log("record_gate", serde_json::json!({"action": "session_ended", "watch": n.clip_watch.is_some()}));
                            }
                            n.last_active_seen = now_active;
                            if n.view.as_ref() != Some(&view) {
                                crate::diag::log("native_recording.state", serde_json::json!({
                                    "mode":view.mode,"cached_clips":view.clips.len(),
                                    "active_session_observed":matches!(&view.cache, steam_bridge::recording::Snapshot::Cached(c) if matches!(c.session, steam_bridge::recording::SessionObservation::ActiveGame(_))),
                                    "command_pending":n.pending.is_some()
                                }));
                            }
                            n.view=Some(view);
                        });
                    },
                    Err(_) => {
                        crate::diag::log("native_recording.observation_error", serde_json::json!({"source":"bridge"}));
                        NATIVE.with(|n| { let mut n=n.borrow_mut(); n.view=None; n.notification_baseline=None; n.record_chord_at=None; n.note=i18n::tr_ref("capture.steam_reading_error_the_list_may_not").into(); });
                    },
                }
                a.check_clip_watch();
                a.schedule_pending_poll();
                a.schedule_gate_poll();
                a.push_native_recording();
                a.schedule_native_recording_poll();
                a.take_record_chord();
            });
        });
        if spawned.is_err() {
            NATIVE.with(|n| { let mut n=n.borrow_mut(); n.reading=false; n.view=None; n.notification_baseline=None; n.record_chord_at=None; n.note=i18n::tr_ref("capture.failed_to_start_reading_the_condition").into(); });
            self.push_native_recording();
        }
    }

    /// Poll on every route, including before the first externally started recording.
    /// Rearm before reading so an in-flight request cannot break the polling chain.
    pub fn schedule_native_recording_poll(&mut self) {
        if NATIVE.with(|n| { let mut n=n.borrow_mut(); if n.poll_scheduled { true } else { n.poll_scheduled=true; false } }) { return; }
        slint::Timer::single_shot(RECORDING_POLL_STEP, || super::defer(|a| {
            NATIVE.with(|n| n.borrow_mut().poll_scheduled=false);
            a.schedule_native_recording_poll();
            a.refresh_native_recording();
        }));
    }

    /// After a Stop: announce the first new permanent clip, or give up after `CLIP_WATCH_WINDOW`.
    /// While waiting, re-read the cache every `CLIP_WATCH_STEP` (independent of the Media route).
    fn check_clip_watch(&mut self) {
        let outcome = NATIVE.with(|n| {
            let mut n = n.borrow_mut();
            let Some(watch) = n.clip_watch.clone() else { return WatchOutcome::Idle };
            let found = n.view.as_ref().and_then(|v| new_permanent_clip(&watch.before, &v.clips)).map(|c| c.duration_ms);
            if let Some(ms) = found {
                n.clip_watch = None;
                WatchOutcome::Found(ms)
            } else if Instant::now() >= watch.deadline {
                n.clip_watch = None;
                n.note = i18n::tr_ref("capture.the_clip_didn_t_appear_in_steam").into();
                WatchOutcome::Expired
            } else {
                WatchOutcome::Waiting
            }
        });
        match outcome {
            WatchOutcome::Idle => {}
            WatchOutcome::Found(ms) => {
                crate::diag::log("clip_watch", serde_json::json!({"result": "found", "duration_ms": ms}));
                self.toast(&i18n::tr_args("capture.clip_saved_sec", &[("ms", &(ms / 1000).to_string())]));
            }
            WatchOutcome::Expired => {
                crate::diag::log("clip_watch", serde_json::json!({"result": "expired"}));
            }
            WatchOutcome::Waiting => {
                let scheduled = NATIVE.with(|n| {
                    let mut n = n.borrow_mut();
                    if n.clip_watch_scheduled { true } else { n.clip_watch_scheduled = true; false }
                });
                if !scheduled {
                    slint::Timer::single_shot(CLIP_WATCH_STEP, || super::defer(|a| {
                        NATIVE.with(|n| n.borrow_mut().clip_watch_scheduled = false);
                        a.refresh_native_recording();
                    }));
                }
            }
        }
    }

    pub fn load_native_clips(&mut self) {
        if crate::actions::dryrun() { self.refresh_native_recording(); return; }
        // A distinct explicit menu action requests the existing native lazy cache once.
        // Its swallowed error/empty result is never labelled an authoritative list.
        if NATIVE.with(|n| n.borrow().pending.is_some()) { self.refresh_native_recording(); return; }
        let Some(bridge)=crate::actions::bridge() else { self.refresh_native_recording(); return; };
        if NATIVE.with(|n| { let mut n=n.borrow_mut(); if n.reading || n.sending { true } else { n.reading=true; n.read_started_at=Some(Instant::now()); false } }) { return; }
        let spawned=std::thread::Builder::new().name("native-clips-request".into()).spawn(move || {
            let result=bridge.request_native_clip_cache();
            super::defer(move |a| {
                NATIVE.with(|n| { let mut n=n.borrow_mut(); n.reading=false;
                    n.note=if result.is_ok() { i18n::tr_ref("capture.shown_famous_steam_clips_cache_may_be") } else { i18n::tr_ref("capture.cache_request_not_confirmed_empty_list_not") }.into(); });
                a.refresh_native_recording();
            });
        });
        if spawned.is_err() { NATIVE.with(|n| n.borrow_mut().reading=false); }
    }

    pub fn native_recording_can_acknowledge(&self) -> bool {
        NATIVE.with(|n| { let n=n.borrow(); n.pending.is_some() && !n.sending && !n.reading })
    }

    pub fn native_recording_acknowledge_unknown(&mut self) {
        // Local UI acknowledgement only: does not reset any Steam flag, cancel
        // a native call, resend it, or establish Idle/success.
        NATIVE.with(|n| {
            let mut n=n.borrow_mut();
            if n.sending || n.reading { return; }
            n.pending=None; n.view=None;
            n.note=i18n::tr_ref("capture.unknown_outcome_is_accepted_by_the_user").into();
        });
        self.refresh_native_recording();
    }

    /// Only explicit UI Start/Stop invokes a native mutation, exactly once.
    pub fn native_recording_action(&mut self, start: bool) {
        use steam_bridge::recording::{CommandReply, NativeCommand, PendingCommand};
        if crate::actions::dryrun() { return; }
        let Some(bridge)=crate::actions::bridge() else { self.refresh_native_recording(); return; };
        // Start gate: one Start per finished recording, and only after the previous clip is written.
        if start {
            let wait = NATIVE.with(|n| { let n = n.borrow(); start_wait(Instant::now(), n.session_ended_at, n.clip_watch.is_some()) });
            if let Some(wait) = wait {
                crate::diag::log("record_gate", serde_json::json!({"action": "start_refused", "via": "action"}));
                self.toast(&wait_message(wait));
                self.refresh_native_recording();
                return;
            }
        }
        let command=NATIVE.with(|n| {
            let mut n=n.borrow_mut();
            if n.sending || n.reading || n.pending.is_some() { return None; }
            let view=n.view.as_ref()?;
            let game=if start { view.start_game()? } else { view.stop_game()? };
            let command=if start { NativeCommand::Start(game) } else { NativeCommand::Stop(game) };
            let before: Vec<String> = view.clips.iter().map(|c| c.clip_id.clone()).collect();
            n.pending=Some(PendingCommand::new(command.clone(),view)); n.pending_since=Some(Instant::now()); n.sending=true;
            n.stop_sent_at = if start { None } else { Some(Instant::now()) };
            if !start {
                // The gate counts from the Stop we send, not from the later read that sees the session end.
                n.session_ended_at=Some(Instant::now());
                n.clip_watch=Some(ClipWatch { before, deadline: Instant::now() + CLIP_WATCH_WINDOW });
            }
            n.note=i18n::tr_ref("capture.the_command_is_sent_once_confirmation_of").into();
            Some(command)
        });
        let Some(command)=command else { self.refresh_native_recording(); return; };
        crate::diag::log("native_recording.command", serde_json::json!({"action":if start {"start"} else {"stop"},"dispatch":"once"}));
        self.push_native_recording();
        let spawned=std::thread::Builder::new().name("native-recording-command".into()).spawn(move || {
            let reply=bridge.native_recording_command(&command);
            let (outcome,code) = match &reply {
                Ok(CommandReply::StartAccepted) => ("start_accepted",None),
                Ok(CommandReply::StopDispatched) => ("stop_dispatched",None),
                Ok(CommandReply::Rejected(code)) => ("rejected",Some(*code)),
                Ok(CommandReply::Unavailable) => ("unavailable",None),
                Err(_) => ("unknown",None),
            };
            crate::diag::log("native_recording.reply", serde_json::json!({"outcome":outcome,"eresult":code}));
            super::defer(move |a| {
                NATIVE.with(|n| {
                    let mut n=n.borrow_mut(); n.sending=false;
                    if !start && !matches!(reply, Ok(CommandReply::StopDispatched)) { n.stop_sent_at=None; }
                    match reply {
                        Ok(CommandReply::Rejected(code)) => { n.pending=None; n.note=i18n::tr_args("capture.steam_rejected_the_launch_eresult", &[("code", &(code).to_string())]); },
                        Ok(CommandReply::Unavailable) => { n.pending=None; n.note=i18n::tr_ref("capture.steam_s_terms_or_features_have_changed").into(); },
                        Ok(CommandReply::StartAccepted) => n.note=i18n::tr_ref("capture.steam_accepts_launch_session_observation_expected").into(),
                        Ok(CommandReply::StopDispatched) => n.note=i18n::tr_ref("capture.the_stop_is_called_the_end_of").into(),
                        Err(_) => n.note=i18n::tr_ref("capture.the_outcome_of_the_command_is_unknown").into(),
                    }
                });
                a.refresh_native_recording();
                a.push_native_recording();
            });
        });
        if spawned.is_err() {
            NATIVE.with(|n| { let mut n=n.borrow_mut(); n.sending=false; n.pending=None; n.note=i18n::tr_ref("capture.command_not_sent_stream_not_created").into(); });
            self.push_native_recording();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rec_indicator_follows_start_and_observed_session() {
        let t0 = Instant::now();
        assert!(rec_badge_visible(true, false, None, t0));
        assert!(!rec_badge_visible(false, false, None, t0));
        assert!(rec_badge_visible(false, true, None, t0));
    }

    #[test]
    fn rec_indicator_hides_at_once_after_our_stop_even_if_cache_is_stale() {
        let t0 = Instant::now();
        // Stop sent 1 s ago; the cached snapshot still says the session is active.
        assert!(!rec_badge_visible(false, true, Some(t0), t0 + Duration::from_secs(1)));
        // The hold has passed and Steam still reads a session: the observation decides again.
        assert!(rec_badge_visible(false, true, Some(t0), t0 + STOP_BADGE_HOLD + Duration::from_secs(1)));
        // Without an active session the indicator is off whatever the stop time.
        assert!(!rec_badge_visible(false, false, Some(t0), t0 + Duration::from_secs(40)));
    }

    #[test]
    fn names_start_at_one_and_increment_on_clash() {
        let dir = Path::new("/u/760/remote/7");
        let (a, t, n) = shot_names(dir, "20261108120000", |_| false);
        assert_eq!(n, 1);
        assert_eq!(a, dir.join("screenshots/20261108120000_1.jpg"));
        assert_eq!(t, dir.join("screenshots/thumbnails/20261108120000_1.jpg"));
        let taken = [dir.join("screenshots/20261108120000_1.jpg"), dir.join("screenshots/20261108120000_2.jpg")];
        let (_, _, n) = shot_names(dir, "20261108120000", |p| taken.iter().any(|x| x == p));
        assert_eq!(n, 3);
    }

    #[test]
    fn convert_makes_200px_thumb() {
        let a = convert_args(Path::new("/tmp/a.png"), Path::new("/o/x.jpg"), Path::new("/o/t/x.jpg"));
        assert!(a.iter().any(|s| s == "scale=200:-1"));
        assert_eq!(a.last().unwrap(), "/o/t/x.jpg");
    }
}

#[cfg(test)]
mod clip_watch_tests {
    use super::*;

    fn clip(id: &str, temporary: bool) -> ClipSummary {
        let value: serde_json::Value = serde_json::from_str(include_str!("../../../crates/steam-bridge/tests/fixtures/native-clip-sanitized.json")).unwrap();
        let mut c = steam_bridge::recording::parse_clip_summary(&value).unwrap();
        c.clip_id = id.into();
        c.temporary = temporary;
        c
    }

    #[test]
    fn only_a_new_permanent_clip_is_announced() {
        let before = vec!["old".to_string()];
        assert!(new_permanent_clip(&before, &[clip("old", false)]).is_none());
        assert!(new_permanent_clip(&before, &[clip("old", false), clip("new", true)]).is_none());
        let clips = [clip("old", false), clip("new", false)];
        let found = new_permanent_clip(&before, &clips).unwrap();
        assert_eq!(found.clip_id, "new");
    }

    #[test]
    fn empty_before_list_accepts_first_permanent_clip() {
        assert!(new_permanent_clip(&[], &[]).is_none());
        assert!(new_permanent_clip(&[], &[clip("a", false)]).is_some());
    }

    #[test]
    fn test_override_needs_socket_and_explicit_flag() {
        assert!(test_chord_override(true, Some("1")));
        assert!(!test_chord_override(false, Some("1")));
        assert!(!test_chord_override(true, None));
        assert!(!test_chord_override(true, Some("yes")));
    }

    #[test]
    fn press_during_a_read_waits_for_the_next_read() {
        let started = Instant::now();
        let pressed = started + Duration::from_millis(5);
        assert!(read_is_stale(Some(pressed), Some(started)));
        assert!(!read_is_stale(Some(started), Some(pressed)));
        assert!(!read_is_stale(None, Some(started)));
    }
}

#[cfg(test)]
#[path = "capture_record_gate_tests.rs"]
mod record_gate_tests;
