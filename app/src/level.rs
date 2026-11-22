//! Brightness and volume level popup and the one level model behind it.
//!
//! Hardware changes go through `App::level_changed`; QAM writes update the same model silently. The
//! QAM slider rows and the popup both read the same `qm.st` values, so a change appears in both
//! at once. In gamescope the popup uses its own passive external-overlay window. It shows only for
//! hardware actions (STEAM / "..." + stick, physical volume buttons);
//! QAM slider changes update the rows silently. Its look is the owner's DellyVolume CSS Loader theme
//! (see `ui/widgets/level_bar.slint`). It fades and slides over 0.5 s and hides about 1 s after the last change.
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, TimerMode};

use crate::actions;
use crate::app::App;
use crate::data::DataSource;
use crate::{Shell, AppWindow, LevelPopupWindow};

/// How long the popup stays after the last change (Decky: displayDuration 1000 ms).
pub const POPUP_MS: u64 = 1000;

/// Existing LevelBar slide/fade duration. Unmap only after its exit finishes.
const FADE_MS: u64 = 500;
pub const POPUP_TITLE: &str = "DeckShell Level";

thread_local! { pub(crate) static CREATING_POPUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Set only the popup's native attributes, before mapping. Override-redirect/ inactive creation prevents
/// even a transient WM focus request; EXTERNAL_OVERLAY and empty pointer hit testing follow on show.
pub fn select_overlay_backend() -> Result<(), slint::PlatformError> {
    if !crate::gamescope::rec_badge_external() { return Ok(()); }
    slint::BackendSelector::new().with_winit_window_attributes_hook(popup_window_attributes).select()
}

fn popup_window_attributes(attrs: slint::winit_030::winit::window::WindowAttributes) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit::platform::x11::WindowAttributesExtX11;
    if CREATING_POPUP.get() { attrs.with_active(false).with_override_redirect(true) } else { attrs }
}

fn icon_name(kind: Kind, value: f32, muted: bool) -> &'static str {
    match kind {
        Kind::Brightness => "brightness",
        Kind::Volume if muted || value <= 0.0 => "volume.muted",
        Kind::Volume => "volume",
    }
}

pub struct Popup {
    window: Option<LevelPopupWindow>,
    external: bool,
    hold: std::rc::Rc<slint::Timer>,
    retire: std::rc::Rc<slint::Timer>,
}

impl Default for Popup {
    fn default() -> Self { Self::new(crate::gamescope::rec_badge_external()) }
}

impl Popup {
    fn new(external: bool) -> Self {
        Self { window: None, external, hold: Default::default(), retire: Default::default() }
    }

    fn show(&mut self, main: &AppWindow, kind: Kind, value: f32, muted: bool) {
        let external = self.external;
        self.retire.stop();
        let shell = main.global::<Shell>();
        let icon = crate::assets::icon_image(icon_name(kind, value, muted));
        shell.set_level_kind(kind.code());
        shell.set_level_value(value.clamp(0.0, 1.0));
        shell.set_level_icon(icon.clone());
        shell.set_level_external(external);
        shell.set_level_shown(true);
        if external && !main.global::<crate::AnimState>().get_active() {
            if self.window.is_none() {
                CREATING_POPUP.set(true);
                let window = LevelPopupWindow::new();
                CREATING_POPUP.set(false);
                let window = window.expect("create level popup");
                let weak = window.as_weak();
                let main_weak = main.as_weak();
                let hold = self.hold.clone();
                let retire = self.retire.clone();
                // This callback owns no App borrow and works even while the page clock is paused.
                main.global::<crate::AnimState>().on_level_cover_changed(move || {
                    if let Some(main) = main_weak.upgrade() {
                        if main.global::<crate::AnimState>().get_active() {
                            hold.stop();
                            retire.stop();
                            main.global::<Shell>().set_level_shown(false);
                            if let Some(win) = weak.upgrade() { win.set_shown(false); let _ = win.hide(); }
                        }
                    }
                });
                self.window = Some(window);
            }
            let win = self.window.as_ref().unwrap();
            win.window().set_size(main.window().size());
            win.set_kind(kind.code());
            win.set_value(value.clamp(0.0, 1.0));
            win.set_glyph(icon);
            // Map transparent, disable native pointer hit testing and mark before showing any content.
            // Repeated presses only change properties/timers, never reconnect to X11 or remap the window.
            let mapped = if win.window().is_visible() { Ok(()) } else {
                win.show().map_err(|e| e.to_string()).and_then(|_| crate::gamescope::mark_level_popup(win.window()))
            };
            if let Err(e) = mapped {
                let _ = win.hide();
                eprintln!("[deckshell] level overlay: {e}");
            } else {
                win.set_shown(true);
            }
        }
        let main = main.as_weak();
        let win = self.window.as_ref().map(ComponentHandle::as_weak);
        let retire = self.retire.clone();
        self.hold.start(TimerMode::SingleShot, Duration::from_millis(POPUP_MS), move || {
            if let Some(main) = main.upgrade() { main.global::<Shell>().set_level_shown(false); }
            if let Some(win) = win.as_ref().and_then(slint::Weak::upgrade) {
                win.set_shown(false);
                let weak = win.as_weak();
                retire.start(TimerMode::SingleShot, Duration::from_millis(FADE_MS), move || {
                    if let Some(win) = weak.upgrade() { let _ = win.hide(); }
                });
            }
            crate::diag::log("level.popup", serde_json::json!({"state": "hide"}));
        });
    }

