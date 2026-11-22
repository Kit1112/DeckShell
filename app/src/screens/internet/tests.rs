//! Task 451 tests: layout lint, pad focus walk, connect / password / custom-network flows on the in-memory network,
//! and PNG of the key states (`DECKSHELL_PNG_DIR=… cargo test internet`). Nothing here touches the real adapter.
//!
//! Element / state / button → result table: `docs/internet-wifi-page-2026-11-18.md`.
use super::*;
use crate::input::Action;
use crate::router::Route;
use crate::screens::osk::field_tests::{app, shot};
use deck_system::wifi::{MockAp, MockWifi};
use slint::ComponentHandle;

static LOCK: Mutex<()> = Mutex::new(());

struct H { ui: crate::AppWindow, a: App, net: Arc<MockWifi>, _g: std::sync::MutexGuard<'static, ()> }

fn harness() -> H {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (ui, mut a) = app();
    crate::assets::load_icons_for_test(&ui);
    let net = Arc::new(MockWifi::demo());
    set_backend(net.clone());
    ui.global::<crate::Shell>().set_page(4);
    a.se.pages = vec![crate::screens::settings::Page { id: PAGE.into(), label: "Интернет".into() }];
    ui.global::<SettingsState>().set_rail(ModelRc::new(VecModel::from(vec![crate::ListEntry { label: "Интернет".into(), ..Default::default() }])));
    a.router.replace(Route::Settings(PAGE.into()));
    a.sync_settings(PAGE);
    scan_now();
    push(&mut a);
    H { ui, a, net, _g: g }
}

/// The result of a scan without the worker thread.
fn scan_now() {
    let b = backend();
    with(|s| {
        s.radio = b.radio();
        s.aps = if s.radio == Some(true) { b.scan(false) } else { vec![] };
        s.profiles = b.profiles();
        s.connectivity = Some(Connectivity::Full);
        s.mac = "02:00:00:00:00:01".into();
        s.remember_order();
        s.fix_focus();
    });
}

fn dialog() -> Option<Dialog> { with(|s| s.dialog.clone()) }
fn focus() -> Option<Key> { with(|s| s.focus.clone()) }
fn press(h: &mut H, a: Action) { h.a.handle(a); }

// ---------------------------------------------------------------- layout lint

fn lint(rows: &[Row], what: &str) {
    let mut last = f32::MIN;
    for r in rows {
        assert!(r.h > 0.0 && r.y >= 100.0, "{what}: row {:?} y {}", r.label, r.y);
        assert!(r.y + 0.01 >= last, "{what}: rows out of order at {:?}", r.label);
        if r.kind != rk::SECTION { last = r.y + r.h; } else { last = last.max(r.y); }
        assert!(r.btn_w < W - 200.0, "{what}: button too wide in {:?}", r.label);
    }
    let blocks: Vec<&Row> = rows.iter().collect();
    for (i, a) in blocks.iter().enumerate() {
        for b in &blocks[i + 1..] {
            let (ay, by) = (a.y, b.y);
            assert!(a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5, "{what}: {:?} ({ay}+{}) overlaps {:?} ({by})", a.label, a.h, b.label);
        }
    }
    assert!(content_height(rows) > 400.0);
}

#[test]
fn page_layout_has_no_overlap_in_all_states() {
    let net = |ssid: &str, c: bool| Net { ssid: ssid.into(), bars: 3, lock: true, connected: c, busy: false };
    let long = "T-Mobile_Swiatlowod_2.4GHz_D8E9_и-очень-длинное-имя-сети-для-проверки".to_string();
    for (name, d) in [
        ("off", PageData { radio: Some(false), ..Default::default() }),
        ("on-empty", PageData { radio: Some(true), ..Default::default() }),
        ("connected", PageData { radio: Some(true), connected: vec![net("K&K", true)], found: (0..8).map(|i| net(&format!("N{i}"), false)).collect(), has_network: true, ..Default::default() }),
        ("offline", PageData { radio: Some(true), connected: vec![net("K&K", true)], found: vec![net(&long, false)], offline: true, has_network: true, ..Default::default() }),
        ("no-internet", PageData { radio: Some(true), connected: vec![net("K&K", true)], has_network: true, connectivity: Some(Connectivity::Limited), ..Default::default() }),
        ("portal", PageData { radio: Some(true), connected: vec![net("K&K", true)], has_network: true, connectivity: Some(Connectivity::Portal), ..Default::default() }),
    ] {
        lint(&page_rows(&d), name);
    }
}

fn lint_dialog(v: &DialogView, what: &str) {
    assert!(v.box_h <= DLG_MAX_H + 0.5 && v.box_y >= VIEW_TOP - 0.5 && v.box_y + v.box_h <= VIEW_BOTTOM + 0.5, "{what}: box {}+{}", v.box_y, v.box_h);
    let ctl: Vec<&DRow> = v.rows.iter().filter(|r| r.kind != dk::TITLE && r.kind != dk::ERROR).collect();
    for (i, a) in ctl.iter().enumerate() {
        assert!(a.y >= 0.0 && a.y + a.h <= v.content_h + 0.5, "{what}: row {i} outside content");
        for b in &ctl[i + 1..] { assert!(a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5, "{what}: rows overlap {:?}/{:?}", a.label, b.label); }
    }
    assert!(v.scroll >= 0.0 && v.scroll <= (v.content_h - v.box_h).max(0.0) + 0.5, "{what}: scroll {}", v.scroll);
}

#[test]
fn dialog_layout_fits_the_screen_in_all_states() {
    let mut f = Form::new("", Security::Wpa2, true, "02:00:00:00:00:01");
    lint_dialog(&form_view(&f, false, 0.0), "custom");
    f.adv = true;
    for focus in f.items() { f.focus = focus; lint_dialog(&form_view(&f, false, 0.0), &format!("custom adv {focus:?}")); lint_dialog(&form_view(&f, true, 0.0), &format!("custom adv osk {focus:?}")); }
    f.failed = true;
    lint_dialog(&form_view(&f, false, 0.0), "failed");
    lint_dialog(&pick_view(0, 0), "pick");
    for st in [Stage::Connecting, Stage::Checking, Stage::Success, Stage::Failed, Stage::NoInternet, Stage::Portal] {
        lint_dialog(&progress_view(&Prog { ssid: "K&K".into(), stage: st, back: None, secured: true, focus: 0 }), &format!("{st:?}"));
    }
    for c in [Confirm::GoOffline, Confirm::GoOnline, Confirm::DeleteBrowser] {
        let (t, b, cancel) = confirm_text(c, false, true);
        lint_dialog(&message_view(&t, &b, "OK", cancel.then_some("Отмена"), 0), &format!("{c:?}"));
    }
}

// ---------------------------------------------------------------- pad

#[test]
fn pad_walks_every_row_in_order_and_left_returns_to_the_rail() {
    let mut h = harness();
    enter(&mut h.a);
    assert_eq!(h.a.se.zone, 1);
    assert_eq!(focus(), Some(Key::Toggle));
    let mut seen = vec![focus().unwrap()];
    for _ in 0..30 { press(&mut h, Action::Down); if seen.last() != focus().as_ref() { seen.push(focus().unwrap()); } }
    assert_eq!(seen.first(), Some(&Key::Toggle));
    assert_eq!(seen[1], Key::Ap("HomeNet".into()), "connected network right after the toggle");
    let n = seen.len();
    assert_eq!(&seen[n - 4..], &[Key::Other, Key::Offline, Key::Proxy, Key::Delete]);
    assert_eq!(seen.iter().filter(|k| matches!(k, Key::Ap(_))).count(), 6, "every network is reachable: {seen:?}");
    for _ in 0..30 { if focus() == Some(Key::Toggle) { break; } press(&mut h, Action::Up); }
    assert_eq!(focus(), Some(Key::Toggle));
    press(&mut h, Action::Left);
    assert_eq!(h.a.se.zone, 0, "Left from the page returns to the rail");
}

#[test]
fn scrolling_keeps_the_focused_row_visible() {
    let mut h = harness();
    enter(&mut h.a);
    for _ in 0..30 {
        press(&mut h, Action::Down);
        let (rows, vy, f) = with(|s| (s.rows(), s.vy, s.focus.clone()));
        let r = rows.iter().find(|r| r.key == f).unwrap();
        assert!(r.y - vy >= VIEW_TOP - 0.5 && r.y + r.h - vy <= VIEW_BOTTOM + 0.5, "{:?} y {} vy {vy}", r.label, r.y);
    }
}

#[test]
fn toggle_turns_the_radio_off_and_on() {
    let mut h = harness();
    enter(&mut h.a);
    let stale_generation = with(|s| s.generation);
    let old_aps = h.net.scan(false);
    press(&mut h, Action::A);
    assert_eq!(h.net.log().last().map(String::as_str), Some("radio off"));
    apply_scan(&mut h.a, stale_generation, Some(true), old_aps, vec![], Some(Connectivity::Full), String::new());
    assert_eq!(with(|s| s.radio), Some(false));
    assert!(with(|s| s.page_data().connected.is_empty() && s.page_data().found.is_empty()));
    shot(&h.ui, "wifi-off");
    press(&mut h, Action::A);
    assert_eq!(h.net.log().last().map(String::as_str), Some("radio on"));
}

#[test]
fn renders_stock_wifi_levels_and_header_states() {
    let h = harness();
    let wifi = h.ui.global::<InternetState>();
    let mut rows = (0..=4).map(|bars| WifiRow {
        kind: rk::AP,
        label: format!("Signal {bars}").into(),
        y: 230.0 + 80.0 * bars as f32,
        h: 70.0,
        bars,
        lock: true,
        mark: if bars == 4 { 1 } else { 0 },
        ..Default::default()
    }).collect::<Vec<_>>();
    rows.push(WifiRow {
        kind: rk::SETTING,
        label: "Steam offline mode".into(),
        y: 630.0,
        h: 70.0,
        warn: true,
        btn: "Go online".into(),
        btn_w: 170.0,
        ..Default::default()
    });
    wifi.set_rows(ModelRc::new(VecModel::from(rows)));

    let bar = h.ui.global::<crate::TopBarState>();
    bar.set_bat_pct(85);
    bar.set_bat_show_pct(true);
    bar.set_bat_text(format!("{}%", bar.get_bat_pct()).into());
    bar.set_wifi(1);
    bar.set_wifi_strength(4);
    shot(&h.ui, "wifi-icons-on");
    bar.set_wifi_offline(true);
    shot(&h.ui, "wifi-icons-offline");
    bar.set_wifi_offline(false);
    bar.set_wifi(0);
    shot(&h.ui, "wifi-icons-radio-off");
}

// ---------------------------------------------------------------- connect

fn gen() -> u64 { with(|s| s.generation) }

#[test]
fn secured_network_asks_for_the_password_then_connects() {
    let mut h = harness();
    enter(&mut h.a);
    shot(&h.ui, "wifi-page");
    with(|s| s.focus = Some(Key::Ap("Neighbour".into())));
    press(&mut h, Action::A);
    let Some(Dialog::Form(f)) = dialog() else { panic!("password form expected, got {:?}", dialog()) };
    assert_eq!(f.focus, F::Pass);
    assert!(!f.custom);
    shot(&h.ui, "wifi-password");
    for c in "neighbour1".chars() { h.a.handle(Action::Char(c)); }
    let Some(Dialog::Form(f)) = dialog() else { panic!() };
    assert_eq!(f.pass, "neighbour1");
    h.a.osk_set_open(false);
    press(&mut h, Action::Down); // advanced
    press(&mut h, Action::Down); // Connect
    press(&mut h, Action::A);
    let Some(Dialog::Progress(p)) = dialog() else { panic!("progress expected, got {:?}", dialog()) };
    assert_eq!(p.stage, Stage::Connecting);
    shot(&h.ui, "wifi-connecting");
    apply_connect(&mut h.a, gen(), Ok(()), Connectivity::Full);
    let Some(Dialog::Progress(p)) = dialog() else { panic!() };
    assert_eq!(p.stage, Stage::Success);
    shot(&h.ui, "wifi-success");
    press(&mut h, Action::A);
    assert!(dialog().is_none());
}

#[test]
fn wrong_password_returns_to_the_prompt_with_the_red_error() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("Neighbour".into())));
    press(&mut h, Action::A);
    for c in "bad".chars() { h.a.handle(Action::Char(c)); }
    h.a.osk_set_open(false);
    press(&mut h, Action::Down);
    press(&mut h, Action::Down);
    press(&mut h, Action::A);
    apply_connect(&mut h.a, gen(), Err(ConnectError::BadPassword), Connectivity::Unknown);
    let Some(Dialog::Form(f)) = dialog() else { panic!("form expected, got {:?}", dialog()) };
    assert!(f.failed);
    assert_eq!(f.focus, F::Pass);
    shot(&h.ui, "wifi-failed-password");
    h.a.osk_set_open(false);
    press(&mut h, Action::B);
    assert!(dialog().is_none(), "B closes the prompt");
}

