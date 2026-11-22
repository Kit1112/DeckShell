//! Launch cover (task 330): the cat shown from Play/Resume until gamescope focuses the game window.
//!
//! The focus handover itself (gamescope layers, `request_game`) is untouched. This only paints a cover over
//! DeckShell while the handover is pending. It ends when (a) the requested game is the focused app, (b) the
//! safety timeout passes, (c) the launch request fails, or (d) the user asks for Steam/QAM (guide) first.
//!
//! `judge` is the pure decision. The UI-thread state lives in `STATE`; the Slint global is `LaunchSplashState`.
use std::cell::RefCell;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, TimerMode};

use crate::launch_cat;
use crate::{AppWindow, LaunchSplashState};

/// Safety cap: the cover is removed after this even if gamescope never focuses the game.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// How often the focus is checked while the cover is up.
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Wait,
    Focused,
    TimedOut,
}

/// `focused` is the game gamescope currently shows (`gamescope::focused_game`), if any.
pub fn judge(appid: u32, started: Instant, focused: Option<u32>, now: Instant) -> Outcome {
    if focused == Some(appid) {
        Outcome::Focused
    } else if now.saturating_duration_since(started) >= TIMEOUT {
        Outcome::TimedOut
    } else {
        Outcome::Wait
    }
}

/// The owner's Custom Loader "Cat" GIF, decoded to Slint images, and the frame on screen.
struct CatFrames {
    frames: Vec<(slint::Image, Duration)>,
    next: usize,
}

#[derive(Default)]
struct State {
    ui: Option<slint::Weak<AppWindow>>,
    timer: slint::Timer,
    frame_timer: slint::Timer,
    pending: Option<(u32, Instant)>,
    cat: Option<CatFrames>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// UI thread, once at start-up. Decodes the built-in cat GIF in the background.
pub fn init(ui: &AppWindow) {
    STATE.with(|s| s.borrow_mut().ui = Some(ui.as_weak()));
    std::thread::spawn(move || match launch_cat::decode_embedded() {
        Ok(frames) => {
            let _ = slint::invoke_from_event_loop(move || install_cat(frames));
        }
        Err(e) => eprintln!("[launch-splash] built-in cat not used: {e}"),
    });
}

/// UI thread: the decoded frames arrived; the cover now uses the Cat look from the next Play/Resume.
fn install_cat(frames: Vec<launch_cat::Frame>) {
    let frames: Vec<(slint::Image, Duration)> = frames
        .into_iter()
        .map(|f| {
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&f.rgba, launch_cat::SIZE, launch_cat::SIZE);
            (slint::Image::from_rgba8(buffer), f.delay)
        })
        .collect();
    let count = frames.len();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) else { return };
        ui.global::<LaunchSplashState>().set_theme_cat(true);
        s.cat = Some(CatFrames { frames, next: 0 });
        // A cover that is already up (Play pressed before the decode finished) starts the animation now.
        if s.pending.is_some() {
            show_next_frame(&mut s);
        }
    });
    eprintln!("[launch-splash] theme cat loaded frames={count}");
}

/// UI thread, under the STATE borrow: put the next GIF frame on screen and schedule the one after it.
fn show_next_frame(s: &mut State) {
    let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) else { return };
    let Some(cat) = s.cat.as_mut() else { return };
    let (image, delay) = cat.frames[cat.next].clone();
    cat.next = (cat.next + 1) % cat.frames.len();
    ui.global::<LaunchSplashState>().set_cat_frame(image);
    s.frame_timer.start(TimerMode::SingleShot, delay, advance_frame);
}

fn advance_frame() {
    STATE.with(|s| show_next_frame(&mut s.borrow_mut()));
}

/// UI thread: a real (not dry-run) Play/Resume was dispatched for `appid`. A newer request replaces an older one.
pub fn begin(appid: u32) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) else { return };
        s.pending = Some((appid, Instant::now()));
        ui.global::<LaunchSplashState>().set_active(true);
        s.timer.start(TimerMode::Repeated, POLL, poll);
        if let Some(cat) = s.cat.as_mut() {
            cat.next = 0;
        }
        show_next_frame(&mut s);
        eprintln!("[launch-splash] shown app={appid}");
    });
}

