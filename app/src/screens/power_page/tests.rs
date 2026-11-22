//! Task 466 tests: layout lint, pad walk, drop-down / toggle / slider flows on the live-snapshot mock, PNG of key states
//! (`DECKSHELL_PNG_DIR=… cargo test power_page`). DRYRUN: nothing here writes to Steam or touches power.
use super::*;
use crate::input::Action;
use crate::router::Route;
use crate::screens::osk::field_tests::{app, shot};
use slint::{ComponentHandle, Model};

fn russian() -> &'static Locale { i18n::by_code("ru").unwrap() }

fn full() -> Value {
    json!({"c":{"steamos_charge_limit_enabled":true,"steamos_charge_limit":90,
        "system_idle_suspend_battery_sec":900,"system_idle_suspend_ac_sec":3600,
        "system_idle_screensaver_battery_sec":0,"system_idle_screensaver_ac_sec":300,
        "system_enable_low_power_downloads":false,"system_allow_battery_low_power_downloads":false,
        "steamos_cec_control":true,"steamos_cec_wake_device":false,"steamos_cec_suspend_device":true,
        "steamos_cec_wake_tv":false,"steamos_cec_suspend_tv":true},
        "power":{"battery":true,"percent":true,"dim_battery":60,"dim_ac":300,"cec_wake_supported":false}})
}

fn harness(snapshot: Value) -> (crate::AppWindow, App) {
    let (ui, mut a) = app();
    ui.global::<crate::Shell>().set_page(4);
    let label = rail_label();
    ui.global::<SettingsState>().set_rail(ModelRc::new(VecModel::from(vec![ListEntry { label: label.as_str().into(), ..Default::default() }])));
    a.se.pages = vec![Page { id: PAGE.into(), label }];
    a.router.replace(Route::Settings(PAGE.into()));
    a.se.zone = 1;
    a.se.snapshot = snapshot;
    a.se.rows = rows_for(&a.se.snapshot);
    a.se.on = vec![0; a.se.rows.len()];
    a.se.cf = a.se.rows.iter().position(|r| r.f);
    a.push_settings();
    (ui, a)
}
fn tok(a: &App) -> String { a.se.rows[a.se.cf.unwrap()].tok.clone() }
fn focus(a: &mut App, t: &str) { a.se.cf = a.se.rows.iter().position(|r| r.tok == t); assert!(a.se.cf.is_some(), "{t}"); }
fn titles(rows: &[PRow]) -> Vec<String> { rows.iter().filter(|r| r.k == 31).map(|r| r.lab.clone()).collect() }

#[test]
fn layout_lint_for_every_device_variant() {
    for (name, mut s) in [("deck", full()), ("no-battery", json!({"c":{"system_enable_low_power_downloads":true},"power":{"battery":false}})), ("no-snapshot", Value::Null)] {
        if name == "deck" { s["power"]["battery"] = json!(true); }
        let rows = rows_for(&s);
        let mut last = 0f32;
        for r in &rows {
            assert!(r.w > 0. && r.h > 0. && r.y * SC >= 160., "{name}: {r:?}");
            assert!(r.x * SC >= 395. && (r.x + r.w) * SC <= 1246., "{name}: row leaves the column {:?}", r.lab);
            assert!(r.y + 0.01 >= last, "{name}: rows out of order at {:?}", r.lab);
            last = r.y + r.h - 0.01;
        }
        let f: Vec<&PRow> = rows.iter().filter(|r| r.f).collect();
        for (i, a) in f.iter().enumerate() { for b in &f[i + 1..] {
            let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
            assert!(!overlap, "{name}: focusable rows overlap {} / {}", a.tok, b.tok);
        } }
    }
}

#[test]
fn sections_follow_the_stock_page() {
    let rows = rows_for_locale(&full(), russian());
    assert_eq!(titles(&rows), ["Батарея", "При работе от батареи и бездействии", "При работе от сети и бездействии",
        "Завершить загрузки перед переходом в спящий режим", "HDMI CEC"]);
    let toks: Vec<&str> = rows.iter().filter(|r| r.f).map(|r| r.tok.as_str()).collect();
    assert_eq!(&toks[..4], ["power-tg:pct", "power-tg:limit", "power-slider", "power-dd:dim_battery"]);
    // no battery: no battery section, one idle section «При бездействии», no «от батареи» download option
    let nb = rows_for_locale(&json!({"c":{"system_enable_low_power_downloads":true},"power":{"battery":false}}), russian());
    assert_eq!(titles(&nb)[0], "При бездействии");
    assert!(!nb.iter().any(|r| r.tok == "power-tg:pct" || r.tok == "power-slider" || r.tok == "power-tg:lowpower_battery"));
    // charge limit unsupported (key absent from live settings): no limit rows
    let nl = rows_for(&json!({"c":{"system_enable_low_power_downloads":false}}));
    assert!(nl.iter().any(|r| r.tok == "power-tg:pct") && !nl.iter().any(|r| r.tok == "power-slider" || r.tok == "power-tg:limit"));
}

