//! Bluetooth backend for Settings → «Bluetooth» (task 488). Mirrors the stock `BluetoothManagerService` view of the world
//! (docs/steam-bluetooth-page-contract-2026-11-18.md §8): an adapter state plus a list of devices with the fields of
//! `CMsgBluetoothManagerDeviceDetails`. `Bluez` talks to BlueZ through `busctl` (like the rest of this crate); `MockBluetooth`
//! is the in-memory radio used by tests, `DECKSHELL_DRYRUN` and `DECKSHELL_MOCK` — nothing real can change through it.
use std::sync::Mutex;

use super::{busctl_ok, get_prop, prop_bool, prop_str, prop_u32, split_type, tree, Bus, BLUEZ};

/// Stock `etype` codes (contract §5).
pub mod etype {
    pub const UNKNOWN: u8 = 0;
    pub const MISC: u8 = 1;
    pub const PHONE: u8 = 2;
    pub const DISPLAY: u8 = 3;
    pub const HEADPHONES: u8 = 4;
    pub const HEADSET: u8 = 5;
    pub const SPEAKER: u8 = 6;
    pub const AUDIO_SINK: u8 = 7;
    pub const GENERIC: u8 = 8;
    pub const CONTROLLER: u8 = 9;
    pub const GAMEPAD: u8 = 10;
    pub const KEYBOARD: u8 = 11;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub id: u32,
    pub etype: u8,
    pub mac: String,
    pub name: String,
    pub connected: bool,
    pub paired: bool,
    pub pairing: bool,
    pub trusted: bool,
    pub wake_allowed: bool,
    pub wake_supported: bool,
    /// 0..=100, `None` when the device does not report it
    pub battery: Option<u8>,
    pub busy: bool,
    /// stock `should_hide_hint`: beacons and the like, shown only with «Показать все устройства»
    pub hide_hint: bool,
    /// `strength_raw` (dBm); larger is closer
    pub rssi: i32,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// stock `is_service_available`: the service answers and the machine has an adapter
    pub available: bool,
    pub enabled: bool,
    pub discovering: bool,
    pub devices: Vec<Device>,
}

pub trait BluetoothBackend: Send + Sync {
    fn state(&self) -> State;
    fn set_enabled(&self, on: bool) -> Result<(), String>;
    fn set_discovering(&self, on: bool) -> Result<(), String>;
    /// Blocking on the real stack (pairing takes seconds): call from a worker thread.
    fn pair(&self, id: u32) -> Result<(), String>;
    fn cancel_pair(&self, id: u32) -> Result<(), String>;
    fn connect(&self, id: u32) -> Result<(), String>;
    fn disconnect(&self, id: u32) -> Result<(), String>;
    fn forget(&self, id: u32) -> Result<(), String>;
    fn set_trusted(&self, id: u32, on: bool) -> Result<(), String>;
    fn set_wake_allowed(&self, id: u32, on: bool) -> Result<(), String>;
    fn is_mock(&self) -> bool { false }
}

/// Stock `D(A)` / `Ue` visibility (contract §5): types hidden unless «Показать все устройства» is on.
pub fn shown_by_default(d: &Device) -> bool {
    !d.hide_hint && !matches!(d.etype, etype::UNKNOWN | etype::MISC | etype::PHONE | etype::DISPLAY) || d.name == "SteamController"
}

/// Stock sort `O(A, M)` (sp.js:424034): connected first, then the stronger signal.
pub fn sort_devices(v: &mut [Device]) {
    v.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.rssi.cmp(&a.rssi)).then(a.mac.cmp(&b.mac)));
}

// ---------------------------------------------------------------- BlueZ through busctl

pub struct Bluez;

/// BlueZ ties a discovery session to the D-Bus connection that called `StartDiscovery`; a one-shot `busctl call` disconnects at once
/// and BlueZ stops scanning, so nearby devices never appeared. The scan is therefore owned by a long-lived `bluetoothctl` child
/// (`scan on` on its stdin, stdin kept open) that is killed — which drops the session — when the page stops discovery.
static SCAN: Mutex<Option<std::process::Child>> = Mutex::new(None);

