//! UI controller core: shared state (library data, images, router, overlays), input dispatch and the glue to the
//! per-screen modules in `crate::screens` (each owns its state struct and `impl App` block).
use steam_assets::i18n::tr_ref;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use steam_data::{Game, GameKind, Library};

use crate::actions::{self, Act};
use crate::assets::{glyph_image, tu};
use crate::backend::Update;
use crate::clips::{ClipPlay, ClipSlot};
use crate::data::DataSource;
use crate::input::Action;
use crate::loader::{AnimFrames, Job, Kind, Loader};
use crate::model::Extra;
use crate::router::{Route, Router};
use crate::screens::{downloads, game, home, library, qam, settings, steam_menu};
use crate::sound::{play, Snd};
use crate::{AppWindow, BackdropState, CardData, FooterState, GameState, Hint, HeroFrame, Shell, TopBarState};

// ------------------------------------------------------------------ formatting helpers (shared by screens)
/// Hero crop of `id` as the Slint `HeroFrame` (src/sgdb/hero_crop.rs; default = whole cover frame).
fn hero_frame(id: u32) -> HeroFrame {
    let c = crate::sgdb::hero_crop::get(id);
    HeroFrame { zoom: c.zoom, pan_x: c.pan_x, pan_y: c.pan_y }
}

pub fn comma(s: String) -> String {
    if steam_assets::i18n::current().code() == "ru" { s.replace('.', ",") } else { s }
}
pub fn fmt_size(b: u64) -> String {
    use steam_assets::i18n::tr_args;
    let g = b as f64 / 1e9;
    if b == 0 {
        "-".into()
    } else if g >= 1.0 {
        tr_args("screen_format.size_gb", &[("number", &comma(format!("{g:.1}")))])
    } else {
        tr_args("screen_format.size_mb", &[("number", &format!("{:.0}", b as f64 / 1e6))])
    }
}
pub fn fmt_playtime(m: u32) -> String {
    use steam_assets::i18n::{tr, tr_args};
    if m == 0 {
        tr("screen_format.no_playtime")
    } else if m < 60 {
        tr_args("screen_format.playtime_minutes", &[("minutes", &m.to_string())])
    } else {
        tr_args("screen_format.playtime_hours", &[("hours", &comma(format!("{:.1}", m as f64 / 60.0)))])
    }
}
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
pub fn days_ago(t: u64) -> u64 {
    now_secs().saturating_sub(t) / 86_400
}
pub fn fmt_ago(t: Option<u64>) -> String {
    use steam_assets::i18n::{tr, tr_args};
    let Some(t) = t else { return tr("screen_format.never") };
    match days_ago(t) {
        0 => tr("screen_format.today"),
        1 => tr("screen_format.yesterday"),
        d if d < 60 => tr_args("screen_format.days_ago", &[("days", &d.to_string())]),
        d => tr_args("screen_format.months_ago", &[("months", &(d / 30).to_string())]),
    }
}

/// Longest slide-out of a side menu (QAM 200 ms; the Steam menu's curve is within a few pixels of
/// its end by then). The game gets the screen back after it.
const OVERLAY_CLOSE_HOLD: Duration = Duration::from_millis(200);
/// If the compositor has not confirmed the overlay by then, show the menu anyway; the 3 s
/// rejection path in `sync_game_overlay` still decides whether the overlay failed.
const OVERLAY_OPEN_WAIT: Duration = Duration::from_millis(400);

/// Animation clock counters (control command `clockstats`): how often the clock ran, stopped and was woken.
static CLOCK_STARTS: AtomicU64 = AtomicU64::new(0);
static CLOCK_STOPS: AtomicU64 = AtomicU64::new(0);
static CLOCK_TICKS: AtomicU64 = AtomicU64::new(0);
static SHELL_SIGNALS: AtomicU64 = AtomicU64::new(0);

pub fn clock_report() -> String {
    format!(
        "clock starts={} stops={} ticks={} shell_signals={}",
        CLOCK_STARTS.load(Ordering::Relaxed),
        CLOCK_STOPS.load(Ordering::Relaxed),
        CLOCK_TICKS.load(Ordering::Relaxed),
        SHELL_SIGNALS.load(Ordering::Relaxed),
    )
}

/// The one UI clock runs only while something of the shell is on screen and needs its frames.
fn clock_wanted(window_hidden: bool, shell_hidden: bool, route_animates: bool, content_playing: bool) -> bool {
    !window_hidden && !shell_hidden && (route_animates || content_playing)
}

/// Longest time the page may stay hidden with no game menu requested. The worker normally acknowledges a closed
/// overlay within its focus poll; past this bound the page is shown again whatever the worker observed.
const SUPPRESS_ORPHAN_LIMIT: Duration = Duration::from_millis(1500);

/// The page is hidden, no menu over a game is requested, and the acknowledgement did not come in time.
fn suppression_expired(menu_requested: bool, suppressed: bool, orphan_for: Option<Duration>) -> bool {
    !menu_requested && suppressed && orphan_for.is_some_and(|t| t >= SUPPRESS_ORPHAN_LIMIT)
}

/// The page was hidden under a game menu and is shown again: one full redraw is due.
fn unpause_needs_redraw(was_suppressed: bool, suppressed: bool) -> bool {
    was_suppressed && !suppressed
}

/// Hold the slide-in of a menu over a game until our window is on screen.
fn overlay_open_must_wait(active: bool, since_request: Option<Duration>) -> bool {
    !active && since_request.is_some_and(|t| t < OVERLAY_OPEN_WAIT)
}

/// Keep the overlay up while the closed menu slides out. `shown`: the overlay reached the screen.
fn overlay_close_must_wait(shown: bool, since_close: Duration) -> bool {
    shown && since_close < OVERLAY_CLOSE_HOLD
}

#[cfg(test)]
mod overlay_timing_tests {
    use super::*;

    #[test]
    fn slide_in_waits_for_the_compositor_but_not_forever() {
        assert!(overlay_open_must_wait(false, Some(Duration::ZERO)));
        assert!(overlay_open_must_wait(false, Some(Duration::from_millis(399))));
        assert!(!overlay_open_must_wait(true, Some(Duration::from_millis(10))));
        assert!(!overlay_open_must_wait(false, Some(OVERLAY_OPEN_WAIT)));
        assert!(!overlay_open_must_wait(false, None));
    }

    #[test]
    fn game_returns_after_the_slide_out_and_at_once_when_nothing_was_shown() {
        assert!(overlay_close_must_wait(true, Duration::from_millis(50)));
        assert!(!overlay_close_must_wait(true, OVERLAY_CLOSE_HOLD));
        assert!(!overlay_close_must_wait(false, Duration::ZERO));
    }
}

fn cap(kind: Kind) -> usize {
    match kind {
        Kind::Capsule => 72,
        Kind::Hero => 6,
        Kind::Logo => 4,
        Kind::Header => 10,
    }
}

fn art_path(art: &steam_data::Art, kind: Kind) -> Option<&std::path::PathBuf> {
    match kind {
        Kind::Capsule => art.capsule.as_ref(),
        Kind::Hero => art.hero.as_ref().or(art.header.as_ref()),
        Kind::Logo => art.logo.as_ref(),
        Kind::Header => art.header.as_ref().or(art.hero.as_ref()),
    }
}

fn changed_art_kinds<'a>(old: &'a steam_data::Art, new: &'a steam_data::Art) -> impl Iterator<Item = Kind> + 'a {
    [Kind::Capsule, Kind::Hero, Kind::Logo, Kind::Header].into_iter()
        .filter(move |&kind| art_path(old, kind) != art_path(new, kind))
}

/// Live data from the Steam bridge (None/empty until the worker delivers it).
#[derive(Default)]
pub struct Live {
    pub bridge_up: bool,
    pub running: Vec<u32>,
    pub downloads: Option<steam_bridge::Downloads>,
    pub unread: u32,
    pub system: Option<steam_bridge::SystemInfo>,
}

pub struct App {
    pub ui: slint::Weak<AppWindow>,
    pub ds: Arc<DataSource>,
    /// File-based library (steam-data): cold start + art lookup + fallback when the bridge is down.
    pub base: Arc<Library>,
    /// Library the UI shows (bridge data merged over `base` when available).
    pub lib: Arc<Library>,
    pub extras: Arc<HashMap<u32, Extra>>,
    pub bridge_recent: Vec<u32>,
    pub live: Live,
    pub header_profile: crate::screens::chrome::HeaderProfile,
    /// A menu opened from the observed game graphics base, not from a library page.
    pub game_overlay: Option<u32>,
    game_overlay_requested: bool,
    game_overlay_clearing: bool,
    game_overlay_since: Option<Instant>,
    /// The last menu over the game closed at this moment; the game gets the screen back once the
    /// close animation has played (see `OVERLAY_CLOSE_HOLD`).
    game_overlay_closing: Option<Instant>,
    /// Since when the page is hidden without a requested game menu (see `suppression_expired`).
    suppress_orphan_since: Option<Instant>,
    /// Last logged (menu game, clearing, flagged, page hidden): `overlay.sync` is written on each change.
    overlay_diag: (Option<u32>, bool, bool, bool),
    pub loader: Loader,
    pub gidx: HashMap<u32, usize>,
    pub imgs: HashMap<(u32, Kind), Image>,
    pub requested: HashSet<(u32, Kind)>,
    pub lru: Vec<(u32, Kind)>,
    pub glow: HashMap<u32, Color>,
    pub pad_mode: bool,

    pub router: Router,
    pub last_guide: Instant,

    // background hero
    pub bg_game: Option<u32>,
    pub bg_front_a: bool,

    pub hm: home::State,
    pub lb: library::State,
    pub search: crate::screens::search::State,
    pub gm: game::State,
    pub account: crate::screens::account_switch::State,
    pub install: crate::screens::install::State,
    pub dl: downloads::State,
    pub sm: steam_menu::State,
    pub dm: crate::screens::downloads_menu::State,
    pub qm: qam::State,
    pub se: settings::State,

