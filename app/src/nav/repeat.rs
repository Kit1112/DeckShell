//! D-pad auto-repeat. Port of classes `i` and `r` (sp.js:80663-80735, module 20505) with the config at sp.js:80790:
//! `inputsThatRepeat = {DIR_UP, DIR_DOWN, DIR_LEFT, DIR_RIGHT}`, `firstRepeatInterval_ms = 400`, `repeatInterval_ms = 50`.
//! Semantics: the press fires immediately (`is_repeat=false`); after 400 ms one `is_repeat=true` fire, then every
//! 50 ms. Axis lock: while one direction is held, further direction presses are ignored until it is released.
//! Time is injected (`now_ms`) so the whole thing is a deterministic state machine; the caller polls `tick`.

use super::tree::Button;

pub const FIRST_REPEAT_MS: f64 = 400.0;
pub const REPEAT_MS: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockAxis {
    None,
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fire {
    pub button: Button,
    pub is_repeat: bool,
}

#[derive(Debug)]
pub struct Repeater {
    axis: LockAxis,
    /// `m_ActiveInputId`
    active: Option<Button>,
    next_ms: f64,
    allowed: bool,
}
impl Default for Repeater {
    fn default() -> Self {
        Repeater { axis: LockAxis::None, active: None, next_ms: 0.0, allowed: true }
    }
}
fn repeats(b: Button) -> bool {
    b.is_dir()
}
impl Repeater {
    fn reset_generator(&mut self) {
        self.active = None;
    }
    /// `HandleInputButtonDown(button, fire)` (sp.js:80704). Returns the immediate fire, or `None` when the original
    /// ignores the press: while a direction is held (axis locked) every other repeating direction is dropped entirely
    /// (`m_repeatOnAxis == 0 && (...)` short-circuits before `f()`).
    pub fn down(&mut self, b: Button, now_ms: f64) -> Option<Fire> {
        if self.allowed && repeats(b) {
            if self.axis != LockAxis::None {
                return None;
            }
            self.axis = if matches!(b, Button::DirUp | Button::DirDown) { LockAxis::Vertical } else { LockAxis::Horizontal };
            // inner generator: arms only for a different button than the active one
            if self.active != Some(b) {
                self.reset_generator();
                self.active = Some(b);
                self.next_ms = now_ms + FIRST_REPEAT_MS;
            }
        } else {
            self.reset_generator();
        }
        Some(Fire { button: b, is_repeat: false })
    }
    /// `HandleInputButtonUp(button)`.
    pub fn up(&mut self, b: Button) {
        if repeats(b) && self.axis != LockAxis::None {
            let v = matches!(b, Button::DirUp | Button::DirDown);
            let h = matches!(b, Button::DirLeft | Button::DirRight);
            if (self.axis == LockAxis::Vertical && v) || (self.axis == LockAxis::Horizontal && h) {
                self.axis = LockAxis::None;
                self.reset_generator();
            }
        } else {
            self.reset_generator();
        }
    }
    /// Poll: returns the repeat fires due at `now_ms` (several if the caller polled late).
    pub fn tick(&mut self, now_ms: f64) -> Vec<Fire> {
        let mut out = vec![];
        if let Some(b) = self.active {
            // setTimeout(400) then setInterval(50)
            while now_ms >= self.next_ms {
                out.push(Fire { button: b, is_repeat: true });
                self.next_ms += REPEAT_MS;
                if out.len() > 100 {
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn press_then_400_then_every_50() {
        let mut r = Repeater::default();
        assert_eq!(r.down(Button::DirDown, 0.0), Some(Fire { button: Button::DirDown, is_repeat: false }));
        assert!(r.tick(399.0).is_empty());
        let f = r.tick(400.0);
        assert_eq!(f, vec![Fire { button: Button::DirDown, is_repeat: true }]);
        assert!(r.tick(449.0).is_empty());
        assert_eq!(r.tick(450.0).len(), 1);
        assert_eq!(r.tick(550.0).len(), 2); // late poll catches up
        r.up(Button::DirDown);
        assert!(r.tick(10_000.0).is_empty());
    }
    #[test]
    fn axis_lock_ignores_second_direction() {
        let mut r = Repeater::default();
        r.down(Button::DirDown, 0.0);
        assert_eq!(r.down(Button::DirRight, 100.0), None);
        assert_eq!(r.down(Button::DirUp, 100.0), None);
        let fires = r.tick(400.0);
        assert_eq!(fires[0].button, Button::DirDown);
        // releasing a button of the other axis does not stop the first
        r.up(Button::DirRight);
        assert_eq!(r.tick(450.0).len(), 1);
        r.up(Button::DirDown);
        assert!(r.down(Button::DirLeft, 1000.0).is_some());
        // non-dir button does not repeat
        let mut r2 = Repeater::default();
        assert!(r2.down(Button::Ok, 0.0).is_some());
        assert!(r2.tick(1000.0).is_empty());
    }
}
