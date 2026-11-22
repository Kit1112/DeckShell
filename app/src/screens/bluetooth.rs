//! Settings → «Bluetooth» (route `/settings/bluetooth`, task 488): the stock Steam Deck Bluetooth page and its device dialog.
//!
//! Source of truth: `docs/steam-bluetooth-page-contract-2026-11-18.md` (stock module 56439 page, 81480 components, 15004 store;
//! element / state / button → result table in `docs/bluetooth-page-2026-11-19.md`). Everything that touches the radio goes through
//! `deck_system::bluetooth::BluetoothBackend`: BlueZ on the Deck, an in-memory radio in tests, `DECKSHELL_DRYRUN` and
//! `DECKSHELL_MOCK` — a test never changes a real adapter.
//!
//! The painting is shared with the Internet page (`ui/screens/internet.slint`, `InternetState`): rows and dialog rows are computed
//! here in window px so tests can lint them and walk the pad focus without Slint.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deck_system::bluetooth::{self as bt, BluetoothBackend, Bluez, Device, MockBluetooth};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::input::Action;
use crate::screens::internet::{self as net, dk, lt, rk, DRow, DialogView, Row, B, CTRL_W, DLG_PITCH, DLG_ROW_H, VIEW_BOTTOM, VIEW_TOP};
use crate::{InternetState, SettingsState, WifiDRow, WifiRow};

pub const PAGE: &str = "bluetooth";

/// Gaming Mode device cards at 1280×800 (owner reference 004026).
const DEVICE_H: f32 = 66.0;
const DEVICE_GAP: f32 = 9.0;
const FOCUS_BOTTOM_MARGIN: f32 = 29.0;
const TOGGLE_H: f32 = 68.0;
const SEP: f32 = 1.0;
/// stock `Z == 8`: the eighth connection flip of an «Xbox Wireless Controller» shows the firmware warning
const XBOX_FLIPS: u32 = 8;
const XBOX_NAME: &str = "Xbox Wireless Controller";

// ---------------------------------------------------------------- page model

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key { Toggle, ShowAll, Paired(u32), Avail(u32) }

impl Key {
    /// The row focus tag the shared painter carries: device id in the high bits, kind in the low two.
    pub fn tag(&self) -> net::Key {
        net::Key::Tag(match self { Key::Toggle => 0, Key::ShowAll => 1, Key::Paired(id) => (*id as u64) << 2 | 2, Key::Avail(id) => (*id as u64) << 2 | 3 })
    }
    pub fn from_tag(k: &net::Key) -> Option<Key> {
        let net::Key::Tag(t) = k else { return None };
        Some(match t & 3 { 0 => Key::Toggle, 1 => Key::ShowAll, 2 => Key::Paired((t >> 2) as u32), _ => Key::Avail((t >> 2) as u32) })
    }
}

#[derive(Clone, Debug, Default)]
pub struct PageData {
    pub available: bool,
    pub enabled: bool,
    pub discovering: bool,
    pub show_all: bool,
    pub paired: Vec<Device>,
    pub avail: Vec<Device>,
}

/// Stock lists (`Bi` / `Qi`): paired = paired or connected; available = the rest, filtered unless «Показать все устройства».
pub fn split(devices: &[Device], show_all: bool, busy: &HashSet<u32>) -> (Vec<Device>, Vec<Device>) {
    let mut paired = Vec::new();
    let mut avail = Vec::new();
    for d in devices {
        let mut d = d.clone();
        d.busy |= busy.contains(&d.id);
        if d.paired || d.connected { paired.push(d); } else if show_all || bt::shown_by_default(&d) { avail.push(d); }
    }
    bt::sort_devices(&mut paired);
    bt::sort_devices(&mut avail);
    (paired, avail)
}

fn device_title(d: &Device) -> String { if d.name.is_empty() { d.mac.clone() } else { d.name.clone() } }