#[test]
fn idle_options_follow_stock_and_keep_the_current_value() {
    let o = options_for_locale("system_idle_suspend_battery_sec", &full(), russian()).unwrap();
    let names: Vec<&str> = o.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["Режим отключён", "1 мин.", "5 мин.", "15 мин.", "30 мин.", "1 час"]);
    let custom = json!({"c":{"system_idle_suspend_battery_sec":120}});
    let o = options_for_locale("system_idle_suspend_battery_sec", &custom, russian()).unwrap();
    assert_eq!(o[2].0, "2 мин.");
    assert!(options("system_idle_suspend_battery_sec", &Value::Null).is_none());
    assert_eq!(idle_label(90, russian()), "1 мин. 30 сек.");
    assert_eq!((idle_label(1200, russian()).as_str(), idle_label(7200, russian()).as_str(), idle_label(3900, russian()).as_str(), idle_label(45, russian()).as_str()), ("20 мин.", "2 ч.", "1 час и 5 мин.", "45 сек."));
}

#[test]
fn english_titles_and_idle_options() {
    let en = i18n::by_code("en").unwrap();
    let rows = rows_for_locale(&full(), en);
    assert_eq!(titles(&rows), ["Battery", "When On Battery and Idle", "When Plugged In and Idle",
        "Finish Downloads Before Sleep", "HDMI CEC"]);
    let names = options_for_locale("system_idle_suspend_battery_sec", &full(), en).unwrap();
    assert_eq!(names.into_iter().map(|(name, _)| name).collect::<Vec<_>>(),
        ["Disabled", "1 minute", "5 minutes", "15 minutes", "30 minutes", "1 hour"]);
    assert_eq!(idle_label(121, en), "2 minutes, 1 second");
    assert_eq!(idle_label(3900, en), "1 hour and 5 minutes");
    assert_eq!(idle_label(93_600, russian()), "1 день и 2 ч.");
}

#[test]
fn control_states() {
    let s = full();
    let st = |t: &str| available_on(rows_for(&s).iter().find(|r| r.tok == t).unwrap(), &s);
    assert_eq!((st("power-tg:pct"), st("power-tg:limit"), st("power-slider"), st("power-dd:dim_ac")), (1, 1, 1, 1));
    assert_eq!((st("power-tg:cec_wake_device"), st("power-tg:cec_suspend_tv"), st("power-tg:cec_wake_tv")), (-1, 1, 0));
    let mut off = full();
    off["c"]["steamos_cec_control"] = json!(false);
    off["c"]["steamos_charge_limit_enabled"] = json!(false);
    let st = |t: &str| available_on(rows_for(&off).iter().find(|r| r.tok == t).unwrap(), &off);
    assert_eq!(st("power-tg:cec_suspend_tv"), -1); // stock displayOffIfDisabled
    assert!(rows_for(&off).iter().all(|r| r.tok != "power-slider"));
    // unread values are never shown as a fake off
    let none = Value::Null;
    assert!(rows_for(&none).iter().filter(|r| matches!(r.k, 2 | 4 | 27 | 29 | 30)).all(|r| available_on(r, &none) <= 0));
    assert_eq!(available_on(rows_for(&none).iter().find(|r| r.tok == "power-tg:limit").unwrap(), &none), -1);
}

#[test]
fn pad_walk_visits_every_control_in_order_and_stops_at_the_ends() {
    let (_ui, mut a) = harness(full());
    let expect: Vec<String> = a.se.rows.iter().filter(|r| r.f).map(|r| r.tok.clone()).collect();
    let mut seen = vec![tok(&a)];
    for _ in 0..expect.len() + 3 {
        a.handle_settings(Action::Down);
        if seen.last() != Some(&tok(&a)) { seen.push(tok(&a)); }
    }
    assert_eq!(seen, expect, "Down walks every control once and stops at the last");
    for _ in 0..expect.len() + 3 { a.handle_settings(Action::Up); }
    assert_eq!(tok(&a), expect[0]);
    a.handle_settings(Action::Left);
    assert_eq!(a.se.zone, 0, "Left from a toggle returns to the rail");
}

#[test]
fn toggle_writes_only_when_available_and_updates_the_snapshot() {
    let (_ui, mut a) = harness(full());
    focus(&mut a, "power-tg:lowpower");
    a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["system_enable_low_power_downloads"], json!(true));
    a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["system_enable_low_power_downloads"], json!(false));
    // unsupported wake-on-TV and the unread percentage preference never change anything
    let before = a.se.snapshot.clone();
    focus(&mut a, "power-tg:cec_wake_device"); a.handle_settings(Action::A);
    focus(&mut a, "power-tg:pct"); a.se.snapshot["power"]["percent"] = Value::Null; a.push_settings(); a.handle_settings(Action::A);
    a.se.snapshot["power"]["percent"] = before["power"]["percent"].clone();
    assert_eq!(a.se.snapshot, before);
    // a readable percentage preference toggles (DRYRUN: only the log line, the snapshot keeps the value)
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
    focus(&mut a, "power-tg:pct"); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["power"]["percent"], json!(false));
    a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
    // CEC control off blocks the dependent switches
    focus(&mut a, "power-tg:cec"); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["steamos_cec_control"], json!(false));
    focus(&mut a, "power-tg:cec_suspend_tv"); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["steamos_cec_suspend_tv"], json!(true));
}

