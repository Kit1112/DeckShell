//! Native replacement for the Decky plugin SDH-AnimationChanger (context/archive/docs/animation-changer.md).
//!
//! * `config`  - import of the plugin's settings + randomize semantics (port of main.py)
//! * `cache`   - ffmpeg -> lz4 frame cache (+ PCM audio), built ahead of time in the background
//! * `player`  - one video + one audio stream, A/V clock, Slint overlay (`AnimState`)
//! * this file - triggers: boot at start, suspend / shutdown / restart, throbber variant, logind delay inhibitor, control commands
//!
//! Power flow:
//!  * started from DeckShell (power menu): the animation plays FIRST, then the original action runs (`actions::run`);
//!  * started from outside (power button, Steam, `systemctl`): we hold a logind *delay* inhibitor (`shutdown:sleep`); on
//!    `PrepareForSleep/Shutdown(true)` the animation plays, trimmed to `InhibitDelayMaxUSec` minus a safety margin, and the
//!    lock is released when it ends - so the machine can no longer suspend / power off in the middle of it.
use crate::actions::dryrun;
use crate::data::PowerAction;
use crate::AppWindow;
use deck_system::logind::{self, Inhibitor, PrepareEvent, Watcher};
use player::{End, Outcome, PlayOpts};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub mod cache;
pub mod config;
pub mod player;
pub mod repo;

use config::{Config, ShuffleSlot};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Boot,
    Suspend,
    Shutdown,
    Throbber,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Boot => "boot",
            Kind::Suspend => "suspend",
            Kind::Shutdown => "shutdown",
            Kind::Throbber => "throbber",
        }
    }
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "boot" => Kind::Boot,
            "suspend" | "sleep" => Kind::Suspend,
            "shutdown" | "restart" | "poweroff" | "reboot" => Kind::Shutdown,
            "throbber" => Kind::Throbber,
            _ => return None,
        })
    }
}

/// Safety margin inside logind's delay window (animation start latency + final audio fade + signal latency).
const DELAY_SAFETY: Duration = Duration::from_millis(900);
const HOLD_FAILSAFE_SLEEP: Duration = Duration::from_secs(8);
const HOLD_FAILSAFE_SHUTDOWN: Duration = Duration::from_secs(25);

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOADING: AtomicBool = AtomicBool::new(false);
static DELAY_MAX_MS: AtomicU64 = AtomicU64::new(5000);
static INHIBITOR: Mutex<Option<Inhibitor>> = Mutex::new(None);
static WATCHER: Mutex<Option<Watcher>> = Mutex::new(None);
static LAST: Mutex<String> = Mutex::new(String::new());

struct Mgr {
    cfg: Config,
    /// boot animation id for THIS start (the plugin's pick of the previous load; `cfg.boot` is already the next one)
    boot_now: String,
    /// a DeckShell-initiated power action was handed to the system (kind, when)
    handed_over: Option<(Kind, Instant)>,
    prep: Sender<Vec<PathBuf>>,
}

thread_local! {
    static MGR: RefCell<Option<Mgr>> = const { RefCell::new(None) };
    // Present only while a DeckShell-initiated power animation is pending.
    static INTERNAL_POWER: RefCell<Option<std::rc::Rc<PowerLifecycle>>> = const { RefCell::new(None) };
}

fn with_mgr<R>(f: impl FnOnce(&mut Mgr) -> R) -> Option<R> {
    MGR.with(|m| match m.try_borrow_mut() {
        Ok(mut b) => b.as_mut().map(f),
        Err(_) => None,
    })
}

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| v != "0" && !v.is_empty())
}

fn session_mode() -> bool {
    std::env::var_os("DECKSHELL_SESSION_MODE").is_some() || std::env::var_os("DECKSHELL_GAMESCOPE").is_some()
}

fn note(s: String) {
    eprintln!("[anim] {s}");
    *LAST.lock().unwrap() = s;
}

fn decode_opts(cfg: &Config, background: bool) -> cache::Opts {
    cache::Opts { maxw: cfg.x.max_width, maxh: cfg.x.max_height, background }
}

// ---------------------------------------------------------------- init