    fn update_value(&self, value: f32) {
        if let Some(win) = &self.window { win.set_value(value); }
    }
}

/// How long a QAM volume write may still come back as a pactl echo. A drag writes many values and the
/// echoes of the early ones can arrive after the newest value was already stored.
const OWN_VOLUME_MS: u64 = 2000;

/// Volumes the QAM slider wrote, newest last. Their echoes are not hardware presses and must not show the popup.
static OWN_VOLUME: Mutex<Vec<(f32, Instant)>> = Mutex::new(Vec::new());

/// Pure check: is `value` (within one sink percent) a recent write of ours?
fn is_own_echo(writes: &[(f32, Instant)], value: f32, now: Instant) -> bool {
    writes.iter().any(|&(w, at)| now.duration_since(at) < Duration::from_millis(OWN_VOLUME_MS) && (w - value).abs() < 0.011)
}

/// Remembers a volume the QAM slider just wrote (see `is_own_echo`).
pub fn note_own_volume(value: f32) {
    let now = Instant::now();
    let mut writes = OWN_VOLUME.lock().unwrap();
    writes.retain(|&(_, at)| now.duration_since(at) < Duration::from_millis(OWN_VOLUME_MS));
    writes.push((value, now));
    if writes.len() > 64 { writes.remove(0); }
}

/// One slider step on a 0..1 range (Steam's `dpadStep` 0.01). Brightness, the QAM slider and the STEAM+stick
/// chord all step by this, so their values and speeds match the stock slider.
pub const SLIDER_STEP: f32 = 0.01;

/// Stock Steam slider hold (SliderField.OnGamepadDirection). A repeat moves `SLIDER_STEP * 2^floor(t / interval)`,
/// capped at 0.05. `t` counts ms from the first repeat (not the press). The interval shrinks from 500 to 250 ms
/// over the first 2 s. `elapsed_ms` is 0 for the press itself.
pub fn hold_step(elapsed_ms: f32) -> f32 {
    let t = elapsed_ms.clamp(0.0, 2000.0);
    let interval = 500.0 - t * 0.125;
    (SLIDER_STEP * 2.0_f32.powf((elapsed_ms / interval).floor())).min(0.05)
}

/// Next brightness of one chord step. Never below the stock minimum (`display::FLOOR`): a step down
/// from there stays on the floor instead of driving the backlight to 0.
pub fn chord_next(cur: f32, up: bool, step: f32) -> f32 {
    crate::display::clamp(if up { cur + step } else { cur - step })
}

#[cfg(test)]
mod chord_tests {
    use super::{chord_next, hold_step};
    use crate::display::FLOOR;

    #[test]
    fn five_second_hold_down_stops_at_the_floor() {
        let mut cur = 0.5;
        for ms in (0..=5000).step_by(20) {
            cur = chord_next(cur, false, hold_step(ms as f32));
            assert!(cur >= FLOOR, "fell below the floor at {ms} ms: {cur}");
        }
        assert_eq!(cur, FLOOR);
    }

