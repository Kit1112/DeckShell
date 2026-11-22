//! Steam Deck hotkeys: STEAM / "..." chord state machine (pure, no I/O, no clock of its own).
//!
//! Fed by button press/release with millisecond timestamps (`Machine::event`) and by `Machine::tick(now)`; every
//! call returns the outputs to perform. The driver (`input.rs`) asks `Machine::next_deadline()` how long it may sleep.
//!
//! Rules (context/archive/docs/hotkeys/SPEC.md):
//!  * STEAM alone, released            -> `SteamMenu` (on release, like real Steam)
//!  * STEAM + any chord button         -> `Chord(..)`, the menu is suppressed
//!  * STEAM held >= `STEAM_HOLD_MS`    -> `PowerMenu` (once), the menu on release is suppressed
//!  * "..." short, released            -> `Qam`
//!  * "..." held >= `QAM_HOLD_MS`      -> `HintShow`; on release `HintHide` (no QAM)
//!
//! Thresholds: `FORCE_CLOSE_MS` (2400) is from the controller layout (chord_neptune.vdf long_press_time). The STEAM hold
//! and "..." hold times are NOT in sp.js: Big Picture receives `NotifyShowPowerMenu` and the chord-summary `visible`
//! flag from the native client (steamclient), so the values below are estimates, not facts (see context/archive/docs/PC_CLAUDE_LOG.md).
pub mod dispatch;
pub mod hint;

/// STEAM hold that opens the power menu. ESTIMATE: native in steamclient (not in sp.js). SPEC: "3+ s opens for sure".
pub const STEAM_HOLD_MS: u64 = 3000;
/// "..." hold that shows the chord hint. ESTIMATE: native (sp.js 352677 only reads a `visible` flag).
pub const QAM_HOLD_MS: u64 = 500;
/// STEAM + B long press (chord_neptune.vdf `long_press_time` 2400).
pub const FORCE_CLOSE_MS: u64 = 2400;
/// Brightness hold timing is the stock gamepad repeat (Steam's ButtonRepeatHandler, `input.rs`): the first
/// repeat 400 ms after the press, then every 50 ms. The step per repeat is `level::hold_step`.
pub const BRIGHTNESS_REPEAT_FIRST_MS: u64 = 400;
pub const BRIGHTNESS_REPEAT_MS: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Steam,
    Qam,
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    View,
    Menu,
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    ForceClose,
    ScreenReader,
    Magnifier,
    ShowKeyboard,
    Enter,
    Tab,
    Escape,
    /// L2 = right mouse click, R2 = left mouse click (needs a virtual mouse: not implemented)
    MouseRight,
    MouseLeft,
    Screenshot,
    Marker,
    RecordToggle,
    Clip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    SteamMenu,
    PowerMenu,
    Qam,
    HintShow,
    HintHide,
    Chord(Chord),
    /// Brightness step of the STEAM / "..." + stick chord. `elapsed_ms` counts from the first repeat of this hold
    /// (0 for the press and the first repeat), the same clock the stock slider uses.
    Brightness { up: bool, elapsed_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    Press(Btn),
    Release(Btn),
    /// Left stick vertical, digital: 1 up, -1 down, 0 neutral.
    LStickY(i8),
}

#[derive(Debug, Default)]
pub struct Machine {
    steam: Option<u64>,
    /// a chord fired (or the power menu opened): releasing STEAM must not open the menu
    steam_used: bool,
    power_fired: bool,
    qam: Option<u64>,
    /// "..." + left stick adjusted brightness: its release must not open the QAM or the hint
    qam_used: bool,
    hint_shown: bool,
    /// B pressed under STEAM at this time, not yet fired / not released
    b_since: Option<u64>,
    stick: i8,
    /// next brightness repeat time
    bright_next: Option<u64>,
    /// time of the first brightness repeat of the current hold
    bright_base: Option<u64>,
}

impl Machine {
    pub fn new() -> Self {
        Self::default()
    }

    /// STEAM is down: other button presses belong to the chord layer and must not reach the UI.
    pub fn steam_held(&self) -> bool {
        self.steam.is_some()
    }

    /// STEAM or "..." is down: the left stick is the brightness chord, not navigation.
    pub fn chord_held(&self) -> bool {
        self.steam.is_some() || self.qam.is_some()
    }

