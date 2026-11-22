//! Idle timers of Настройки → Питание: dim, screensaver, sleep (task 518).
//!
//! Who applies them. In the stock UI session the Steam client counts the idle time and tells the UI (Screensaver
//! notification, dim through the system settings, `SuspendPC`). In DeckShell's hybrid session Steam runs headless
//! (`-silent`, no Gamepad UI) and DeckShell owns the screen and the input, so nothing counted the idle time:
//! the settings were stored, read back and shown, but never acted on. This module is that counter:
//!
//! * activity = any Deck pad report change (`input::spawn_hidraw`), an external pad event, a keyboard / pointer / touch
//!   window event (winit hook), any wake of the screensaver. A game on screen, a covered shell or a launch cover counts
//!   as constant activity: no dim / screensaver / sleep happens under a game (stock sleeps there too; we stay conservative);
//! * the timers are read from Steam (`idle_settings.js`: the same values the page shows) every 15 s and right after the
//!   page writes one; the set for AC or battery is picked from the battery state;
//! * dim = the Steam brightness scale ×0.25 through `display` (restored on activity unless the user changed it meanwhile);
//!   screensaver = black cover (`IdleCover`), the first input only wakes it; sleep = the same `Act::Power(Sleep)` as the
//!   power menu (suspend animation, then logind), after which the idle clock restarts.
//!
//! `Machine` is the pure decision, `timers_from` the pure settings parser; both are unit-tested without a display.
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use slint::{ComponentHandle, Timer, TimerMode};

use crate::app::App;
use crate::{AppWindow, IdleState};

/// Timers in seconds for one power source; 0 = «Режим отключён».
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timers {
    pub dim: u64,
    pub saver: u64,
    pub sleep: u64,
}

/// Both sets as stored by Steam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub battery: Timers,
    pub ac: Timers,
}

impl Settings {
    pub fn for_power(&self, ac: bool) -> Timers {
        if ac { self.ac } else { self.battery }
    }
}

fn secs(v: &Value) -> Option<u64> {
    let f = v.as_f64()?;
    (f.is_finite() && f >= 0.0).then(|| f.round() as u64)
}

/// `snapshot` has the shape of the power page snapshot (`c.<client key>`, `power.dim_*`). `None` while nothing was read
/// (Steam not ready): the previous settings stay in force. A single unread value counts as disabled.
pub fn timers_from(snapshot: &Value) -> Option<Settings> {
    let c = &snapshot["c"];
    let p = &snapshot["power"];
    let all = [
        secs(&c["system_idle_suspend_battery_sec"]), secs(&c["system_idle_suspend_ac_sec"]),
        secs(&c["system_idle_screensaver_battery_sec"]), secs(&c["system_idle_screensaver_ac_sec"]),
        secs(&p["dim_battery"]), secs(&p["dim_ac"]),
    ];
    if all.iter().all(Option::is_none) {
        return None;
    }
    let g = |i: usize| all[i].unwrap_or(0);
    Some(Settings {
        battery: Timers { sleep: g(0), saver: g(2), dim: g(4) },
        ac: Timers { sleep: g(1), saver: g(3), dim: g(5) },
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Dim,
    Undim,
    Saver,
    Unsaver,
    Sleep,
}

/// What is currently applied: the decision is a function of (idle seconds, timers, this).
#[derive(Debug, Default)]
pub struct Machine {
    dimmed: bool,
    saver: bool,
    slept: bool,
}

impl Machine {
    pub fn dimmed(&self) -> bool { self.dimmed }
    pub fn saver(&self) -> bool { self.saver }

    /// One tick. Commands come in the order Dim, Saver, Sleep; each timer fires once per idle period, and activity
    /// (idle below the threshold again) takes dim and screensaver back. Sleep is issued once; the caller restarts the idle clock.
    pub fn step(&mut self, idle: u64, t: Timers) -> Vec<Cmd> {
        let mut out = Vec::new();
        let hit = |limit: u64| limit > 0 && idle >= limit;
        let (dim, saver, sleep) = (hit(t.dim), hit(t.saver), hit(t.sleep));
        if sleep != self.slept {
            self.slept = sleep;
            if sleep { out.push(Cmd::Sleep); }
        }
        if dim != self.dimmed {
            self.dimmed = dim;
            out.insert(0, if dim { Cmd::Dim } else { Cmd::Undim });
        }
        if saver != self.saver {
            self.saver = saver;
            let at = out.iter().position(|c| *c == Cmd::Sleep).unwrap_or(out.len());
            out.insert(at, if saver { Cmd::Saver } else { Cmd::Unsaver });
        }
        out
    }
}

/// Dimmed brightness for a stored scale position: a quarter, never below 0.02, never above the current position.
pub fn dim_level(saved: f32) -> f32 {
    (saved * 0.25).max(0.02).min(saved).max(crate::display::FLOOR)
}

/// True when the wall clock ran ahead of the monotonic clock: the machine slept (the monotonic clock stops in suspend).
pub fn resumed(wall: Duration, mono: Duration) -> bool {
    wall > mono + Duration::from_secs(5)
}

// ------------------------------------------------------------------------------------------------ activity

fn now_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

static LAST_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
static SAVER_SHOWN: AtomicBool = AtomicBool::new(false);

/// Any user activity (any thread). Cheap: one atomic store.
pub fn touch() {
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::Relaxed);
}

/// Seconds since the last activity.
pub fn idle_secs() -> u64 {
    now_ms().saturating_sub(LAST_ACTIVITY_MS.load(Ordering::Relaxed).max(1)) / 1000
}

/// Input event arrived (any thread): counts as activity. True while the screensaver is up: the event only wakes it
/// (the tick takes the cover down) and must not act on the screen underneath.
pub fn swallow() -> bool {
    touch();
    SAVER_SHOWN.load(Ordering::Relaxed)
}

// ------------------------------------------------------------------------------------------------ settings poll

static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);
static REFRESH: AtomicBool = AtomicBool::new(false);
const POLL: Duration = Duration::from_secs(15);