/// Called once from main() after the window exists and before the event loop runs. Covers the window with the boot
/// animation immediately (no flash of the UI), prepares the frame caches in the background, sets up the logind hooks.
pub fn init(ui: &AppWindow) {
    if env_flag("DECKSHELL_ANIM") == Some(false) {
        eprintln!("[anim] disabled (DECKSHELL_ANIM=0)");
        return;
    }
    let mut cfg = Config::load();
    if !cfg.x.enabled {
        eprintln!("[anim] disabled in config");
        return;
    }
    ENABLED.store(true, Ordering::Relaxed);
    player::set_ui(ui.as_weak());
    ui.global::<crate::AnimState>().on_clicked(|| {
        consume_input("pointer", true);
    });

    // Сохраняем выбор для следующего запуска. В этом сеансе пробуждение повторяет текущий ролик.
    let boot_now = cfg.get("boot");
    if let Some(id) = cfg.shuffled(ShuffleSlot::Boot) {
        cfg.set("boot", &id);
        cfg.set("current_set", "");
    }
    cfg.save();
    eprintln!(
        "[anim] randomize_boot={} randomize_suspend={} now: boot={} ({}) | next boot={} ({}) suspend={} ({}) throbber={} ({})",
        cfg.shuffle_enabled(ShuffleSlot::Boot),
        cfg.shuffle_enabled(ShuffleSlot::Suspend),
        boot_now,
        cfg.name_of(&boot_now),
        cfg.get("boot"),
        cfg.name_of(&cfg.get("boot")),
        cfg.get("suspend"),
        cfg.name_of(&cfg.get("suspend")),
        cfg.get("throbber"),
        cfg.name_of(&cfg.get("throbber"))
    );

    let (tx, rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let (cache_dir, budget) = (cfg.dirs.cache.clone(), cfg.x.cache_budget_mb << 20);
    let dopts = decode_opts(&cfg, true);
    std::thread::Builder::new()
        .name("anim-prepare".into())
        .spawn(move || {
            while let Ok(mut paths) = rx.recv() {
                // debounce: take the newest request that is already queued
                while let Ok(p) = rx.try_recv() {
                    paths = p;
                }
                let mut keep = vec![];
                for p in paths {
                    match cache::ensure(&cache_dir, &p, &dopts) {
                        Ok(c) => keep.push(c),
                        Err(e) => eprintln!("[anim] cache {}: {e}", p.display()),
                    }
                    keep.push(cache::cache_path(&cache_dir, &p, &dopts));
                }
                cache::gc(&cache_dir, budget, &keep);
            }
        })
        .ok();

    let power_hooks = cfg.x.inhibit && (session_mode() || env_flag("DECKSHELL_ANIM_POWER") == Some(true));
    let boot_play = env_flag("DECKSHELL_ANIM_BOOT").unwrap_or_else(session_mode);
    MGR.with(|m| *m.borrow_mut() = Some(Mgr { cfg, boot_now, handed_over: None, prep: tx }));

    if boot_play {
        play_boot_at_start();
    }
    prepare_all(Duration::from_secs(if boot_play { 15 } else { 1 }));
    if power_hooks {
        setup_power_hooks();
    } else {
        eprintln!("[anim] logind hooks off (not a session; DECKSHELL_ANIM_POWER=1 to force)");
    }
}

fn prepare_all(delay: Duration) {
    let Some((paths, tx)) = with_mgr(|m| {
        let mut ids = vec![m.boot_now.clone(), m.cfg.get("boot"), m.cfg.get("suspend"), m.cfg.get("throbber")];
        ids.push(m.cfg.slot("shutdown"));
        let mut paths: Vec<PathBuf> = vec![];
        // order = urgency: what plays soonest first
        for (i, id) in ids.iter().enumerate() {
            let kind = ["boot", "boot", "suspend", "throbber", "shutdown"][i];
            if let Some(p) = m.cfg.resolve(id).or_else(|| m.cfg.fallback(kind)) {
                if !paths.contains(&p) {
                    paths.push(p);
                }
            }
        }
        (paths, m.prep.clone())
    }) else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = tx.send(paths);
    });
}