    /// The "..." hint overlay is up.
    pub fn hint_shown(&self) -> bool {
        self.hint_shown
    }

    pub fn event(&mut self, ev: Ev, now: u64) -> Vec<Out> {
        let mut out = vec![];
        match ev {
            Ev::Press(Btn::Steam) => {
                if self.steam.is_none() {
                    self.steam = Some(now);
                    self.steam_used = false;
                    self.power_fired = false;
                }
            }
            Ev::Release(Btn::Steam) => {
                if self.steam.take().is_some() {
                    if !self.steam_used {
                        out.push(Out::SteamMenu);
                    }
                    self.b_since = None;
                    if !self.chord_held() {
                        self.bright_next = None;
                    }
                }
            }
            Ev::Press(Btn::Qam) => {
                if self.qam.is_none() {
                    self.qam = Some(now);
                    self.qam_used = false;
                }
            }
            Ev::Release(Btn::Qam) => {
                if self.qam.take().is_some() {
                    if self.hint_shown {
                        self.hint_shown = false;
                        out.push(Out::HintHide);
                    } else if !self.qam_used {
                        out.push(Out::Qam);
                    }
                    self.qam_used = false;
                    if !self.chord_held() {
                        self.bright_next = None;
                    }
                }
            }
            Ev::Press(b) if self.steam.is_some() => {
                let c = match b {
                    Btn::B => {
                        self.b_since = Some(now);
                        self.steam_used = true;
                        None
                    }
                    Btn::View => Some(Chord::ScreenReader),
                    Btn::L1 => Some(Chord::Magnifier),
                    Btn::Right => Some(Chord::Enter),
                    Btn::Down => Some(Chord::Tab),
                    Btn::Left => Some(Chord::Escape),
                    Btn::Up => Some(Chord::Clip),
                    Btn::L2 => Some(Chord::MouseRight),
                    Btn::R2 => Some(Chord::MouseLeft),
                    Btn::R1 => Some(Chord::Screenshot),
                    Btn::Y => Some(Chord::Marker),
                    Btn::A => Some(Chord::RecordToggle),
                    // X fires on release (SHOW_KEYBOARD is "end of press")
                    Btn::X => {
                        self.steam_used = true;
                        None
                    }
                    Btn::Menu | Btn::Steam | Btn::Qam => None,
                };
                if let Some(c) = c {
                    self.steam_used = true;
                    out.push(Out::Chord(c));
                }
            }
            Ev::Release(Btn::B) => self.b_since = None,
            Ev::Release(Btn::X) if self.steam.is_some() => out.push(Out::Chord(Chord::ShowKeyboard)),
            Ev::LStickY(d) => {
                if self.chord_held() && d != 0 {
                    if self.stick != d {
                        if self.steam.is_some() {
                            self.steam_used = true;
                        }
                        if self.qam.is_some() {
                            self.qam_used = true;
                        }
                        self.bright_base = None;
                        self.bright_next = Some(now + BRIGHTNESS_REPEAT_FIRST_MS);
                        out.push(Out::Brightness { up: d > 0, elapsed_ms: 0 });
                    }
                } else {
                    self.bright_next = None;
                }
                self.stick = d;
            }
            _ => {}
        }
        out
    }

    /// Time-driven outputs: power menu, force close, hint overlay, brightness repeat.
    pub fn tick(&mut self, now: u64) -> Vec<Out> {
        let mut out = vec![];
        if let Some(t) = self.steam {
            if !self.steam_used && !self.power_fired && now >= t + STEAM_HOLD_MS {
                self.power_fired = true;
                self.steam_used = true;
                out.push(Out::PowerMenu);
            }
        }
        if let Some(t) = self.b_since {
            if now >= t + FORCE_CLOSE_MS {
                self.b_since = None;
                out.push(Out::Chord(Chord::ForceClose));
            }
        }
        if let Some(t) = self.qam {
            if !self.hint_shown && !self.qam_used && now >= t + QAM_HOLD_MS {
                self.hint_shown = true;
                out.push(Out::HintShow);
            }
        }
        if let Some(n) = self.bright_next {
            if now >= n && self.chord_held() && self.stick != 0 {
                self.bright_next = Some(now + BRIGHTNESS_REPEAT_MS);
                let base = *self.bright_base.get_or_insert(now);
                out.push(Out::Brightness { up: self.stick > 0, elapsed_ms: now - base });
            }
        }
        out
    }