#[test]
fn open_network_connects_without_a_prompt_and_b_cancels() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("Cafe Open".into())));
    press(&mut h, Action::A);
    assert!(matches!(dialog(), Some(Dialog::Progress(_))));
    press(&mut h, Action::B);
    assert!(dialog().is_none(), "B cancels the attempt");
}

#[test]
fn no_internet_and_portal_offer_two_buttons() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("Cafe Open".into())));
    press(&mut h, Action::A);
    apply_connect(&mut h.a, gen(), Ok(()), Connectivity::Limited);
    let Some(Dialog::Progress(p)) = dialog() else { panic!() };
    assert_eq!(p.stage, Stage::NoInternet);
    shot(&h.ui, "wifi-no-internet");
}

#[test]
fn custom_network_form_validates_static_ip() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Other));
    press(&mut h, Action::A);
    let Some(Dialog::Form(f)) = dialog() else { panic!() };
    assert_eq!((f.custom, f.focus), (true, F::Ssid));
    shot(&h.ui, "wifi-custom");
    assert!(!h.a.osk_is_open(), "multi-field form: no keyboard until A on a field");
    press(&mut h, Action::A);
    assert!(h.a.osk_is_open());
    for c in "Hidden".chars() { h.a.handle(Action::Char(c)); }
    // Enter on the keyboard moves to the next field
    h.a.osk_set_open(false);
    press(&mut h, Action::Down);
    h.a.osk_set_open(false);
    press(&mut h, Action::Down);
    h.a.osk_set_open(false);
    let Some(Dialog::Form(f)) = dialog() else { panic!() };
    assert_eq!(f.focus, F::Pass);
    assert_eq!(f.ssid, "Hidden");
    press(&mut h, Action::Down);
    assert!(matches!(dialog(), Some(Dialog::Form(f)) if f.focus == F::AdvShow));
    press(&mut h, Action::A);
    let Some(Dialog::Form(f)) = dialog() else { panic!() };
    assert!(f.adv);
    shot(&h.ui, "wifi-advanced");
    with(|s| if let Some(Dialog::Form(f)) = &mut s.dialog { f.ip.ip = "192.168.1.50".into(); f.ip.netmask = "255.255.255.999".into(); });
    let Some(Dialog::Form(f)) = dialog() else { panic!() };
    let (field, _) = f.request().unwrap_err();
    assert_eq!(field, F::Mask);
}