    // animated cover
    pub focus_game: Option<u32>,
    pub anim_delay: Timer,
    pub anim_timer: Timer,
    pub trim_timer: Timer,
    pub pulse_t0: Instant,
    pub kb_t0: Instant,
    pub kb_tick: u32,
    pub anim_cache: HashMap<u32, Rc<Vec<(Image, u32)>>>,
    pub anim_requested: HashSet<u32>,
    /// Animated hero banner (Home focus / game page) and animated game logo (game page); see clips.rs.
    pub hero_clips: ClipSlot,
    pub logo_clips: ClipSlot,
    pub anim_play: HashMap<u32, (Rc<Vec<(Image, u32)>>, usize, Instant)>,

    pub level_popup: crate::level::Popup,
    pub clock_timer: Timer,
    game_overlay_timer: Timer,
    pub script_timer: Timer,
    pub script: std::collections::VecDeque<String>,
    pub shot_n: usize,
}

fn page_focus_signature(home: &crate::screens::home::State, settings: &crate::screens::settings::State) -> (i32, usize, usize, u8, usize, Option<usize>) {
    (home.zone, home.tab, home.item, settings.zone, settings.cursor, settings.cf)
}

#[cfg(test)]
mod unpause_redraw_tests {
    use super::unpause_needs_redraw;

    #[test]
    fn showing_the_page_again_after_a_menu_forces_a_redraw() {
        // Page hidden under a game menu, then shown: one full redraw.
        assert!(unpause_needs_redraw(true, false));
        // Staying hidden or staying shown: no extra redraw from this edge.
        assert!(!unpause_needs_redraw(true, true));
        assert!(!unpause_needs_redraw(false, false));
        // Hiding the page is not a redraw of the page.
        assert!(!unpause_needs_redraw(false, true));
    }
}

#[cfg(test)]
mod suppression_bound_tests {
    use super::{suppression_expired, SUPPRESS_ORPHAN_LIMIT};
    use std::time::Duration;

    #[test]
    fn a_hidden_page_without_a_menu_is_shown_again_after_the_bound() {
        let short = Some(Duration::from_millis(300));
        let long = Some(SUPPRESS_ORPHAN_LIMIT);
        // An open menu over a game hides the page for as long as it is open.
        assert!(!suppression_expired(true, true, long));
        // The ordinary close hand-over is shorter than the bound.
        assert!(!suppression_expired(false, true, short));
        assert!(!suppression_expired(false, true, None));
        // No menu and no acknowledgement in time: the page comes back.
        assert!(suppression_expired(false, true, long));
        // A shown page has nothing to expire.
        assert!(!suppression_expired(false, false, long));
    }
}

#[cfg(test)]
mod clock_policy_tests {
    use super::clock_wanted;

    #[test]
    fn clock_runs_only_with_a_visible_shell_that_animates() {
        // Home with its animations, shell on screen: runs.
        assert!(clock_wanted(false, false, true, false));
        // A game covers the shell (or a game menu suppresses its page): stopped, whatever is playing.
        assert!(!clock_wanted(false, true, true, true));
        // Minimized or hidden window: stopped.
        assert!(!clock_wanted(true, false, true, true));
        // Another route with nothing playing: stopped; a playing cover or clip keeps it running.
        assert!(!clock_wanted(false, false, false, false));
        assert!(clock_wanted(false, false, false, true));
    }
}

#[cfg(test)]
mod page_focus_signature_tests {
    use super::*;

    #[test]
    fn list_moves_are_not_mistaken_for_header_boundaries() {
        let mut home = crate::screens::home::State::default();
        let mut settings = crate::screens::settings::State::default();
        home.zone = 2;
        for item in (1..12).rev() {
            home.item = item;
            let before = page_focus_signature(&home, &settings);
            home.item -= 1;
            assert_ne!(before, page_focus_signature(&home, &settings), "Home row {item}");
        }
        for cursor in (1..12).rev() {
            settings.cursor = cursor;
            let before = page_focus_signature(&home, &settings);
            settings.cursor -= 1;
            assert_ne!(before, page_focus_signature(&home, &settings), "Settings rail {cursor}");
        }
    }
}

impl App {
    pub fn new(ui: slint::Weak<AppWindow>, ds: Arc<DataSource>, lib: Arc<Library>, loader: Loader) -> App {
        let gidx = lib.games.iter().enumerate().map(|(i, g)| (g.app_id, i)).collect();
        let tabs = library::load_tabs(&lib);
        App {
            ui,
            ds,
            base: lib.clone(),
            lib,
            extras: Default::default(),
            bridge_recent: vec![],
            live: Live::default(),
            header_profile: Default::default(),
            game_overlay: None,
            game_overlay_requested: false,
            game_overlay_clearing: false,
            game_overlay_since: None,
            game_overlay_closing: None,
            suppress_orphan_since: None,
            overlay_diag: (None, false, false, false),
            loader,
            gidx,
            imgs: HashMap::new(),
            requested: HashSet::new(),
            lru: vec![],
            glow: HashMap::new(),
            pad_mode: true,
            router: Router::new(),
            last_guide: Instant::now() - Duration::from_secs(10),
            bg_game: None,
            bg_front_a: false,
            hm: home::State::default(),
            lb: library::State::new(tabs),
            search: Default::default(),
            gm: game::State::default(),
            dl: downloads::State::default(),
            sm: steam_menu::State::default(),
            dm: Default::default(),
            qm: qam::State::default(),
            se: settings::State::default(),
            focus_game: None,
            anim_delay: Timer::default(),
            anim_timer: Timer::default(),
            trim_timer: Timer::default(),
            pulse_t0: Instant::now(),
            kb_t0: Instant::now(),
            kb_tick: 0,
            anim_cache: HashMap::new(),
            anim_requested: HashSet::new(),
            hero_clips: ClipSlot::default(),
            logo_clips: ClipSlot::default(),
            anim_play: HashMap::new(),
            level_popup: crate::level::Popup::default(),
            account: crate::screens::account_switch::State::default(),
            install: Default::default(),
            clock_timer: Timer::default(),
            game_overlay_timer: Timer::default(),
            script_timer: Timer::default(),
            script: Default::default(),
            shot_n: 0,
        }
    }