    #[test]
    fn stepping_up_from_the_floor_moves_off_it() {
        assert!(chord_next(FLOOR, true, 0.01) > FLOOR);
    }
}

#[cfg(test)]
mod hold_tests {
    use super::{hold_step, SLIDER_STEP};

    #[test]
    fn press_and_first_repeat_move_one_step() {
        assert_eq!(hold_step(0.0), SLIDER_STEP);
    }

    #[test]
    fn repeat_acceleration_follows_steam_curve() {
        assert_eq!(hold_step(500.0), 0.02);
        assert_eq!(hold_step(2000.0), 0.05);
        assert_eq!(hold_step(10_000.0), 0.05);
    }

    #[test]
    fn steps_never_shrink_during_a_hold() {
        let mut prev = 0.0;
        for ms in (0..=3000).step_by(50) {
            let s = hold_step(ms as f32);
            assert!(s >= prev, "step fell at {ms} ms");
            prev = s;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Brightness,
    Volume,
}

impl Kind {
    fn code(self) -> i32 {
        match self {
            Kind::Brightness => 0,
            Kind::Volume => 1,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Kind::Brightness => "brightness",
            Kind::Volume => "volume",
        }
    }
}

impl App {
    /// One entry point for every level change: STEAM / "..." + stick, the QAM slider, and external
    /// (physical volume) changes. Stores the value, shows the popup, and refreshes the QAM slider rows.
    /// `muted` is the sink's mute state; brightness ignores it.
    pub(crate) fn level_changed(&mut self, kind: Kind, value: f32, muted: bool, source: &'static str) {
        let v = match kind {
            Kind::Brightness => crate::display::clamp(value),
            Kind::Volume => value.clamp(0.0, 1.0),
        };
        match kind {
            Kind::Brightness => self.qm.st.brightness = Some(v),
            Kind::Volume => self.qm.st.volume = Some((v, muted)),
        }
        crate::diag::log("level.change", serde_json::json!({"kind": kind.name(), "value": v, "source": source}));
        self.level_show(kind, v);
        if self.qm.open {
            self.qam_rebuild(false);
            self.qam_refresh_light();
        }
    }

    /// Popup only (the caller already stored the value): shows the bar and restarts the hide timer.
    pub(crate) fn level_show(&mut self, kind: Kind, value: f32) {
        let muted = self.qm.st.volume.is_some_and(|(_, muted)| muted);
        self.level_popup.show(&self.ui(), kind, value, muted);
    }

    /// Keep the visible popup on accepted brightness readback without opening/extending it.
    pub(crate) fn level_update_value(&self, value: f32) {
        self.ui().global::<Shell>().set_level_value(value);
        self.level_popup.update_value(value);
    }

    /// A level read back from the system (volume watcher). Skipped while our own write is pending or the
    /// value is unchanged, so a write echo never moves the bar back to a stale value.
    pub(crate) fn level_external(&mut self, kind: Kind, value: f32, muted: bool) {
        if actions::level_in_flight(kind == Kind::Brightness).is_some() {
            return;
        }
        if kind == Kind::Volume && is_own_echo(&OWN_VOLUME.lock().unwrap(), value, Instant::now()) {
            return;
        }
        let same = match kind {
            Kind::Brightness => self.qm.st.brightness.is_some_and(|c| (c - value).abs() < 0.005),
            // The sink reports whole percent: an echo of the QAM's own write may differ by up to one percent.
            Kind::Volume => self.qm.st.volume.is_some_and(|(c, m)| (c - value).abs() < 0.011 && m == muted),
        };
        if same {
            return;
        }
        self.level_changed(kind, value, muted, "external");
    }
}

/// PipeWire/PulseAudio sink events (`pactl subscribe`) re-read the default sink at once, so a physical
/// volume button shows in the QAM and the popup without waiting for the 4 s status poll.
/// Read-only; restarts itself if `pactl` exits.
pub fn spawn_volume_watch(ds: Arc<DataSource>) {
    let _ = std::thread::Builder::new().name("volume-watch".into()).spawn(move || loop {
        let Ok(mut child) = Command::new("pactl").arg("subscribe").stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                // e.g. "Event 'change' on sink #53"
                if !(line.contains("'change'") && line.contains("on sink")) {
                    continue;
                }
                let Some((v, muted)) = ds.volume() else { continue };
                crate::diag::log("level.volume_event", serde_json::json!({"value": v, "muted": muted}));
                let _ = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| a.level_external(Kind::Volume, v, muted));
                });
            }
        }
        let _ = child.wait();
        std::thread::sleep(Duration::from_secs(2));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_slint_codes() {
        assert_eq!(Kind::Brightness.code(), 0);
        assert_eq!(Kind::Volume.code(), 1);
    }

    #[test]
    fn qam_volume_echoes_match_any_recent_write() {
        let now = Instant::now();
        // a drag wrote 0.30 then 0.33; the echo of 0.30 can arrive after 0.33 is already stored
        let writes = [(0.30, now), (0.33, now)];
        assert!(is_own_echo(&writes, 0.30, now));
        assert!(is_own_echo(&writes, 0.331, now), "sink rounds to whole percent");
        assert!(!is_own_echo(&writes, 0.40, now), "a hardware step is not an echo");
    }

    #[test]
    fn old_qam_volume_writes_no_longer_hide_hardware_presses() {
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_millis(OWN_VOLUME_MS + 100)).unwrap_or(now);
        assert!(!is_own_echo(&[(0.30, old)], 0.30, now));
    }

    #[test]
    fn popup_timing_matches_decky_plugin() {
        // Decky dist/index.js: displayDuration = 1000, transition 0.22 s
        assert_eq!(POPUP_MS, 1000);
    }
}

