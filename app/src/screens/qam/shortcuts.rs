use steam_assets::i18n;
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
        row.label = t_or("#QuickAccess_Tab_Bluetooth_AddDevice", i18n::tr_ref("shortcuts.add_device")).into();
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
                        ("#QuickAccess_Tab_Bluetooth_Disconnect", i18n::tr_ref("shortcuts.disconnect"))
                    } else {
                        ("#QuickAccess_Tab_Bluetooth_Connect", i18n::tr_ref("shortcuts.connect"))
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
        Some(if steam_assets::has_loc(token) { token } else { i18n::tr_ref("shortcuts.bluetooth_settings") })
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
                BluetoothAction::Power(on) if !state.available || state.enabled != on => Err(i18n::tr_ref("shortcuts.bluez_has_not_confirmed_the_power_adapter").into()),
                BluetoothAction::Device { id, connect }
                    if state.devices.iter().any(|d| d.id == id && d.connected) != connect =>
                {
                    Err(i18n::tr_ref("shortcuts.bluez_confirmed_the_status_of_the_connection").into())
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
            self.toast(i18n::tr_ref("shortcuts.failed_to_change_the_status_of_bluetooth"));
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
                self.toast(i18n::tr_ref("shortcuts.failed_to_change_the_night_mode"));
            }
        }
        if self.qm.open { self.qam_rebuild(false); }
    }
}

#[cfg(test)]
#[path = "shortcuts_tests.rs"]
mod tests;