fn setup_power_hooks() {
    std::thread::Builder::new()
        .name("anim-power".into())
        .spawn(|| {
            let max = logind::inhibit_delay_max();
            DELAY_MAX_MS.store(max.as_millis() as u64, Ordering::Relaxed);
            // The monitor has the same PDEATHSIG contract as the inhibitor:
            // its creating thread must outlive this temporary setup worker.
            let _ = slint::invoke_from_event_loop(|| {
                match logind::watch(|ev| {
                    let _ = slint::invoke_from_event_loop(move || on_prepare(ev));
                }) {
                    Ok(w) => *WATCHER.lock().unwrap() = Some(w),
                    Err(e) => eprintln!("[anim] cannot watch logind: {e}"),
                }
                queue_inhibitor();
            });
        })
        .ok();
}

// logind's helper uses PR_SET_PDEATHSIG, whose parent is the creating Linux
// thread. Spawning it on a temporary worker kills the helper when that worker
// exits, even though DeckShell itself is alive. Keep its parent on the UI thread
// for both initial acquisition and re-acquisition after resume.
fn queue_inhibitor() {
    let _ = slint::invoke_from_event_loop(|| {
        take_inhibitor();
        eprintln!("[anim] logind: InhibitDelayMaxUSec={} ms, delay lock {}", DELAY_MAX_MS.load(Ordering::Relaxed), if INHIBITOR.lock().unwrap().is_some() { "held" } else { "NOT held" });
    });
}

fn take_inhibitor() {
    let mut g = INHIBITOR.lock().unwrap();
    if g.is_none() {
        match logind::inhibit_delay("shutdown:sleep", "DeckShell", "Playing the suspend / shutdown animation") {
            Ok(i) => *g = Some(i),
            Err(e) => eprintln!("[anim] cannot take the logind delay lock: {e}"),
        }
    }
}

fn release_inhibitor() {
    if INHIBITOR.lock().unwrap().take().is_some() {
        eprintln!("[anim] logind delay lock released");
    }
}

// ---------------------------------------------------------------- playing

struct Req {
    kind: Kind,
    /// Override the slot (previews may choose a specific id).
    id: Option<String>,
    /// Override the file (settings page: a catalog preview that is not in the config).
    path: Option<PathBuf>,
    end: End,
    skippable: bool,
    budget: Option<Duration>,
    on_done: Box<dyn FnOnce(Outcome)>,
}

// Boot and every wake share the selection for this process. Randomize has
// already persisted the next boot; reading that slot here would change videos.
fn playback_id(m: &Mgr, kind: Kind, override_id: Option<&str>) -> String {
    override_id.map(str::to_owned).unwrap_or_else(|| {
        if kind == Kind::Boot { m.boot_now.clone() } else { m.cfg.slot(kind.name()) }
    })
}

/// Start a playback; false when there is nothing to play (caller proceeds without animation).
fn play(r: Req) -> bool {
    let Some((path, dir, dopts, vol, delay)) = with_mgr(|m| {
        let id = playback_id(m, r.kind, r.id.as_deref());
        let path = r.path.clone().or_else(|| m.cfg.resolve(&id)).or_else(|| m.cfg.fallback(match r.kind {
            Kind::Shutdown => "shutdown",
            k => k.name(),
        }));
        (path, m.cfg.dirs.cache.clone(), decode_opts(&m.cfg, false), m.cfg.x.volume, m.cfg.x.audio_latency_ms)
    }) else {
        return false;
    };
    let Some(path) = path else {
        note(format!("{}: no animation file", r.kind.name()));
        return false;
    };
    note(format!("{}: playing {}", r.kind.name(), path.display()));
    player::start(
        path,
        PlayOpts {
            label: r.kind.name().into(),
            in_catalog: false,
            defer_initial_show: r.kind == Kind::Boot,
            skippable: r.skippable,
            end: r.end,
            budget: r.budget,
            volume: vol,
            video_delay_ms: delay,
            decode: dopts,
            cache_dir: dir,
            on_done: Box::new(move |o| {
                *LAST.lock().unwrap() = format!("{}: {:?}", r.kind.name(), o);
                (r.on_done)(o)
            }),
        },
    );
    true
}

fn play_boot_at_start() {
    let ok = play(Req { path: None, kind: Kind::Boot, id: None, end: End::FadeOut, skippable: true, budget: None, on_done: Box::new(|_| {}) });
    if !ok {
        eprintln!("[anim] no boot animation available");
    }
}

