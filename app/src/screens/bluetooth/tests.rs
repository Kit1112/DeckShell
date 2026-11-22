//! Task 488 tests: layout lint, pad focus walk, device dialog and pairing flows on the in-memory radio, Xbox flapping, rail
//! separators, and PNG of the key states (`DECKSHELL_PNG_DIR=<dir> cargo test -p deckshell bluetooth`). Nothing here touches a real adapter.
//!
//! Element / state / button → result table: `docs/bluetooth-page-2026-11-19.md`.
use super::*;
use crate::router::Route;
use slint::Model;
use crate::screens::osk::field_tests::{app, shot};
use deck_system::bluetooth::etype;

static LOCK: Mutex<()> = Mutex::new(());

struct H { ui: crate::AppWindow, a: App, radio: Arc<MockBluetooth>, _g: std::sync::MutexGuard<'static, ()> }

fn harness() -> H {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (ui, mut a) = app();
    crate::assets::load_icons_for_test(&ui);
    let radio = Arc::new(MockBluetooth::demo());
    set_backend(radio.clone());
    ui.global::<crate::Shell>().set_page(4);
    a.build_settings_rail();
    a.router.replace(Route::Settings(PAGE.into()));
    a.sync_settings(PAGE);
    H { ui, a, radio, _g: g }
}

fn press(h: &mut H, a: Action) { h.a.handle(a); }
fn focus() -> Option<Key> { with(|s| s.focus.clone()) }
fn dialog() -> Option<Dialog> { with(|s| s.dialog.clone()) }
fn label_of(k: &Key) -> String { with(|s| s.rows().into_iter().find(|r| r.key.as_ref() == Some(&k.tag())).map(|r| r.label).unwrap_or_default()) }
fn go_to(h: &mut H, k: Key) {
    for _ in 0..40 { if focus() == Some(k.clone()) { return; } press(h, Action::Down); }
    panic!("cannot reach {k:?}, at {:?}", focus());
}
fn dev(id: u32) -> Device { with(|s| s.device(id)).unwrap() }

// ---------------------------------------------------------------- layout lint

fn lint(rows: &[Row], what: &str) {
    let mut last = f32::MIN;
    for r in rows {
        assert!(r.h > 0.0 && r.y >= 100.0, "{what}: row {:?} y {}", r.label, r.y);
        assert!(r.y + 0.01 >= last, "{what}: rows out of order at {:?}", r.label);
        if r.kind != rk::SECTION { last = r.y + r.h; } else { last = last.max(r.y); }
    }
    for (i, a) in rows.iter().enumerate() {
        for b in &rows[i + 1..] { assert!(a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5, "{what}: {:?} overlaps {:?}", a.label, b.label); }
    }
}

fn d(id: u32, et: u8, name: &str) -> Device { Device { id, etype: et, mac: format!("AA:BB:CC:DD:EE:{id:02X}"), name: name.into(), ..Default::default() } }

#[test]
fn page_layout_has_no_overlap_in_all_states() {
    let long = "Очень длинное название беспроводного устройства для проверки обрезки".to_string();
    let paired = |n: u32| (0..n).map(|i| Device { paired: true, connected: i % 2 == 0, battery: (i % 2 == 0).then_some(55), ..d(i, etype::GAMEPAD, &format!("Pad {i}")) }).collect::<Vec<_>>();
    let avail = |n: u32| (100..100 + n).map(|i| d(i, etype::HEADPHONES, &format!("Phones {i}"))).collect::<Vec<_>>();
    for (name, p) in [
        ("unavailable", PageData { available: false, ..Default::default() }),
        ("off", PageData { available: true, ..Default::default() }),
        ("on-searching", PageData { available: true, enabled: true, discovering: true, ..Default::default() }),
        ("on-nothing", PageData { available: true, enabled: true, ..Default::default() }),
        ("paired-only", PageData { available: true, enabled: false, paired: paired(3), ..Default::default() }),
        ("full", PageData { available: true, enabled: true, discovering: true, paired: paired(4), avail: avail(8), ..Default::default() }),
        ("long-name", PageData { available: true, enabled: true, paired: vec![Device { paired: true, ..d(1, etype::KEYBOARD, &long) }], ..Default::default() }),
    ] {
        lint(&page_rows(&p), name);
    }
}

#[test]
fn dialog_layout_fits_the_screen_in_all_states() {
    for wake in [false, true] {
        for connected in [false, true] {
            let dv = Device { paired: true, connected, wake_supported: wake, ..d(1, etype::GAMEPAD, "Pad") };
            let n = info_items(&dv).len();
            for focus in 0..n {
                let v = info_view(&dv, focus, 0.0);
                assert!(v.box_h <= net::DLG_MAX_H + 0.5 && v.box_y >= VIEW_TOP - 0.5 && v.box_y + v.box_h <= VIEW_BOTTOM + 0.5);
                assert_eq!(v.items, n);
                let ctl: Vec<&DRow> = v.rows.iter().filter(|r| r.kind != dk::TITLE).collect();
                for (i, a) in ctl.iter().enumerate() { for b in &ctl[i + 1..] { assert!(a.y + a.h <= b.y + 0.5, "overlap {:?}/{:?}", a.label, b.label); } }
            }
        }
    }
}

// ---------------------------------------------------------------- lists

#[test]
fn lists_follow_the_stock_split_filter_and_sort() {
    let mut h = harness();
    let p = with(|s| s.page_data());
    assert_eq!(p.paired.iter().map(|d| d.id).collect::<Vec<_>>(), vec![1, 2], "connected first");
    assert!(p.enabled && with(|s| s.state.discovering), "opening the page requests discovery");
    assert_eq!(p.avail.iter().map(|d| d.id).collect::<Vec<_>>(), vec![3, 4], "phone and the beacon hidden by default; strongest first");
    activate(&mut h.a, Some(Key::ShowAll));
    let p = with(|s| s.page_data());
    assert_eq!(p.avail.len(), 4, "show all adds the phone and the beacon");
    assert_eq!(label_of(&Key::Avail(5)), "02:00:00:00:00:05", "unnamed device shows its MAC");
}

#[test]
fn section_title_follows_the_discovery_state() {
    let mk = |avail: Vec<Device>, discovering| PageData { available: true, enabled: true, discovering, avail, ..Default::default() };
    let titles = |p: &PageData| page_rows(p).into_iter().filter(|r| r.kind == rk::SECTION).map(|r| (r.label, r.mark)).collect::<Vec<_>>();
    assert_eq!(titles(&mk(vec![], true)), vec![("Поиск устройств...".to_string(), 2)]);
    assert_eq!(titles(&mk(vec![], false)), vec![("Устройства не обнаружены".to_string(), 0)]);
    assert_eq!(titles(&mk(vec![d(1, etype::SPEAKER, "S")], true)), vec![("Доступно для соединения".to_string(), 2)]);
    let off = PageData { available: true, enabled: false, ..Default::default() };
    assert!(titles(&off).is_empty(), "radio off and nothing found: no section");
    let paired = PageData { available: true, enabled: false, paired: vec![Device { paired: true, ..d(1, etype::GAMEPAD, "P") }], ..Default::default() };
    assert_eq!(titles(&paired), vec![("Привязано".to_string(), 0)]);
}

#[test]
fn device_rows_show_state_battery_and_spinner() {
    let rows = page_rows(&PageData { available: true, enabled: true, paired: vec![
        Device { paired: true, connected: true, battery: Some(80), ..d(1, etype::GAMEPAD, "A") },
        Device { paired: true, ..d(2, etype::HEADPHONES, "B") },
        Device { paired: true, connected: true, busy: true, ..d(3, etype::KEYBOARD, "C") },
    ], avail: vec![Device { pairing: true, ..d(4, etype::SPEAKER, "D") }], ..Default::default() });
    let by = |l: &str| rows.iter().find(|r| r.label == l).unwrap().clone();
    assert_eq!((by("A").btn.as_str(), by("A").btn_w, by("A").bars, by("A").on), ("80%", 80.0, 3, 0));
    assert_eq!((by("B").desc.as_str(), by("B").on, by("B").bars), ("НЕ ПОДКЛЮЧЕНО", 1, 1));
    assert_eq!(by("C").mark, 2, "operation in progress: spinner");
    assert_eq!((by("D").mark, by("D").bars), (2, 2), "pairing: spinner, speaker icon");
}

#[test]
fn icons_follow_the_stock_type_table() {
    use crate::assets::bt_device_icon as i;
    assert_eq!([i(4, ""), i(5, ""), i(6, ""), i(7, ""), i(9, ""), i(10, ""), i(2, ""), i(3, ""), i(11, ""), i(0, ""), i(1, ""), i(8, ""), i(0, "SteamController")], [1, 1, 2, 2, 3, 3, 4, 5, 6, 0, 0, 0, 3]);
}

// ---------------------------------------------------------------- pad

#[test]
fn pad_walks_every_row_in_order_and_left_returns_to_the_rail() {
    let mut h = harness();
    enter(&mut h.a);
    assert_eq!(h.a.se.zone, 1);
    assert_eq!(focus(), Some(Key::Toggle));
    let mut seen = vec![focus().unwrap()];
    for _ in 0..20 { press(&mut h, Action::Down); if seen.last() != focus().as_ref() { seen.push(focus().unwrap()); } }
    assert_eq!(&seen[..4], &[Key::Toggle, Key::ShowAll, Key::Paired(1), Key::Paired(2)]);
    assert!(seen.contains(&Key::Avail(3)) && seen.contains(&Key::Avail(4)), "{seen:?}");
    for _ in 0..20 { if focus() == Some(Key::Toggle) { break; } press(&mut h, Action::Up); }
    assert_eq!(focus(), Some(Key::Toggle));
    press(&mut h, Action::Left);
    assert_eq!(h.a.se.zone, 0, "Left returns to the rail");
}

#[test]
fn hints_name_the_action_of_the_focused_row() {
    let mut h = harness();
    enter(&mut h.a);
    let a_text = || hints().iter().find(|x| x.4 == Action::A).unwrap().1;
    assert_eq!(a_text(), "#ActionButtonLabelSelect");
    go_to(&mut h, Key::Paired(2));
    assert_eq!(a_text(), "Подробнее");
    go_to(&mut h, Key::Avail(3));
    assert_eq!(a_text(), "Привязать");
    h.radio.set_hold_pairing(true);
    press(&mut h, Action::A);
    assert_eq!(a_text(), "Отвязать устройство");
}

// ---------------------------------------------------------------- toggles

#[test]
fn main_toggle_powers_the_radio_and_scanning() {
    let mut h = harness();
    enter(&mut h.a);
    assert_eq!(h.radio.log(), vec!["discovering true"]);
    press(&mut h, Action::A);
    assert_eq!(h.radio.log()[1..], ["enabled false", "discovering false"]);
    let p = with(|s| s.page_data());
    assert!(!p.enabled && p.avail.is_empty());
    assert_eq!(p.paired.len(), 2, "paired devices stay");
    assert!(p.paired.iter().all(|d| !d.connected), "…but show «НЕ ПОДКЛЮЧЕНО»");
    let titles: Vec<String> = with(|s| s.rows()).into_iter().filter(|r| r.kind == rk::SECTION).map(|r| r.label).collect();
    assert_eq!(titles, vec!["Привязано"], "no available section while the radio is off");
    shot(&h.ui, "bluetooth-off");
    press(&mut h, Action::A);
    assert_eq!(h.radio.log()[3..], ["enabled true", "discovering true"]);
    assert!(with(|s| s.page_data().enabled && s.state.discovering));
}

#[test]
fn main_toggle_is_disabled_without_the_service() {
    let mut h = harness();
    enter(&mut h.a);
    h.radio.set_available(false);
    refresh();
    assert_eq!(with(|s| s.rows())[0].on, -1);
    let before = h.radio.log().len();
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().len(), before, "a disabled switch changes nothing");
}