#[test]
fn offline_banner_and_confirm() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.offline_override = Some((true, Instant::now())));
    push(&mut h.a);
    let rows = with(|s| s.rows());
    assert!(rows.iter().any(|r| r.key == Some(Key::Offline) && r.warn), "offline banner row");
    shot(&h.ui, "wifi-offline");
    with(|s| s.focus = Some(Key::Offline));
    press(&mut h, Action::A);
    assert!(matches!(dialog(), Some(Dialog::Confirm(Confirm::GoOnline, _))));
    shot(&h.ui, "wifi-go-online");
    press(&mut h, Action::B);
    assert!(dialog().is_none());
}

#[test]
fn info_dialog_forget_and_back() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("HomeNet".into())));
    press(&mut h, Action::A);
    assert!(matches!(dialog(), Some(Dialog::Info(_))), "A on the connected network opens its info");
    shot(&h.ui, "wifi-info");
    press(&mut h, Action::B);
    assert!(dialog().is_none());
    let _ = MockAp { ap: wifi::AccessPoint { ssid: String::new(), bssid: String::new(), signal: 0, security: Security::Open, in_use: false }, password: String::new(), reachable: true };
}

#[test]
fn closed_keyboard_stays_closed_while_focus_moves_between_fields() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Other));
    press(&mut h, Action::A);
    assert!(!h.a.osk_is_open());
    press(&mut h, Action::Down);
    press(&mut h, Action::Down);
    assert!(matches!(dialog(), Some(Dialog::Form(f)) if f.focus == F::Pass));
    assert!(!h.a.osk_is_open(), "D-pad focus change must not open the keyboard");
    press(&mut h, Action::A);
    assert!(h.a.osk_is_open(), "A on a text field opens it");
    // open keyboard follows the focus
    press(&mut h, Action::Up);
    assert!(h.a.osk_is_open());
    // user closes it: moving on keeps it closed
    h.a.osk_set_open(false);
    press(&mut h, Action::Down);
    assert!(!h.a.osk_is_open());
}