/// Account switch finished (task 444): the same clip as on wake-up (the boot slot of this process, so the resume
/// shuffle applies). Plays regardless of `boot_on_resume`: the owner asked for it explicitly. UI thread; false when
/// animations are off or nothing can be played (the caller just drops its cover).
pub fn play_resume_clip() -> bool {
    if !ENABLED.load(Ordering::Relaxed) || (dryrun() && env_flag("DECKSHELL_ANIM") != Some(true)) {
        return false;
    }
    player::cancel_catalog();
    play(Req { path: None, kind: Kind::Boot, id: None, end: End::FadeOut, skippable: true, budget: None, on_done: Box::new(|_| {}) })
}

fn power_kind(a: PowerAction) -> Option<Kind> {
    match a {
        PowerAction::Sleep => Some(if LOADING.load(Ordering::Relaxed) { Kind::Throbber } else { Kind::Suspend }),
        PowerAction::Restart | PowerAction::Shutdown => Some(Kind::Shutdown),
        _ => None,
    }
}

/// Hook for `actions::run`: play the animation first, then run `finish` (the original action). Returns true when the action
/// was taken over (it will run later); false = caller runs it immediately. Callable from any thread.
pub fn power(a: PowerAction, finish: Box<dyn FnOnce() + Send>) -> bool {
    if !ENABLED.load(Ordering::Relaxed) || (dryrun() && env_flag("DECKSHELL_ANIM") != Some(true)) {
        return false;
    }
    let Some(kind) = power_kind(a) else { return false };
    let cell = Mutex::new(Some(finish));
    let f = move || {
        let fin = cell.lock().unwrap().take();
        start_power(kind, fin);
    };
    slint::invoke_from_event_loop(f).is_ok()
}

// Вызывается только при начале запроса. Повторный сигнал logind использует уже запущенный ролик.
fn sleep_pick(cfg: &mut Config, kind: Kind) -> Option<String> {
    if kind != Kind::Suspend || !cfg.shuffle_enabled(ShuffleSlot::Suspend) {
        return None;
    }
    let pick = cfg.shuffled(ShuffleSlot::Suspend);
    cfg.save();
    pick
}

fn start_power(kind: Kind, finish: Option<Box<dyn FnOnce() + Send>>) {
    if let Some(l) = player::playing_label() {
        if l != "boot" {
            eprintln!("[anim] power request while '{l}' is playing: ignored");
            return;
        }
    }
    player::cancel_catalog();
    let lifecycle = std::rc::Rc::new(PowerLifecycle::default());
    INTERNAL_POWER.with(|p| *p.borrow_mut() = Some(lifecycle.clone()));
    let fin = std::rc::Rc::new(std::cell::RefCell::new(finish));
    let fin2 = fin.clone();
    let ok = play(Req {
        path: None,
        kind,
        id: with_mgr(|m| sleep_pick(&mut m.cfg, kind)).flatten(),
        end: End::Hold,
        skippable: false,
        budget: None,
        on_done: Box::new(move |outcome| {
            INTERNAL_POWER.with(|p| { p.borrow_mut().take(); });
            lifecycle.complete(outcome, || handover(kind, fin2.borrow_mut().take()), release_inhibitor);
        }),
    });
    if !ok {
        // Hand over immediately and let logind know this request was already handled.
        INTERNAL_POWER.with(|p| { p.borrow_mut().take(); });
        handover(kind, fin.borrow_mut().take());
    }
}

// An external request supersedes a pending internal request; its delay lock must be released.
#[derive(Default)]
struct PowerLifecycle { external: std::cell::Cell<bool> }
impl PowerLifecycle {
    fn external_prepare(&self) { self.external.set(true); }
    fn complete(&self, outcome: Outcome, handover: impl FnOnce(), release: impl FnOnce()) {
        if self.external.get() { release(); }
        else if outcome != Outcome::Cancelled { handover(); }
    }
}

/// Animation done (or none): run the real action and arm the failsafe that uncovers the UI if the system never goes down.
fn handover(kind: Kind, finish: Option<Box<dyn FnOnce() + Send>>) {
    with_mgr(|m| m.handed_over = Some((kind, Instant::now())));
    if let Some(f) = finish {
        f();
    }
    let wait = if dryrun() {
        Duration::from_millis(1200)
    } else if kind == Kind::Shutdown {
        HOLD_FAILSAFE_SHUTDOWN
    } else {
        HOLD_FAILSAFE_SLEEP
    };
    let gen = player::ACTIVE_GEN.load(Ordering::Relaxed);
    slint::Timer::single_shot(wait, move || {
        let still = with_mgr(|m| m.handed_over.is_some()).unwrap_or(false);
        if still && player::ACTIVE_GEN.load(Ordering::Relaxed) == gen && !player::is_playing() {
            eprintln!("[anim] {} did not happen within {} s - uncovering the UI", kind.name(), wait.as_secs());
            with_mgr(|m| m.handed_over = None);
            player::uncover(300);
        }
    });
}