fn device_row(rows: &mut Vec<Row>, y: &mut f32, d: &Device, key: Key, last: bool) {
    let not_connected = d.paired && !d.connected;
    let spinner = d.pairing || d.busy;
    let (desc, btn, level) = if spinner { (String::new(), String::new(), 0.0) }
        else if not_connected { (lt("#QuickAccess_Tab_Bluetooth_Not_Connected", "НЕ ПОДКЛЮЧЕНО").to_uppercase(), String::new(), 0.0) }
        else if let Some(p) = d.battery { (String::new(), format!("{p}%"), p as f32) }
        else { (String::new(), String::new(), 0.0) };
    rows.push(Row { key: Some(key.tag()), kind: rk::DEVICE, y: *y, h: DEVICE_H, label: device_title(d), desc, btn, btn_w: level, on: i32::from(not_connected && !spinner),
        bars: crate::assets::bt_device_icon(d.etype, &d.name), mark: if spinner { 2 } else { 0 }, warn: true, ..Default::default() });
    *y += DEVICE_H + if last { 0.0 } else { DEVICE_GAP };
}

/// Page layout (scroll 0). The first block starts where the stock toggle row does (y 153 on a 1280×800 Deck screenshot).
pub fn page_rows(d: &PageData) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut y = 153.0;
    rows.push(Row { key: Some(Key::Toggle.tag()), kind: rk::TOGGLE, y, h: TOGGLE_H, label: lt("#QuickAccess_Tab_Bluetooth_ToggleLabel", "Bluetooth"), on: if !d.available { -1 } else { i32::from(d.enabled) }, mark: 3, warn: true, ..Default::default() });
    y += TOGGLE_H + SEP;
    rows.push(Row { key: Some(Key::ShowAll.tag()), kind: rk::TOGGLE, y, h: TOGGLE_H, label: lt("#QuickAccess_Tab_Bluetooth_ShowAllDevices", "Показать все устройства"), on: i32::from(d.show_all), warn: true, ..Default::default() });
    y += TOGGLE_H;
    if !d.paired.is_empty() {
        net::section(&mut rows, &mut y, &lt("#QuickAccess_Tab_Bluetooth_Paired", "Привязано"));
        rows.last_mut().unwrap().warn = true;
        for (i, dev) in d.paired.iter().enumerate() { device_row(&mut rows, &mut y, dev, Key::Paired(dev.id), i + 1 == d.paired.len()); }
    }
    if d.enabled || !d.avail.is_empty() {
        let title = if !d.avail.is_empty() { lt("#QuickAccess_Tab_Bluetooth_Available_To_Pair", "Доступно для соединения") }
            else if d.discovering { lt("#QuickAccess_Tab_Bluetooth_Searching", "Поиск устройств...") }
            else { lt("#QuickAccess_Tab_Bluetooth_No_Devices_Found", "Устройства не обнаружены") };
        net::section(&mut rows, &mut y, &title);
        if let Some(h) = rows.last_mut() {
            h.warn = true;
            h.mark = if d.discovering { 2 } else { 0 };
        }
        for (i, dev) in d.avail.iter().enumerate() { device_row(&mut rows, &mut y, dev, Key::Avail(dev.id), i + 1 == d.avail.len()); }
    }
    rows
}

pub fn content_height(rows: &[Row]) -> f32 { net::content_height(rows) }

// ---------------------------------------------------------------- dialogs

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog { Info { id: u32, focus: usize }, Xbox }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item { Mac, Trusted, Wake, Forget, Toggle }

/// Focus items of the device dialog, top to bottom (contract §6.2); the connect / disconnect button is last and focused first.
pub fn info_items(d: &Device) -> Vec<Item> {
    let mut v = vec![Item::Mac, Item::Trusted];
    if d.wake_supported { v.push(Item::Wake); }
    v.extend([Item::Forget, Item::Toggle]);
    v
}

fn wake_label() -> String {
    // stock `g.yu()`: the Steam Deck name goes into the known-hardware template
    let deck = lt("#HardwareVariant_SteamDeck", "Steam Deck");
    if steam_assets::has_loc("#Settings_Bluetooth_AllowWake_KnownHardware") { crate::assets::tf("#Settings_Bluetooth_AllowWake_KnownHardware", &[&deck]) }
    else { format!("Разрешить этому устройству выводить {deck} из спящего режима") }
}