#[test]
fn show_all_devices_toggle_is_page_state() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::ShowAll);
    press(&mut h, Action::A);
    assert!(with(|s| s.show_all));
    assert!(h.radio.log().iter().all(|l| !l.starts_with("pair")), "no radio call");
    press(&mut h, Action::A);
    assert!(!with(|s| s.show_all));
}

// ---------------------------------------------------------------- pairing

#[test]
fn a_on_an_available_device_pairs_it() {
    let mut h = harness();
    enter(&mut h.a);
    shot(&h.ui, "bluetooth-page");
    go_to(&mut h, Key::Avail(3));
    press(&mut h, Action::A);
    assert!(h.radio.log().contains(&"pair 3".to_string()));
    assert!(with(|s| s.page_data().paired.iter().any(|d| d.id == 3 && d.connected)), "now in «Привязано»");
}

#[test]
fn pairing_in_progress_shows_a_spinner_and_a_cancels_it() {
    let mut h = harness();
    enter(&mut h.a);
    h.radio.set_hold_pairing(true);
    go_to(&mut h, Key::Avail(4));
    press(&mut h, Action::A);
    let row = with(|s| s.rows()).into_iter().find(|r| r.key == Some(Key::Avail(4).tag())).unwrap();
    assert_eq!(row.mark, 2);
    shot(&h.ui, "bluetooth-pairing");
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("cancel_pair 4"));
    let row = with(|s| s.rows()).into_iter().find(|r| r.key == Some(Key::Avail(4).tag())).unwrap();
    assert_eq!(row.mark, 0);
}

