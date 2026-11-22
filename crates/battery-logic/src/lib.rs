//! Stock Steam battery decisions (sp.js `SystemPowerStore`, QAM «Производительность», header percent, `BatteryPreferences`).
//!
//! The decisions themselves live in `port.rs`, produced by the rust-anything cycle (record on the JS original → port → diff →
//! gate; see docs/rust-anything-pilot.md) and included here verbatim. This file only types them and adds the timers' clock.
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[path = "../../../tools/rust-anything/battery/rust/battery-rs/src/port.rs"]
#[allow(clippy::all, dead_code)]
mod port;

fn s(v: Value) -> String { v.as_str().unwrap_or("").to_string() }
fn n(v: Value) -> i64 { v.as_i64().unwrap_or(0) }

/// Steam `eBatteryState`: 1 discharging, 2 charging, 3 full (module 30677); 0 unknown.
pub const DISCHARGING: i64 = 1;
pub const CHARGING: i64 = 2;
pub const FULL: i64 = 3;

/// Confidence in the projected time: Low 0 → (10 s) Medium 1 → (1 min) High 2.
pub fn confidence_on_update(prev_ac: i64, new_ac: i64, conf: i64) -> i64 {
    n(port::confidence_on_battery_update(json!(prev_ac), json!(new_ac), json!(conf)).unwrap_or_default())
}
pub fn timer_delay(conf: i64) -> Option<Duration> {
    u64::try_from(n(port::confidence_timer_delay_ms(json!(conf)).unwrap_or_default())).ok().map(Duration::from_millis)
}
pub fn after_timer(conf: i64) -> i64 { n(port::confidence_after_timer(json!(conf)).unwrap_or_default()) }

/// Stock QAM «time remaining» pieces: the value line is empty (zero-width space) at Low confidence.
pub fn time_shown(conf: i64) -> bool { port::battery_time_shown(json!(conf)).ok().and_then(|v| v.as_bool()).unwrap_or(false) }
/// `(hours, minutes)` strings, `"?"` while unknown.
pub fn time_parts(seconds: Option<f64>) -> (String, String) {
    let v = match seconds {
        Some(x) => s(port::battery_time_parts(json!(x)).unwrap_or_default()),
        None => "?|?".to_string(),
    };
    let (h, m) = v.split_once('|').unwrap_or(("?", "?"));
    (h.to_string(), m.to_string())
}
/// Caption token under the time, `None` = block hidden (stock returns null).
pub fn caption_token(battery_state: i64, conf: i64) -> Option<String> {
    Some(s(port::battery_time_caption_token(json!(battery_state), json!(conf)).unwrap_or_default())).filter(|t| !t.is_empty())
}
/// Header / QAM percent text: level 0..1, capped at 99 like stock.
pub fn percent_text(level: f64) -> String { s(port::battery_percent_text(json!(level)).unwrap_or_default()) }
/// Header percentage visibility (stock `Tr`); `region` 2 never shows it.
pub fn show_header_percent(pref: bool, never: bool, region: i64, full: bool, hint_visible: bool) -> bool {
    port::show_header_percent(json!(pref), json!(never), json!(region), json!(full), json!(hint_visible)).ok().and_then(|v| v.as_bool()).unwrap_or(false)
}
/// `JSON.stringify(settingsStore.GetBatteryPreferences())` → the flag; `None` when Steam returned nothing.
pub fn pref_show(stored: &Value) -> Option<bool> {
    // The Deck's GetBatteryPreferences() is an observable: JSON.stringify of it is {"m_callbacks":…,"m_currentValue":{…}} (task 497);
    // the port only knows the bare object, which it read as «off» for every wrapper.
    let t = stored.as_str()?;
    let inner = serde_json::from_str::<Value>(t).ok().and_then(|v| v.get("m_currentValue").map(|c| c.to_string()));
    Some(port::battery_pref_show(json!(inner.as_deref().unwrap_or(t))).ok().and_then(|v| v.as_bool()).unwrap_or(false))
}
/// Object the stock page hands to `SetBatteryPreferences`.
pub fn pref_stored(v: bool) -> String { s(port::battery_pref_stored(json!(v)).unwrap_or_default()) }