fn scan_start() -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut g = SCAN.lock().unwrap();
    if let Some(c) = g.as_mut() { if matches!(c.try_wait(), Ok(None)) { return Ok(()); } }
    let mut c = Command::new("bluetoothctl").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    if let Some(i) = c.stdin.as_mut() { i.write_all(b"scan on\n").and_then(|_| i.flush()).map_err(|e| e.to_string())?; }
    *g = Some(c);
    Ok(())
}
fn scan_stop() {
    if let Some(mut c) = SCAN.lock().unwrap().take() { drop(c.stdin.take()); let _ = c.kill(); let _ = c.wait(); }
}

fn adapter() -> Option<String> {
    tree(Bus::System, BLUEZ).into_iter().find(|p| p.starts_with("/org/bluez/hci") && p.matches('/').count() == 3)
}
fn path_of(id: u32) -> Option<String> {
    tree(Bus::System, BLUEZ).into_iter().find(|p| p.contains("/dev_") && stable_id(p) == id)
}
/// Stock ids are small integers handed out by the service; BlueZ has object paths, so the id is a hash of the path (FNV-1a).
fn stable_id(path: &str) -> u32 {
    path.bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193))
}
/// BlueZ `Icon` property → stock etype.
pub fn etype_of_icon(icon: &str) -> u8 {
    match icon {
        "audio-headphones" => etype::HEADPHONES,
        "audio-headset" => etype::HEADSET,
        "audio-speakers" | "audio-card" => etype::SPEAKER,
        "input-gaming" => etype::GAMEPAD,
        "input-keyboard" => etype::KEYBOARD,
        "phone" => etype::PHONE,
        "video-display" => etype::DISPLAY,
        "input-mouse" | "input-tablet" => etype::GENERIC,
        "" => etype::UNKNOWN,
        _ => etype::MISC,
    }
}
/// Bluetooth Class of Device → stock etype, for devices BlueZ gave no `Icon` (major 0x04 audio/video, 0x05 peripheral, 0x02 phone).
pub fn etype_of_class(class: u32) -> u8 {
    let (major, minor) = ((class >> 8) & 0x1f, (class >> 2) & 0x3f);
    match (major, minor) {
        (4, 1) | (4, 2) => etype::HEADSET,
        (4, 6) => etype::HEADPHONES,
        (4, 5) | (4, 4) | (4, 3) | (4, 7) | (4, 8) | (4, 9) | (4, 10) | (4, 11) => etype::SPEAKER,
        (4, 12..=16) => etype::AUDIO_SINK,
        (4, _) => etype::SPEAKER,
        (5, _) if (class >> 6) & 3 == 1 => etype::KEYBOARD,
        (5, _) if (class >> 6) & 3 == 2 => etype::GENERIC,
        (5, 2) | (5, 1) => etype::GAMEPAD,
        (2, _) => etype::PHONE,
        _ => etype::UNKNOWN,
    }
}
fn prop_i32(path: &str, iface: &str, prop: &str) -> Option<i32> {
    let o = get_prop(Bus::System, BLUEZ, path, iface, prop)?;
    split_type(&o)?.1.parse().ok()
}

impl Bluez {
    fn device(path: &str) -> Option<Device> {
        let dev = "org.bluez.Device1";
        let mac = prop_str(Bus::System, BLUEZ, path, dev, "Address")?;
        let wake = prop_bool(Bus::System, BLUEZ, path, dev, "WakeAllowed");
        let icon = prop_str(Bus::System, BLUEZ, path, dev, "Icon").unwrap_or_default();
        let class = prop_u32(Bus::System, BLUEZ, path, dev, "Class").unwrap_or(0);
        let et = if icon.is_empty() { etype_of_class(class) } else { etype_of_icon(&icon) };
        let name = prop_str(Bus::System, BLUEZ, path, dev, "Alias").or_else(|| prop_str(Bus::System, BLUEZ, path, dev, "Name")).unwrap_or_default();
        Some(Device {
            id: stable_id(path), etype: et, name: if name == mac.replace(':', "-") { String::new() } else { name }, mac,
            connected: prop_bool(Bus::System, BLUEZ, path, dev, "Connected").unwrap_or(false),
            paired: prop_bool(Bus::System, BLUEZ, path, dev, "Paired").unwrap_or(false),
            trusted: prop_bool(Bus::System, BLUEZ, path, dev, "Trusted").unwrap_or(false),
            wake_allowed: wake.unwrap_or(false), wake_supported: wake.is_some(),
            battery: prop_i32(path, "org.bluez.Battery1", "Percentage").map(|p| p.clamp(0, 100) as u8),
            rssi: prop_i32(path, dev, "RSSI").unwrap_or(-127),
            hide_hint: icon.is_empty() && class == 0,
            ..Default::default()
        })
    }
    fn call(path: &str, iface: &str, method: &str) -> Result<(), String> {
        busctl_ok(Bus::System, &["call", BLUEZ, path, iface, method]).map_err(|e| e.to_string())
    }
    fn dev_path(id: u32) -> Result<String, String> { path_of(id).ok_or_else(|| "устройство не найдено".to_string()) }
}