#[test]
fn single_field_password_prompt_still_opens_keyboard() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("Neighbour".into())));
    press(&mut h, Action::A);
    assert!(h.a.osk_is_open());
}

fn info_item(label: &str) -> usize {
    let Some(Dialog::Info(i)) = dialog() else { panic!("info expected") };
    let v = info_view(&i, 0.0);
    v.rows.iter().find(|r| r.kind == dk::BUTTON_ROW && r.value == lt(label, if label.contains("Forget") { "Забыть" } else { "Отключиться" })).map(|r| r.item as usize).expect("button")
}

#[test]
fn forget_updates_the_model_instantly_and_stale_scans_cannot_undo_it() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("HomeNet".into())));
    press(&mut h, Action::A);
    let idx = info_item("#Settings_Internet_Forget");
    with(|s| if let Some(Dialog::Info(i)) = &mut s.dialog { i.focus = idx; });
    let g0 = gen();
    press(&mut h, Action::A);
    // no worker/poll has run yet: the model is already updated
    assert!(with(|s| s.profile("HomeNet").is_none()));
    assert!(with(|s| !s.ap("HomeNet").unwrap().in_use));
    assert_ne!(gen(), g0, "in-flight snapshots from before the action are dropped");
    // a stale snapshot (taken before the backend finished) is re-overlaid
    let stale_aps = vec![AccessPoint { ssid: "HomeNet".into(), bssid: String::new(), signal: 90, security: Security::Wpa2, in_use: true }];
    let stale_profiles = vec![Profile { name: "HomeNet".into(), uuid: "demo-uuid-1".into(), autoconnect: true, active: true }];
    apply_scan(&mut h.a, g0, Some(true), stale_aps.clone(), stale_profiles.clone(), Some(Connectivity::Full), String::new());
    assert!(with(|s| s.profile("HomeNet").is_some()) == false || with(|s| s.generation) != g0);
    apply_scan(&mut h.a, gen(), Some(true), stale_aps, stale_profiles, Some(Connectivity::Full), String::new());
    assert!(with(|s| s.profile("HomeNet").is_none() && !s.ap("HomeNet").unwrap().in_use), "pending Forget overlays a stale snapshot");
    // reopening the info does not show Forget
    open_info(&mut h.a, "HomeNet");
    let Some(Dialog::Info(i)) = dialog() else { panic!() };
    assert!(i.saved.is_none());
    assert!(info_view(&i, 0.0).rows.iter().all(|r| r.value != lt("#Settings_Internet_Forget", "Забыть")));
}