#[cfg(test)]
mod popup_regressions {
    use super::*;

    fn advance(ms: u64) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
    }

    fn fixture(external: bool) -> (AppWindow, App) {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()), mock_time: true, ..Default::default()
            },
        ))).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        let ds = Arc::new(DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut app = App::new(ui.as_weak(), ds, Arc::new(steam_data::Library::default()), loader);
        // Model compositor availability without changing process-wide session environment.
        app.level_popup = Popup::new(external);
        (ui, app)
    }

    #[test]
    fn popup_creation_does_not_activate_it_or_change_main_window_attributes() {
        use slint::winit_030::winit::window::WindowAttributes;
        CREATING_POPUP.set(true);
        let popup = popup_window_attributes(WindowAttributes::default().with_active(true));
        CREATING_POPUP.set(false);
        assert!(!popup.active);
        let main = popup_window_attributes(WindowAttributes::default().with_active(true));
        assert!(main.active);
        assert_eq!(icon_name(Kind::Brightness, 0.42, true), "brightness");
        assert_eq!(icon_name(Kind::Volume, 0.42, false), "volume");
        assert_eq!(icon_name(Kind::Volume, 0.42, true), "volume.muted");
        assert_eq!(icon_name(Kind::Volume, 0.0, false), "volume.muted");
    }

    #[test]
    fn hardware_events_over_game_update_extend_fade_and_unmap_without_page_clock() {
        let (ui, mut app) = fixture(true);
        app.game_overlay = Some(2379780);
        app.clock_sync();
        assert!(!app.anim_timer.running());
        let route = app.router.current().clone();
        let focus = crate::gamescope::popup_focus_snapshot();
        app.level_changed(Kind::Brightness, 0.42, false, "chord");
        let win = app.level_popup.window.as_ref().unwrap().clone_strong();
        assert!(ui.global::<Shell>().get_level_external());
        assert!(win.window().is_visible());
        assert!(win.get_shown());
        assert_eq!((win.get_kind(), win.get_value()), (0, 0.42));
        // Starting the transition then advancing time exercises the actual Slint window independently.
        let _ = win.window().take_snapshot().unwrap();
        advance(700);
        let shown = win.window().take_snapshot().unwrap();
        assert!(shown.as_slice().iter().any(|p| p.a > 0), "popup must render with the shell clock stopped");
        assert!(shown.as_slice()[50 * 1280..].iter().all(|p| p.a == 0), "popup must leave the game area transparent");
        app.level_external(Kind::Volume, 0.63, false);
        assert_eq!((win.get_kind(), win.get_value()), (1, 0.63));
        advance(400); // past the original brightness deadline
        assert!(win.get_shown());
        advance(600); // 1000 ms from the latest press
        assert!(!win.get_shown());
        assert!(!ui.global::<Shell>().get_level_shown());
        assert!(win.window().is_visible(), "exit animation must finish before unmapping");
        let leaving = win.window().take_snapshot().unwrap();
        advance(250);
        let fading = win.window().take_snapshot().unwrap();
        let alpha = |shot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| shot.as_slice().iter().map(|p| u64::from(p.a)).sum::<u64>();
        assert!(alpha(&fading) < alpha(&leaving), "exit must animate on its own window while the shell clock is stopped");
        app.level_changed(Kind::Brightness, 0.81, false, "chord"); // interrupt fade
        advance(300); // old retire deadline must not hide the new popup
        assert!(win.window().is_visible() && win.get_shown());
        assert_eq!(win.get_value(), 0.81);
        app.qam_level_result(true, Some((0.80, false)), Ok(())); // accepted readback, no deadline extension
        assert_eq!(win.get_value(), 0.80);
        advance(700);
        assert!(!win.get_shown());
        advance(500);
        assert!(!win.window().is_visible());
        assert!(!app.anim_timer.running(), "popup must not restart the page clock");
        assert_eq!(crate::gamescope::popup_focus_snapshot(), focus, "popup must not request shell/Steam input or focus");
        assert_eq!(app.game_overlay, Some(2379780));
        assert_eq!(app.router.current(), &route);
        assert!(!app.sm.open && !app.qm.open);
    }

    #[test]
    fn qam_volume_echoes_and_unchanged_reads_never_open_or_extend_popup() {
        let (ui, mut app) = fixture(true);
        app.game_overlay = Some(2379780);
        app.qm.st.volume = Some((0.33, false));
        note_own_volume(0.30);
        note_own_volume(0.33);
        app.level_external(Kind::Volume, 0.30, false); // delayed earlier QAM write
        app.level_external(Kind::Volume, 0.331, false);
        assert!(!ui.global::<Shell>().get_level_shown());
        assert!(app.level_popup.window.is_none());
        app.qam_level_result(true, Some((0.45, false)), Ok(())); // QAM brightness confirmation
        app.level_external(Kind::Brightness, 0.45, false); // identical brightness echo
        assert!(!ui.global::<Shell>().get_level_shown());
        assert!(app.level_popup.window.is_none());
        assert_eq!(app.qm.st.volume, Some((0.33, false)));
        app.level_external(Kind::Volume, 0.60, true); // new physical event, mute included
        assert!(app.level_popup.window.as_ref().unwrap().get_shown());
        advance(700);
        app.level_external(Kind::Volume, 0.60, true); // unchanged read
        app.level_external(Kind::Volume, 0.30, false); // software echo
        advance(300);
        assert!(!ui.global::<Shell>().get_level_shown(), "reads/echoes must not extend hardware popup");
        assert_eq!(app.qm.st.volume, Some((0.60, true)));
    }

    #[test]
    fn shell_mode_uses_inline_bar_and_cover_cancels_external_popup() {
        let (ui, mut app) = fixture(false);
        app.level_changed(Kind::Brightness, 0.25, false, "chord");
        assert!(ui.global::<Shell>().get_level_shown());
        assert!(!ui.global::<Shell>().get_level_external());
        assert!(app.level_popup.window.is_none());
        advance(1000);
        assert!(!ui.global::<Shell>().get_level_shown());
        app.level_popup = Popup::new(true);
        app.level_changed(Kind::Volume, 0.5, false, "external");
        assert!(app.level_popup.window.as_ref().unwrap().window().is_visible());
        ui.global::<crate::AnimState>().set_active(true);
        advance(0);
        assert!(!app.level_popup.window.as_ref().unwrap().window().is_visible());
        assert!(!app.level_popup.hold.running() && !app.level_popup.retire.running());
        ui.global::<crate::AnimState>().set_active(true);
        app.level_changed(Kind::Brightness, 0.9, false, "chord");
        assert!(!app.level_popup.window.as_ref().unwrap().window().is_visible());
        ui.global::<crate::AnimState>().set_active(false);
        advance(1600);
        assert!(!app.level_popup.window.as_ref().unwrap().window().is_visible(), "cover must not replay stale levels");
    }
}