pub fn info_view(d: &Device, focus: usize, scroll: f32) -> DialogView {
    let mut b = B::new();
    net::title_block(&mut b, &device_title(d), None);
    let items = info_items(d);
    let mut focus_row = None;
    for (n, it) in items.iter().enumerate() {
        let (kind, label, value, on, h, w2, pitch) = match it {
            Item::Mac => (dk::INFO, lt("#Settings_Internet_MAC_Address", "MAC-адрес"), d.mac.clone(), 0, net::ROW_H, 0.0, 80.0),
            Item::Trusted => (dk::TOGGLE, lt("#Settings_Bluetooth_Trusted", "Доверенное"), String::new(), i32::from(d.trusted), 68.0, 0.0, 78.0),
            Item::Wake => (dk::TOGGLE, wake_label(), String::new(), i32::from(d.wake_allowed), 68.0, 0.0, 78.0),
            Item::Forget => (dk::BUTTON_ROW, lt("#QuickAccess_Tab_Bluetooth_Unpair", "Забыть устройство"), lt("#QuickAccess_Tab_Bluetooth_Forget", "Забыть"), 0, DLG_ROW_H, CTRL_W, DLG_PITCH),
            Item::Toggle if d.connected => (dk::BUTTON_ROW, lt("#QuickAccess_Tab_Bluetooth_DisconnectLabel", "Отключить устройство"), lt("#QuickAccess_Tab_Bluetooth_Disconnect", "Отключить"), 0, DLG_ROW_H, CTRL_W, DLG_PITCH),
            Item::Toggle => (dk::BUTTON_ROW, lt("#QuickAccess_Tab_Bluetooth_ConnectLabel", "Подключить устройство"), lt("#QuickAccess_Tab_Bluetooth_Connect", "Подключиться"), 0, DLG_ROW_H, CTRL_W, DLG_PITCH),
        };
        if focus == n { focus_row = Some(b.rows.len()); }
        b.push(DRow { kind, label, value, on, h, w2, focus: i32::from(focus == n), item: n as i32, ..Default::default() }, pitch);
    }
    net::finish(b, items.len(), focus, focus_row, false, 37.0, scroll)
}

fn xbox_view() -> DialogView {
    net::message_view(&lt("#Settings_Bluetooth_XboxIssue_Title", "Ошибка привязки контроллера Xbox"),
        &lt("#Settings_Bluetooth_XboxIssue_Desc", "У контроллеров Xbox со старой прошивкой есть проблема, которая не позволяет подключать их к устройствам со SteamOS. Подключите свой контроллер к ПК на Windows и воспользуйтесь приложением «Аксессуары Xbox», чтобы обновить прошивку до последней версии."),
        &lt("#Generic_GotIt", "Понятно!"), None, 0)
}

// ---------------------------------------------------------------- state

#[derive(Default)]
pub struct St {
    pub open: bool,
    pub state: bt::State,
    pub show_all: bool,
    pub focus: Option<Key>,
    pub vy: f32,
    pub dialog: Option<Dialog>,
    pub dscroll: f32,
    generation: u64,
    /// devices with an operation in flight (spinner on the row)
    busy: HashSet<u32>,
    /// scanning requested by this page (stock `RequestDiscovery` refcount, 0 or 1 here)
    discovery: bool,
    /// per MAC: (last seen connected, flips in a row, time of the last flip) for the Xbox firmware warning
    flaps: HashMap<String, (bool, u32, Instant)>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
    static TIMER: slint::Timer = slint::Timer::default();
    #[cfg(test)]
    static TEST_BACKEND: RefCell<Option<Arc<dyn BluetoothBackend>>> = const { RefCell::new(None) };
}

pub fn with<R>(f: impl FnOnce(&mut St) -> R) -> R { ST.with(|s| f(&mut s.borrow_mut())) }

static BACKEND: Mutex<Option<Arc<dyn BluetoothBackend>>> = Mutex::new(None);