fn current() -> Option<Settings> {
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The power page wrote an idle value: read the settings again shortly (Steam applies the write asynchronously).
pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

fn fetch() {
    let Some(b) = crate::actions::bridge() else { return };
    match b.eval_json(include_str!("idle_settings.js")) {
        Ok(v) => match timers_from(&v) {
            Some(s) => {
                let mut cur = SETTINGS.lock().unwrap_or_else(|e| e.into_inner());
                if *cur != Some(s) {
                    eprintln!("[deckshell] idle settings: battery {:?} ac {:?}", s.battery, s.ac);
                    *cur = Some(s);
                }
            }
            None => eprintln!("[deckshell] idle settings: Steam returned no values, keeping the previous ones"),
        },
        Err(e) => eprintln!("[deckshell] idle settings: {e}"),
    }
}

fn spawn_poller() {
    let _ = std::thread::Builder::new().name("idle-settings".into()).spawn(|| {
        let mut last = Instant::now().checked_sub(POLL).unwrap_or_else(Instant::now);
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if REFRESH.swap(false, Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(1500));
                fetch();
                last = Instant::now();
            } else if last.elapsed() >= POLL {
                fetch();
                last = Instant::now();
            }
        }
    });
}

// ------------------------------------------------------------------------------------------------ brightness

static DIM_ORDER: Mutex<Option<f32>> = Mutex::new(None);

/// Worker thread (Steam calls block): remember the brightness and lower it.
fn dim_screen() {
    if crate::actions::dryrun() {
        eprintln!("[deckshell] DRYRUN idle: dim");
        return;
    }
    let _ = std::thread::Builder::new().name("idle-dim".into()).spawn(|| {
        let mut saved = DIM_ORDER.lock().unwrap_or_else(|e| e.into_inner());
        let Some(now) = crate::display::read() else { eprintln!("[deckshell] idle: dim skipped, brightness unknown"); return };
        let to = dim_level(now);
        match crate::display::write(to) {
            Ok(()) => { *saved = Some(now); eprintln!("[deckshell] idle: dim {now:.3} -> {to:.3}"); }
            Err(e) => eprintln!("[deckshell] idle: dim failed: {e}"),
        }
    });
}