    pub fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }
    pub fn games(&self) -> &[Game] {
        &self.lib.games
    }
    pub fn page(&self) -> i32 {
        self.router.page()
    }
    pub fn act(&self, a: Act) {
        if let Act::Install(id) = a {
            return crate::screens::defer(move |app| crate::screens::install::open(app, id));
        }
        if let Act::Controller(id) = a {
            return crate::screens::defer(move |app| crate::screens::controller::open(app, id));
        }
        if let Act::Properties(id) = a {
            // Properties is a native page now (screens/properties.rs); the steam:// page stays the bridge-less fallback only
            return crate::screens::defer(move |app| crate::screens::properties::open(app, id, None));
        }
        // Play/Resume cover (task 330): a real launch only; a dry run never starts the game.
        if let Act::Launch { app_id, .. } = a {
            if !actions::dryrun() {
                crate::launch_splash::begin(app_id);
            }
        }
        actions::run(a, &self.ds);
    }

    pub fn start(&mut self, cols: i32) {
        self.lb.cols = (cols.max(1) as usize).max(1);
        let ui = self.ui();
        home::wire_models(self, &ui);
        library::wire_models(self, &ui);
        downloads::wire_models(self, &ui);
        crate::screens::downloads_menu::wire_models(self, &ui);
        self.init_strings();
        self.sync_library_status();
        self.init_user();
        self.recompute_recent();
        self.rebuild_view(None);
        // Startup enters Home without navigate(): queue its row before the first input.
        crate::diag::log("home_art_refresh", serde_json::json!({"reason":"startup","rows":self.hm.recent.len()}));
        self.home_request_all();
        self.home_refresh();
        self.sync_lib();
        self.update_hint();
        self.build_menu();
        self.build_settings_rail();
        self.update_bg();

        self.tick_clock();
        let mut n = 0u32;
        self.clock_timer.start(TimerMode::Repeated, Duration::from_secs(5), move || {
            // Under a game the clock text is not drawn; `shell_signal` refreshes it on return.
            crate::with_app(|a| if !a.shell_hidden() { a.tick_clock() });
            n += 1;
            if n % 6 == 0 {
                // return freed image/frame buffers to the OS (glibc keeps per-thread arenas otherwise)
                unsafe { libc::malloc_trim(0) };
            }
        });
        qam::spawn_status_poller(self.ds.clone());
        crate::level::spawn_volume_watch(self.ds.clone());
        self.focus_changed_home();
        if let Ok(k) = std::env::var("DECKSHELL_KEYS") {
            self.script = k.split(',').map(|s| s.trim().to_string()).collect();
            self.script_timer.start(TimerMode::Repeated, Duration::from_millis(900), || {
                crate::with_app(|a| a.script_step());
            });
        }
    }

    /// All localized strings (steam-assets `loc`, Russian at runtime); screens own their part.
    fn init_strings(&mut self) {
        let ui = self.ui();
        home::init_strings(&ui);
        library::init_strings(&ui);
        game::init_strings(&ui);
        downloads::init_strings(&ui);
        qam::init_strings(&ui);
        settings::init_strings(&ui);
        crate::screens::media::init_strings(&ui);
        crate::screens::search::init_strings(&ui);
        let tb = ui.global::<TopBarState>();
        tb.set_search_placeholder(crate::assets::t_or("#SearchPlaceholder", tr_ref("search.placeholder")).into());
        // Steam shows this warning for an unformatted SD card, not mere presence.
        // Until that state is available, do not fabricate a warning.
        tb.set_has_sdcard(false);
    }

    fn script_step(&mut self) {
        let Some(tk) = self.script.pop_front() else {
            self.script_timer.stop();
            if std::env::var_os("DECKSHELL_DUMP_TREE").is_some() {
                crate::dumptree::dump_app(self);
            }
            if std::env::var("DECKSHELL_EXIT").is_ok() {
                let _ = slint::quit_event_loop();
            }
            return;
        };
        let a = match tk.as_str() {
            "up" => Action::Up,
            "down" => Action::Down,
            "left" => Action::Left,
            "right" => Action::Right,
            "a" => Action::A,
            "b" => Action::B,
            "x" => Action::X,
            "y" => Action::Y,
            "lb" => Action::LB,
            "rb" => Action::RB,
            "steam" => Action::Steam,
            "qam" => Action::Qam,
            // Deterministic existing script driver: preview a UI-only power
            // modal without synthesizing hardware holds or enabling actions.
            "power-menu" if crate::actions::dryrun() => {
                self.hk_power_menu();
                return;
            }
            "mock-running" if crate::actions::dryrun() && self.ds.is_mock() => {
                // Explicit demo fixture for offline menu navigation, never a
                // fallback for missing production running-app data.
                if let Some(id) = self.games().first().map(|g| g.app_id) {
                    self.on_backend(Update::Running(vec![id]));
                }
                return;
            }
            "search" => Action::Search,
            "menu" => Action::Menu,
            "view" => Action::View,
            "pad" => {
                self.set_pad(true);
                return;
            }
            "shot" => {
                if let Ok(prefix) = std::env::var("DECKSHELL_SHOT") {
                    if let Ok(buf) = self.ui().window().take_snapshot() {
                        let path = format!("{prefix}{}.png", self.shot_n);
                        self.shot_n += 1;
                        if let Some(img) = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec()) {
                            let _ = img.save(&path);
                            eprintln!("[deckshell] snapshot {path}");
                        }
                    }
                }
                return;
            }
            "wait" => return,
            "dump" => {
                crate::dumptree::dump_app(self);
                return;
            }
            s if s.starts_with("t:") => {
                for c in s[2..].chars() {
                    self.handle(Action::Char(c));
                }
                return;
            }
            _ => return,
        };
        self.handle(a);
    }

    fn tick_clock(&mut self) {
        let twelve = clock_is_12h();
        let s = chrono::Local::now().format(if twelve { "%-I:%M %p" } else { "%H:%M" }).to_string();
        let ui = self.ui();
        let tb = ui.global::<TopBarState>();
        tb.set_clock_12h(twelve);
        tb.set_clock(s.into());
        self.clock_sync();
        crate::screens::controller::refresh_connection(self);
        self.sync_game_overlay();
    }

    // ------------------------------------------------------------------ backend updates (steam-bridge worker)
    pub fn on_backend(&mut self, u: Update) {
        match u {
            Update::Connected(up) => {
                crate::diag::log("backend.connection",serde_json::json!({"up":up,"previous":self.live.bridge_up}));
                self.live.bridge_up = up;
                eprintln!("[deckshell] bridge {}", if up { "connected (primary data source)" } else { "disconnected (steam-data fallback)" });
            }
            Update::Library(d) => {
                self.ds.library_received();
                self.sync_library_status();
                crate::diag::log("backend.library",serde_json::json!({"apps":d.apps.len(),"art":d.art.len(),"recent":d.recent,"account_known":d.account!=0}));
                let (lib, extras) = crate::model::from_bridge(&self.base, &d);
                if d.account != 0 && self.base.active_user != Some(d.account) {
                    // Media, screenshots and controller file paths use base. Move
                    // that snapshot to the confirmed live identity as one unit.
                    self.base = Arc::new(lib.clone());
                    self.media_account_changed();
                    self.live.downloads = None;
                    self.live.system = None;
                    self.live.unread = 0;
                    self.ui().global::<TopBarState>().set_unread(0);
                }
                self.bridge_recent = d.recent.clone();
                self.apply_library(lib, extras);
            }
            Update::Running(ids) => {
                self.live.running = ids;
                self.sync_overlays();
                self.recompute_recent();
                self.home_refresh();
                self.refresh_game_page();
                self.update_bg();
                self.update_hint();
            }
            Update::Downloads(d) => {
                self.live.downloads = Some(*d);
                self.on_downloads_data();
                self.refresh_game_page();
            }
            Update::Notifications(n) => {
                self.live.unread = n.unviewed;
                self.ui().global::<TopBarState>().set_unread(n.unviewed as i32);
            }
            Update::System(s) => {
                self.apply_system(&s);
                self.live.system = Some(*s);
            }
            Update::Profile { steamid, name, sampled_at } => {
                self.update_header_profile(steamid, &name, sampled_at);
            }
        }
    }

    fn apply_system(&mut self, s: &steam_bridge::SystemInfo) {
        let ui = self.ui();
        let tb = ui.global::<TopBarState>();
        tb.set_wifi(if s.network.wifi_enabled { 1 } else { 0 });
        tb.set_wifi_strength(i32::from(s.network.wifi_strength.unwrap_or(0)));
        tb.set_wifi_offline(crate::screens::internet::header_offline(s.network.offline));
    }

    /// Replace the shown library (bridge snapshot), keeping focus/route where possible.
    fn apply_library(&mut self, lib: Library, extras: HashMap<u32, Extra>) {
        self.lib_read_filter();
        let keep = self.focused_lib_game().map(|i| self.games()[i].app_id);
        let home_keep = self.home_selected_id();
        // Art resolved by Steam (steam-bridge art) may differ from the file-based guess used before: drop cached images
        // whose source file changed so they are requested again from the new path.
        {
            let old: HashMap<u32, &steam_data::Art> = self.lib.games.iter().map(|g| (g.app_id, &g.art)).collect();
            let mut stale = Vec::new();
            for g in &lib.games {
                if let Some(o) = old.get(&g.app_id) {
                    for k in changed_art_kinds(o, &g.art) {
                        crate::diag::log("art.source_changed", serde_json::json!({"id":g.app_id,"kind":format!("{k:?}"),"old":art_path(o, k),"new":art_path(&g.art, k)}));
                        stale.push((g.app_id, k));
                    }
                    if o.capsule_animated != g.art.capsule_animated {
                        self.loader.invalidate(g.app_id, Kind::Capsule);
                        self.anim_play.remove(&g.app_id);
                        self.anim_cache.remove(&g.app_id);
                        self.anim_requested.remove(&g.app_id);
                    }
                }
            }
            if std::env::var_os("DECKSHELL_ARTLOG").is_some() {
                eprintln!("[art] library update: {} stale image keys (art source changed), e.g. {:?}", stale.len(), stale.iter().take(4).collect::<Vec<_>>());
            }
            for key in stale {
                if key.1 == Kind::Hero {
                    self.hero_clips.forget(key.0);
                    if self.bg_game == Some(key.0) { self.bg_game = None; }
                } else if key.1 == Kind::Logo {
                    self.logo_clips.forget(key.0);
                }
                self.loader.invalidate(key.0, key.1);
                self.imgs.remove(&key);
                self.requested.remove(&key);
                self.lru.retain(|k| *k != key);
            }
        }
        // Tile actions carry indices; a new snapshot may reorder them. Never let an
        // open popup target another game after refresh. Confirmation already owns appid.
        if matches!(self.lb.menu.as_ref().map(|m| m.kind), Some(crate::screens::library::MenuKind::Tile(_) | crate::screens::library::MenuKind::Manage(_))) {
            self.close_menu();
        }
        self.lib = Arc::new(lib);
        self.extras = Arc::new(extras);
        self.gidx = self.lib.games.iter().enumerate().map(|(i, g)| (g.app_id, i)).collect();
        self.logo_sync();
        self.recompute_recent_keeping(home_keep);
        self.rebuild_view(keep);
        self.search_catalog_changed();
        // Recents can change even when existing artwork paths stay identical.
        // request() deduplicates resident and pending images.
        if matches!(self.router.current(), Route::Home) {
            crate::diag::log("home_art_refresh", serde_json::json!({"reason":"library_snapshot","rows":self.hm.recent.len()}));
            self.home_request_all();
        }
        self.home_refresh();
        self.sync_lib();
        self.refresh_game_page();
        self.update_bg();
        self.on_downloads_data();
    }

    pub fn is_running(&self, id: u32) -> bool {
        self.live.running.contains(&id)
    }

    // ------------------------------------------------------------------ images
    pub fn card(&self, gi: usize) -> CardData {
        let g = &self.games()[gi];
        let frame = self.anim_play.get(&g.app_id).map(|(f, i, _)| f[*i].0.clone());
        let img = frame.as_ref().or_else(|| self.imgs.get(&(g.app_id, Kind::Capsule)));
        let wide = self.imgs.get(&(g.app_id, Kind::Header)).or_else(|| self.imgs.get(&(g.app_id, Kind::Hero)));
        CardData {
            id: g.app_id as i32,
            name: g.name.as_str().into(),
            img: img.cloned().unwrap_or_default(),
            has_img: img.is_some(),
            wide: wide.cloned().unwrap_or_default(),
            has_wide: wide.is_some(),
            installed: g.installed || g.kind == GameKind::Shortcut,
            glow: self.glow.get(&g.app_id).copied().unwrap_or(Color::from_argb_u8(0, 0, 0, 0)),
            caption: self.caption(g),
            x: 0.0,
            w: 161.0,
        }
    }

    fn touch(&mut self, key: (u32, Kind)) {
        let kind = key.1;
        let protect = self.bg_game;
        if let Some(old) = crate::art_cache::touch(&mut self.lru, key, |k| k.1 == kind, cap(kind), |k| Some(k.0) == protect) {
            crate::diag::log("art.evict", serde_json::json!({"id":old.0,"kind":format!("{:?}",old.1),"trigger_id":key.0,"capacity":cap(kind),"home_idx":self.hm.idx}));
            self.imgs.remove(&old);
            self.requested.remove(&old);
            if old.1 == Kind::Capsule {
                self.refresh_card(old.0);
            }
        }
    }

    pub fn request(&mut self, gi: usize, kind: Kind) {
        let lib = self.lib.clone();
        let g = &lib.games[gi];
        let key = (g.app_id, kind);
        if self.imgs.contains_key(&key) {
            if crate::diag::enabled() && self.hm.recent.contains(&gi) {
                crate::diag::log("art.request", serde_json::json!({"id":key.0,"kind":format!("{kind:?}"),"decision":"cache_hit","home_idx":self.hm.idx}));
            }
            self.touch(key);
            return;
        }
        if !self.requested.insert(key) {
            if crate::diag::enabled() && self.hm.recent.contains(&gi) {
                crate::diag::log("art.request", serde_json::json!({"id":key.0,"kind":format!("{kind:?}"),"decision":"already_requested","home_idx":self.hm.idx}));
            }
            return;
        }
        let path = art_path(&g.art, kind).cloned();
        crate::diag::log("art.request", serde_json::json!({"id":key.0,"kind":format!("{kind:?}"),"decision":if path.is_some(){"queue"}else{"no_source"},"path":path,"home_idx":self.hm.idx}));
        if let Some(path) = path {
            self.loader.push(Job::Still { id: g.app_id, kind, path });
        }
    }

    /// Same as `request`, for an app that is not in the shown library (the Storage list shows every installed app; the bridge
    /// can hide one from the library model). Loads the file the librarycache holds for it.
    pub fn request_path(&mut self, id: u32, kind: Kind, path: std::path::PathBuf) {
        let key = (id, kind);
        if self.imgs.contains_key(&key) || !self.requested.insert(key) { return; }
        self.loader.push(Job::Still { id, kind, path });
    }

    /// Debounced: hand freed decode buffers back to the OS once the image burst is over.
    fn schedule_trim(&self) {
        self.trim_timer.start(TimerMode::SingleShot, Duration::from_millis(1500), || unsafe {
            libc::malloc_trim(0);
        });
    }

    pub fn on_still(&mut self, id: u32, kind: Kind, res: Option<(Image, [u8; 3])>) {
        self.schedule_trim();
        crate::diag::log("art.apply", serde_json::json!({"id":id,"kind":format!("{kind:?}"),"success":res.is_some(),"size":res.as_ref().map(|(i,_)| (i.size().width,i.size().height)),"home_rows":self.hm.recent.iter().enumerate().filter_map(|(row,&gi)|self.games().get(gi).filter(|g|g.app_id==id).map(|_|row)).collect::<Vec<_>>(),"home_idx":self.hm.idx}));
        let Some((img, avg)) = res else { return };
        self.imgs.insert((id, kind), img);
        self.touch((id, kind));
        if kind == Kind::Capsule {
            // brighten a bit so the glow reads on dark backgrounds
            let b = |v: u8| ((v as u32 * 12 / 10).min(255)) as u8;
            self.glow.insert(id, Color::from_rgb_u8(b(avg[0]), b(avg[1]), b(avg[2])));
        }
        match kind {
            Kind::Capsule => {
                self.refresh_card(id);
                self.anim_sync();
            }
            Kind::Header => {
                if self.lb.search {
                    self.search_header_ready(id);
                    return;
                }
                self.home_refresh();
                self.rebuild_dl();
                // Render only: a finished load must not start another storage pass. A pass requests the whole visible
                // working set, which can evict and reload its own members; only input starts a pass.
                if matches!(self.router.current(), Route::Settings(page) if page == "storage") { self.render_settings(); }
            }
            Kind::Hero => {
                self.update_bg();
                self.rebuild_dl_art(id);
            }
            Kind::Logo => {
                self.update_game_art();
                self.rebuild_dl_art(id);
            }
        }
    }

    fn refresh_card(&mut self, id: u32) {
        let Some(&gi) = self.gidx.get(&id) else { return };
        self.refresh_lib_card(id, gi);
        if self.hm.recent.contains(&gi) {
            self.home_refresh();
        }
    }

    // ------------------------------------------------------------------ background hero
    fn bg_target(&self) -> Option<usize> {
        match self.router.current() {
            Route::Home => self.hm.recent.get(self.hm.idx).copied(),
            Route::App(id) => self.gidx.get(id).copied(),
            _ => None,
        }
    }

    pub fn update_bg(&mut self) {
        let ui = self.ui();
        let bd = ui.global::<BackdropState>();
        let Some(gi) = self.bg_target() else {
            bd.set_bg_on(false);
            self.bg_game = None;
            self.hero_clip_sync(None);
            return;
        };
        let id = self.games()[gi].app_id;
        self.request(gi, Kind::Hero);
        // the crop is per app and the transform is applied at draw time (no reload): the slot shown is `bg_front_a`
        let crop = hero_frame(id);
        if self.bg_game == Some(id) {
            if self.bg_front_a {
                bd.set_bg_a_crop(crop);
            } else {
                bd.set_bg_b_crop(crop);
            }
            bd.set_bg_on(true);
            self.hero_clip_sync(Some(id));
            return;
        }
        if let Some(img) = self.imgs.get(&(id, Kind::Hero)).cloned() {
            if self.bg_front_a {
                bd.set_bg_b(img);
                bd.set_bg_b_crop(crop);
            } else {
                bd.set_bg_a(img);
                bd.set_bg_a_crop(crop);
            }
            self.bg_front_a = !self.bg_front_a;
            bd.set_bg_use_a(self.bg_front_a);
            bd.set_bg_on(true);
            self.bg_game = Some(id);
            self.kb_t0 = Instant::now();
            self.hero_clip_sync(Some(id));
        } else {
            // Do not show the previous game while the selected hero is loading.
            bd.set_bg_on(false);
            self.bg_game = None;
            self.hero_clip_sync(None);
        }
    }

    // ------------------------------------------------------------------ animated hero banners and logos
    // Steam shows a hero and a game logo as plain <img>: an animated WebP/APNG/GIF (Steam grid custom image) plays forever,
    // looping, from the moment it is on screen. The still shows at once, the loader builds an on-disk clip (decimated to the
    // cover fps, kind size), then ONE frame at a time is swapped in (`clip_tick`). See clips.rs.
    fn clip_slot(&mut self, kind: Kind) -> &mut ClipSlot {
        // only Hero and Logo have clip slots; every caller passes one of them
        if kind == Kind::Logo { &mut self.logo_clips } else { &mut self.hero_clips }
    }

    /// Whether `id` is what the kind's image shows now: the background hero, or the game page's logo.
    fn clip_wanted(&self, kind: Kind, id: u32) -> bool {
        match kind {
            Kind::Logo => matches!(self.router.current(), Route::App(a) if *a == id),
            _ => self.bg_game == Some(id),
        }
    }

    /// Start the clip of `id` (if its image is animated) or stop the playing one (`None`).
    fn clip_sync(&mut self, kind: Kind, id: Option<u32>) {
        let Some(id) = id else {
            self.clip_slot(kind).play = None;
            return;
        };
        let slot = self.clip_slot(kind);
        if slot.play.as_ref().is_some_and(|p| p.id == id) || slot.req == Some(id) {
            return;
        }
        slot.play = None;
        let path = self.gidx.get(&id).and_then(|&gi| {
            let art = &self.games()[gi].art;
            if kind == Kind::Logo { art.logo.clone() } else { art.hero.clone() }
        });
        let Some(path) = path else { return };
        let slot = self.clip_slot(kind);
        if !*slot.animated.entry(id).or_insert_with(|| steam_bridge::art::is_animated(&path)) {
            return;
        }
        slot.req = Some(id);
        self.loader.push(Job::Clip { id, kind, path });
    }

    pub fn on_clip(&mut self, id: u32, kind: Kind, clip: Option<Arc<media::Clip>>) {
        let wanted = self.clip_wanted(kind, id);
        let slot = self.clip_slot(kind);
        if slot.req == Some(id) {
            slot.req = None;
        }
        // a clip of one frame is a still: nothing to play
        let Some(clip) = clip.filter(|c| c.len() > 1) else {
            slot.animated.insert(id, false);
            return;
        };
        if wanted {
            slot.play = Some(ClipPlay::new(id, clip, Instant::now()));
            self.clock_sync();
        }
    }

    /// Advance the clip of `kind`; returns true when a new frame was put on screen.
    fn clip_tick(&mut self, kind: Kind) -> bool {
        let Some(img) = self.clip_slot(kind).play.as_mut().and_then(|p| p.step(Instant::now())) else { return false };
        let ui = self.ui();
        if kind == Kind::Logo {
            let st = ui.global::<GameState>();
            st.set_logo_has(true);
            st.set_logo(img);
        } else {
            let bd = ui.global::<BackdropState>();
            if self.bg_front_a {
                bd.set_bg_a(img);
            } else {
                bd.set_bg_b(img);
            }
        }
        true
    }

    /// Hero clip follows the background hero (`update_bg`); the logo clip follows the route.
    fn hero_clip_sync(&mut self, id: Option<u32>) {
        self.clip_sync(Kind::Hero, id);
    }

    /// The game logo animates only while its game page is the current route; leaving the page stops it.
    pub fn logo_sync(&mut self) {
        let id = match self.router.current() {
            Route::App(id) => Some(*id),
            _ => None,
        };
        self.clip_sync(Kind::Logo, id);
        self.clock_sync();
    }

    // ------------------------------------------------------------------ router glue
    /// history.push + enter. Closes overlays like Navigator does (CloseSideMenus).
    pub fn navigate(&mut self, r: Route) {
        self.leave_game_overlay_for_page();
        if self.lb.search && r != Route::Search {
            self.close_search();
        }
        self.sm.open = false;
        self.qm.open = false;
        self.lb.sort_open = false;
        if self.router.push(r) {
            self.enter_route();
        } else {
            self.sync_overlays();
            self.update_hint();
        }
    }

    /// history.goBack
    pub fn go_back(&mut self) {
        self.leave_game_overlay_for_page();
        if self.router.back() {
            if self.lb.search && self.router.current() != &Route::Search {
                self.close_search();
            }
            self.enter_route();
        }
    }

    pub fn enter_route(&mut self) {
        let route = self.router.current().clone();
        eprintln!("[deckshell] route {}", route.path());
        self.ui().global::<Shell>().set_page(route.page());
        self.dl.timer.stop();
        if !matches!(route, Route::Settings(_)) {
            crate::screens::settings::controller_devices::leave(self);
            crate::screens::bluetooth::leave(self);
        }
        match &route {
            Route::Home => {
                self.recompute_recent();
                self.home_request_all();
                self.home_refresh();
                self.focus_changed_home();
            }
            Route::Library(id) => {
                self.select_tab_by_id(id);
                self.lib_focus_changed();
            }
            Route::Search => {
                self.enter_search();
                self.focus_changed(None);
            }
            Route::App(id) => {
                if let Some(&gi) = self.gidx.get(id) {
                    self.fill_game_page(gi);
                }
                self.focus_changed(None);
            }
            Route::Downloads => {
                self.dl.vy = 0.0;
                self.on_downloads_data();
                self.focus_changed(None);
            }
            Route::Settings(p) => {
                self.sync_settings(p);
                self.focus_changed(None);
            }
            Route::Media => self.focus_changed(None),
        }
        self.sync_overlays();
        self.update_bg();
        self.logo_sync();
        self.update_hint();
        self.build_menu();
        self.clock_sync();
    }

    // ------------------------------------------------------------------ overlays
    pub fn sync_overlays(&mut self) {
        // The downloads panel lives inside the side menu and closes with it.
        if !self.sm.open {
            self.dl_menu_reset();
        }
        self.sync_game_overlay();
        let ui = self.ui();
        crate::screens::qam::set_open_flag(self.qm.open);
        ui.global::<crate::LibraryState>().set_sort_open(self.lb.sort_open);
        self.sync_menu_visuals();
        let tb = ui.global::<TopBarState>();
        tb.set_search_active(self.lb.search);
    }

    /// The side menus slide in only once the compositor shows our window over the game. Before
    /// that the window is not on screen, and an animation started with the press was mostly over
    /// by the time the menu appeared.
    fn sync_menu_visuals(&self) {
        let waiting = self.game_overlay.is_some() && self.game_overlay_requested
            && overlay_open_must_wait(crate::gamescope::overlay_active(), self.game_overlay_since.map(|t| t.elapsed()));
        let ui = self.ui();
        ui.global::<crate::SteamMenuState>().set_open(self.sm.open && !waiting);
        ui.global::<crate::QamState>().set_open(self.qm.open && !waiting);
    }

    /// Presses are dropped while the closing menu slides out over the game: the shell still has
    /// input focus then, and the page under the menu is hidden.
    pub fn input_suspended(&self) -> bool {
        self.game_overlay_closing.is_some()
    }

    fn sync_library_status(&self) {
        self.ui().global::<Shell>().set_library_status(if self.ds.library_unavailable() {
            crate::assets::t_or("#DeckShell_LibraryUnavailable", tr_ref("library.library_unavailable")).into()
        } else { "".into() });
    }

    fn leave_game_overlay_for_page(&mut self) {
        self.game_overlay_closing = None;
        if self.game_overlay.take().is_some() || self.game_overlay_requested {
            self.game_overlay_clearing = true;
            self.game_overlay_requested = false;
            self.game_overlay_since = None;
            crate::gamescope::request_reclaim();
        }
    }

    /// The page is suppressed before requesting an overlay frame. Keep it suppressed
    /// on close until the gamescope worker observes the cleared overlay's new focus.
    fn sync_game_overlay(&mut self) {
        // An explicit native UI action (for example Store from the game menu)
        // already published STEAM_UI. Detach the menu's game anchor without
        // publishing GAME/HOME over that newer delegation.
        if self.game_overlay.is_some() && crate::gamescope::steam_ui_requested() {
            self.game_overlay = None;
            self.game_overlay_requested = false;
            self.game_overlay_since = None;
            self.game_overlay_clearing = true;
        }
        let mut requested = self.game_overlay;
        if let Some(id) = requested {
            if !self.live.running.contains(&id) {
                self.game_overlay_clearing = true;
                self.game_overlay = None;
                self.game_overlay_requested = false;
                self.game_overlay_since = None;
                self.sm.open = false;
                self.qm.open = false;
                self.ui().global::<crate::SteamMenuState>().set_open(false);
                self.ui().global::<crate::QamState>().set_open(false);
                crate::gamescope::request_reclaim();
                requested = None;
            } else if !self.overlay_surface_open() {
                // Hand the screen back only after the menu has slid out; an overlay that never
                // reached the screen has nothing to animate.
                let since = *self.game_overlay_closing.get_or_insert_with(Instant::now);
                let shown = self.game_overlay_requested && crate::gamescope::overlay_active();
                if !overlay_close_must_wait(shown, since.elapsed()) {
                    self.game_overlay_closing = None;
                    self.game_overlay_clearing = true;
                    self.game_overlay = None;
                    self.game_overlay_requested = false;
                    self.game_overlay_since = None;
                    crate::gamescope::request_game(id);
                    requested = None;
                }
            } else if !self.game_overlay_requested {
                self.game_overlay_closing = None;
                self.game_overlay_clearing = false;
                let ui = self.ui();
                crate::gamescope::set_page_suppressed(true);
                ui.global::<Shell>().set_game_overlay(true);
                crate::gamescope::request_overlay(id);
                ui.window().request_redraw();
                self.game_overlay_requested = true;
                self.game_overlay_since = Some(Instant::now());
            } else if crate::gamescope::overlay_rejected() || (!crate::gamescope::overlay_active()
                && self.game_overlay_since.is_some_and(|t| t.elapsed() > Duration::from_secs(3)))
            {
                self.game_overlay_clearing = true;
                // Failed alpha/focus negotiation is not a usable menu. Reclaim the
                // ordinary shell instead of leaving an invisible input overlay.
                self.game_overlay = None;
                self.game_overlay_requested = false;
                self.game_overlay_since = None;
                crate::gamescope::request_reclaim();
                requested = None;
            }
        }
        if self.game_overlay.is_none() || self.overlay_surface_open() {
            self.game_overlay_closing = None;
        }
        if self.game_overlay_clearing && crate::gamescope::overlay_close_complete() {
            self.game_overlay_clearing = false;
        }
        self.sync_menu_visuals();
        let mut transparent = requested.is_some() || self.game_overlay_clearing || crate::gamescope::overlay_window_flagged();
        // No menu is requested, yet the page is still hidden: that state is a hand-over and must end.
        if transparent && requested.is_none() {
            let since = *self.suppress_orphan_since.get_or_insert_with(Instant::now);
            if suppression_expired(false, true, Some(since.elapsed())) {
                crate::diag::log("page.suppress.timeout", serde_json::json!({
                    "after_ms": since.elapsed().as_millis() as u64, "clearing": self.game_overlay_clearing,
                    "flagged": crate::gamescope::overlay_window_flagged(), "close_complete": crate::gamescope::overlay_close_complete(),
                    "route": format!("{:?}", self.router.current()),
                }));
                self.game_overlay_clearing = false;
                crate::gamescope::drop_overlay_window_flag("ui timeout");
                self.suppress_orphan_since = None;
                transparent = false;
            }
        } else {
            self.suppress_orphan_since = None;
        }
        let was_suppressed = self.ui().global::<Shell>().get_game_overlay();
        let state = (requested, self.game_overlay_clearing, crate::gamescope::overlay_window_flagged(), transparent);
        if state != self.overlay_diag {
            self.overlay_diag = state;
            crate::diag::log("overlay.sync", serde_json::json!({
                "menu_game": requested, "clearing": state.1, "flagged": state.2, "page_hidden": transparent,
                "menu_open": self.sm.open, "qam_open": self.qm.open, "route": format!("{:?}", self.router.current()),
            }));
        }
        // The render notifier must see the page state before the property change is drawn.
        crate::gamescope::set_page_suppressed(transparent);
        self.ui().global::<Shell>().set_game_overlay(transparent);
        if transparent && !self.game_overlay_timer.running() {
            // Short period: this tick starts the slide-in and ends the slide-out hold.
            self.game_overlay_timer.start(TimerMode::Repeated, Duration::from_millis(16), || {
                crate::with_app(|app| app.sync_game_overlay());
            });
        } else if !transparent {
            self.game_overlay_timer.stop();
        }
        // The page under a menu is suppressed while `game_overlay` is set; the clock follows that edge.
        self.clock_sync();
        // The page came back from under a menu: redraw the whole shell now (clock text and frames refreshed too),
        // so the first shown frame is the page, not the menu alone.
        if unpause_needs_redraw(was_suppressed, transparent) {
            self.shell_signal();
        }
    }

    /// Resume only an existing running app; never dispatch another RunGame.
    pub fn resume_running_game(&mut self, id: u32) {
        if !game::can_resume_running_game(&self.live.running, id) { return; }
        if self.game_overlay == Some(id) && self.game_overlay_requested {
            // The menu is over this very game: close it the ordinary way, so it slides out and
            // `sync_game_overlay` then hands the screen back to the game.
            self.sm.open = false;
            self.qm.open = false;
            self.sync_overlays();
            self.update_hint();
            return;
        }
        self.game_overlay_clearing = self.game_overlay.is_some() || crate::gamescope::overlay_window_flagged();
        self.game_overlay = None;
        self.game_overlay_requested = false;
        self.game_overlay_since = None;
        self.sm.open = false;
        self.qm.open = false;
        crate::gamescope::request_game(id);
        self.sync_overlays();
        self.update_hint();
    }

    /// Called only after the running-game submenu's explicit exit confirmation.
    pub fn exit_running_game(&mut self, id: u32) {
        if !self.live.running.contains(&id) { return; }
        self.act(Act::Stop(id));
        self.sm.open = false;
        self.qm.open = false;
        self.sync_overlays();
        self.update_hint();
    }

    pub fn toast(&mut self, msg: &str) {
        crate::notifications::enqueue(&self.ui(), crate::notifications::Event::message(msg), !self.live.running.is_empty());
    }

    // ------------------------------------------------------------------ input
    pub fn set_pad(&mut self, on: bool) {
        if self.pad_mode != on {
            self.pad_mode = on;
            self.update_hint();
        }
    }
    pub fn handle_pad(&mut self, a: Action) { self.handle_pad_repeat(a, false); }

    pub fn handle_pad_repeat(&mut self, a: Action, is_repeat: bool) {
        if crate::anim::consume_input("pad", !is_repeat) {
            return;
        }
        self.set_pad(true);
        if crate::screens::settings::controller_devices::testing(self) { return; }
        self.handle_with_repeat(a, is_repeat);
    }

    pub fn on_key(&mut self, text: &str, ctrl: bool, _repeat: bool) -> bool {
        if crate::input::pad_busy() {
            // Steam "lizard mode" echo of a Deck button as a key: already handled via hidraw.
            return true;
        }
        if crate::anim::consume_input("keyboard", false) { return true; }
        use slint::platform::Key;
        let mut chars = text.chars();
        let Some(c) = chars.next() else { return false };
        if chars.next().is_some() {
            return false;
        }
        let k = |k: Key| -> char { k.into() };
        let searching = crate::screens::osk::wants_text(self) || crate::screens::controller::browser_keyboard_target() || crate::sgdb::wants_text() || crate::screens::properties::wants_text();
        let act = if c == k(Key::UpArrow) {
            Action::Up
        } else if c == k(Key::DownArrow) {
            Action::Down
        } else if c == k(Key::LeftArrow) {
            Action::Left
        } else if c == k(Key::RightArrow) {
            Action::Right
        } else if c == k(Key::Return) || c == '\n' || c == '\r' {
            if crate::screens::controller::pad_number_active() { Action::R2 } else { Action::A }
        } else if c == k(Key::Escape) {
            Action::B
        } else if c == k(Key::Backspace) {
            if searching { Action::Backspace } else { Action::B }
        } else if c == k(Key::F1) || c == '\t' {
            Action::Steam
        } else if c == k(Key::F2) {
            Action::Qam
        } else if c == k(Key::F9) {
            Action::Menu
        } else if c == k(Key::F10) && crate::screens::controller::is_open() {
            Action::View
        } else if c == k(Key::F3) {
            Action::X
        } else if c == k(Key::F4) {
            Action::Y
        } else if c == k(Key::PageUp) {
            Action::LB
        } else if c == k(Key::PageDown) {
            Action::RB
        } else if ctrl && (c == 'f' || c == '\u{6}') {
            Action::Search
        } else if searching {
            if c.is_control() || ctrl || ((c as u32) >= 0xF700 && (c as u32) <= 0xF8FF) {
                return false;
            }
            Action::Char(c)
        } else {
            match c {
                '`' => Action::Qam,
                '/' => Action::Search,
                ' ' => Action::A,
                'q' => Action::LB,
                'e' => Action::RB,
                'x' => Action::X,
                'y' => Action::Y,
                _ => return false,
            }
        };
        self.set_pad(false);
        if !crate::screens::account_switch::handle(self, act, _repeat)
            && !crate::screens::install::handle(self, act, _repeat) { self.handle(act); }
        true
    }

    /// Screen-local focus omitted from the older sound signature. It must participate in
    /// boundary detection too: a move inside a list is not an unhandled Up.
    fn page_focus_signature(&self) -> (i32, usize, usize, u8, usize, Option<usize>) {
        page_focus_signature(&self.hm, &self.se)
    }

    /// Everything that changes when the user moves; used to decide which sound Steam would play.
    fn sig(&self) -> [i64; 20] {
        [
            self.page() as i64, self.hm.idx as i64, self.lb.frow as i64, self.lb.fcol as i64, self.lb.zone as i64,
            self.sm.focus as i64, self.dl.focus as i64, self.qm.focus as i64, self.lb.sort_focus as i64, self.lb.tab as i64,
            self.sm.open as i64, self.qm.open as i64, self.lb.sort_open as i64, self.lb.search as i64, self.lb.query.len() as i64,
            self.gm.focus as i64, self.se.focus as i64, crate::sgdb::sig() ^ (crate::screens::properties::sig() << 24) ^ (crate::screens::controller::sig() << 40),
            // Modal focus is state too: without it a move inside the power menu or a
            // confirmation looked like "nothing happened".
            crate::screens::hotkeys::dispatch::sig() ^ (crate::screens::anim_page::sig() << 8) ^ (((crate::screens::internet::sig() << 12) ^ (crate::screens::bluetooth::sig() << 28)) as i64),
            self.sm.confirm.map_or(0, |_| 1 + self.sm.confirm_focus as i64),
        ]
    }

    pub fn handle(&mut self, a: Action) { self.handle_with_repeat(a, false); }

    fn handle_with_repeat(&mut self, a: Action, is_repeat: bool) {
        if crate::screens::account_switch::handle(self, a, is_repeat) { return; }
        if crate::screens::install::handle(self, a, is_repeat) { return; }
        // Виртуальная клавиатура открыта: ввод (в том числе физической клавиатуры) принадлежит ей.
        if self.osk_is_open() && self.osk_handle(a) { return; }
        if self.input_suspended() && !matches!(a, Action::Steam | Action::Qam) {
            return;
        }
        // While the launch cover is up, only guide/QAM act (they cancel it below).
        if crate::launch_splash::active() && !matches!(a, Action::Steam | Action::Qam) {
            return;
        }
        let before = self.sig();
        let media_before = (self.router.current() == &Route::Media).then(crate::screens::media::navigation_signature);
        let page_focus_before = self.page_focus_signature();
        let page_before = self.page();
        let scope_before = self.focus_scope();
        if matches!(a, Action::Up) && std::env::var_os("DECKSHELL_TEST_HID_SOCKET").is_some() { eprintln!("[task200-up] before home_zone={} home_item={} settings_zone={} settings_cursor={}", self.hm.zone, self.hm.item, self.se.zone, self.se.cursor); }
        let controller_before = (crate::screens::controller::is_open() && !self.osk_is_open()).then(crate::screens::controller::nav_snap);
        self.handle_inner(a, is_repeat);
        // Конфигуратор: звук по правилам стока считает сам экран (его фокус не виден общей сигнатуре).
        if let Some(snap) = controller_before.filter(|_| !self.osk_is_open()) {
            if let Some(snd) = crate::screens::controller::nav_sound(a, &snap) { play(snd); }
            return;
        }
        if matches!(a, Action::Up) && std::env::var_os("DECKSHELL_TEST_HID_SOCKET").is_some() { eprintln!("[task200-up] after home_zone={} home_item={} settings_zone={} settings_cursor={}", self.hm.zone, self.hm.item, self.se.zone, self.se.cursor); }
        let after = self.sig();
        let dir = matches!(a, Action::Up | Action::Down | Action::Left | Action::Right);
        // Steam: FailedNav when a D-pad press is unhandled, DefaultOk on A, BasicNav on every gamepad focus move
        // (docs/engine/sounds.json). The sound engine applies the 50 ms throttle / lowest-enum-wins rule.
        if before == after && page_focus_before == self.page_focus_signature() && media_before == (self.router.current() == &Route::Media).then(crate::screens::media::navigation_signature) {
            // Up with nothing left above the content: the cursor goes to the header (Steam: header items are focusable)
            if crate::nav::scope::header_entry_allowed(a, scope_before, self.focus_scope(), true)
                && (!matches!(self.router.current(), Route::Media) || crate::screens::media::grid_header_boundary()) && self.header_try_enter() {
                if std::env::var_os("DECKSHELL_TEST_HID_SOCKET").is_some() { eprintln!("[task200-up] header entered home_zone={} home_item={}", self.hm.zone, self.hm.item); }
                play(Snd::BasicNav);
                return;
            }
            match a {
                Action::A => play(Snd::DefaultOk),
                _ if dir && !self.qm.open => play(Snd::FailedNav),
                _ => {}
            }
            return;
        }
        let snd = if before[10] != after[10] {
            if after[10] != 0 { Snd::OpenSideMenu } else { Snd::CloseSideMenu }
        } else if before[11] != after[11] {
            if after[11] != 0 { Snd::OpenSideMenu } else { Snd::CloseSideMenu }
        } else if before[12] != after[12] || before[13] != after[13] {
            if after[12] + after[13] > 0 { Snd::ShowModal } else { Snd::HideModal }
        } else if page_before != after[0] as i32 {
            if after[0] == 2 { Snd::IntoGameDetail } else if page_before == 2 { Snd::OutOfGameDetail } else { Snd::DefaultOk }
        } else if before[9] != after[9] {
            Snd::ChangeTabs
        } else if before[14] != after[14] {
            Snd::Typing
        } else {
            Snd::BasicNav
        };
        if !(self.qm.open && matches!(a, Action::Left | Action::Right)) {
            play(snd);
        }
    }

    fn handle_inner(&mut self, a: Action, is_repeat: bool) {
        if self.hk_handle(a) {
            return;
        }
        if crate::screens::controller::is_open() {
            return crate::screens::controller::handle(self, a);
        }
        if crate::screens::controller_gyro::active(crate::screens::controller_gyro::Owner::Settings) {
            return crate::screens::controller_gyro::act(self, a);
        }
        if crate::screens::properties::is_open() {
            if !matches!(a, Action::Steam | Action::Qam) {
                return crate::screens::properties::handle(self, a);
            }
            crate::screens::properties::close(self);
        }
        if crate::sgdb::is_open() {
            if !matches!(a, Action::Steam | Action::Qam) {
                return crate::sgdb::handle(self, a);
            }
            crate::sgdb::close(self);
        }
        if crate::screens::chrome::hf() >= 0 {
            // The header belongs to a plain page; any surface above it takes the press.
            if self.surface_scope() != crate::nav::scope::Scope::Page {
                self.header_clear();
            } else if self.handle_header(a) {
                return;
            }
        }
        if self.lb.search && !self.sm.open && !self.qm.open && !self.lb.sort_open {
            return self.handle_search(a);
        }
        // STEAM / QAM: 300 ms debounce (engine/input.json)
        if matches!(a, Action::Steam | Action::Qam) {
            if self.last_guide.elapsed() < Duration::from_millis(300) {
                return;
            }
            self.last_guide = Instant::now();
            // Guide/QAM takes the screen back: a pending launch cover must not hide the menu.
            crate::launch_splash::end("guide");
            // Only an accepted press may publish compositor intent. In particular,
            // a debounced reopen during a pending close must not request HOME.
            if self.game_overlay.is_none() {
                self.game_overlay = crate::gamescope::focused_game()
                    .filter(|id| self.live.running.contains(id));
            }
            if self.game_overlay.is_none() { crate::gamescope::request_reclaim(); }
        }
        match a {
            Action::Steam => return self.toggle_side(),
            Action::Qam => return self.toggle_qam(),
            _ => {}
        }
        if self.lb.sort_open {
            return self.handle_sort(a);
        }
        if self.qm.open {
            return self.handle_qam(a, is_repeat);
        }
        if self.sm.open {
            return self.handle_side(a, is_repeat);
        }
        let route = self.router.current().clone();
        if crate::screens::settings::controller_devices::active(self) && self.se.zone == 1 && matches!(a, Action::Menu | Action::X) {
            return self.handle_settings(a);
        }
        // Settings → «Анимации» owns X / Y / LB / RB / START while its content has focus (task 412)
        if matches!(&route, Route::Settings(p) if p == crate::screens::internet::PAGE) && self.se.zone == 1 && (crate::screens::internet::owns(a)) {
            return self.handle_settings(a);
        }
        // Settings → «Bluetooth» (task 488) handles its own directions; B closes its dialog through the settings back path
        if matches!(&route, Route::Settings(p) if p == crate::screens::bluetooth::PAGE) && self.se.zone == 1 && !matches!(a, Action::B | Action::Search | Action::View) {
            return self.handle_settings(a);
        }
        if matches!(&route, Route::Settings(p) if p == crate::screens::anim_page::PAGE) && self.se.zone == 1 && !matches!(a, Action::B | Action::Search | Action::View) {
            return self.handle_settings(a);
        }
        if a == Action::Search || a == Action::View || (a == Action::Y && !matches!(route, Route::Library(_) | Route::Downloads | Route::Media)) {
            return self.open_search();
        }
        if a == Action::Menu && matches!(route, Route::App(_)) {
            return self.game_menu_open();
        }
        if a == Action::Menu && matches!(route, Route::Downloads) {
            return self.handle_downloads(a);
        }
        if a == Action::Menu && matches!(route, Route::Media) {
            return self.handle_media(a);
        }
        if a == Action::Menu && matches!(route, Route::Home) {
            return self.handle_home(a); // START: tile context menu (Играть/Возобновить, Добавить в избранное, ...)
        }
        if a == Action::Menu && matches!(route, Route::Library(_)) {
            return self.handle_library(a); // tile/START menu lives in the library screen
        }
        if a == Action::Menu {
            // START: game context menu -> we open the focused game's page
            let gi = match &route {
                Route::Home => self.hm.recent.get(self.hm.idx).copied(),
                Route::Library(_) if self.lb.zone == 1 => self.focused_lib_game(),
                _ => None,
            };
            if let Some(gi) = gi {
                self.open_game(gi);
            }
            return;
        }
        if a == Action::B {
            if self.game_back() {
                return;
            }
            if matches!(route, Route::Media) && self.media_back() {
                return;
            }
            if matches!(route, Route::Settings(_)) && self.settings_back() {
                return;
            }
            if matches!(route, Route::Home) && self.home_back() {
                return;
            }
            return self.go_back();
        }
        match route {
            Route::Home => self.handle_home(a),
            Route::Library(_) => self.handle_library(a),
            Route::Search => self.handle_search(a),
            Route::App(_) => self.handle_game(a),
            Route::Downloads => self.handle_downloads(a),
            Route::Settings(_) => self.handle_settings(a),
            Route::Media => self.handle_media(a),
        }
    }

    // ------------------------------------------------------------------ footer (loc tokens + Deck glyphs)
    pub fn hint(&self, key: &str, token: &str, kind: i32, kb: &str) -> Hint {
        let label = if token.starts_with('#') { tu(token) } else { token.to_uppercase() };
        if self.pad_mode {
            // the STEAM key keeps Steam's own text pill (the sd_button_steam glyph is a tiny logo)
            if let (Some(img), true) = (glyph_image(key), key != "steam") {
                return Hint { glyph: "".into(), label: label.into(), kind, icon: img, has_icon: true };
            }
            let g = match key {
                "a" | "b" | "x" | "y" => key.to_uppercase(),
                "steam" => "STEAM".into(),
                _ => "".into(),
            };
            return Hint { glyph: g.into(), label: label.into(), kind, icon: Image::default(), has_icon: false };
        }
        Hint { glyph: kb.into(), label: label.into(), kind: 0, icon: Image::default(), has_icon: false }
    }

    /// Single source of the footer hints: (pad key, loc token, glyph kind, keyboard label, action on click).
    pub fn hint_spec(&self) -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
        let a = ("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A);
        let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
        let opts = ("menu", "#ActionButtonLabelContextMenu", 2, "F9", Action::Menu);
        if crate::screens::controller::is_open() {
            return crate::screens::controller::hints();
        }
        if crate::screens::controller_gyro::active(crate::screens::controller_gyro::Owner::Settings) {
            return crate::screens::controller_gyro::hints();
        }
        if crate::screens::properties::is_open() {
            return crate::screens::properties::hints();
        }
        if crate::sgdb::is_open() {
            return crate::sgdb::hints();
        }
        if crate::screens::chrome::hf() >= 0 {
            return vec![("a", crate::screens::chrome::hint_token(), 1, "Enter", Action::A), b];
        }
        if self.qm.open {
            let mut hints = Vec::new();
            if let Some(label) = self.qam_bluetooth_options_hint() {
                hints.push(("menu", label, 2, "F9", Action::Menu));
            }
            hints.extend([("a", self.qam_select_hint(), 1, "Enter", Action::A), b]);
            return hints;
        }
        if self.sm.open || self.lb.search || self.lb.sort_open {
            return vec![a, b];
        }
        if crate::screens::settings::controller_devices::active(self) && self.se.zone == 1 {
            return crate::screens::settings::controller_devices::hints(self);
        }
        if matches!(self.router.current(), Route::Settings(p) if p == crate::screens::internet::PAGE) && self.se.zone == 1 {
            return crate::screens::internet::hints();
        }
        if matches!(self.router.current(), Route::Settings(p) if p == crate::screens::bluetooth::PAGE) && self.se.zone == 1 {
            return crate::screens::bluetooth::hints();
        }
        if matches!(self.router.current(), Route::Settings(p) if p == crate::screens::anim_page::PAGE) && self.se.zone == 1 {
            return crate::screens::anim_page::hints();
        }
        match self.router.current() {
            Route::Home => vec![opts, a, b],
            Route::Downloads => {
                // как в стоке: A на герое — «Приостановить загрузку» / «Продолжить загрузку»
                let on_hero = self.dl.lines.get(self.dl.focus).is_some_and(|l| l.kind == 1) && self.dl.col == 1;
                let paused = self.live.downloads.as_ref().and_then(|d| d.overview.paused) == Some(true);
                let a_label = match (on_hero, paused) {
                    (true, true) => crate::screens::stock_hint("#Downloads_Resume_Tooltip", "downloads.resume_download"),
                    (true, false) => crate::screens::stock_hint("#Downloads_Pause_Tooltip", "downloads.pause_download"),
                    _ => "#ActionButtonLabelSelect",
                };
                vec![("y", crate::screens::stock_hint("#Downloads_ChangeDevice", "downloads.switch_device"), 1, "F4", Action::Y), opts, ("a", a_label, 1, "Enter", Action::A), b]
            }
            Route::Media if self.media_selecting() || self.media_batch_busy() => self.media_selection_hints(),
            Route::Media => vec![("x", tr_ref("footer.select_game"), 1, "F3", Action::X), ("y", tr_ref("footer.actions"), 1, "F4", Action::Y), ("menu", "#MediaPage_Filter", 2, "F9", Action::Menu), a, b],
            Route::Library(_) => vec![("x", self.filter_token(), 1, "F3", Action::X), ("y", "#ActionButtonLabelSort", 1, "F4", Action::Y), opts, a, b],
            Route::App(id) if self.is_running(*id) => vec![("x", "#GameAction_Stop", 1, "F3", Action::X), a, b],
            _ => vec![a, b],
        }
    }

    pub fn update_hint(&self) {
        let left = if self.pad_mode { self.hint("steam", "#ActionButtonLabelMenu", 3, "") } else { self.hint("steam", "#ActionButtonLabelMenu", 0, "F1") };
        let r: Vec<Hint> = self.hint_spec().into_iter().map(|(k, t, kind, kb, _)| self.hint(k, t, kind, kb)).collect();
        let ui = self.ui();
        let f = ui.global::<FooterState>();
        f.set_left_hint(left);
        f.set_hints(ModelRc::new(VecModel::from(r)));
    }

    // ------------------------------------------------------------------ animated covers
    // Steam plays animated capsules continuously (plain <img>). Budget: only games on screen (Home recent tiles / library
    // cards whose still is resident), at most ANIM_MAX players, 200x300 frames decoded once by the loader and kept in anim_cache.
    const ANIM_MAX: usize = 12;
    const ANIM_BYTES: usize = 32 << 20;

    pub fn focus_changed(&mut self, id: Option<u32>) {
        if matches!(self.router.current(), Route::Home) && self.focus_game != id {
            // The selected Home card starts its own delayed outline pulse.
            self.pulse_t0 = Instant::now();
            self.ui().global::<Shell>().set_pulse(1.0);
        }
        self.focus_game = id;
        self.anim_sync();
    }

    fn anim_wanted(&self) -> Vec<u32> {
        let games = self.games();
        let cands: Vec<usize> = match self.router.current() {
            Route::Home => self.hm.recent.clone(),
            Route::Library(_) => {
                // only rows near the focused one are on screen
                let r0 = self.lb.frow.saturating_sub(1);
                self.lb.flat.iter().skip(r0).take(5).flatten().copied().collect()
            }
            _ => vec![],
        };
        let mut v: Vec<u32> = vec![];
        for gi in cands {
            let Some(g) = games.get(gi) else { continue };
            if g.art.capsule_animated.is_some() && self.imgs.contains_key(&(g.app_id, Kind::Capsule)) {
                v.push(g.app_id);
                if v.len() >= Self::ANIM_MAX {
                    break;
                }
            }
        }
        v
    }

    /// Reconcile players with the on-screen set; request frames for missing ones.
    pub fn anim_sync(&mut self) {
        let want = self.anim_wanted();
        let gone: Vec<u32> = self.anim_play.keys().copied().filter(|i| !want.contains(i)).collect();
        for id in gone {
            crate::diag::log("art.animation.stop",serde_json::json!({"id":id,"reason":"not_wanted"}));
            self.anim_play.remove(&id);
            self.refresh_card(id);
        }
        self.anim_cache.retain(|k, _| want.contains(k));
        self.anim_requested.retain(|k| want.contains(k) || self.anim_cache.contains_key(k));
        for id in want {
            if self.anim_play.contains_key(&id) {
                continue;
            }
            if let Some(f) = self.anim_cache.get(&id).cloned() {
                crate::diag::log("art.animation.start",serde_json::json!({"id":id,"frames":f.len(),"home_idx":self.hm.idx}));
                self.anim_play.insert(id, (f, 0, Instant::now()));
                self.refresh_card(id);
            } else if self.anim_requested.insert(id) {
                if std::env::var_os("DECKSHELL_ARTLOG").is_some() {
                    eprintln!("[anim] request {id}");
                }
                if let Some(path) = self.gidx.get(&id).and_then(|&gi| self.games()[gi].art.capsule_animated.clone()) {
                    crate::diag::log("art.animation.request",serde_json::json!({"id":id,"path":path}));
                    self.loader.push(Job::Anim { id, path });
                }
            }
        }
        self.clock_sync();
    }

    /// The single UI clock (25 Hz): focus-outline pulse, ken-burns and animated-cover frames. Slint's own infinite animations
    /// would redraw the whole window at display rate (90 fps here); this caps redraws and only runs when something is visible.
    fn clock_needed(&self) -> bool {
        let ui = self.ui();
        let w = ui.window();
        clock_wanted(
            w.is_minimized() || !w.is_visible(),
            self.shell_hidden(),
            matches!(self.router.current(), Route::Home | Route::Library(_)),
            !self.anim_play.is_empty() || self.hero_clips.play.is_some() || self.logo_clips.play.is_some(),
        )
    }

    /// Nothing of the shell is on screen: a game owns the screen, or the game menu's page is suppressed under it.
    /// The clock, the clock text and every animation stop here; `shell_signal` restarts them.
    pub fn shell_hidden(&self) -> bool {
        self.game_overlay.is_some() || crate::gamescope::shell_covered()
    }

    pub fn clock_sync(&mut self) {
        if !self.clock_needed() {
            if self.anim_timer.running() {
                CLOCK_STOPS.fetch_add(1, Ordering::Relaxed);
                crate::diag::log("clock.pause", serde_json::json!({"paused": true, "menu_game": self.game_overlay,
                    "covered": crate::gamescope::shell_covered()}));
            }
            self.anim_timer.stop();
        } else if !self.anim_timer.running() {
            CLOCK_STARTS.fetch_add(1, Ordering::Relaxed);
            crate::diag::log("clock.pause", serde_json::json!({"paused": false}));
            self.anim_timer.start(TimerMode::Repeated, Duration::from_millis(crate::loader::ANIM_STEP_MS as u64), || {
                crate::with_app(|a| a.anim_tick());
            });
        }
    }

    /// The shell's visibility may have changed (Guide, Home, Steam, a game edge observed by the gamescope worker):
    /// resume now, not on the next clock tick. Runs on the UI thread through `gamescope::wake_ui`.
    pub fn shell_signal(&mut self) {
        SHELL_SIGNALS.fetch_add(1, Ordering::Relaxed);
        self.clock_sync();
        if !self.shell_hidden() {
            // Refresh the clock text and the animated frames once, so the first shown frame is current.
            self.tick_clock();
            self.anim_tick();
        }
        // Always one new frame: a HOME reveal after a game waits for it (gamescope `reveal_waiting`), even while a
        // game menu still hides the page and the frame shows only the menu.
        self.ui().window().request_redraw();
    }

    pub fn on_anim(&mut self, id: u32, frames: Option<AnimFrames>) {
        crate::diag::log("art.animation.decoded",serde_json::json!({"id":id,"frames":frames.as_ref().map(|v|v.len()).unwrap_or(0)}));
        self.schedule_trim();
        if std::env::var_os("DECKSHELL_ARTLOG").is_some() {
            eprintln!("[anim] {id}: {} frames", frames.as_ref().map(|f| f.len()).unwrap_or(0));
        }
        let Some(frames) = frames.filter(|f| !f.is_empty()) else { return };
        let v: Rc<Vec<(Image, u32)>> = Rc::new(frames.into_iter().map(|(b, d)| (Image::from_rgba8(b), d)).collect());
        self.anim_cache.insert(id, v);
        // byte budget for decoded cover frames (CPU copy + GPU texture each): drop players far from the focus first
        let bytes = |m: &HashMap<u32, Rc<Vec<(Image, u32)>>>| -> usize { m.values().map(|f| f.iter().map(|(i, _)| { let s = i.size(); (s.width * s.height * 4) as usize }).sum::<usize>()).sum() };
        while bytes(&self.anim_cache) > Self::ANIM_BYTES {
            let victim = self.anim_cache.keys().copied().find(|k| Some(*k) != self.focus_game && *k != id);
            let Some(v) = victim else { break };
            self.anim_cache.remove(&v);
            self.anim_play.remove(&v);
            self.refresh_card(v);
        }
        self.anim_sync();
    }

    fn anim_tick(&mut self) {
        if !self.clock_needed() {
            self.anim_timer.stop();
            return;
        }
        CLOCK_TICKS.fetch_add(1, Ordering::Relaxed);
        let ui = self.ui();
        let sh = ui.global::<Shell>();
        let page = self.router.current().page();
        let cards = matches!(page, 0 | 1);
        let focused_cards = cards && !self.sm.open && !self.qm.open;
        let now = Instant::now();
        let mut due = vec![];
        for (id, (f, i, t)) in self.anim_play.iter_mut() {
            if now.duration_since(*t).as_millis() as u32 + 3 >= f[*i].1 {
                *i = (*i + 1) % f.len();
                *t = now;
                due.push(*id);
            }
        }
        let hero_due = self.clip_tick(Kind::Hero);
        let logo_due = self.clip_tick(Kind::Logo);
        // outline pulse + ken-burns refresh at 10 Hz, or with any cover/hero frame that redraws the window anyway
        self.kb_tick = self.kb_tick.wrapping_add(1);
        let clip_playing = self.hero_clips.play.is_some() || self.logo_clips.play.is_some();
        let step = !due.is_empty() || hero_due || logo_due || (self.anim_play.is_empty() && !clip_playing && self.kb_tick % 5 == 0);
        if focused_cards && step && sh.get_kenburns() {
            // 1.0 -> 0.4 -> 1.0, ease-in-out, 600ms half period (CSS alternate)
            let elapsed = self.pulse_t0.elapsed().as_millis();
            let elapsed = if page == 0 { elapsed.saturating_sub(400) } else { elapsed };
            let ph = (elapsed % 1200) as f32 / 1200.0;
            let tri = if ph < 0.5 { ph * 2.0 } else { 2.0 - ph * 2.0 };
            sh.set_pulse(1.0 - 0.6 * tri * tri * (3.0 - 2.0 * tri));
            // ken-burns scale 1.0 <-> 1.1 over 25 s, linear alternate; hero only on Home, refreshed at 12.5 Hz
            if page == 0 {
                let ph = (self.kb_t0.elapsed().as_millis() % 50_000) as f32 / 50_000.0;
                let tri = if ph < 0.5 { ph * 2.0 } else { 2.0 - ph * 2.0 };
                sh.set_kb(1.0 + 0.1 * tri);
            }
        }
        for id in due {
            self.set_card_image(id);
        }
    }

    /// Swap only the image of one card row (no caption/glow/model rebuild).
    fn set_card_image(&mut self, id: u32) {
        let Some((f, i, _)) = self.anim_play.get(&id) else { return };
        let img = f[*i].0.clone();
        let Some(&gi) = self.gidx.get(&id) else { return };
        if matches!(self.router.current(), Route::Home) {
            if let Some(pos) = self.hm.recent.iter().position(|&g| g == gi) {
                let m = self.hm.recent_model.clone();
                let mut c = m.row_data(pos).unwrap();
                c.img = img;
                c.has_img = true;
                m.set_row_data(pos, c);
            }
        } else if let Some(&(r, c)) = self.lb.view_pos.get(&id) {
            if let Some(Some(m)) = self.lb.card_rows.get(r).and_then(|&f| self.lb.flat_models.get(f)) {
                if c < m.row_count() {
                    let mut cd = m.row_data(c).unwrap();
                    cd.img = img;
                    cd.has_img = true;
                    m.set_row_data(c, cd);
                }
            }
        }
    }
}