/// logind signal (UI thread).
fn on_prepare(ev: PrepareEvent) {
    eprintln!("[anim] logind: {ev:?}");
    let (kind, starting) = match ev {
        PrepareEvent::Sleep(s) => (if LOADING.load(Ordering::Relaxed) { Kind::Throbber } else { Kind::Suspend }, s),
        PrepareEvent::Shutdown(s) => (Kind::Shutdown, s),
    };
    let budget = Duration::from_millis(DELAY_MAX_MS.load(Ordering::Relaxed)).saturating_sub(DELAY_SAFETY).max(Duration::from_millis(500));
    if !starting {
        // resumed (or shutdown cancelled): drop the frozen last frame, re-arm the lock
        with_mgr(|m| m.handed_over = None);
        player::cancel(Outcome::Cancelled);
        player::uncover(250);
        if ENABLED.load(Ordering::Relaxed) {
            queue_inhibitor();
        }
        let resume = matches!(ev, PrepareEvent::Sleep(false));
        let enabled = with_mgr(|m| m.cfg.x.boot_on_resume).unwrap_or(false);
        if resume { eprintln!("[anim] resume decision: boot_on_resume={enabled}"); }
        if resume && enabled {
            play(Req { path: None, kind: Kind::Boot, id: None, end: End::FadeOut, skippable: true, budget: None, on_done: Box::new(|_| {}) });
        }
        return;
    }
    // already animating for this request (play-first flow still running, or already handed over)
    if player::playing_label().is_some_and(|l| l != "boot") {
        INTERNAL_POWER.with(|p| {
            if let Some(lifecycle) = p.borrow().as_ref() { lifecycle.external_prepare(); }
        });
        player::tighten_budget(budget);
        return;
    }
    if with_mgr(|m| m.handed_over.is_some()).unwrap_or(false) {
        // we played first and then called the system: let it through now
        release_inhibitor();
        return;
    }
    player::cancel_catalog();
    // external request: play within the delay window, then release the lock
    let external = PowerLifecycle::default();
    external.external_prepare();
    let ok = play(Req {
        path: None,
        kind,
        id: with_mgr(|m| sleep_pick(&mut m.cfg, kind)).flatten(),
        end: End::Hold,
        skippable: false,
        budget: Some(budget),
        on_done: Box::new(move |outcome| external.complete(outcome, || {}, release_inhibitor)),
    });
    if !ok {
        release_inhibitor();
    }
}

// ---------------------------------------------------------------- input

/// Called by the input paths (key press / pad action / click) BEFORE normal handling. True = swallowed by the overlay;
/// during boot only a fresh intentional action skips it.
fn input_disposition(mode: u8, fresh: bool) -> (bool, bool) {
    (mode != 0, mode == 2 && fresh)
}

pub fn consume_input(source: &'static str, fresh: bool) -> bool {
    let (swallow, skip) = input_disposition(player::INPUT_MODE.load(Ordering::Relaxed), fresh);
    if skip { player::request_skip(source); }
    swallow
}

#[cfg(test)]
mod input_disposition_tests {
    use super::input_disposition;
    #[test]
    fn overlay_swallow_and_skip_policy() {
        assert_eq!(input_disposition(0, true), (false, false));
        assert_eq!(input_disposition(1, true), (true, false));
        assert_eq!(input_disposition(2, false), (true, false));
        assert_eq!(input_disposition(2, true), (true, true));
    }
}

/// The loader (throbber) screen is up: a suspend now plays the `throbber` animation (Steam: `steam_os_suspend_from_throbber`).
#[allow(dead_code)]
pub fn set_loading(on: bool) {
    LOADING.store(on, Ordering::Relaxed);
}

// ---------------------------------------------------------------- settings page (task 412)