/// The Bluetooth backend: BlueZ only outside tests, dry runs and mock mode.
pub fn backend() -> Arc<dyn BluetoothBackend> {
    #[cfg(test)]
    if let Some(backend) = TEST_BACKEND.with(|slot| slot.borrow().clone()) {
        return backend;
    }
    let mut g = BACKEND.lock().unwrap();
    g.get_or_insert_with(|| {
        let mock = cfg!(test) || crate::actions::dryrun() || std::env::var("DECKSHELL_MOCK").map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
        if mock { Arc::new(MockBluetooth::demo()) as Arc<dyn BluetoothBackend> } else { Arc::new(Bluez) }
    }).clone()
}
/// Tests and fixtures install their own radio.
#[cfg(test)]
pub fn set_backend(b: Arc<dyn BluetoothBackend>) { TEST_BACKEND.with(|slot| slot.replace(Some(b))); }

/// Stock `is_service_available`: the rail row exists only with a working Bluetooth service.
pub fn service_available() -> bool { backend().state().available }

pub fn sig() -> u64 { with(|s| match &s.dialog { None => 0, Some(Dialog::Xbox) => 1, Some(Dialog::Info { id, focus }) => 2 + (*focus as u64) * 4 + ((*id as u64) << 8) }) }

impl St {
    pub fn page_data(&self) -> PageData {
        let (paired, avail) = split(&self.state.devices, self.show_all, &self.busy);
        PageData { available: self.state.available, enabled: self.state.enabled, discovering: self.state.discovering, show_all: self.show_all, paired, avail }
    }
    pub fn rows(&self) -> Vec<Row> { page_rows(&self.page_data()) }
    fn focusable(&self) -> Vec<Key> { self.rows().into_iter().filter_map(|r| r.key.as_ref().and_then(Key::from_tag)).collect() }
    fn device(&self, id: u32) -> Option<Device> {
        let mut d = self.state.devices.iter().find(|d| d.id == id)?.clone();
        d.busy |= self.busy.contains(&id);
        Some(d)
    }
    fn fix_focus(&mut self) {
        let keys = self.focusable();
        if self.focus.as_ref().is_none_or(|k| !keys.contains(k)) { self.focus = keys.first().cloned(); }
        // a dialog of a vanished device closes
        if let Some(Dialog::Info { id, .. }) = &self.dialog { if self.device(*id).is_none() { self.dialog = None; } }
    }
    fn dialog_view(&self) -> Option<DialogView> {
        Some(match self.dialog.as_ref()? {
            Dialog::Info { id, focus } => info_view(&self.device(*id)?, *focus, self.dscroll),
            Dialog::Xbox => xbox_view(),
        })
    }
    /// Stock flapping detector (`sp.js:345568`): counts connection flips of an Xbox controller; true on the eighth in a row.
    /// Flips more than a minute apart start a new series.
    pub fn note_flip(&mut self, name: &str, mac: &str, connected: bool, now: Instant) -> bool {
        if name != XBOX_NAME { return false; }
        let e = self.flaps.entry(mac.to_string()).or_insert((connected, 0, now));
        if e.0 == connected { return false; }
        e.1 = if now.duration_since(e.2) > Duration::from_secs(60) { 1 } else { e.1 + 1 };
        *e = (connected, e.1, now);
        e.1 == XBOX_FLIPS
    }
}

// ---------------------------------------------------------------- polling

fn refresh() {
    let generation = with(|s| s.generation);
    if backend().is_mock() {
        // the in-memory radio answers at once: no worker, no event-loop hop
        absorb(generation, backend().state());
        return;
    }
    std::thread::spawn(move || {
        let state = backend().state();
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| apply_state(a, generation, state)); });
    });
}

/// Takes a backend snapshot into the page state. False when the page was left or the snapshot is from an older visit.
fn absorb(generation: u64, state: bt::State) -> bool {
    with(|s| {
        if !s.open || s.generation != generation { return false; }
        let now = Instant::now();
        let mut alert = false;
        for d in &state.devices { alert |= s.note_flip(&d.name, &d.mac, d.connected, now); }
        s.state = state;
        s.fix_focus();
        if alert && s.dialog.is_none() { s.dialog = Some(Dialog::Xbox); }
        true
    })
}

pub fn apply_state(a: &mut App, generation: u64, state: bt::State) {
    if absorb(generation, state) { push(a); }
}

