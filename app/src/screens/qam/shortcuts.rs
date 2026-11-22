use std::sync::Arc;

use super::*;

fn visible_device(d: &bt::Device) -> bool {
    (d.paired || d.connected) && bt::shown_by_default(d)
}

impl App {
    pub(super) fn qam_bluetooth_rows(&mut self, b: &mut B, state: &bt::State) {
        if !state.available { return; }

        b.toggle(Item::Bluetooth, Some(self.qam_icon("Rail-Bluetooth")),
            &t_or("#QuickAccess_Tab_Bluetooth_ToggleLabel", "Bluetooth"),
            i32::from(state.enabled), H_SHORTCUT, "#quickaccess_tab_bluetooth_togglelabel");
        if !state.enabled { return; }

        for d in state.devices.iter().filter(|d| visible_device(d)) {
            let mut row = B::row(14);
            let icon = crate::assets::bt_device_icon(d.etype, &d.name) as usize;
            row.icon = self.qam_icon(crate::assets::BT_DEVICE_ICONS[icon]);
            row.label = if d.name.is_empty() { d.mac.as_str() } else { d.name.as_str() }.into();
            row.on = i32::from(d.connected);
            row.value = d.battery.filter(|n| d.connected && *n > 0).map_or(-1.0, |n| f32::from(n) / 100.0);
            row.notches = i32::from(d.busy || matches!(self.qm.bluetooth_pending, Some(BluetoothAction::Device { id, .. }) if id == d.id));
            row.meta = "button||A,START,FOCUS,touch,mouse".into();
            b.push(row, H_BT_DEVICE, Some(Item::BluetoothDevice(d.id)));
        }

        let mut row = B::row(16);
        row.icon = self.qam_icon("Add");
        row.label = t_or("#QuickAccess_Tab_Bluetooth_AddDevice", "Добавить устройство").into();
        row.on = 1;
        row.value = -1.0;
        row.meta = "button|#quickaccess_tab_bluetooth_adddevice|A,FOCUS,touch,mouse".into();
        b.push(row, H_BT_DEVICE, Some(Item::BluetoothAdd));
    }

    pub(super) fn qam_quick_cursor(&self, item: Option<Item>) -> Option<i32> {
        let find = |item| self.qm.items.iter().position(|i| *i == item).map(|i| i as i32);
        item.and_then(|item| find(item).or_else(|| match item {
            Item::BluetoothDevice(_) | Item::BluetoothAdd => find(Item::Bluetooth).or_else(|| find(Item::Wifi)),
            _ => None,
        }))
    }

    pub(super) fn qam_scroll_max(&self) -> f32 {
        if self.qm.tab == 2 {
            (self.qm.content_h + QUICK_BOTTOM_PAD - PANEL_H).max(0.0)
        } else {
            qam_max_scroll(self.qm.content_h)
        }
    }