// ---------------------------------------------------------------- device dialog

#[test]
fn connected_device_dialog_focuses_disconnect_and_closes_on_it() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::Paired(1));
    press(&mut h, Action::A);
    let Some(Dialog::Info { id, focus }) = dialog() else { panic!("dialog expected") };
    assert_eq!(id, 1);
    let d1 = dev(1);
    assert_eq!(info_items(&d1)[focus], Item::Toggle, "the connect/disconnect button has the initial focus");
    let v = with(|s| s.dialog_view()).unwrap();
    let last = v.rows.last().unwrap();
    assert_eq!((last.label.as_str(), last.value.as_str(), last.focus), ("Отключить устройство", "Отключить", 1));
    shot(&h.ui, "bluetooth-dialog-connected");
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("disconnect 1"));
    assert!(dialog().is_none(), "the dialog closes");
    assert!(!dev(1).connected);
}

#[test]
fn disconnected_device_dialog_offers_connect() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::Paired(2));
    press(&mut h, Action::A);
    let v = with(|s| s.dialog_view()).unwrap();
    let last = v.rows.last().unwrap();
    assert_eq!((last.label.as_str(), last.value.as_str()), ("Подключить устройство", "Подключиться"));
    shot(&h.ui, "bluetooth-dialog-disconnected");
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("connect 2"));
    assert!(dev(2).connected);
}