#[test]
fn dropdown_opens_selects_and_cancels_with_b() {
    let (_ui, mut a) = harness(full());
    focus(&mut a, "power-dd:sleep_battery");
    let row = a.se.cf.unwrap();
    a.handle_settings(Action::A);
    assert!(a.se.recording_menu.is_some());
    let start = a.se.rows.iter().position(|r| r.tok.starts_with("recording-option:")).unwrap();
    assert_eq!(a.se.cf, Some(start + 3), "the current value (15 мин) is focused");
    a.handle_settings(Action::Up); assert_eq!(a.se.cf, Some(start + 2));
    assert!(a.settings_back());
    assert!(a.se.recording_menu.is_none() && a.se.cf == Some(row));
    assert_eq!(a.se.snapshot["c"]["system_idle_suspend_battery_sec"], json!(900), "cancel writes nothing");
    a.handle_settings(Action::A);
    let start = a.se.rows.iter().position(|r| r.tok.starts_with("recording-option:")).unwrap();
    a.handle_settings(Action::Up); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["system_idle_suspend_battery_sec"], json!(300));
    assert!(a.se.recording_menu.is_none() && a.se.rows.iter().all(|r| !r.tok.starts_with("recording-option:")));
    let _ = start;
    // dim goes through the display store, not the client settings
    focus(&mut a, "power-dd:dim_ac"); a.handle_settings(Action::A);
    a.handle_settings(Action::Up); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["power"]["dim_ac"], json!(60));
    assert!(a.se.snapshot["c"].get("dim_ac").is_none());
}

#[test]
fn slider_steps_clamp_and_write_once_after_the_debounce() {
    let (_ui, mut a) = harness(full());
    focus(&mut a, "power-slider");
    for _ in 0..15 { a.handle_settings(Action::Right); }
    assert_eq!(a.se.snapshot["c"]["steamos_charge_limit"], json!(100));
    assert_eq!(a.se.zone, 1, "Right at the end does not leave the row");
    for _ in 0..60 { a.handle_settings(Action::Left); }
    assert_eq!(a.se.snapshot["c"]["steamos_charge_limit"], json!(50), "stock floor max(50, charge_limit_min)");
    assert_eq!(tok(&a), "power-slider");
    assert!(PENDING_LIMIT.with(|p| p.get()) == Some(50));
    a.handle_settings(Action::Up); // leaving the row flushes the pending write
    assert!(PENDING_LIMIT.with(|p| p.get()).is_none());
    // limit disabled: the slider disappears (stock dynamic visibility) and the value stays
    a.se.snapshot["c"]["steamos_charge_limit_enabled"] = json!(false); a.push_settings();
    assert!(!a.se.rows.iter().any(|r| r.tok == "power-slider"));
    assert_eq!(a.se.snapshot["c"]["steamos_charge_limit"], json!(50));
    a.se.snapshot["c"]["steamos_charge_limit_enabled"] = json!(true); a.push_settings();
    assert!(a.se.rows.iter().any(|r| r.tok == "power-slider"));
}

#[test]
fn render_pngs() {
    let (ui, mut a) = harness(full());
    let st = ui.global::<SettingsState>();
    assert!(st.get_is_recording());
    shot(&ui, "power-top");
    for (name, t) in [("limit", "power-slider"), ("cec", "power-tg:cec_suspend_tv")] {
        focus(&mut a, t); a.settings_scroll_to_focus(); a.push_settings(); shot(&ui, &format!("power-{name}"));
    }
    // the owner's stock captures: dim 20 min, screensaver off, sleep 20 min on battery; 10 / 5 / 15 min on AC
    a.se.snapshot["power"]["dim_battery"] = json!(1200); a.se.snapshot["power"]["dim_ac"] = json!(600);
    a.se.snapshot["c"]["system_idle_suspend_battery_sec"] = json!(1200); a.se.snapshot["c"]["system_idle_suspend_ac_sec"] = json!(900);
    a.se.snapshot["c"]["system_idle_screensaver_battery_sec"] = json!(0); a.se.snapshot["c"]["system_idle_screensaver_ac_sec"] = json!(300);
    focus(&mut a, "power-tg:pct"); a.se.vy = 0.; a.push_settings(); shot(&ui, "power-stock-top");
    focus(&mut a, "power-dd:dim_battery"); a.se.vy = 0.; a.handle_settings(Action::A); shot(&ui, "power-menu");
    a.settings_back();
    focus(&mut a, "power-dd:sleep_ac"); a.settings_scroll_to_focus(); a.push_settings(); shot(&ui, "power-stock-mid");
    a.se.snapshot = json!({"c":{"system_enable_low_power_downloads":true},"power":{"battery":false}});
    a.se.cf = None; a.se.vy = 0.; a.push_settings(); shot(&ui, "power-no-battery");
}