thread_local! {
    // Used by the settings page when the manager is not running (animations disabled, DECKSHELL_ANIM=0, tests).
    static PAGE_CFG: RefCell<Option<(Config, String)>> = const { RefCell::new(None) };
}

/// The page's view of the config: the running manager's (so `boot_now` and the caches follow at once), else a page copy
/// loaded from disk. The closure gets the config and the boot id of this start.
pub fn with_config<R>(f: impl FnOnce(&mut Config, &mut String) -> R) -> R {
    let mut f = Some(f);
    if let Some(r) = with_mgr(|m| (f.take().unwrap())(&mut m.cfg, &mut m.boot_now)) {
        return r;
    }
    PAGE_CFG.with(|p| {
        let mut p = p.borrow_mut();
        let (cfg, now) = p.get_or_insert_with(|| {
            let c = Config::load();
            let now = c.get("boot");
            (c, now)
        });
        (f.take().unwrap())(cfg, now)
    })
}

/// Tests: the page works on this config instead of the user's.
#[cfg(test)]
pub fn install_page_config(cfg: Config) {
    let now = cfg.get("boot");
    PAGE_CFG.with(|p| *p.borrow_mut() = Some((cfg, now)));
}

/// Save after a page change and refresh the frame caches of the slots.
pub fn page_commit() {
    with_config(|c, _| c.save());
    prepare_all(Duration::ZERO);
}

/// Whether the animation player runs in this process (else page changes apply at the next start).
pub fn active() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Full-screen preview of a specific animation with sound; any fresh input skips it. Nothing else changes.
pub fn preview(id: &str, suspend: bool) -> Result<(), String> {
    if !active() {
        return Err("Проигрыватель анимаций выключен: предпросмотр на весь экран недоступен".into());
    }
    if player::is_playing() {
        return Err("Уже идёт воспроизведение".into());
    }
    let kind = if suspend { Kind::Suspend } else { Kind::Boot };
    // "" = the default: the slot resolver falls back to uioverrides / Steam's stock movie
    let ok = play(Req { path: None, kind, id: Some(id.to_string()), end: End::FadeOut, skippable: true, budget: None, on_done: Box::new(|_| {}) });
    ok.then_some(()).ok_or_else(|| "Файл анимации не найден".into())
}

/// Full-screen preview of a file that is not in the config (a catalog preview video).
pub fn preview_file(path: PathBuf) -> Result<(), String> {
    if !active() {
        return Err("Проигрыватель анимаций выключен: предпросмотр на весь экран недоступен".into());
    }
    if player::is_playing() {
        return Err("Уже идёт воспроизведение".into());
    }
    let ok = play(Req { path: Some(path), kind: Kind::Boot, id: None, end: End::FadeOut, skippable: true, budget: None, on_done: Box::new(|_| {}) });
    ok.then_some(()).ok_or_else(|| "Файл не найден".into())
}

/// Предпросмотр в карточке каталога с тем же декодером и микшером, что у ролика запуска.
pub fn preview_catalog(ui: &AppWindow, path: PathBuf, on_done: impl FnOnce(Outcome) + 'static) -> Result<(), String> {
    if player::INPUT_MODE.load(Ordering::Relaxed) != 0 {
        return Err("Уже идёт воспроизведение".into());
    }
    let (cache_dir, volume, delay) = with_config(|c, _| (c.dirs.cache.clone(), c.x.volume, c.x.audio_latency_ms));
    player::set_ui(ui.as_weak());
    player::start(path, PlayOpts {
        label: "Предпросмотр каталога".into(),
        in_catalog: true,
        defer_initial_show: false,
        skippable: false,
        end: End::FadeOut,
        budget: None,
        volume,
        video_delay_ms: delay,
        decode: cache::Opts { maxw: 560, maxh: 350, background: false },
        cache_dir,
        on_done: Box::new(on_done),
    });
    Ok(())
}

// ---------------------------------------------------------------- control socket