/// Marker "appid" of a cover that is not tied to a game (account switch, task 430): no focus judge, no 60 s cap.
/// It ends only through `end_hold`; the guide/QAM press does not remove it (that would show a stock Steam UI).
pub const HOLD_ID: u32 = u32::MAX;

/// UI thread: show the cat cover while Steam restarts in the background. `caption` is the line under the cat.
pub fn begin_hold(caption: &str) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) else { return };
        s.timer.stop();
        s.pending = Some((HOLD_ID, Instant::now()));
        ui.global::<LaunchSplashState>().set_caption(caption.into());
        ui.global::<LaunchSplashState>().set_active(true);
        if let Some(cat) = s.cat.as_mut() {
            cat.next = 0;
        }
        show_next_frame(&mut s);
        eprintln!("[launch-splash] hold cover shown");
    });
}

/// UI thread: change the caption of the hold cover (no-op if another cover is up).
pub fn set_caption(caption: &str) {
    STATE.with(|s| {
        let s = s.borrow();
        if s.pending.is_some_and(|(id, _)| id == HOLD_ID) {
            if let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) {
                ui.global::<LaunchSplashState>().set_caption(caption.into());
            }
        }
    });
}

/// UI thread: remove the hold cover.
pub fn end_hold(reason: &str) {
    end_if(HOLD_ID, reason);
    STATE.with(|s| {
        if let Some(ui) = s.borrow().ui.as_ref().and_then(|w| w.upgrade()) {
            ui.global::<LaunchSplashState>().set_caption("".into());
        }
    });
}

/// UI thread: hide the cover for `appid` only if that launch is still the pending one.
pub fn end_if(appid: u32, reason: &str) {
    let matches = STATE.with(|s| s.borrow().pending.is_some_and(|(id, _)| id == appid));
    if matches {
        end(reason);
    }
}

/// UI thread: hide the cover whatever launch is pending (guide/QAM cancel). No-op when nothing is pending.
pub fn end(reason: &str) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if reason == "guide" && s.pending.is_some_and(|(id, _)| id == HOLD_ID) {
            return;
        }
        s.timer.stop();
        s.frame_timer.stop();
        if let Some((appid, _)) = s.pending.take() {
            if let Some(ui) = s.ui.as_ref().and_then(|w| w.upgrade()) {
                ui.global::<LaunchSplashState>().set_active(false);
            }
            eprintln!("[launch-splash] hidden app={appid} reason={reason}");
        }
    });
}

/// UI thread: whether the cover is up. Pad input is held back meanwhile (guide/QAM still act).
pub fn active() -> bool {
    STATE.with(|s| s.borrow().pending.is_some())
}

/// Any thread: the launch request for `appid` failed. Hides the cover on the UI thread.
pub fn fail(appid: u32) {
    let _ = slint::invoke_from_event_loop(move || end_if(appid, "launch failed"));
}

fn poll() {
    if STATE.with(|s| s.borrow().pending.is_some_and(|(id, _)| id == HOLD_ID)) {
        return;
    }
    let verdict = STATE.with(|s| {
        s.borrow().pending.map(|(appid, started)| judge(appid, started, crate::gamescope::focused_game(), Instant::now()))
    });
    match verdict {
        Some(Outcome::Focused) => end("game focused"),
        Some(Outcome::TimedOut) => end("timeout"),
        Some(Outcome::Wait) | None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_focus_ends_the_wait() {
        let t = Instant::now();
        assert_eq!(judge(7, t, Some(7), t), Outcome::Focused);
    }

    #[test]
    fn another_window_does_not_end_it() {
        let t = Instant::now();
        assert_eq!(judge(7, t, Some(8), t + Duration::from_secs(1)), Outcome::Wait);
        assert_eq!(judge(7, t, None, t + Duration::from_secs(1)), Outcome::Wait);
    }

    #[test]
    fn timeout_is_a_safety_cap() {
        let t = Instant::now();
        assert_eq!(judge(7, t, None, t + TIMEOUT - Duration::from_millis(1)), Outcome::Wait);
        assert_eq!(judge(7, t, None, t + TIMEOUT), Outcome::TimedOut);
    }

    #[test]
    fn a_focus_at_the_deadline_still_counts_as_focused() {
        let t = Instant::now();
        assert_eq!(judge(7, t, Some(7), t + TIMEOUT), Outcome::Focused);
    }
}