#[test]
fn card_heights_match_the_stock_captures() {
    let rows = rows_for_locale(&full(), russian());
    let h = |t: &str| rows.iter().find(|r| r.tok == t).map(|r| (r.h * SC).round() as i32).unwrap();
    // measured on the owner's 1280x800 captures: 96 (label + 1 line), 68 (1 line), 96 two-line drop-down, 174 / 150 (2-line label + 3 / 2 lines)
    assert_eq!(h("power-tg:pct"), 96);
    assert_eq!(h("power-tg:limit"), 68);
    assert_eq!(h("power-slider"), 78);
    assert_eq!(h("power-dd:dim_battery"), 96);
    assert!((h("power-tg:lowpower") - 173).abs() <= 1, "{}", h("power-tg:lowpower"));
    assert!((h("power-tg:lowpower_battery") - 150).abs() <= 1, "{}", h("power-tg:lowpower_battery"));
    assert!((h("power-tg:cec_wake_device") - 126).abs() <= 1);
    assert_eq!(h("power-tg:cec"), 68);
    assert_eq!((h("power-tg:cec_suspend_device"), h("power-tg:cec_wake_tv"), h("power-tg:cec_suspend_tv")), (96, 68, 68));
    // positions from the first capture: first card at y 207, slider card 391, second section title centred 531
    let y = |t: &str| rows.iter().find(|r| r.tok == t).map(|r| (r.y * SC).round() as i32).unwrap();
    assert_eq!((y("power-tg:pct"), y("power-tg:limit"), y("power-slider"), y("power-dd:dim_battery")), (207, 312, 391, 558));
}

#[test]
fn slider_exists_only_while_the_limit_is_on_and_uses_the_stock_range() {
    let mut s = full();
    assert!(rows_for(&s).iter().any(|r| r.tok == "power-slider"));
    assert_eq!(limit_range(&s), (50, 100, 80));
    assert_eq!((slider_fill(&s).as_str(), slider_tick(&s).as_str()), ("80", "60"));
    s["c"]["steamos_charge_limit_devmode"] = json!(true); s["power"]["charge_limit_min"] = json!(10);
    assert_eq!(limit_range(&s).0, 10);
    // SteamOSManager range: without dev mode the floor is max(50, min), with it the raw min; max and default come from Steam
    s["power"]["charge_limit_min"] = json!(20); s["power"]["charge_limit_max"] = json!(95); s["power"]["charge_limit_default"] = json!(70);
    assert_eq!(limit_range(&s), (20, 95, 70));
    s["c"]["steamos_charge_limit_devmode"] = json!(false);
    assert_eq!(limit_range(&s), (50, 95, 70));
    s["c"]["steamos_charge_limit"] = json!(95);
    assert_eq!(slider_fill(&s), "100");
    s["power"]["charge_limit_default"] = json!(200); // never outside the track
    assert_eq!(limit_range(&s).2, 95);
    s["power"]["charge_limit_available"] = json!(false);
    assert!(!rows_for(&s).iter().any(|r| r.tok == "power-slider" || r.tok == "power-tg:limit"), "is_charge_limit_available=false hides the rows");
    s["power"]["charge_limit_available"] = json!(true); s["power"]["charge_limit_default"] = json!(80);
    s["c"]["steamos_charge_limit_enabled"] = json!(false);
    assert!(!rows_for(&s).iter().any(|r| r.tok == "power-slider"));
    s["c"]["steamos_charge_limit_enabled"] = Value::Null;
    assert!(!rows_for(&s).iter().any(|r| r.tok == "power-slider"));
}

#[test]
fn cec_wake_row_follows_the_steam_flag() {
    let mut s = full();
    let wake = |s: &Value| { let r = rows_for(s); let r = r.iter().find(|r| r.tok == "power-tg:cec_wake_device").unwrap().clone(); (available_on(&r, s), r.desc.clone()) };
    let (state, desc) = wake(&s);
    assert_eq!(state, -1); assert_eq!(desc, label(i18n::current(), "Settings_CEC_WakeDeviceSupportedDesc"));
    s["power"]["cec_wake_supported"] = json!(true);
    let (state, desc) = wake(&s);
    assert_eq!(state, 0); assert!(desc.is_empty());
}

#[test]
fn dim_is_written_as_the_stock_system_settings_message() {
    // CMsgSystemSettings field 1 (battery) / 2 (ac), float: tag 0x0d / 0x15 + f32 LE of 1200.0 = 00 00 96 44
    assert_eq!(crate::setters::encode_idle_dim(false, 1200), "DQAAlkQ=");
    assert_eq!(crate::setters::encode_idle_dim(true, 1200), "FQAAlkQ=");
}

#[test]
fn wrap_estimates_agree_with_the_captures() {
    assert_eq!(wrap_lines(&russian().tr("Settings_LowPowerDownloads_Enable"), LABEL_PX, LABEL_W), 2);
    assert_eq!(wrap_lines(&russian().tr("Settings_CEC_WakeDevice"), LABEL_PX, LABEL_W), 2);
    assert_eq!(wrap_lines(&russian().tr("Settings_CEC_SuspendDevice"), LABEL_PX, LABEL_W), 2);
    assert_eq!(wrap_lines(&russian().tr("Settings_CEC_SuspendTV"), LABEL_PX, LABEL_W), 1);
    assert_eq!(wrap_lines(&russian().tr("Settings_CEC_WakeTV"), LABEL_PX, LABEL_W), 1);
    assert_eq!(wrap_lines(&russian().tr("Settings_Battery_ChargeLimitEnable"), LABEL_PX, LABEL_W), 1);
    assert_eq!(wrap_lines(&russian().tr("Settings_LowPowerDownloads_Details").trim().to_string(), DESC_PX, LABEL_W), 3);
    assert_eq!(wrap_lines(&russian().tr("Settings_LowPowerDownloads_AllowBatteryDetails"), DESC_PX, LABEL_W), 2);
    assert_eq!(wrap_lines(&russian().tr("Settings_System_Idle_Sleep_Label"), LABEL_PX, DD_LABEL_W), 2);
    assert_eq!(wrap_lines(&russian().tr("Settings_System_Idle_Screensaver_Label"), LABEL_PX, DD_LABEL_W), 1);
    assert_eq!(wrap_lines(&russian().tr("Settings_System_Idle_Dim_Label"), LABEL_PX, DD_LABEL_W), 1);
}