/// `anim ...` commands (UI thread, from control.rs).
pub fn command(args: &[&str]) -> String {
    if !ENABLED.load(Ordering::Relaxed) {
        return "err animations disabled (DECKSHELL_ANIM=0 or config)".into();
    }
    match args {
        [k @ ("boot" | "suspend" | "shutdown" | "throbber")] => {
            // preview: plays the slot, then the overlay fades out; no system action
            let kind = Kind::parse(k).unwrap();
            if play(Req { path: None, kind, id: None, end: End::FadeOut, skippable: kind == Kind::Boot, budget: None, on_done: Box::new(|_| {}) }) {
                format!("ok playing {}", kind.name())
            } else {
                format!("err no animation for {}", kind.name())
            }
        }
        ["power", what] => {
            // full DeckShell-initiated flow without touching the system: animation, then a log line instead of logind
            let Some(kind) = Kind::parse(what).filter(|k| *k != Kind::Boot && *k != Kind::Throbber) else { return "err power sleep|shutdown|restart".into() };
            let name = kind.name();
            start_power(kind, Some(Box::new(move || eprintln!("[anim] (dry) system action '{name}' would run now"))));
            format!("ok power {}", kind.name())
        }
        ["fake", what] => {
            // simulate the logind signals (PrepareForSleep/Shutdown) to exercise the external-request path
            let ev = match *what {
                "sleep" => PrepareEvent::Sleep(true),
                "resume" => PrepareEvent::Sleep(false),
                "shutdown" => PrepareEvent::Shutdown(true),
                "cancel" => PrepareEvent::Shutdown(false),
                _ => return "err fake sleep|resume|shutdown|cancel".into(),
            };
            on_prepare(ev);
            format!("ok fake {what}")
        }
        ["stop"] => {
            player::cancel(Outcome::Cancelled);
            player::uncover(200);
            with_mgr(|m| m.handed_over = None);
            "ok".into()
        }
        ["loading", v] => {
            set_loading(*v == "on" || *v == "1");
            format!("ok loading={}", LOADING.load(Ordering::Relaxed))
        }
        ["status"] => status(),
        ["list"] => list(None),
        ["list", t] => list(Some(t)),
        ["set", slot @ ("boot" | "suspend" | "throbber" | "shutdown"), id] => {
            let id = if *id == "none" { "" } else { id };
            let r = with_mgr(|m| {
                if !id.is_empty() && m.cfg.resolve(id).is_none() {
                    return Err(format!("err unknown or missing animation {id}"));
                }
                if *slot == "shutdown" {
                    m.cfg.x.shutdown = id.to_string();
                    if let Some(d) = m.cfg.v.get_mut("deckshell").and_then(|d| d.as_object_mut()) {
                        d.insert("shutdown".into(), serde_json::json!(id));
                    }
                } else {
                    m.cfg.set(slot, id);
                    if *slot == "boot" {
                        m.boot_now = id.to_string();
                    }
                }
                m.cfg.save();
                Ok(())
            });
            prepare_all(Duration::ZERO);
            match r {
                Some(Ok(())) => format!("ok {slot}={id}"),
                Some(Err(e)) => e,
                None => "err busy".into(),
            }
        }
        ["randomize", mode] => {
            let m = *mode;
            with_mgr(|g| {
                match m {
                    "all" => g.cfg.randomize_all(),
                    "set" => g.cfg.randomize_set(),
                    _ => {}
                }
                g.cfg.save();
            });
            prepare_all(Duration::ZERO);
            status()
        }
        ["mode", mode @ ("all" | "set" | "off")] => {
            with_mgr(|g| {
                g.cfg.set_shuffle(ShuffleSlot::Boot, *mode != "off");
                g.cfg.set_shuffle(ShuffleSlot::Suspend, *mode != "off");
                g.cfg.save();
            });
            format!("ok randomize={mode}")
        }
        ["import"] => {
            let n = with_mgr(|g| {
                let n = g.cfg.import_plugin();
                g.cfg.scan_local();
                g.cfg.save();
                n
            });
            prepare_all(Duration::ZERO);
            format!("ok imported {} downloads", n.unwrap_or(0))
        }
        ["prepare"] => {
            prepare_all(Duration::ZERO);
            "ok preparing".into()
        }
        ["cache"] => cache_report(),
        _ => "err anim boot|suspend|shutdown|throbber | power sleep|shutdown|restart | fake sleep|resume|shutdown|cancel | stop | status | list [boot|suspend] | set <slot> <id|none> | randomize all|set | mode all|set|off | import | prepare | cache | loading on|off".into(),
    }
}