    /// Absolute time of the next timer-driven output, if any (the driver may sleep until then).
    pub fn next_deadline(&self) -> Option<u64> {
        let mut d: Option<u64> = None;
        let mut up = |t: u64| d = Some(d.map_or(t, |x| x.min(t)));
        if let Some(t) = self.steam {
            if !self.steam_used && !self.power_fired {
                up(t + STEAM_HOLD_MS);
            }
        }
        if let Some(t) = self.b_since {
            up(t + FORCE_CLOSE_MS);
        }
        if let Some(t) = self.qam {
            if !self.hint_shown && !self.qam_used {
                up(t + QAM_HOLD_MS);
            }
        }
        if let Some(n) = self.bright_next {
            up(n);
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Btn::*;

    fn p(m: &mut Machine, b: Btn, t: u64) -> Vec<Out> {
        m.event(Ev::Press(b), t)
    }
    fn r(m: &mut Machine, b: Btn, t: u64) -> Vec<Out> {
        m.event(Ev::Release(b), t)
    }

    #[test]
    fn steam_alone_opens_menu_on_release() {
        let mut m = Machine::new();
        assert!(p(&mut m, Steam, 0).is_empty());
        assert!(m.tick(100).is_empty());
        assert_eq!(r(&mut m, Steam, 200), vec![Out::SteamMenu]);
    }

    #[test]
    fn chord_suppresses_menu() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        assert_eq!(p(&mut m, R1, 50), vec![Out::Chord(Chord::Screenshot)]);
        assert!(r(&mut m, R1, 80).is_empty());
        assert!(r(&mut m, Steam, 100).is_empty());
    }

    #[test]
    fn chord_table() {
        let t = [
            (View, Chord::ScreenReader),
            (L1, Chord::Magnifier),
            (Right, Chord::Enter),
            (Down, Chord::Tab),
            (Left, Chord::Escape),
            (Up, Chord::Clip),
            (L2, Chord::MouseRight),
            (R2, Chord::MouseLeft),
            (R1, Chord::Screenshot),
            (Y, Chord::Marker),
            (A, Chord::RecordToggle),
        ];
        for (b, c) in t {
            let mut m = Machine::new();
            p(&mut m, Steam, 0);
            assert_eq!(p(&mut m, b, 10), vec![Out::Chord(c)], "{b:?}");
        }
    }

    #[test]
    fn button_without_steam_is_not_a_chord() {
        let mut m = Machine::new();
        assert!(p(&mut m, A, 0).is_empty());
        assert!(!m.steam_held());
    }

    #[test]
    fn x_fires_on_release() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        assert!(p(&mut m, X, 10).is_empty());
        assert_eq!(r(&mut m, X, 60), vec![Out::Chord(Chord::ShowKeyboard)]);
        assert!(r(&mut m, Steam, 80).is_empty());
    }

    #[test]
    fn power_menu_on_long_hold_once_and_no_menu_on_release() {
        let mut m = Machine::new();
        p(&mut m, Steam, 1000);
        assert!(m.tick(1000 + STEAM_HOLD_MS - 1).is_empty());
        assert_eq!(m.tick(1000 + STEAM_HOLD_MS), vec![Out::PowerMenu]);
        assert!(m.tick(1000 + STEAM_HOLD_MS + 500).is_empty());
        assert!(r(&mut m, Steam, 9000).is_empty());
    }

