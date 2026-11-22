//! Task 518 tests: the idle decision, the settings parser, the Deck snapshot of 2026-11-20. Nothing here touches power or Steam.
use super::*;
use serde_json::json;

const T: Timers = Timers { dim: 60, saver: 120, sleep: 300 };

#[test]
fn nothing_fires_before_its_timer() {
    let mut m = Machine::default();
    assert!(m.step(0, T).is_empty());
    assert!(m.step(59, T).is_empty());
    assert!(!m.dimmed() && !m.saver());
}

#[test]
fn dim_then_screensaver_then_sleep_each_once() {
    let mut m = Machine::default();
    assert_eq!(m.step(60, T), vec![Cmd::Dim]);
    assert!(m.step(61, T).is_empty());
    assert_eq!(m.step(120, T), vec![Cmd::Saver]);
    assert!(m.step(200, T).is_empty());
    assert_eq!(m.step(300, T), vec![Cmd::Sleep]);
    assert!(m.step(301, T).is_empty(), "sleep is issued once per idle period");
}

#[test]
fn activity_takes_dim_and_screensaver_back_and_rearms_sleep() {
    let mut m = Machine::default();
    m.step(300, T);
    assert_eq!(m.step(0, T), vec![Cmd::Undim, Cmd::Unsaver]);
    assert!(!m.dimmed() && !m.saver());
    assert_eq!(m.step(300, T), vec![Cmd::Dim, Cmd::Saver, Cmd::Sleep], "a new idle period fires everything again, sleep last");
}

#[test]
fn one_long_gap_fires_in_order() {
    let mut m = Machine::default();
    assert_eq!(m.step(10_000, T), vec![Cmd::Dim, Cmd::Saver, Cmd::Sleep]);
}

#[test]
fn zero_means_disabled() {
    let mut m = Machine::default();
    assert!(m.step(100_000, Timers::default()).is_empty());
    let only_saver = Timers { saver: 60, ..Default::default() };
    assert_eq!(m.step(100_000, only_saver), vec![Cmd::Saver]);
}

#[test]
fn the_timer_set_changing_while_active_is_followed() {
    let mut m = Machine::default();
    assert_eq!(m.step(70, T), vec![Cmd::Dim]);
    // the owner turned the dim off on the page: the screen comes back at once
    assert_eq!(m.step(71, Timers { dim: 0, ..T }), vec![Cmd::Undim]);
    // a shorter screensaver than the elapsed idle time fires on the next tick
    assert_eq!(m.step(71, Timers { dim: 0, saver: 30, sleep: 300 }), vec![Cmd::Saver]);
}

#[test]
fn dim_is_a_quarter_but_never_brighter_nor_below_the_floor() {
    assert!((dim_level(1.0) - 0.25).abs() < 1e-6);
    assert!((dim_level(0.4) - 0.1).abs() < 1e-6);
    assert_eq!(dim_level(0.05), 0.02);
    assert_eq!(dim_level(0.01), 0.01, "already darker than the dim level: not raised");
    assert!(dim_level(0.0) >= crate::display::FLOOR);
}

#[test]
fn a_wall_clock_jump_is_a_resume() {
    assert!(!resumed(Duration::from_millis(255), Duration::from_millis(250)));
    assert!(resumed(Duration::from_secs(900), Duration::from_millis(250)));
}

#[test]
fn activity_resets_idle_and_screensaver_swallows_input() {
    touch();
    assert_eq!(idle_secs(), 0);
    SAVER_SHOWN.store(false, Ordering::Relaxed);
    assert!(!swallow());
    SAVER_SHOWN.store(true, Ordering::Relaxed);
    assert!(swallow(), "the first input only wakes the screensaver");
    SAVER_SHOWN.store(false, Ordering::Relaxed);
}

#[test]
fn deck_snapshot_20_11_gives_the_page_values() {
    let snap: Value = serde_json::from_str(include_str!("../screens/power_page/fixtures/deck_snapshot_2026-11-20.json")).unwrap();
    let s = timers_from(&snap).unwrap();
    // the page shows sleep 1200 / 900, screensaver 60 / 300, dim 1200 / 600 (battery / AC)
    assert_eq!(s.battery, Timers { sleep: 1200, saver: 60, dim: 1200 });
    assert_eq!(s.ac, Timers { sleep: 900, saver: 300, dim: 600 });
    assert_eq!(s.for_power(true), s.ac);
    assert_eq!(s.for_power(false), s.battery);
}

#[test]
fn the_deck_battery_screensaver_fires_after_one_minute() {
    let snap: Value = serde_json::from_str(include_str!("../screens/power_page/fixtures/deck_snapshot_2026-11-20.json")).unwrap();
    let t = timers_from(&snap).unwrap().for_power(false);
    let mut m = Machine::default();
    assert!(m.step(59, t).is_empty());
    assert_eq!(m.step(60, t), vec![Cmd::Saver], "«через 1 минуту» now actually turns the screensaver on");
    assert_eq!(m.step(1200, t), vec![Cmd::Dim, Cmd::Sleep]);
}

#[test]
fn unread_settings_keep_the_previous_ones() {
    assert_eq!(timers_from(&Value::Null), None);
    assert_eq!(timers_from(&json!({"c":{},"power":{"dim_battery":null,"dim_ac":null}})), None);
    let part = timers_from(&json!({"c":{"system_idle_suspend_ac_sec":900},"power":{}})).unwrap();
    assert_eq!(part.ac, Timers { sleep: 900, saver: 0, dim: 0 });
    assert_eq!(part.battery, Timers::default());
    assert_eq!(timers_from(&json!({"c":{"system_idle_suspend_ac_sec":-5},"power":{}})), None, "negative is not a timer");
}

#[test]
fn the_script_reads_the_same_keys_the_page_writes() {
    let js = include_str!("../idle_settings.js");
    for (_, key) in crate::screens::settings::power_page::DROPDOWNS.iter().filter(|d| !d.1.starts_with("dim_")) {
        assert!(js.contains(key), "{key}");
    }
    assert!(js.contains("RegisterForSettingsChanges") && js.contains("unregister"));
    assert!(!js.contains(".Get()") && !js.contains("UpdateSettings") && !js.contains("SetSetting"), "read-only");
}