/// Worker thread: put the brightness back unless the user moved the slider while it was dimmed.
fn undim_screen() {
    if crate::actions::dryrun() {
        eprintln!("[deckshell] DRYRUN idle: undim");
        return;
    }
    let _ = std::thread::Builder::new().name("idle-undim".into()).spawn(|| {
        let mut saved = DIM_ORDER.lock().unwrap_or_else(|e| e.into_inner());
        let Some(was) = saved.take() else { return };
        let dimmed = dim_level(was);
        match crate::display::read() {
            Some(cur) if (cur - dimmed).abs() > 0.01 => eprintln!("[deckshell] idle: brightness changed while dimmed, kept"),
            _ => match crate::display::write(was) {
                Ok(()) => eprintln!("[deckshell] idle: brightness restored {was:.3}"),
                Err(e) => eprintln!("[deckshell] idle: restore failed: {e}"),
            },
        }
    });
}

// ------------------------------------------------------------------------------------------------ UI-thread glue

struct Glue {
    ui: Option<slint::Weak<AppWindow>>,
    timer: Timer,
    machine: Machine,
    ac: bool,
    ac_checked: Option<Instant>,
    clock: Option<(Instant, SystemTime)>,
}

thread_local! {
    static GLUE: RefCell<Glue> = RefCell::new(Glue { ui: None, timer: Timer::default(), machine: Machine::default(), ac: true, ac_checked: None, clock: None });
}

/// UI thread, once at start-up: the 250 ms tick, the settings poll and the window-event hook (keyboard / pointer / touch).
pub fn init(ui: &AppWindow) {
    touch();
    use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
    ui.window().on_winit_window_event(|_, ev| {
        let input = matches!(ev, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
            | WindowEvent::Touch(_) | WindowEvent::CursorMoved { .. });
        if input && swallow() { EventResult::PreventDefault } else { EventResult::Propagate }
    });
    GLUE.with(|g| {
        let mut g = g.borrow_mut();
        g.ui = Some(ui.as_weak());
        g.timer.start(TimerMode::Repeated, Duration::from_millis(250), || { crate::with_app(|a| a.idle_tick()); });
    });
    spawn_poller();
}

impl App {
    /// One tick of the idle machine (UI thread).
    pub fn idle_tick(&mut self) {
        // A game, the store or a launch cover owns the screen, or a power animation plays: constant activity.
        let busy = !self.live.running.is_empty() || self.shell_hidden() || crate::launch_splash::active();
        let (resume, ac, ui) = GLUE.with(|g| {
            let mut g = g.borrow_mut();
            let (now, wall) = (Instant::now(), SystemTime::now());
            let resume = g.clock.is_some_and(|(m, w)| resumed(wall.duration_since(w).unwrap_or_default(), now.duration_since(m)));
            g.clock = Some((now, wall));
            if g.ac_checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                g.ac_checked = Some(now);
                g.ac = self.ds.battery().and_then(|b| b.online).unwrap_or(true);
            }
            (resume, g.ac, g.ui.as_ref().and_then(|w| w.upgrade()))
        });
        if busy || resume {
            touch();
        }
        let Some(settings) = current() else { return };
        let cmds = GLUE.with(|g| g.borrow_mut().machine.step(idle_secs(), settings.for_power(ac)));
        for c in cmds {
            eprintln!("[deckshell] idle: {c:?} (idle {} s, {})", idle_secs(), if ac { "ac" } else { "battery" });
            match c {
                Cmd::Dim => dim_screen(),
                Cmd::Undim => undim_screen(),
                Cmd::Saver | Cmd::Unsaver => {
                    SAVER_SHOWN.store(c == Cmd::Saver, Ordering::Relaxed);
                    if let Some(ui) = &ui { ui.global::<IdleState>().set_saver(c == Cmd::Saver); }
                }
                Cmd::Sleep => {
                    self.act(crate::actions::Act::Power(crate::data::PowerAction::Sleep));
                    touch(); // the idle period restarts at the resume, not at the next tick
                }
            }
        }
        if SAVER_SHOWN.load(Ordering::Relaxed) {
            if let Some(ui) = &ui {
                let text = chrono::Local::now().format("%H:%M").to_string();
                if ui.global::<IdleState>().get_clock() != text.as_str() { ui.global::<IdleState>().set_clock(text.into()); }
            }
        }
    }
}

#[cfg(test)]
mod tests;