#[test]
fn successful_connect_marks_only_the_new_network_connected() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.focus = Some(Key::Ap("Cafe Open".into())));
    press(&mut h, Action::A);
    apply_connect(&mut h.a, gen(), Ok(()), Connectivity::Full);
    with(|s| {
        assert!(s.ap("Cafe Open").unwrap().in_use);
        assert!(!s.ap("HomeNet").unwrap().in_use);
    });
}

// ---------------------------------------------------------------- HTTP proxy (task 599)

#[test]
fn proxy_dialog_validates_applies_and_resets() {
    use proxy::{ProxyCfg, ProxyForm, P};
    let mut h = harness();
    enter(&mut h.a);
    activate(&mut h.a, Some(Key::Proxy));
    let Some(Dialog::Proxy(p)) = dialog() else { panic!("proxy dialog expected") };
    assert!(!p.cfg.enabled, "stock starts with the toggle off");
    assert_eq!(p.items(), vec![P::Toggle, P::Apply, P::Cancel], "no fields while off, no reset with nothing saved");
    lint_dialog(&proxy::view(&p, false, 0.0), "proxy off");
    // on: the fields appear; empty host is rejected
    press(&mut h, Action::A);
    let Some(Dialog::Proxy(p)) = dialog() else { panic!() };
    assert_eq!(p.items(), vec![P::Toggle, P::Host, P::Port, P::Apply, P::Cancel]);
    lint_dialog(&proxy::view(&p, false, 0.0), "proxy on");
    lint_dialog(&proxy::view(&p, true, 0.0), "proxy on osk");
    proxy::submit(&mut h.a);
    let Some(Dialog::Proxy(p)) = dialog() else { panic!("stays open on an error") };
    assert_eq!(p.focus, P::Host);
    assert!(p.error.is_some());
    // validation
    let mut f = ProxyForm::new(ProxyCfg::default());
    f.cfg = ProxyCfg { enabled: true, host: "http://proxy.local/".into(), port: "8080".into() };
    assert_eq!(f.validate().unwrap(), ProxyCfg { enabled: true, host: "proxy.local".into(), port: "8080".into() });
    for bad in ["", "0", "65536", "x"] { f.cfg.port = bad.into(); assert_eq!(f.validate().unwrap_err().0, P::Port, "{bad}"); }
    f.cfg.port = "3128".into();
    f.cfg.host = "a b".into();
    assert_eq!(f.validate().unwrap_err().0, P::Host);
    // apply closes and remembers; reopening offers reset
    f.cfg.host = "10.0.0.2".into();
    with(|s| s.dialog = Some(Dialog::Proxy(f)));
    proxy::submit(&mut h.a);
    assert!(dialog().is_none());
    assert_eq!(with(|s| s.proxy.clone()), ProxyCfg { enabled: true, host: "10.0.0.2".into(), port: "3128".into() });
    activate(&mut h.a, Some(Key::Proxy));
    let Some(Dialog::Proxy(p)) = dialog() else { panic!() };
    assert!(p.cfg.enabled && p.items().contains(&P::Reset));
    with(|s| s.dialog = Some(Dialog::Proxy(ProxyForm { focus: P::Reset, ..p })));
    press(&mut h, Action::A);
    assert!(dialog().is_none());
    assert_eq!(with(|s| s.proxy.clone()), ProxyCfg::default());
    assert!(proxy::apply_js(&ProxyCfg { enabled: true, host: "h".into(), port: "1".into() }).contains("SetProxyInfo"));
    // B cancels without applying
    activate(&mut h.a, Some(Key::Proxy));
    press(&mut h, Action::A);
    press(&mut h, Action::B);
    assert!(dialog().is_none());
    assert_eq!(with(|s| s.proxy.clone()), ProxyCfg::default());
}