/// The stock `SystemPowerStore` timers as data: AC change → Low; after 10 s Medium; after 1 min more High.
#[derive(Debug, Clone)]
pub struct Clock { ac: i64, state: i64, conf: i64, next: Option<Instant>, say_full: bool, say_full_clear: Option<Instant> }
impl Default for Clock {
    /// Stock starts at High confidence with AC state 0 (module 98146 `wS`) and battery state 0.
    fn default() -> Self { Clock { ac: 0, state: 0, conf: 2, next: None, say_full: false, say_full_clear: None } }
}
impl Clock {
    pub fn confidence(&self) -> i64 { self.conf }
    pub fn say_full(&self) -> bool { self.say_full }
    /// `OnBatteryStateChanged`: `ac` and `state` as Steam enums.
    pub fn update(&mut self, now: Instant, ac: i64, state: i64) {
        self.tick(now);
        let conf = confidence_on_update(self.ac, ac, self.conf);
        if ac != self.ac {
            self.conf = conf;
            self.next = timer_delay(conf).map(|d| now + d);
        }
        match port::say_full_action(json!(self.state), json!(state)).ok().and_then(|v| v.as_str().map(String::from)).as_deref() {
            Some("set") => { self.say_full = true; self.say_full_clear = None; }
            Some("clear-after-5000") => self.say_full_clear = Some(now + Duration::from_millis(5000)),
            _ => {}
        }
        self.ac = ac;
        self.state = state;
    }
    /// Applies timers that have expired by `now`.
    pub fn tick(&mut self, now: Instant) {
        while let Some(at) = self.next.filter(|at| *at <= now) {
            self.conf = after_timer(self.conf);
            self.next = timer_delay(self.conf).map(|d| at + d);
        }
        if self.say_full_clear.is_some_and(|at| at <= now) { self.say_full = false; self.say_full_clear = None; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confidence_climbs_after_ac_change() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        c.update(t0, 2, 1); // AC state changed 0 → 2: Low
        assert_eq!(c.confidence(), 0);
        c.tick(t0 + Duration::from_secs(9));
        assert_eq!(c.confidence(), 0);
        c.tick(t0 + Duration::from_secs(10));
        assert_eq!(c.confidence(), 1);
        c.tick(t0 + Duration::from_secs(69));
        assert_eq!(c.confidence(), 1);
        c.tick(t0 + Duration::from_secs(70));
        assert_eq!(c.confidence(), 2);
        c.update(t0 + Duration::from_secs(80), 2, 1); // same AC state: unchanged
        assert_eq!(c.confidence(), 2);
        c.update(t0 + Duration::from_secs(81), 1, 1); // unplugged: Low again
        assert_eq!(c.confidence(), 0);
    }
    #[test]
    fn say_full_latches_for_five_seconds() {
        let t0 = Instant::now();
        let mut c = Clock::default();
        c.update(t0, 2, FULL);
        assert!(c.say_full());
        c.update(t0 + Duration::from_secs(1), 2, CHARGING);
        c.tick(t0 + Duration::from_secs(5));
        assert!(c.say_full());
        c.tick(t0 + Duration::from_secs(6));
        assert!(!c.say_full());
    }
    #[test]
    fn labels() {
        assert_eq!(time_parts(Some(7384.0)), ("2".into(), "3".into()));
        assert_eq!(time_parts(None), ("?".into(), "?".into()));
        assert_eq!(caption_token(DISCHARGING, 0).as_deref(), Some("#QuickAccess_Tab_Perf_CalculatingBatteryLife"));
        assert_eq!(caption_token(CHARGING, 2).as_deref(), Some("#QuickAccess_Tab_Perf_ProjectedChargingTime"));
        assert_eq!(caption_token(0, 2), None);
        assert!(!time_shown(0) && time_shown(1));
        assert_eq!(percent_text(1.0), "99%");
        assert!(!show_header_percent(true, false, 2, false, true) && show_header_percent(true, false, 1, false, false));
        assert_eq!(pref_show(&json!("{\"bShowBatteryPercentage\":true}")), Some(true));
        // real Deck reply of JSON.stringify(settingsStore.GetBatteryPreferences()) (2026-11-19)
        assert_eq!(pref_show(&json!("{\"m_callbacks\":{\"m_vecCallbacks\":[]},\"m_currentValue\":{\"bShowBatteryPercentage\":true}}")), Some(true));
        assert_eq!(pref_show(&json!("{\"m_callbacks\":{\"m_vecCallbacks\":[]},\"m_currentValue\":{\"bShowBatteryPercentage\":false}}")), Some(false));
        assert_eq!(pref_show(&Value::Null), None);
        assert_eq!(pref_stored(false), "{\"bShowBatteryPercentage\":false}");
    }
}