#[test]
fn dialog_rows_follow_the_contract_order_and_wake_visibility() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::Paired(1)); // gamepad: wake supported
    press(&mut h, Action::A);
    let v = with(|s| s.dialog_view()).unwrap();
    let labels: Vec<&str> = v.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Беспроводной геймпад", "MAC-адрес", "Доверенное", "Разрешить этому устройству выводить Steam Deck из спящего режима", "Забыть устройство", "Отключить устройство"]);
    assert_eq!(v.rows[1].value, "02:00:00:00:00:01");
    press(&mut h, Action::B);
    go_to(&mut h, Key::Paired(2)); // headphones: no wake support
    press(&mut h, Action::A);
    let v = with(|s| s.dialog_view()).unwrap();
    assert!(v.rows.iter().all(|r| !r.label.starts_with("Разрешить")), "wake row hidden when unsupported");
    assert_eq!(v.items, 4);
}

#[test]
fn dialog_toggles_trusted_and_wake_and_forget_removes_the_device() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::Paired(1));
    press(&mut h, Action::A);
    // Up from the connect button: Forget, Wake, Trusted, MAC
    press(&mut h, Action::Up); press(&mut h, Action::Up);
    assert_eq!(info_items(&dev(1))[match dialog() { Some(Dialog::Info { focus, .. }) => focus, _ => 9 }], Item::Wake);
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("wake 1 true"));
    press(&mut h, Action::Up);
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("trusted 1 false"));
    press(&mut h, Action::Up);
    let before = h.radio.log().len();
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().len(), before, "MAC row: information only");
    assert!(dialog().is_some());
    for _ in 0..4 { press(&mut h, Action::Down); }
    press(&mut h, Action::Up); // Forget
    press(&mut h, Action::A);
    assert_eq!(h.radio.log().last().map(String::as_str), Some("forget 1"));
    assert!(dialog().is_none());
    assert!(with(|s| s.page_data().paired.iter().all(|d| d.id != 1)));
}