fn start_polling() {
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(3), || {
        if with(|s| s.open) { refresh(); if backend().is_mock() { let _ = crate::with_app(|a| push(a)); } }
    }));
}

/// Stock `RequestDiscovery`: scanning runs while the page is mounted and the radio is on.
fn want_discovery(on: bool) {
    let change = with(|s| { let c = s.discovery != on; s.discovery = on; c });
    if !change { return; }
    let work = move || { let _ = backend().set_discovering(on); };
    if backend().is_mock() { work() } else { std::thread::spawn(work); }
}

// ---------------------------------------------------------------- entering / leaving

pub fn open(a: &mut App) {
    with(|s| {
        s.open = true;
        s.generation = s.generation.wrapping_add(1);
        s.dialog = None;
        s.vy = 0.0;
        s.focus = None;
        s.busy.clear();
        s.state = backend().state();
        s.flaps.clear();
    });
    if with(|s| s.state.enabled) { want_discovery(true); }
    refresh();
    start_polling();
    a.se.rows.clear();
    a.se.on.clear();
    a.se.cf = None;
    a.se.zone = 0;
    push(a);
}

pub fn leave(a: &mut App) {
    let was = with(|s| std::mem::replace(&mut s.open, false));
    if was {
        TIMER.with(|t| t.stop());
        want_discovery(false);
        with(|s| { s.generation = s.generation.wrapping_add(1); s.dialog = None; });
        let ui = a.ui();
        ui.global::<InternetState>().set_dialog_open(false);
    }
}

/// Rail → content (Right / A on the rail): focus the toggle.
pub fn enter(a: &mut App) {
    with(|s| s.fix_focus());
    a.se.zone = 1;
    scroll_to_focus();
    push(a);
}

// ---------------------------------------------------------------- input

pub fn hints() -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    let a_sel = ("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A);
    let (dlg, focus, page) = with(|s| (s.dialog.clone(), s.focus.clone(), s.page_data()));
    if matches!(dlg, Some(Dialog::Xbox)) { return vec![("a", "Понятно!", 1, "Enter", Action::A), b]; }
    if dlg.is_some() { return vec![a_sel, b]; }
    match focus {
        Some(Key::Paired(_)) => vec![("a", "Подробнее", 1, "Enter", Action::A), b],
        Some(Key::Avail(id)) if page.avail.iter().any(|d| d.id == id && d.pairing) => vec![("a", "Отвязать устройство", 1, "Enter", Action::A), b],
        Some(Key::Avail(_)) => vec![("a", "Привязать", 1, "Enter", Action::A), b],
        _ => vec![a_sel, b],
    }
}

fn step(len: usize, cur: usize, a: Action) -> usize {
    match a { Action::Up => cur.saturating_sub(1), Action::Down => (cur + 1).min(len.saturating_sub(1)), _ => cur }
}

pub fn handle(app: &mut App, a: Action) {
    if with(|s| s.dialog.is_some()) { return handle_dialog(app, a); }
    let keys = with(|s| s.focusable());
    let cur = with(|s| s.focus.as_ref().and_then(|k| keys.iter().position(|x| x == k))).unwrap_or(0);
    match a {
        Action::Up | Action::Down => {
            let n = step(keys.len(), cur, a);
            if n != cur { with(|s| s.focus = keys.get(n).cloned()); scroll_to_focus(); }
        }
        Action::Left => { app.se.zone = 0; app.se.cursor = app.se.focus; app.se.cf = None; }
        Action::A => return activate(app, keys.get(cur).cloned()),
        _ => return,
    }
    push(app);
}

/// Runs a backend operation: spinner on the device while it is in flight, state re-read afterwards. Blocking calls
/// (pairing, connecting) go to a worker thread on the real stack; the mock answers at once.
fn run_op(id: Option<u32>, op: impl FnOnce(&dyn BluetoothBackend) -> Result<(), String> + Send + 'static) {
    let b = backend();
    if b.is_mock() {
        let _ = op(&*b);
        refresh();
        return;
    }
    if let Some(id) = id { with(|s| { s.busy.insert(id); }); }
    std::thread::spawn(move || {
        let r = op(&*backend());
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| {
            if let Some(id) = id { with(|s| { s.busy.remove(&id); }); }
            if let Err(e) = r { a.toast(&e); }
            refresh();
            push(a);
        }); });
    });
}