    #[test]
    fn chord_cancels_power_hold() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        p(&mut m, Y, 100);
        assert!(m.tick(STEAM_HOLD_MS + 10).is_empty());
    }

    #[test]
    fn force_close_needs_2400() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        assert!(p(&mut m, B, 100).is_empty());
        assert!(m.tick(100 + FORCE_CLOSE_MS - 1).is_empty());
        assert_eq!(m.tick(100 + FORCE_CLOSE_MS), vec![Out::Chord(Chord::ForceClose)]);
        assert!(m.tick(100 + FORCE_CLOSE_MS + 10).is_empty());
        assert!(r(&mut m, Steam, 5000).is_empty());
    }

    #[test]
    fn force_close_cancelled_by_early_release() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        p(&mut m, B, 100);
        r(&mut m, B, 600);
        assert!(m.tick(5000).is_empty());
        // B was a chord press: the menu stays suppressed
        assert!(r(&mut m, Steam, 5100).is_empty());
    }

    #[test]
    fn qam_short_opens_on_release() {
        let mut m = Machine::new();
        assert!(p(&mut m, Qam, 0).is_empty());
        assert!(m.tick(QAM_HOLD_MS - 1).is_empty());
        assert_eq!(r(&mut m, Qam, QAM_HOLD_MS - 1), vec![Out::Qam]);
    }

    #[test]
    fn qam_hold_shows_hint_and_hides_on_release_without_qam() {
        let mut m = Machine::new();
        p(&mut m, Qam, 0);
        assert_eq!(m.tick(QAM_HOLD_MS), vec![Out::HintShow]);
        assert!(m.hint_shown());
        assert!(m.tick(QAM_HOLD_MS + 50).is_empty());
        assert_eq!(r(&mut m, Qam, 2000), vec![Out::HintHide]);
        assert!(!m.hint_shown());
    }

    #[test]
    fn brightness_press_then_stock_repeat_timing() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        assert_eq!(m.event(Ev::LStickY(1), 100), vec![Out::Brightness { up: true, elapsed_ms: 0 }]);
        assert!(m.tick(499).is_empty(), "first repeat comes 400 ms after the press");
        assert_eq!(m.tick(500), vec![Out::Brightness { up: true, elapsed_ms: 0 }]);
        assert_eq!(m.tick(550), vec![Out::Brightness { up: true, elapsed_ms: 50 }]);
        assert_eq!(m.tick(900), vec![Out::Brightness { up: true, elapsed_ms: 400 }]);
        assert!(m.tick(901).is_empty(), "late ticks must not catch up in a burst");
        m.event(Ev::LStickY(0), 910);
        assert!(m.tick(1000).is_empty());
        assert_eq!(m.event(Ev::LStickY(-1), 1100), vec![Out::Brightness { up: false, elapsed_ms: 0 }]);
        assert!(r(&mut m, Steam, 1200).is_empty());
    }

    #[test]
    fn qam_held_stick_is_brightness_and_release_does_not_open_qam() {
        let mut m = Machine::new();
        p(&mut m, Qam, 0);
        assert!(m.chord_held());
        assert_eq!(m.event(Ev::LStickY(1), 100), vec![Out::Brightness { up: true, elapsed_ms: 0 }]);
        assert_eq!(m.tick(500), vec![Out::Brightness { up: true, elapsed_ms: 0 }]);
        m.event(Ev::LStickY(0), 510);
        // the hint must not appear for a chord, even after the hold time
        assert!(m.tick(QAM_HOLD_MS + 600).is_empty());
        assert!(r(&mut m, Qam, QAM_HOLD_MS + 700).is_empty());
        assert!(!m.chord_held());
        // a later plain short press still opens the QAM
        p(&mut m, Qam, 2000);
        assert_eq!(r(&mut m, Qam, 2050), vec![Out::Qam]);
    }

    #[test]
    fn qam_held_stick_does_not_stop_when_steam_releases_first() {
        let mut m = Machine::new();
        p(&mut m, Steam, 0);
        p(&mut m, Qam, 10);
        assert_eq!(m.event(Ev::LStickY(-1), 20), vec![Out::Brightness { up: false, elapsed_ms: 0 }]);
        assert!(r(&mut m, Steam, 30).is_empty());
        assert_eq!(m.tick(420), vec![Out::Brightness { up: false, elapsed_ms: 0 }]);
        m.event(Ev::LStickY(0), 110);
        assert!(r(&mut m, Qam, 120).is_empty());
    }

    #[test]
    fn stick_without_steam_does_nothing() {
        let mut m = Machine::new();
        assert!(m.event(Ev::LStickY(1), 0).is_empty());
        assert!(m.tick(100).is_empty());
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn deadlines() {
        let mut m = Machine::new();
        assert_eq!(m.next_deadline(), None);
        p(&mut m, Steam, 10);
        assert_eq!(m.next_deadline(), Some(10 + STEAM_HOLD_MS));
        p(&mut m, B, 20);
        assert_eq!(m.next_deadline(), Some(20 + FORCE_CLOSE_MS));
        let mut q = Machine::new();
        p(&mut q, Qam, 5);
        assert_eq!(q.next_deadline(), Some(5 + QAM_HOLD_MS));
    }
}