#[test]
fn b_closes_the_dialog_without_actions() {
    let mut h = harness();
    enter(&mut h.a);
    go_to(&mut h, Key::Paired(2));
    let before = h.radio.log().len();
    press(&mut h, Action::A);
    assert!(dialog().is_some());
    assert!(back(&mut h.a));
    assert!(dialog().is_none());
    assert_eq!(h.radio.log().len(), before);
    assert!(!back(&mut h.a), "nothing to close on the page itself");
    assert_eq!(focus(), Some(Key::Paired(2)), "focus stays on the row");
}

// ---------------------------------------------------------------- Xbox flapping

#[test]
fn eighth_connection_flip_of_an_xbox_controller_warns_about_firmware() {
    let t = Instant::now();
    let mut s = St::default();
    let mut fired = 0;
    for i in 0..10u64 { if s.note_flip(XBOX_NAME, "M", i % 2 == 0, t + Duration::from_secs(i)) { fired += 1; } }
    assert_eq!(fired, 1, "exactly once, on the 8th flip");
    let mut s = St::default();
    assert!((0..20u64).all(|i| !s.note_flip("Other pad", "M", i % 2 == 0, t + Duration::from_secs(i))), "only the Xbox controller");
    let mut s = St::default();
    assert!((0..20u64).all(|i| !s.note_flip(XBOX_NAME, "M", i % 2 == 0, t + Duration::from_secs(i * 120))), "slow flips do not count");
}

#[test]
fn xbox_warning_dialog_closes_with_a_or_b() {
    let mut h = harness();
    enter(&mut h.a);
    with(|s| s.dialog = Some(Dialog::Xbox));
    push(&mut h.a);
    let v = with(|s| s.dialog_view()).unwrap();
    assert!(v.rows.iter().any(|r| r.label.contains("Ошибка привязки контроллера Xbox")));
    assert!(v.rows.iter().any(|r| r.value == "Понятно!"));
    shot(&h.ui, "bluetooth-xbox");
    press(&mut h, Action::A);
    assert!(dialog().is_none());
}

// ---------------------------------------------------------------- rail

#[test]
fn rail_has_bluetooth_after_storage_and_a_separator_after_it() {
    let h = harness();
    let ids: Vec<String> = h.a.se.pages.iter().map(|p| p.id.clone()).collect();
    let i = ids.iter().position(|x| x == "Bluetooth").expect("Bluetooth on the rail");
    assert_eq!(ids[i - 1], "Storage");
    assert_eq!(ids[i + 1], "Display");
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let extra = crate::screens::settings::rail_extra(&refs);
    assert_eq!(extra[i], 0.0);
    assert_eq!(extra[i + 1], 18.0, "separator between Bluetooth and Display");
    let n = ids.iter().position(|x| x == "Notifications").unwrap();
    assert_eq!(extra[n], 36.0);
    let hm = ids.iter().position(|x| x == "Home").unwrap();
    assert_eq!(extra[hm], 54.0);
    assert_eq!(h.ui.global::<SettingsState>().get_rail().row_count(), ids.len());
    shot(&h.ui, "bluetooth-rail");
}

#[test]
fn rail_extra_keeps_the_separator_without_bluetooth() {
    let e = crate::screens::settings::rail_extra(&["System", "Internet", "Storage", "Display", "Power"]);
    assert_eq!(e, vec![0.0, 0.0, 0.0, 18.0, 18.0]);
}