    pub(crate) fn qam_select_hint(&self) -> &'static str {
        if self.qm.rail_focus.is_none() {
            if let Some(Item::BluetoothDevice(id)) = self.qm.items.get(self.qm.cursor as usize) {
                if let Some(d) = self.qm.st.bluetooth.devices.iter().find(|d| d.id == *id) {
                    let (token, fallback) = if d.connected {
                        ("#QuickAccess_Tab_Bluetooth_Disconnect", "Отключить")
                    } else {
                        ("#QuickAccess_Tab_Bluetooth_Connect", "Подключиться")
                    };
                    return if steam_assets::has_loc(token) { token } else { fallback };
                }
            }
        }
        "#ActionButtonLabelSelect"
    }

    pub(crate) fn qam_bluetooth_options_hint(&self) -> Option<&'static str> {
        let item = self.qm.items.get(self.qm.cursor as usize);
        if self.qm.rail_focus.is_some() || !matches!(item, Some(Item::Bluetooth | Item::BluetoothDevice(_))) {
            return None;
        }
        let token = "#Settings_Bluetooth_SettingsAction";
        Some(if steam_assets::has_loc(token) { token } else { "Настройки Bluetooth" })
    }

    pub(super) fn qam_open_bluetooth_settings(&mut self) {
        let page = super::super::bluetooth::PAGE;
        if !self.se.pages.iter().any(|p| p.id.eq_ignore_ascii_case(page)) {
            self.build_settings_rail();
        }
        self.navigate(crate::router::Route::Settings(page.into()));
    }

    pub(in crate::screens) fn qam_update_wifi(&mut self, observed: Option<bool>) {
        self.qm.st.wifi = observed;
        self.sync_status();
    }

    pub(super) fn qam_bluetooth_action(&self, item: Item) -> Option<BluetoothAction> {
        let radio = &self.qm.st.bluetooth;
        if !radio.available || self.qm.bluetooth_pending.is_some() { return None; }

        match item {
            Item::Bluetooth => Some(BluetoothAction::Power(!radio.enabled)),
            Item::BluetoothDevice(id) if radio.enabled => {
                let d = radio.devices.iter().find(|d| d.id == id && visible_device(d) && !d.busy)?;
                Some(BluetoothAction::Device { id, connect: !d.connected })
            }
            _ => None,
        }
    }

    pub(super) fn qam_write_bluetooth(&mut self, op: BluetoothAction, backend: Arc<dyn BluetoothBackend>) {
        let mock = backend.is_mock();
        if !mock && (cfg!(test) || actions::dryrun()) { return; }

        self.qm.bluetooth_pending = Some(op);
        BLUETOOTH_EPOCH.fetch_add(1, Ordering::SeqCst);
        let work = move || {
            let result = op.apply(&*backend);
            let state = backend.state();
            let result = result.and_then(|()| match op {
                BluetoothAction::Power(on) if !state.available || state.enabled != on => Err("BlueZ не подтвердил питание адаптера".into()),
                BluetoothAction::Device { id, connect }
                    if state.devices.iter().any(|d| d.id == id && d.connected) != connect =>
                {
                    Err("BlueZ не подтвердил состояние подключения".into())
                }
                _ => Ok(()),
            });
            // Store the stock client preference after the radio has changed.
            // BlueZ remains the source for the switch, including without Steam.
            if result.is_ok() && !mock {
                if let BluetoothAction::Power(on) = op {
                    if let Some(steam) = actions::bridge() {
                        if let Err(err) = steam.set_client_setting("system_bluetooth_enabled", if on { "true" } else { "false" }) {
                            eprintln!("[deckshell] QAM Bluetooth Steam preference: {err}");
                        }
                    }
                }
            }
            (state, result)
        };
        if mock {
            let (state, result) = work();
            self.qam_bluetooth_result(state, result);
        } else {
            std::thread::spawn(move || {
                let (state, result) = work();
                let _ = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| a.qam_bluetooth_result(state, result));
                });
            });
        }
    }

    fn qam_bluetooth_result(&mut self, state: bt::State, result: Result<(), String>) {
        BLUETOOTH_EPOCH.fetch_add(1, Ordering::SeqCst);
        self.qm.bluetooth_pending = None;
        self.qm.st.bluetooth = state;
        if let Err(err) = result {
            eprintln!("[deckshell] QAM Bluetooth: {err}");
            self.toast("Не удалось изменить состояние Bluetooth");
        }
        if self.qm.open {
            self.qam_rebuild(false);
            self.update_hint();
        }
    }

    pub(super) fn qam_write_night_mode(&mut self, on: bool) {
        self.qm.night_pending = true;
        NIGHT_EPOCH.fetch_add(1, Ordering::SeqCst);
        let mock = self.ds.is_mock();
        if mock || cfg!(test) || actions::dryrun() {
            self.qam_night_result(crate::display::set_night_mode(on, mock));
        } else {
            std::thread::spawn(move || {
                let result = crate::display::set_night_mode(on, mock);
                let _ = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| a.qam_night_result(result));
                });
            });
        }
    }

    fn qam_night_result(&mut self, result: Result<bool, String>) {
        NIGHT_EPOCH.fetch_add(1, Ordering::SeqCst);
        self.qm.night_pending = false;
        match result {
            Ok(on) => self.qm.st.night_mode = Some(on),
            Err(err) => {
                eprintln!("[deckshell] QAM night mode: {err}");
                self.toast("Не удалось изменить ночной режим");
            }
        }
        if self.qm.open { self.qam_rebuild(false); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::Route;
    use crate::screens::{bluetooth, osk::field_tests::{app, shot}};
    use deck_system::bluetooth::{etype, MockBluetooth};
    use slint::Model;

    fn device(id: u32, name: &str, etype: u8, paired: bool) -> bt::Device {
        bt::Device {
            id, name: name.into(), etype, paired,
            rssi: -(id as i32), mac: format!("02:00:00:00:00:{id:02X}"),
            ..Default::default()
        }
    }

    fn fixture() -> Arc<MockBluetooth> {
        Arc::new(MockBluetooth::new(vec![
            device(1, "Aurora Ear (a)", etype::HEADPHONES, true),
            device(2, "Studio Buds", etype::HEADSET, true),
            device(3, "Wireless Headphones Pro", etype::HEADPHONES, true),
            device(4, "Sound Buds 300", etype::HEADPHONES, true),
            device(5, "WH-1000 Series", etype::HEADSET, true),
            device(6, "Keyboard K380", etype::KEYBOARD, true),
            device(7, "Living Room TV", etype::SPEAKER, false),
            device(8, "Phone", etype::PHONE, true),
        ]))
    }

    fn open(state: bt::State) -> (AppWindow, App) {
        let (ui, mut a) = app();
        crate::assets::load_icons_for_test(&ui);
        a.pad_mode = true;
        a.qm.open = true;
        a.qm.st = Status {
            bluetooth: state,
            night_mode: Some(false),
            airplane: Some(false),
            wifi: Some(true),
            brightness: Some(0.5),
            volume: Some((0.45, false)),
            mic: Some(0.4),
            gs: Some(crate::gamescope_atoms::Snapshot::default()),
            ..Default::default()
        };
        a.qam_rebuild(true);
        ui.global::<QamState>().set_open(true);
        a.update_hint();
        (ui, a)
    }

    fn cursor(a: &App) -> Item {
        a.qm.items[a.qm.cursor as usize]
    }

    fn focus(a: &mut App, item: Item) {
        a.qm.cursor = a.qm.items.iter().position(|&i| i == item).unwrap() as i32;
        a.qam_build_nav();
        a.qam_scroll_into_view();
        a.qam_refresh_light();
    }

    #[test]
    fn qam_shortcuts_follow_stock_order_and_hide_the_paired_list_when_off() {
        let (ui, mut a) = open(fixture().state());
        let first = a.qm.items.iter().position(|it| *it == Item::Airplane).unwrap();
        assert_eq!(&a.qm.items[first..first + 3], &[Item::Airplane, Item::Wifi, Item::Bluetooth]);
        for id in 1..=6 {
            assert_eq!(a.qm.items[first + 2 + id as usize], Item::BluetoothDevice(id));
        }
        assert_eq!(&a.qm.items[first + 9..], &[Item::BluetoothAdd, Item::NightMode]);

        let rows = ui.global::<QamState>().get_rows();
        let devices: Vec<_> = rows.iter().filter(|r| matches!(r.kind, 14 | 16)).collect();
        assert_eq!(devices.len(), 7);
        assert!(devices[..6].iter().all(|r| r.on == 0 && r.h == H_BT_DEVICE && r.icon.size().width > 0));
        assert!(devices.last().unwrap().icon.size().width > 0);
        assert!(rows.iter().find(|r| r.label == "Ночной режим").unwrap().icon.size().width > 0);

        a.qm.st.bluetooth.enabled = false;
        a.qam_rebuild(false);
        assert_eq!(&a.qm.items[first + 2..], &[Item::Bluetooth, Item::NightMode]);
        a.qm.st.bluetooth.available = false;
        a.qam_rebuild(false);
        assert_eq!(&a.qm.items[first + 2..], &[Item::NightMode]);
    }

    #[test]
    fn qam_shortcuts_wifi_uses_the_internet_backend_and_preserves_stock_focus() {
        use crate::screens::internet;
        use deck_system::wifi::MockWifi;

        let radio = Arc::new(MockWifi::demo());
        internet::set_backend(radio.clone());
        let (ui, mut a) = open(fixture().state());
        focus(&mut a, Item::Airplane);
        a.handle_qam(Action::Down, false);
        assert_eq!(cursor(&a), Item::Wifi);
        let wifi_row = || ui.global::<QamState>().get_rows().iter()
            .find(|row| row.meta.starts_with("toggle|#quickaccess_tab_settings_section_shortcuts_wifi|"))
            .unwrap();
        assert!(wifi_row().icon.size().width > 0);
        assert_eq!(wifi_row().on, 1);

        let stale = Status { wifi: Some(true), wifi_epoch: internet::radio_epoch(), ..a.qm.st.clone() };
        a.handle_qam(Action::A, false);
        assert_eq!(internet::backend().radio(), Some(false));
        assert_eq!(wifi_row().on, 0);
        a.on_status(stale);
        assert_eq!(wifi_row().on, 0, "a snapshot read before the write must not restore the old value");
        assert_eq!(cursor(&a), Item::Wifi);
        assert!(a.qm.open);
        a.handle_qam(Action::Down, false);
        assert_eq!(cursor(&a), Item::Bluetooth);
        a.handle_qam(Action::Up, false);
        assert_eq!(cursor(&a), Item::Wifi);

        a.qam_click(wifi_row().fid);
        assert_eq!(internet::backend().radio(), Some(true));
        assert_eq!(wifi_row().on, 1);

        // Another test may advance the process-wide poll epoch. Feed the
        // unavailable state directly to exercise the action handler.
        a.qam_update_wifi(None);
        assert_eq!(wifi_row().on, -1);
        a.handle_qam(Action::A, false);
        assert_eq!(radio.log(), ["radio off", "radio on"]);
    }

    #[test]
    fn qam_shortcuts_device_and_radio_writes_use_the_backend_and_preserve_focus() {
        let radio = fixture();
        let (_ui, mut a) = open(radio.state());
        let device = Item::BluetoothDevice(4);
        focus(&mut a, device);
        assert!(matches!(a.qam_select_hint(), "#QuickAccess_Tab_Bluetooth_Connect" | "Подключиться"));

        let op = a.qam_bluetooth_action(device).unwrap();
        assert_eq!(op, BluetoothAction::Device { id: 4, connect: true });
        a.qam_write_bluetooth(op, radio.clone());
        assert_eq!(cursor(&a), device, "connected row moves to the top without losing focus");
        assert!(matches!(a.qam_select_hint(), "#QuickAccess_Tab_Bluetooth_Disconnect" | "Отключить"));

        let op = a.qam_bluetooth_action(device).unwrap();
        a.qam_write_bluetooth(op, radio.clone());
        assert_eq!(cursor(&a), device);

        let op = a.qam_bluetooth_action(Item::Bluetooth).unwrap();
        a.qam_write_bluetooth(op, radio.clone());
        assert!(!a.qm.st.bluetooth.enabled);
        assert_eq!(cursor(&a), Item::Bluetooth, "disappearing device returns focus to radio");
        assert_eq!(a.qam_bluetooth_action(device), None);

        let op = a.qam_bluetooth_action(Item::Bluetooth).unwrap();
        a.qam_write_bluetooth(op, radio.clone());
        assert!(a.qm.st.bluetooth.enabled);
        assert_eq!(radio.log(), ["connect 4", "disconnect 4", "enabled false", "enabled true"]);
    }

    #[test]
    fn qam_shortcuts_pad_scroll_night_mode_and_back() {
        let (_ui, mut a) = open(fixture().state());
        focus(&mut a, Item::Bluetooth);
        for id in 1..=6 {
            a.handle_qam(Action::Down, false);
            assert_eq!(cursor(&a), Item::BluetoothDevice(id));
        }
        a.handle_qam(Action::Down, false);
        assert_eq!(cursor(&a), Item::BluetoothAdd);
        a.handle_qam(Action::Down, false);
        assert_eq!(cursor(&a), Item::NightMode);
        assert!((a.qm.scroll - a.qam_scroll_max()).abs() < 0.01);
        let (y, h) = a.qm.rects[a.qm.cursor as usize];
        assert!((y + h - a.qm.scroll - (PANEL_H - QUICK_BOTTOM_PAD)).abs() < 0.01);
        assert_eq!(a.hint_spec().iter().map(|hint| hint.4).collect::<Vec<_>>(), [Action::A, Action::B]);

        a.handle_qam(Action::A, false);
        assert_eq!(a.qm.st.night_mode, Some(true));
        a.handle_qam(Action::A, false);
        assert_eq!(a.qm.st.night_mode, Some(false));
        assert_eq!(cursor(&a), Item::NightMode);

        a.qam_wheel(5000.0);
        a.qam_rebuild(false);
        assert_eq!(a.qm.scroll, 0.0, "a status refresh preserves wheel scroll");
        a.handle_qam(Action::B, false);
        assert_eq!(a.qm.rail_focus, Some(2));
        a.handle_qam(Action::B, false);
        assert!(!a.qm.open);
    }

    #[test]
    fn qam_shortcuts_add_routes_to_the_task_488_page() {
        let radio = fixture();
        let (ui, mut a) = open(radio.state());
        bluetooth::set_backend(radio.clone());
        focus(&mut a, Item::BluetoothAdd);
        a.handle_qam(Action::A, false);
        assert_eq!(a.router.current(), &Route::Settings(bluetooth::PAGE.into()));
        assert_eq!(a.se.page_id(), bluetooth::PAGE);
        assert!(!a.qm.open);
        assert!(!ui.global::<QamState>().get_open());
        assert!(ui.global::<crate::SettingsState>().get_is_internet());
        assert!(bluetooth::with(|s| s.open));
        assert!(radio.state().discovering);
        bluetooth::leave(&mut a);
    }

    #[test]
    fn qam_shortcuts_render_owner_reference_positions() {
        // The headless software backend omits Slint's transform-scale. Use a
        // virtual X11 display with the production renderer for pixel geometry.
        let femtovg = std::env::var_os("DECKSHELL_QAM_X11_CAPTURE").is_some();
        if femtovg {
            use slint::winit_030::{winit, SlintEvent};
            use winit::platform::x11::EventLoopBuilderExtX11;
            let mut events = winit::event_loop::EventLoop::<SlintEvent>::with_user_event();
            events.with_x11().with_any_thread(true);
            slint::BackendSelector::new().backend_name("winit".into()).renderer_name("femtovg".into())
                .with_winit_event_loop_builder(events).select().unwrap();
            crate::load_fonts();
        }
        let mut devices = fixture().state().devices;
        devices.retain(|d| d.id != 8);
        devices.push(device(7, "Living Room TV", etype::SPEAKER, false));
        let radio = Arc::new(MockBluetooth::new(devices));
        let (ui, mut a) = open(radio.state());
        bluetooth::set_backend(radio);
        if femtovg { ui.show().unwrap(); }
        let capture = |name: &str| {
            if !femtovg {
                shot(&ui, name);
                return;
            }
            let directory = std::env::var_os("DECKSHELL_PNG_DIR").expect("capture output directory");
            let path = std::path::PathBuf::from(directory).join(format!("{name}.png"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let window = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(450), move || {
                let ui = window.unwrap();
                let snapshot = ui.window().take_snapshot().expect("FemtoVG snapshot");
                assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
                image::save_buffer(path, snapshot.as_bytes(), snapshot.width(), snapshot.height(), image::ColorType::Rgba8).unwrap();
                slint::quit_event_loop().unwrap();
            });
            slint::run_event_loop_until_quit().unwrap();
        };
        init_strings(&ui);
        let top = ui.global::<TopBarState>();
        top.set_clock("12:40".into());
        top.set_unread(1);
        top.set_wifi(1);
        top.set_bat_pct(85);
        top.set_bat_show_pct(true);
        top.set_bat_text("85%".into());
        top.set_battery_icon(crate::assets::battery_icon(0.85, true, false));
        focus(&mut a, Item::Wifi);
        let air_idx = a.qm.items.iter().position(|&i| i == Item::Airplane).unwrap();
        a.qm.scroll = a.qm.rects[air_idx].0 - 64.0;
        a.qam_refresh_light();
        capture("01-qam-wifi");

        focus(&mut a, Item::NightMode);
        capture("02-qam-night-mode");

        a.build_settings_rail();
        focus(&mut a, Item::BluetoothAdd);
        a.handle_qam(Action::A, false);
        bluetooth::with(|s| s.focus = Some(bluetooth::Key::Avail(7)));
        bluetooth::enter(&mut a);
        capture("03-bluetooth-settings");
        bluetooth::leave(&mut a);
    }
}