#[test]
fn rail_uses_the_original_stock_icons_for_every_page() {
    let (ui, mut a) = harness(full());
    crate::assets::load_icons_for_test(&ui);
    a.build_settings_rail();
    let rail = ui.global::<SettingsState>().get_rail();
    assert_eq!(rail.row_count(), a.se.pages.len());
    for (i, p) in a.se.pages.iter().enumerate() {
        let name = crate::assets::settings_rail_icon(&p.id).unwrap_or_else(|| panic!("no stock icon for {}", p.id));
        assert!(crate::assets::try_icon_image(name).is_some(), "{name} does not load");
        assert!(rail.row_data(i).unwrap().has_icon, "{} has no rail icon", p.id);
    }
    assert_eq!(crate::assets::settings_rail_icon("Power"), Some("Rail-Lightning"));
    a.se.zone = 0; a.se.cursor = 4; a.push_settings();
    shot(&ui, "power-rail");
}

// ---- task 490: every visible row is reachable, hidden rows free their space, the percentage switch settles on the stored value ----

fn variants() -> Vec<(&'static str, Value)> {
    let mut limit_off = full(); limit_off["c"]["steamos_charge_limit_enabled"] = json!(false);
    let mut cec_off = full(); cec_off["c"]["steamos_cec_control"] = json!(false);
    vec![("full", full()), ("limit-off", limit_off), ("cec-off", cec_off),
        ("no-battery", json!({"c":{"system_enable_low_power_downloads":true,"system_idle_suspend_ac_sec":0},"power":{"battery":false}})),
        ("no-snapshot", Value::Null)]
}

#[test]
fn down_visits_every_focusable_row_and_up_returns_in_every_variant() {
    for (name, s) in variants() {
        let (_ui, mut a) = harness(s);
        let expect: Vec<String> = a.se.rows.iter().filter(|r| r.f).map(|r| r.tok.clone()).collect();
        assert!(expect.len() >= 5, "{name}: {} focusable rows", expect.len());
        let mut seen = vec![tok(&a)];
        for _ in 0..expect.len() + 3 { a.handle_settings(Action::Down); if seen.last() != Some(&tok(&a)) { seen.push(tok(&a)); } }
        assert_eq!(seen, expect, "{name}: Down");
        let mut back = vec![tok(&a)];
        for _ in 0..expect.len() + 3 { a.handle_settings(Action::Up); if back.last() != Some(&tok(&a)) { back.push(tok(&a)); } }
        back.reverse();
        assert_eq!(back, expect, "{name}: Up");
        // the focus ring is drawn on the row Rust focused, and that row is on screen
        let st = _ui.global::<SettingsState>();
        assert_eq!(st.get_cfocus(), a.se.cf.unwrap() as i32, "{name}");
    }
}

#[test]
fn a_press_on_every_focusable_row_is_handled_not_only_drawn() {
    for (name, s) in variants() {
        let (_ui, mut a) = harness(s);
        let toks: Vec<String> = a.se.rows.iter().filter(|r| r.f).map(|r| r.tok.clone()).collect();
        for t in toks {
            focus(&mut a, &t);
            let (snap, toasts) = (a.se.snapshot.clone(), a.se.recording_menu.is_some());
            let available = available_on(&a.se.rows[a.se.cf.unwrap()].clone(), &a.se.snapshot);
            a.handle_settings(Action::A);
            let opened = a.se.recording_menu.is_some();
            let changed = a.se.snapshot != snap;
            if t.starts_with("power-dd:") && available >= 0 { assert!(opened, "{name}: {t} opens its list"); assert!(a.settings_back()); }
            else if t.starts_with("power-tg:") && available >= 0 { assert!(changed, "{name}: {t} switches"); a.handle_settings(Action::A); assert_eq!(available_on(&a.se.rows[a.se.cf.unwrap()].clone(), &a.se.snapshot), available, "{name}: {t} switches back"); }
            else { assert!(!opened && !changed, "{name}: {t} (unavailable) must not write"); }
            let _ = toasts;
            assert_eq!(tok(&a), t, "{name}: focus stays on {t} after A");
        }
    }
}