pub fn activate(app: &mut App, key: Option<Key>) {
    let Some(key) = key else { return };
    with(|s| s.focus = Some(key.clone()));
    match key {
        Key::Toggle => toggle_radio(app),
        Key::ShowAll => { with(|s| s.show_all = !s.show_all); crate::sound::play(if with(|s| s.show_all) { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff }); }
        Key::Paired(id) => {
            let Some(d) = with(|s| s.device(id)) else { return };
            if d.busy { return; }
            // the stock card focuses «Отключить» for a connected device and «Подключиться» otherwise: both are the last item
            let focus = info_items(&d).len() - 1;
            with(|s| { s.dialog = Some(Dialog::Info { id, focus }); s.dscroll = 0.0; });
        }
        Key::Avail(id) => {
            let Some(d) = with(|s| s.device(id)) else { return };
            if d.busy { return; }
            if d.pairing { run_op(Some(id), move |b| b.cancel_pair(id)); } else { run_op(Some(id), move |b| b.pair(id)); }
        }
    }
    push(app);
}

fn toggle_radio(app: &mut App) {
    let (available, want) = with(|s| (s.state.available, !s.state.enabled));
    if !available { return; } // stock: the switch is disabled without the service
    crate::sound::play(if want { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
    // stock stores `system_bluetooth_enabled`; the service follows it (DRYRUN-gated setter, only logged in tests)
    crate::setters::client_setting("system_bluetooth_enabled", &serde_json::json!(want));
    with(|s| {
        s.state.enabled = want;
        if !want { s.state.discovering = false; s.state.devices.retain(|d| d.paired || d.connected); for d in s.state.devices.iter_mut() { d.connected = false; } }
    });
    // one operation: power, then scanning follows the radio (stock `RequestDiscovery` after power-on, none after power-off)
    with(|s| s.discovery = want);
    run_op(None, move |b| { b.set_enabled(want)?; b.set_discovering(want) });
    let _ = app;
}

fn handle_dialog(app: &mut App, a: Action) {
    let Some(dlg) = with(|s| s.dialog.clone()) else { return };
    match dlg {
        Dialog::Xbox => if matches!(a, Action::A | Action::B) { close(app); },
        Dialog::Info { id, focus } => {
            let Some(d) = with(|s| s.device(id)) else { return close(app) };
            let items = info_items(&d);
            match a {
                Action::Up | Action::Down => { with(|s| s.dialog = Some(Dialog::Info { id, focus: step(items.len(), focus, a) })); push(app); }
                Action::B => close(app),
                Action::A => info_activate(app, &d, items[focus.min(items.len() - 1)]),
                _ => {}
            }
        }
    }
}

fn info_activate(app: &mut App, d: &Device, it: Item) {
    let id = d.id;
    match it {
        Item::Mac => return,
        Item::Trusted => {
            let on = !d.trusted;
            with(|s| if let Some(x) = s.state.devices.iter_mut().find(|x| x.id == id) { x.trusted = on; });
            crate::sound::play(if on { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
            run_op(None, move |b| b.set_trusted(id, on));
        }
        Item::Wake => {
            let on = !d.wake_allowed;
            with(|s| if let Some(x) = s.state.devices.iter_mut().find(|x| x.id == id) { x.wake_allowed = on; });
            crate::sound::play(if on { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
            run_op(None, move |b| b.set_wake_allowed(id, on));
        }
        Item::Forget => {
            close_quiet();
            with(|s| s.state.devices.retain(|x| x.id != id));
            run_op(Some(id), move |b| b.forget(id));
        }
        Item::Toggle => {
            close_quiet();
            if d.connected { run_op(Some(id), move |b| b.disconnect(id)); } else { run_op(Some(id), move |b| b.connect(id)); }
        }
    }
    with(|s| s.fix_focus());
    push(app);
}

fn close_quiet() { with(|s| s.dialog = None); }
fn close(app: &mut App) { close_quiet(); with(|s| s.fix_focus()); push(app); }

/// B: closes an open dialog. False when the page has nothing to close.
pub fn back(app: &mut App) -> bool {
    if with(|s| s.dialog.is_some()) { handle_dialog(app, Action::B); return true; }
    false
}

/// Touch / mouse on a page row.
pub fn row_clicked(app: &mut App, i: i32) {
    app.se.zone = 1;
    let key = with(|s| s.rows().get(i as usize).and_then(|r| r.key.as_ref().and_then(Key::from_tag)));
    activate(app, key);
}

/// Touch / mouse on a dialog control: focus it and press A.
pub fn dialog_clicked(app: &mut App, item: i32, _side: i32) {
    if item < 0 { return; }
    with(|s| if let Some(Dialog::Info { focus, .. }) = &mut s.dialog { *focus = item as usize; });
    handle_dialog(app, Action::A);
}

// ---------------------------------------------------------------- scrolling

fn scroll_to_focus() {
    with(|s| {
        let rows = s.rows();
        let Some(r) = rows.iter().find(|r| r.key.is_some() && r.key.as_ref().and_then(Key::from_tag) == s.focus) else { return };
        let (top, bottom) = (r.y - s.vy, r.y + r.h - s.vy);
        if top < VIEW_TOP + 20.0 { s.vy = (r.y - (VIEW_TOP + 20.0)).max(0.0); }
        else if bottom > VIEW_BOTTOM - FOCUS_BOTTOM_MARGIN { s.vy = r.y + r.h - (VIEW_BOTTOM - FOCUS_BOTTOM_MARGIN); }
        let max = (content_height(&rows) - VIEW_BOTTOM).max(0.0);
        s.vy = s.vy.clamp(0.0, max);
        // the first row shows the page title above it
        if rows.first().is_some_and(|f| f.key.as_ref().and_then(Key::from_tag) == s.focus) { s.vy = 0.0; }
    });
}

// ---------------------------------------------------------------- painting

/// Pushes the whole page and the dialog to Slint (the shared Internet page painter).
pub fn push(app: &mut App) {
    let (rows, dv, vy, focus_idx) = with(|s| {
        s.fix_focus();
        let rows = s.rows();
        let mut dv = s.dialog_view();
        if let Some(v) = &mut dv { s.dscroll = v.scroll; }
        let focus_idx = s.focus.as_ref().and_then(|k| rows.iter().position(|r| r.key.as_ref() == Some(&k.tag())));
        (rows, dv, s.vy, focus_idx)
    });
    app.se.cf = focus_idx;
    let ui = app.ui();
    let st = ui.global::<SettingsState>();
    let show_focus = app.se.zone == 1 && dv.is_none();
    st.set_is_internet(true);
    st.set_is_anim(false);
    st.set_is_recording(false);
    st.set_is_storage(false);
    st.set_storage_modal(false);
    st.set_focus(app.se.focus as i32);
    st.set_cursor(app.se.cursor as i32);
    st.set_zone(app.se.zone as i32);
    st.set_page_title(app.se.pages.get(app.se.focus).map(|p| p.label.as_str()).unwrap_or("").into());
    st.set_content_h(content_height(&rows).into());
    st.set_vy((-vy).into());
    let model: Vec<WifiRow> = rows.iter().enumerate().map(|(i, r)| net::to_wrow(r, show_focus && focus_idx == Some(i))).collect();
    let is = ui.global::<InternetState>();
    is.set_rows(ModelRc::new(VecModel::from(model)));
    match dv {
        Some(v) => {
            is.set_dialog_open(true);
            is.set_box_y(v.box_y);
            is.set_box_h(v.box_h);
            is.set_content_h(v.content_h);
            is.set_scroll(v.scroll);
            is.set_drows(ModelRc::new(VecModel::from(v.rows.iter().map(net::to_drow).collect::<Vec<_>>())));
        }
        None => { is.set_dialog_open(false); is.set_drows(ModelRc::new(VecModel::from(Vec::<WifiDRow>::new()))); }
    }
    app.update_hint();
}

#[cfg(test)]
#[path = "bluetooth/tests.rs"]
mod tests;