#[cfg(test)]
mod art_path_tests {
    use super::*;
    #[test]
    fn fallback_changes_invalidate_effective_path_in_both_directions() {
        let old = steam_data::Art { hero: Some("old".into()), ..Default::default() };
        let new = steam_data::Art { hero: Some("new".into()), ..Default::default() };
        assert!(changed_art_kinds(&old, &new).any(|k| k == Kind::Header));
        let old = steam_data::Art { header: Some("old".into()), ..Default::default() };
        let new = steam_data::Art { header: Some("new".into()), ..Default::default() };
        assert!(changed_art_kinds(&old, &new).any(|k| k == Kind::Hero));
    }
}

/// 12-hour clock when the system time locale (LC_ALL, LC_TIME, LANG) is one that uses it (en_US, en_CA, en_AU, en_IN, ...); 24-hour otherwise.
fn clock_is_12h() -> bool {
    let loc = ["LC_ALL", "LC_TIME", "LANG"].iter().filter_map(|k| std::env::var(k).ok()).find(|v| !v.is_empty());
    loc.is_some_and(|l| locale_uses_12h(&l))
}

fn locale_uses_12h(locale: &str) -> bool {
    let l = locale.split(['.', '@']).next().unwrap_or("");
    matches!(l, "en_US" | "en_CA" | "en_AU" | "en_NZ" | "en_IN" | "en_PH" | "es_MX" | "es_US" | "ar_EG" | "hi_IN" | "ko_KR")
}

#[cfg(test)]
mod clock_locale_tests {
    use super::locale_uses_12h;

    #[test]
    fn twelve_hour_follows_the_system_locale() {
        assert!(locale_uses_12h("en_US.UTF-8"));
        assert!(!locale_uses_12h("pl_PL.UTF-8"));
        assert!(!locale_uses_12h("ru_RU.UTF-8"));
        assert!(!locale_uses_12h("C"));
    }
}