#[test]
fn live_snapshot_without_default_off_flags_keeps_switches_usable() {
    // Steam omits a proto field that holds its default: the CEC / low-power / limit switches are «off», not «unavailable»
    let s = json!({"c":{"system_idle_suspend_battery_sec":900,"system_idle_suspend_ac_sec":3600,
        "system_idle_screensaver_battery_sec":0,"system_idle_screensaver_ac_sec":0,"steamos_charge_limit":80},
        "power":{"battery":true,"percent":false,"dim_battery":60,"dim_ac":300,"charge_limit_available":true}});
    let rows = rows_for(&s);
    for t in ["power-tg:limit", "power-tg:lowpower", "power-tg:cec"] {
        let r = rows.iter().find(|r| r.tok == t).unwrap_or_else(|| panic!("{t} hidden"));
        assert_eq!(available_on(r, &s), 0, "{t}");
    }
    let (_ui, mut a) = harness(s);
    focus(&mut a, "power-tg:cec"); a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["c"]["steamos_cec_control"], json!(true));
    focus(&mut a, "power-tg:limit"); a.handle_settings(Action::A);
    assert!(a.se.rows.iter().any(|r| r.tok == "power-slider"));
}

fn ys(a: &App) -> Vec<(String, i32)> { a.se.rows.iter().map(|r| (format!("{}|{}", r.tok, r.lab), (r.y * SC).round() as i32)).collect() }

#[test]
fn hiding_and_showing_a_row_moves_everything_below_it() {
    let (ui, mut a) = harness(full());
    let with = ys(&a);
    let slider_h = (a.se.rows.iter().find(|r| r.tok == "power-slider").unwrap().h * SC).round() as i32 + CARD_GAP as i32 + 2;
    focus(&mut a, "power-tg:limit");
    a.handle_settings(Action::A); // limit off → the slider goes away
    assert!(a.se.rows.iter().all(|r| r.tok != "power-slider"));
    let without = ys(&a);
    assert_eq!(without.len() + 1, with.len());
    let after_slider = with.iter().position(|r| r.0.starts_with("power-slider")).unwrap();
    for (i, (key, y)) in with.iter().enumerate() {
        if i < after_slider { assert_eq!(without[i], (key.clone(), *y), "rows above stay"); }
        else if i > after_slider { assert_eq!(without[i - 1].1, y - slider_h, "{key} moves up by the freed space"); }
    }
    // the model the screen draws moved too, and the content got shorter
    let model = ui.global::<SettingsState>().get_content();
    use slint::Model;
    for (i, r) in a.se.rows.iter().enumerate() { assert_eq!(model.row_data(i).unwrap().y.round() as i32, (r.y * SC).round() as i32); }
    assert_eq!(model.row_count(), a.se.rows.len());
    assert_eq!(tok(&a), "power-tg:limit", "focus stays on the switch");
    a.handle_settings(Action::A); // and back
    assert_eq!(ys(&a), with, "showing it restores the layout exactly");
    let _ = ui;
}

#[test]
fn no_gap_is_left_where_a_row_is_hidden_in_any_variant() {
    for (name, s) in variants() {
        let rows = rows_for(&s);
        // between neighbouring rows of one section the gap is the stock 9 px (slider card 11), never a hidden row's height
        for w in rows.windows(2) {
            let gap = ((w[1].y - w[0].y - w[0].h) * SC).round() as i32;
            assert!((0..=60).contains(&gap), "{name}: gap {gap} between {} and {}", w[0].lab, w[1].lab);
        }
    }
    // the battery sections go away together with the battery and the page starts at the first idle section
    let rows = rows_for(&json!({"power":{"battery":false}}));
    assert_eq!(titles(&rows)[0], label(i18n::current(), "Settings_System_Idle_NoBattery_Section"));
    assert_eq!((rows[0].y * SC).round() as i32, FIRST_Y as i32);
}

#[test]
fn focus_does_not_vanish_when_the_focused_row_goes_away() {
    let (_ui, mut a) = harness(full());
    focus(&mut a, "power-slider");
    a.se.snapshot["c"]["steamos_charge_limit_enabled"] = json!(false); // changed elsewhere (Steam) while the slider was focused
    a.push_settings();
    assert!(a.se.cf.is_some(), "focus survives the removal");
    assert!(a.se.rows[a.se.cf.unwrap()].f);
    assert_eq!(tok(&a), "power-tg:limit", "the nearest control takes it");
    a.handle_settings(Action::Down);
    assert_eq!(tok(&a), "power-dd:dim_battery");
    // battery goes away: first control of the page
    a.se.snapshot["power"]["battery"] = json!(false); a.push_settings();
    assert!(a.se.cf.is_some());
}

#[test]
fn touch_activates_rows_through_the_screen_model() {
    let (_ui, mut a) = harness(full());
    let i = a.se.rows.iter().position(|r| r.tok == "power-tg:lowpower").unwrap();
    // the touch callback of the screen is `content-activated(i)`; the row must be a control that accepts it
    let model = _ui.global::<SettingsState>().get_content();
    use slint::Model;
    for (j, r) in a.se.rows.iter().enumerate().filter(|(_, r)| r.f) {
        let g = model.row_data(j).unwrap();
        assert!(g.ptr && g.fcs, "{}: touch/focus flags", r.tok);
        assert!(matches!(g.kind, 27 | 29 | 30) && g.on >= -1);
    }
    a.settings_activate(i);
    assert_eq!(a.se.snapshot["c"]["system_enable_low_power_downloads"], json!(true));
}