fn status() -> String {
    with_mgr(|m| {
        let slot = |k: &str| {
            let id = m.cfg.slot(k);
            serde_json::json!({"id": id, "name": m.cfg.name_of(&id), "file": m.cfg.path_for(k).map(|p| p.display().to_string())})
        };
        serde_json::json!({
            "randomize_boot": m.cfg.shuffle_enabled(ShuffleSlot::Boot),
            "randomize_suspend": m.cfg.shuffle_enabled(ShuffleSlot::Suspend),
            "current_set": m.cfg.get("current_set"),
            "boot_now": m.boot_now, "boot_next": slot("boot"), "suspend": slot("suspend"), "throbber": slot("throbber"), "shutdown": slot("shutdown"),
            "playing": player::playing_label(), "last": LAST.lock().unwrap().clone(), "loading": LOADING.load(Ordering::Relaxed),
            "handed_over": m.handed_over.map(|(k, t)| format!("{} {} ms ago", k.name(), t.elapsed().as_millis())),
            "lock_held": INHIBITOR.lock().unwrap().is_some(), "delay_max_ms": DELAY_MAX_MS.load(Ordering::Relaxed),
            "downloads": m.cfg.downloads().len(), "local": m.cfg.local.len(), "dryrun": dryrun(),
        })
        .to_string()
    })
    .unwrap_or_else(|| "err busy".into())
}

fn list(target: Option<&str>) -> String {
    with_mgr(|m| {
        m.cfg
            .all()
            .into_iter()
            .filter(|a| target.map(|t| a.target == t).unwrap_or(true))
            .map(|a| format!("{}\t{}\t{}\t{}", a.id, a.target, a.source, a.name))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap_or_default()
}

fn cache_report() -> String {
    with_mgr(|m| {
        let (mut n, mut bytes) = (0, 0u64);
        if let Ok(rd) = std::fs::read_dir(&m.cfg.dirs.cache) {
            for e in rd.flatten().filter(|e| e.path().extension().map(|x| x == "dsa").unwrap_or(false)) {
                n += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        format!("{} files, {} MB in {} (budget {} MB)", n, bytes >> 20, m.cfg.dirs.cache.display(), m.cfg.x.cache_budget_mb)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod power_lifecycle_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn power_completion_respects_request_origin_and_cancellation() {
        let requests = Cell::new(0);
        let releases = Cell::new(0);
        let finish = |state: &PowerLifecycle, outcome| state.complete(outcome,
            || requests.set(requests.get() + 1), || releases.set(releases.get() + 1));
        finish(&PowerLifecycle::default(), Outcome::Finished);
        assert_eq!((requests.get(), releases.get()), (1, 0));
        finish(&PowerLifecycle::default(), Outcome::Cancelled);
        assert_eq!((requests.get(), releases.get()), (1, 0));
        let pending = PowerLifecycle::default();
        pending.external_prepare();
        finish(&pending, Outcome::Trimmed);
        assert_eq!((requests.get(), releases.get()), (1, 1));
        let external_only = PowerLifecycle::default();
        external_only.external_prepare();
        finish(&external_only, Outcome::Finished);
        assert_eq!((requests.get(), releases.get()), (1, 2));
    }
}

#[cfg(test)]
mod boot_selection_tests {
    use super::*;
    #[test]
    fn randomize_now_is_reused_on_two_resumes_and_explicit_set_still_updates_it() {
        let (prep, _) = std::sync::mpsc::channel();
        let cfg = Config {
            v: serde_json::json!({"boot":"next"}).as_object().unwrap().clone(),
            dirs: config::Dirs::detect(), local: vec![], local_sets: vec![], x: config::Extras::default(),
        };
        let mut mgr = Mgr { cfg, boot_now: "now".into(), handed_over: None, prep };
        for enabled in [true, false] {
            mgr.cfg.set_shuffle(ShuffleSlot::Boot, enabled);
            for _ in 0..3 { // boot and two wakes, using the exact play() resolver
                assert_eq!(playback_id(&mgr, Kind::Boot, None), "now");
                assert_eq!(mgr.cfg.get("boot"), "next");
            }
        }
        // Same state update as control's explicit `anim set boot`.
        mgr.cfg.set("boot", "explicit");
        mgr.boot_now = "explicit".into();
        assert_eq!(playback_id(&mgr, Kind::Boot, None), "explicit");
        assert_eq!(playback_id(&mgr, Kind::Boot, Some("preview")), "preview");
    }
}