impl BluetoothBackend for Bluez {
    fn state(&self) -> State {
        let Some(a) = adapter() else { return State::default() };
        let mut devices: Vec<Device> = tree(Bus::System, BLUEZ).into_iter().filter(|p| p.contains("/dev_")).filter_map(|p| Bluez::device(&p)).collect();
        sort_devices(&mut devices);
        State {
            available: true,
            enabled: prop_bool(Bus::System, BLUEZ, &a, "org.bluez.Adapter1", "Powered").unwrap_or(false),
            discovering: prop_bool(Bus::System, BLUEZ, &a, "org.bluez.Adapter1", "Discovering").unwrap_or(false),
            devices,
        }
    }
    fn set_enabled(&self, on: bool) -> Result<(), String> { crate::set_bluetooth_enabled(on).map_err(|e| e.to_string()) }
    fn set_discovering(&self, on: bool) -> Result<(), String> {
        let a = adapter().ok_or("нет адаптера")?;
        if on { scan_start() } else {
            scan_stop();
            // a scan started by someone else (or a stale session) is stopped explicitly; "not in progress" is not an error
            let _ = Bluez::call(&a, "org.bluez.Adapter1", "StopDiscovery");
            Ok(())
        }
    }
    fn pair(&self, id: u32) -> Result<(), String> { Bluez::call(&Bluez::dev_path(id)?, "org.bluez.Device1", "Pair") }
    fn cancel_pair(&self, id: u32) -> Result<(), String> { Bluez::call(&Bluez::dev_path(id)?, "org.bluez.Device1", "CancelPairing") }
    fn connect(&self, id: u32) -> Result<(), String> { Bluez::call(&Bluez::dev_path(id)?, "org.bluez.Device1", "Connect") }
    fn disconnect(&self, id: u32) -> Result<(), String> { Bluez::call(&Bluez::dev_path(id)?, "org.bluez.Device1", "Disconnect") }
    fn forget(&self, id: u32) -> Result<(), String> {
        let (a, p) = (adapter().ok_or("нет адаптера")?, Bluez::dev_path(id)?);
        busctl_ok(Bus::System, &["call", BLUEZ, &a, "org.bluez.Adapter1", "RemoveDevice", "o", &p]).map_err(|e| e.to_string())
    }
    fn set_trusted(&self, id: u32, on: bool) -> Result<(), String> {
        busctl_ok(Bus::System, &["set-property", BLUEZ, &Bluez::dev_path(id)?, "org.bluez.Device1", "Trusted", "b", if on { "true" } else { "false" }]).map_err(|e| e.to_string())
    }
    fn set_wake_allowed(&self, id: u32, on: bool) -> Result<(), String> {
        busctl_ok(Bus::System, &["set-property", BLUEZ, &Bluez::dev_path(id)?, "org.bluez.Device1", "WakeAllowed", "b", if on { "true" } else { "false" }]).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- in-memory radio

struct Mock { s: State, log: Vec<String>, /// pairing stays `pairing` until cancelled or finished by the test
    hold_pairing: bool, /// nearby devices appear only while discovering
    nearby: Vec<Device> }

pub struct MockBluetooth { m: Mutex<Mock> }

impl MockBluetooth {
    pub fn new(devices: Vec<Device>) -> MockBluetooth {
        let (nearby, known): (Vec<Device>, Vec<Device>) = devices.into_iter().partition(|d| !d.paired && !d.connected);
        MockBluetooth { m: Mutex::new(Mock { s: State { available: true, enabled: true, discovering: false, devices: known }, log: vec![], hold_pairing: false, nearby }) }
    }
    /// Generic demo devices for dry runs.
    pub fn demo() -> MockBluetooth {
        let d = |id: u32, et: u8, name: &str, paired: bool, connected: bool, battery: Option<u8>, rssi: i32| Device {
            id, etype: et, mac: format!("02:00:00:00:00:{id:02X}"), name: name.into(), paired, connected, trusted: paired, wake_supported: et == etype::GAMEPAD || et == etype::KEYBOARD,
            battery, rssi, ..Default::default()
        };
        MockBluetooth::new(vec![
            d(1, etype::GAMEPAD, "Беспроводной геймпад", true, true, Some(80), -48),
            d(2, etype::HEADPHONES, "Наушники", true, false, None, -60),
            d(3, etype::KEYBOARD, "Клавиатура", false, false, None, -52),
            d(4, etype::SPEAKER, "Колонка", false, false, None, -70),
            Device { hide_hint: true, ..d(5, etype::UNKNOWN, "", false, false, None, -80) },
            d(6, etype::PHONE, "Телефон", false, false, None, -66),
        ])
    }
    pub fn set_hold_pairing(&self, on: bool) { self.m.lock().unwrap().hold_pairing = on; }
    pub fn log(&self) -> Vec<String> { self.m.lock().unwrap().log.clone() }
    pub fn set_available(&self, on: bool) { self.m.lock().unwrap().s.available = on; }
    /// Finishes a held pairing: the device becomes paired and connected.
    pub fn finish_pairing(&self, id: u32) {
        let mut m = self.m.lock().unwrap();
        if let Some(d) = m.s.devices.iter_mut().find(|d| d.id == id) { d.pairing = false; d.paired = true; d.connected = true; d.trusted = true; }
    }
}

impl BluetoothBackend for MockBluetooth {
    fn state(&self) -> State {
        let m = self.m.lock().unwrap();
        let mut s = m.s.clone();
        if !s.enabled { for d in s.devices.iter_mut() { d.connected = false; d.busy = false; } s.devices.retain(|d| d.paired); }
        sort_devices(&mut s.devices);
        s
    }
    fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("enabled {on}"));
        m.s.enabled = on;
        if !on { m.s.discovering = false; }
        else { for d in m.s.devices.iter_mut().filter(|d| d.paired) { d.connected = d.id == 1; } }
        Ok(())
    }
    fn set_discovering(&self, on: bool) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("discovering {on}"));
        let on = on && m.s.enabled;
        m.s.discovering = on;
        if on {
            let found: Vec<Device> = m.nearby.drain(..).collect();
            for d in found { if !m.s.devices.iter().any(|x| x.id == d.id) { m.s.devices.push(d); } }
        } else {
            // a stopped scan forgets unpaired devices (BlueZ drops them after a timeout)
            let gone: Vec<Device> = m.s.devices.iter().filter(|d| !d.paired && !d.connected && !d.pairing).cloned().collect();
            m.s.devices.retain(|d| d.paired || d.connected || d.pairing);
            m.nearby.extend(gone);
        }
        Ok(())
    }
    fn pair(&self, id: u32) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("pair {id}"));
        let hold = m.hold_pairing;
        let d = m.s.devices.iter_mut().find(|d| d.id == id).ok_or("устройство не найдено")?;
        if hold { d.pairing = true; } else { d.paired = true; d.connected = true; d.trusted = true; d.pairing = false; }
        Ok(())
    }
    fn cancel_pair(&self, id: u32) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("cancel_pair {id}"));
        if let Some(d) = m.s.devices.iter_mut().find(|d| d.id == id) { d.pairing = false; }
        Ok(())
    }
    fn connect(&self, id: u32) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("connect {id}"));
        let d = m.s.devices.iter_mut().find(|d| d.id == id).ok_or("устройство не найдено")?;
        d.connected = true;
        Ok(())
    }
    fn disconnect(&self, id: u32) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("disconnect {id}"));
        let d = m.s.devices.iter_mut().find(|d| d.id == id).ok_or("устройство не найдено")?;
        d.connected = false;
        Ok(())
    }
    fn forget(&self, id: u32) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("forget {id}"));
        let discovering = m.s.discovering;
        if let Some(i) = m.s.devices.iter().position(|d| d.id == id) {
            let mut d = m.s.devices.remove(i);
            d.paired = false; d.connected = false; d.trusted = false;
            // while scanning the device is seen again as available
            if discovering { m.s.devices.push(d); } else { m.nearby.push(d); }
        }
        Ok(())
    }
    fn set_trusted(&self, id: u32, on: bool) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("trusted {id} {on}"));
        if let Some(d) = m.s.devices.iter_mut().find(|d| d.id == id) { d.trusted = on; }
        Ok(())
    }
    fn set_wake_allowed(&self, id: u32, on: bool) -> Result<(), String> {
        let mut m = self.m.lock().unwrap();
        m.log.push(format!("wake {id} {on}"));
        if let Some(d) = m.s.devices.iter_mut().find(|d| d.id == id) { d.wake_allowed = on; }
        Ok(())
    }
    fn is_mock(&self) -> bool { true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_hides_phones_displays_and_beacons() {
        let b = MockBluetooth::demo();
        b.set_discovering(true).unwrap();
        let shown: Vec<u32> = b.state().devices.iter().filter(|d| shown_by_default(d)).map(|d| d.id).collect();
        assert_eq!(shown, vec![1, 3, 2, 4], "sorted: connected, then by signal");
        assert_eq!(b.state().devices.len(), 6);
    }

    #[test]
    fn sort_is_connected_first_then_signal() {
        let mut v = MockBluetooth::demo().state().devices;
        v.push(Device { id: 9, rssi: -10, mac: "z".into(), ..Default::default() });
        sort_devices(&mut v);
        assert_eq!(v[0].id, 1, "connected first");
        assert_eq!(v[1].id, 9, "then the strongest signal");
    }

    #[test]
    fn mock_radio_follows_the_stock_lifecycle() {
        let b = MockBluetooth::demo();
        assert_eq!(b.state().devices.len(), 2, "nothing is nearby before a scan");
        b.set_discovering(true).unwrap();
        b.pair(3).unwrap();
        let s = b.state();
        assert!(s.devices.iter().any(|d| d.id == 3 && d.paired && d.connected));
        b.set_enabled(false).unwrap();
        let s = b.state();
        assert!(!s.enabled && !s.discovering && s.devices.iter().all(|d| !d.connected && d.paired));
        b.set_enabled(true).unwrap();
        b.forget(3).unwrap();
        assert!(b.state().devices.iter().all(|d| d.id != 3 || !d.paired));
    }

    #[test]
    fn held_pairing_can_be_cancelled() {
        let b = MockBluetooth::demo();
        b.set_discovering(true).unwrap();
        b.set_hold_pairing(true);
        b.pair(4).unwrap();
        assert!(b.state().devices.iter().any(|d| d.id == 4 && d.pairing && !d.paired));
        b.cancel_pair(4).unwrap();
        assert!(b.state().devices.iter().any(|d| d.id == 4 && !d.pairing && !d.paired));
    }

    #[test]
    fn class_of_device_maps_when_icon_is_missing() {
        assert_eq!(etype_of_class(0x240404), etype::HEADSET);
        assert_eq!(etype_of_class(0x240418), etype::HEADPHONES);
        assert_eq!(etype_of_class(0x240414), etype::SPEAKER, "loudspeaker (Bluedroid TV class family)");
        assert_eq!(etype_of_class(0x000540), etype::KEYBOARD);
        assert_eq!(etype_of_class(0x000508), etype::GAMEPAD);
        assert_eq!(etype_of_class(0x5a020c), etype::PHONE);
        assert_eq!(etype_of_class(0), etype::UNKNOWN);
    }

    #[test]
    fn bluez_icons_map_to_stock_types() {
        assert_eq!(etype_of_icon("input-gaming"), etype::GAMEPAD);
        assert_eq!(etype_of_icon("audio-headset"), etype::HEADSET);
        assert_eq!(etype_of_icon(""), etype::UNKNOWN);
        assert_eq!(etype_of_icon("camera-photo"), etype::MISC);
    }
}