#[test]
fn percentage_switch_follows_the_stored_value() {
    let (_ui, mut a) = harness(full());
    a.se.cf = a.se.rows.iter().position(|r| r.tok == "power-tg:pct");
    a.handle_settings(Action::A);
    assert_eq!(a.se.snapshot["power"]["percent"], json!(false), "the switch flips at once");
    assert_eq!(crate::backend::PERCENT_WRITES.load(std::sync::atomic::Ordering::Relaxed), 0, "DRYRUN keeps no write pending");
    // the write was rejected: the read-back still says «on» → the switch goes back and says so
    a.power_percent_settled(false, Some(true), json!(true));
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
    // an unreadable read-back restores the previous value too; a matching one keeps the new value
    a.power_percent_settled(false, None, json!(true));
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
    a.power_percent_settled(false, Some(false), json!(true));
    assert_eq!(a.se.snapshot["power"]["percent"], json!(false));
    // a change made in Steam shows on the open page without a reload
    a.power_percent_external(true);
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
}

#[test]
fn render_hide_show_pngs() {
    let (ui, mut a) = harness(full());
    focus(&mut a, "power-tg:limit"); a.push_settings(); shot(&ui, "power-490-limit-on");
    a.handle_settings(Action::A); shot(&ui, "power-490-limit-off");
    a.se.snapshot["power"]["battery"] = json!(false); a.se.cf = None; a.push_settings(); shot(&ui, "power-490-no-battery");
}

// ---- task 497: real Deck replies (2026-11-19, read over CDP/SSH) ----

/// What `settings_snapshot.js` returned on the Deck, merged with the SteamOSManager limits as `merge_power_limits` does.
fn deck() -> Value { serde_json::from_str(include_str!("fixtures/deck_snapshot_2026-11-19.json")).unwrap() }

#[test]
fn deck_snapshot_has_every_row_usable_except_cec_while_it_is_off() {
    let s = deck();
    let rows = rows_for(&s);
    for r in rows.iter().filter(|r| r.f) {
        let st = available_on(r, &s);
        let cec_off = r.tok.starts_with("power-tg:cec_");
        // «Включить ввод с пульта» is off on the Deck: the dependent CEC rows are shown off by design; the percent row needs `power.percent`
        if cec_off || r.tok == "power-tg:pct" { continue; }
        assert!(st >= 0, "{} must be usable on the Deck snapshot", r.tok);
    }
    assert!(rows.iter().filter(|r| r.k == 30).all(|r| r.desc != i18n::tr("power.unavailable")), "dim/saver/sleep show the Deck values");
    let dd = |t: &str| rows.iter().find(|r| r.tok == t).unwrap().desc.clone();
    assert_eq!(dd("power-dd:sleep_battery"), idle_label(1200, i18n::current()));
    assert_eq!(dd("power-dd:dim_ac"), idle_label(600, i18n::current()));
    assert_eq!(dd("power-dd:saver_battery"), idle_label(0, i18n::current()));
}

/// The old snapshot delivered `c` as `{}` (MobX through returnByValue): every drop-down was unavailable and each toggle showed «off».
#[test]
fn empty_client_settings_object_is_not_taken_for_a_real_snapshot() {
    let s = json!({"c":{},"power":{"battery":null,"dim_battery":null,"dim_ac":null,"pref":"{}"}});
    let dd = rows_for(&s).into_iter().find(|r| r.tok == "power-dd:sleep_battery").unwrap();
    assert_eq!(available_on(&dd, &s), -1, "documents the failure the JS fix removes");
}

#[test]
fn snapshot_script_copies_scalars_instead_of_returning_the_observable() {
    let js = include_str!("../settings_snapshot.js");
    assert!(js.contains("scalars(S.m_ClientSettings)"), "c must be copied key by key");
    assert!(!js.contains("c: S.m_ClientSettings") && !js.contains("c:S.m_ClientSettings"));
    assert!(js.contains("RegisterForSettingsChanges") && js.contains("unregister"), "dim seconds: one message, then unregister");
    assert!(js.contains("p.Value"), "battery preferences are read from the observable's Value");
    assert!(!js.contains(".Get()"), "no store construction from the read-only snapshot");
}

#[test]
fn percent_flag_from_the_real_observable_wrapper() {
    // old reading (JSON.stringify of the observable) — the Deck's actual text
    let wrapper = json!("{\"m_callbacks\":{\"m_vecCallbacks\":[]},\"m_currentValue\":{\"bShowBatteryPercentage\":true}}");
    assert_eq!(battery_logic::pref_show(&wrapper), Some(true));
    assert_eq!(battery_logic::pref_show(&json!("{\"bShowBatteryPercentage\":true}")), Some(true)); // `.Value` text
    let mut s = deck();
    s["power"]["percent"] = json!(true);
    let r = rows_for(&s).into_iter().find(|r| r.tok == "power-tg:pct").unwrap();
    assert_eq!(available_on(&r, &s), 1);
}

