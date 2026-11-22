//! Physical power press classification, independent of the device reader and actions.
//!
//! Only the selected input device may feed this state machine. After a lost stream or
//! ownership transfer, discard its state instead of synthesizing a release.
pub mod device;
pub mod ownership;
pub mod runtime;

use std::time::Duration;

/// The traced Galileo physical button appears on i8042 and LNXPWRBN. SteamOS
/// ignores the latter; do not consume both as independent button presses.
/// Other models remain unsupported until their actual source has been verified.
pub fn is_traced_power_source(product: &str, physical_path: &str) -> bool {
    product == "Galileo" && physical_path == "isa0060/serio0/input0"
}

const LONG_PRESS: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Short,
    Long,
}

#[derive(Default)]
pub struct PowerPress {
    down_at: Option<Duration>,
    long_sent: bool,
}

impl PowerPress {
    /// Linux EV_KEY values: 1=down, 0=up, 2=repeat. Clock must be monotonic.
    pub fn key(&mut self, value: i32, now: Duration) -> Option<Press> {
        match value {
            1 if self.down_at.is_none() => {
                self.down_at = Some(now);
                self.long_sent = false;
                None
            }
            0 => {
                // Account for a delayed timer: a held key is still long, never short.
                let result = self.tick(now);
                let was_down = self.down_at.take().is_some();
                let long_sent = std::mem::take(&mut self.long_sent);
                result.or(if was_down && !long_sent { Some(Press::Short) } else { None })
            }
            _ => None,
        }
    }

    pub fn tick(&mut self, now: Duration) -> Option<Press> {
        if !self.long_sent && self.down_at.is_some_and(|t| now.checked_sub(t).is_some_and(|d| d >= LONG_PRESS)) {
            self.long_sent = true;
            Some(Press::Long)
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A release without a preceding press after arming is ignored.  Events read
/// before the stock daemon has stopped must never be passed to this gate.
#[derive(Default)]
pub struct PowerGate {
    armed: bool,
    press: PowerPress,
}

impl PowerGate {
    /// Arm only after the caller has confirmed the stock handler is stopped,
    /// drained old events, and observed the key up.  This method does not stop
    /// or start any service itself.
    pub fn arm(&mut self, stock_stopped: bool, drained: bool, key_up: bool) -> bool {
        self.disarm();
        self.armed = stock_stopped && drained && key_up;
        self.armed
    }

    pub fn disarm(&mut self) {
        self.armed = false;
        self.press.reset();
    }

    /// Linux event type and code; ignore every other keyboard key.
    pub fn event(&mut self, event_type: u16, code: u16, value: i32, now: Duration) -> Option<Press> {
        if !self.armed { return None; }
        // SYN_DROPPED makes the pending stream ambiguous: fail closed.
        if event_type == 0 && code == 3 {
            self.disarm();
            return None;
        }
        if event_type == 1 && code == 116 { self.press.key(value, now) } else { None }
    }

    pub fn tick(&mut self, now: Duration) -> Option<Press> {
        if self.armed { self.press.tick(now) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(n: u64) -> Duration { Duration::from_millis(n) }

    #[test]
    fn gate_requires_safe_transfer_and_fails_closed_on_lost_events() {
        let mut g = PowerGate::default();
        for (stopped, drained, up) in [(false, true, true), (true, false, true), (true, true, false)] {
            assert!(!g.arm(stopped, drained, up));
            assert_eq!(g.event(1, 116, 1, ms(0)), None);
            assert_eq!(g.event(1, 116, 0, ms(100)), None);
        }
        assert!(g.arm(true, true, true));
        // Unrelated keys never become power presses.
        assert_eq!(g.event(1, 30, 1, ms(0)), None);
        assert_eq!(g.event(1, 116, 1, ms(10)), None);
        assert_eq!(g.event(0, 3, 0, ms(20)), None);
        assert_eq!(g.event(1, 116, 0, ms(30)), None);
        assert_eq!(g.tick(ms(1100)), None);
        assert!(g.arm(true, true, true));
        g.event(1, 116, 1, ms(1200));
        assert_eq!(g.event(1, 116, 0, ms(1300)), Some(Press::Short));
    }

    #[test]
    fn wake_press_is_discarded_until_drained_and_released() {
        let mut g = PowerGate::default();
        assert!(g.arm(true, true, true));
        // A sleep transition invalidates any incomplete press and disables dispatch.
        g.event(1, 116, 1, ms(10));
        g.disarm();
        assert_eq!(g.event(1, 116, 1, ms(2000)), None);
        assert_eq!(g.event(1, 116, 0, ms(2200)), None);
        assert!(!g.arm(true, true, false));
        assert!(!g.arm(true, false, true));
        assert!(g.arm(true, true, true));
        assert_eq!(g.event(1, 116, 0, ms(2300)), None);
        g.event(1, 116, 1, ms(2400));
        assert_eq!(g.event(1, 116, 0, ms(2500)), Some(Press::Short));
    }

    #[test]
    fn accepts_only_the_traced_galileo_source() {
        assert!(is_traced_power_source("Galileo", "isa0060/serio0/input0"));
        assert!(!is_traced_power_source("Galileo", "LNXPWRBN/button/input0"));
        assert!(!is_traced_power_source("Galileo", "PNP0C0C/button/input0"));
        assert!(!is_traced_power_source("Jupiter", "isa0060/serio0/input0"));
    }

    #[test]
    fn short_release_once_and_stray_release_ignored() {
        let mut p = PowerPress::default();
        assert_eq!(p.key(0, ms(0)), None);
        assert_eq!(p.key(1, ms(10)), None);
        assert_eq!(p.key(0, ms(175)), Some(Press::Short));
        assert_eq!(p.key(0, ms(176)), None);
    }

    #[test]
    fn long_fires_once_even_with_repeat_or_delayed_release() {
        let mut p = PowerPress::default();
        p.key(1, ms(0));
        assert_eq!(p.key(2, ms(500)), None);
        assert_eq!(p.key(1, ms(700)), None);
        assert_eq!(p.tick(ms(999)), None);
        assert_eq!(p.tick(ms(1000)), Some(Press::Long));
        assert_eq!(p.tick(ms(1500)), None);
        assert_eq!(p.key(0, ms(1501)), None);
        p.key(1, ms(2000));
        assert_eq!(p.key(0, ms(3100)), Some(Press::Long));
    }

    #[test]
    fn loss_of_stream_does_not_synthesize_a_press() {
        let mut p = PowerPress::default();
        p.key(1, ms(0));
        p.reset();
        assert_eq!(p.key(0, ms(200)), None);
        assert_eq!(p.tick(ms(2000)), None);
        p.key(1, ms(2100));
        assert_eq!(p.key(0, ms(2200)), Some(Press::Short));
    }
}