#[test]
fn percent_write_on_the_wrapper_reply_is_confirmed_not_rolled_back() {
    let (_ui, mut a) = harness(deck());
    a.se.snapshot["power"]["percent"] = json!(false);
    // Steam stored `true`; the read-back text is the wrapper
    let stored = json!("{\"m_callbacks\":{\"m_vecCallbacks\":[]},\"m_currentValue\":{\"bShowBatteryPercentage\":true}}");
    a.power_percent_settled(true, battery_logic::pref_show(&stored), json!(false));
    assert_eq!(a.se.snapshot["power"]["percent"], json!(true));
}

#[test]
fn header_battery_slot_shrinks_when_percent_is_hidden() {
    let (ui, _a) = harness(full());
    let bar = ui.global::<crate::TopBarState>();
    let lay = ui.global::<crate::HeaderLayout>();
    bar.set_bat_pct(27);
    bar.set_bat_show_pct(true);
    let with_pct = lay.get_battery_inset();
    bar.set_bat_show_pct(false);
    let without = lay.get_battery_inset();
    assert!(with_pct - without > 50.0, "the percent column must collapse: {with_pct} vs {without}");
    // everything left of the battery follows the shift
    bar.set_wifi(1);
    assert_eq!(lay.get_wifi_inset(), without + lay.get_wifi_width());
    bar.set_bat_show_pct(true);
    assert_eq!(lay.get_wifi_inset(), with_pct + lay.get_wifi_width());
    bar.set_bat_pct(-1);
    assert_eq!(lay.get_battery_inset(), 0.0);
}

/// Task 499: the owner saw ~100px of nothing between Wi-Fi and battery with percent off. Slot geometry in both states;
/// the focus block / hit area must end at the screen edge (width = inset), not stick out by the collapsed text column.
#[test]
fn header_battery_layout_both_percent_states_with_png() {
    let (ui, _a) = harness(full());
    let bar = ui.global::<crate::TopBarState>();
    let lay = ui.global::<crate::HeaderLayout>();
    bar.set_bat_pct(27);
    bar.set_wifi(1);
    bar.set_bat_text("27%".into());
    bar.set_bat_show_pct(false);
    // Wi-Fi sits right next to the battery icon: slot = icon column (56.99 + 15.3 padding), no text column
    assert!((lay.get_battery_inset() - 72.3).abs() < 0.1, "{}", lay.get_battery_inset());
    assert!((lay.get_wifi_inset() - lay.get_battery_inset() - lay.get_wifi_width()).abs() < 0.01);
    shot(&ui, "header-499-percent-off");
    bar.set_hfocus(5);
    shot(&ui, "header-499-percent-off-battery-focus");
    bar.set_hfocus(-1);
    bar.set_bat_show_pct(true);
    assert_eq!(lay.get_battery_inset(), lay.get_battery_width());
    assert!((lay.get_battery_inset() - 130.375).abs() < 0.01);
    shot(&ui, "header-499-percent-on");
    bar.set_hfocus(5);
    shot(&ui, "header-499-percent-on-battery-focus");
}

#[test]
fn header_focus_block_and_hit_use_the_collapsed_battery_width() {
    for f in ["app/ui/widgets/header_hits.slint"] {
        let s = std::fs::read_to_string(format!("{}/../{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let bat: Vec<_> = s.lines().filter(|l| l.contains("HeaderLayout.battery-inset;")).collect();
        assert_eq!(bat.len(), 2);
        assert!(bat.iter().all(|l| l.contains("width: HeaderLayout.battery-inset;")), "width must follow the inset");
        assert!(!s.contains("HeaderLayout.battery-width"));
    }
}

// ---- task 504: second real capture (2026-11-20, read-only over CDP): charge limit and CEC are now ON, screensaver 60 s on battery ----
fn deck2() -> Value { serde_json::from_str(include_str!("fixtures/deck_snapshot_2026-11-20.json")).unwrap() }

#[test]
fn deck_20_11_every_row_reads_its_real_value() {
    let s = deck2();
    let rows = rows_for(&s);
    for r in rows.iter().filter(|r| r.f) {
        if r.tok == "power-tg:pct" { continue; } // `power.percent` is filled by the live read-back, not by the snapshot
        // GetState says is_extended_cec_wake_device_supported:false on this Deck: the row is greyed, as in stock
        if r.tok == "power-tg:cec_wake_device" { assert_eq!(available_on(r, &s), -1); continue; }
        assert!(available_on(r, &s) >= 0, "{} must be usable on the 20.11 Deck snapshot", r.tok);
    }
    let dd = |t: &str| rows.iter().find(|r| r.tok == t).unwrap().desc.clone();
    assert_eq!(dd("power-dd:sleep_battery"), idle_label(1200, i18n::current()));
    assert_eq!(dd("power-dd:sleep_ac"), idle_label(900, i18n::current()));
    assert_eq!(dd("power-dd:saver_battery"), idle_label(60, i18n::current()));
    assert_eq!(dd("power-dd:saver_ac"), idle_label(300, i18n::current()));
    assert_eq!(dd("power-dd:dim_battery"), idle_label(1200, i18n::current()));
    assert_eq!(dd("power-dd:dim_ac"), idle_label(600, i18n::current()));
}

#[test]
fn deck_20_11_percent_pref_reads_from_value_text() {
    let s = deck2();
    assert_eq!(battery_logic::pref_show(&s["power"]["pref"]), Some(true));
}
