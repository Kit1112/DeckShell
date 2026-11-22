//! Settings → «Интернет» (route `/settings/internet`, task 451): the stock Steam Deck Wi-Fi page and its dialogs.
//!
//! Source of truth: `docs/internet-wifi-page-2026-11-18.md` (element/state/button → result table) and the stock client
//! (`sp.js` modules 29763 page, 29171 connect dialog, 45935 network info). Everything that touches the network goes through
//! `deck_system::wifi::WifiBackend`: NetworkManager on the Deck, an in-memory network in tests, `DECKSHELL_DRYRUN` and
//! `DECKSHELL_MOCK` (never a real change of the owner's connection from a test).
//!
//! Layout is computed here in window px (`page_rows`, `dialog_view`) so tests can lint it and walk the pad focus without
//! Slint; `ui/screens/internet.slint` only paints the result.
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deck_system::wifi::{self, AccessPoint, ConnectError, ConnectRequest, Connectivity, Details, MockWifi, Nmcli, Profile, Security, StaticIp, WifiBackend};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::screens::osk::{self, ExtSink, InputMode};
use crate::{InternetState, SettingsState, WifiDRow, WifiRow};

mod proxy;
use proxy::{ProxyCfg, ProxyForm};

pub const PAGE: &str = "internet";

// ---------------------------------------------------------------- texts (Steam localization, Russian fallback)

pub(crate) fn lt(token: &str, fallback: &str) -> String { t_or(token, fallback) }
fn lf(token: &str, fallback: &str, arg: &str) -> String {
    if steam_assets::has_loc(token) { crate::assets::tf(token, &[arg]) } else { fallback.replace("%1$s", arg) }
}

// ---------------------------------------------------------------- geometry (px of the 1280×800 window, scroll 0)

pub const X: f32 = 396.0;
pub const W: f32 = 849.0;
pub(crate) const VIEW_TOP: f32 = 60.0;
pub(crate) const VIEW_BOTTOM: f32 = 740.0;
pub(crate) const ROW_H: f32 = 70.0;
pub(crate) const GAP: f32 = 10.0;
/// bottom of a block → centre of the next section header; header centre → first row top
pub(crate) const SECTION_GAP: f32 = 64.0;
pub(crate) const SECTION_TO_ROW: f32 = 27.0;
pub(crate) const HEADER_H: f32 = 30.0;

pub const DLG_X: f32 = 148.0;
pub const DLG_W: f32 = 987.0;
pub const DLG_IN: f32 = 36.0;
pub const DLG_IN_W: f32 = DLG_W - 2.0 * DLG_IN;
pub(crate) const DLG_MAX_H: f32 = VIEW_BOTTOM - VIEW_TOP;
pub(crate) const DLG_ROW_H: f32 = 96.0;
pub(crate) const DLG_PITCH: f32 = 105.0;
pub(crate) const DLG_BTN_H: f32 = 60.0;
/// width of the control at the right end of a dialog row
pub const CTRL_W: f32 = 375.0;

// ---------------------------------------------------------------- page model

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key { Toggle, Ap(String), Other, Offline, Proxy, Delete, /** opaque focus tag of another native page sharing this painter (Bluetooth, task 488) */ Tag(u64) }

/// Row kinds (match `ui/screens/internet.slint`).
pub mod rk {
    pub const SECTION: i32 = 1;
    pub const TOGGLE: i32 = 2;
    pub const AP: i32 = 3;
    pub const PLAIN: i32 = 4;
    pub const DISABLED: i32 = 5;
    pub const SETTING: i32 = 6;
    pub const WARNING: i32 = 7;
    /// Bluetooth device row (task 488)
    pub const DEVICE: i32 = 8;
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    pub key: Option<Key>,
    pub kind: i32,
    pub y: f32,
    pub h: f32,
    pub label: String,
    pub desc: String,
    /// right button text (`SETTING`) / right button width in `btn_w`
    pub btn: String,
    pub btn_w: f32,
    pub on: i32,
    pub bars: i32,
    pub lock: bool,
    /// 0 nothing, 1 check mark, 2 spinner
    pub mark: i32,
    /// the row carries the caution triangle (offline banner, connectivity warning)
    pub warn: bool,
}

#[derive(Clone, Debug)]
pub struct Net { pub ssid: String, pub bars: u8, pub lock: bool, pub connected: bool, pub busy: bool }

/// Everything the page shows, decoupled from the backend so the layout can be built from plain data.
#[derive(Clone, Debug, Default)]
pub struct PageData {
    pub radio: Option<bool>,
    pub connected: Vec<Net>,
    pub found: Vec<Net>,
    pub offline: bool,
    /// no network at all: the stock offline button is disabled with a hint
    pub has_network: bool,
    pub connectivity: Option<Connectivity>,
}

pub(crate) fn section(rows: &mut Vec<Row>, y: &mut f32, label: &str) {
    let centre = *y + SECTION_GAP;
    rows.push(Row { kind: rk::SECTION, y: centre - HEADER_H / 2.0, h: HEADER_H, label: label.into(), ..Default::default() });
    *y = centre + SECTION_TO_ROW;
}

fn ap_row(rows: &mut Vec<Row>, y: &mut f32, n: &Net) {
    rows.push(Row { key: Some(Key::Ap(n.ssid.clone())), kind: rk::AP, y: *y, h: ROW_H, label: n.ssid.clone(), bars: n.bars as i32, lock: n.lock, mark: if n.busy { 2 } else if n.connected { 1 } else { 0 }, ..Default::default() });
    *y += ROW_H + GAP;
}

/// Page layout (scroll 0). The first block starts where the stock toggle row does (y 153 on a 1280×800 Deck screenshot).
pub fn page_rows(d: &PageData) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut y = 153.0;
    // connectivity warning (stock `re`): caution row above the toggle
    if !d.offline {
        let text = match d.connectivity {
            Some(Connectivity::Limited) | Some(Connectivity::None) if d.radio == Some(true) && !d.connected.is_empty() => Some(lt("#Settings_Internet_Connected_To_Network_But_Not_Internet", "У Steam Deck нет доступа в интернет.")),
            Some(Connectivity::Portal) => Some(lt("#Settings_Internet_Captive_Portal_Detected", "Для выхода в интернет требуется дополнительная информация.")),
            _ => None,
        };
        if let Some(label) = text {
            rows.push(Row { kind: rk::WARNING, y, h: ROW_H, label, warn: true, ..Default::default() });
            y += ROW_H + GAP;
        }
    }
    rows.push(Row { key: Some(Key::Toggle), kind: rk::TOGGLE, y, h: 68.0, label: lt("#Settings_Internet_Wifi_Enabled", "Включить Wi-Fi"), on: match d.radio { Some(true) => 1, Some(false) => 0, None => -1 }, ..Default::default() });
    y += 68.0;
    let on = d.radio == Some(true);
    if on && !d.connected.is_empty() {
        section(&mut rows, &mut y, &lt("#Settings_Internet_Connected_Networks", "Подключённые сети"));
        for n in &d.connected { ap_row(&mut rows, &mut y, n); }
        y -= GAP;
    }
    section(&mut rows, &mut y, &lt("#Settings_Internet_Networks_Found", "Найденные сети"));
    if d.found.is_empty() {
        let label = if on { lt("#Settings_Internet_No_Other_Networks_Found", "Другие сети не найдены") } else { lt("#Settings_Internet_No_Other_Networks_Found_Wifi_Disabled", "Другие сети не найдены. Wi-Fi отключён.") };
        rows.push(Row { kind: rk::DISABLED, y, h: ROW_H, label, ..Default::default() });
        y += ROW_H + GAP;
    }
    for n in &d.found { ap_row(&mut rows, &mut y, n); }
    if on {
        rows.push(Row { key: Some(Key::Other), kind: rk::PLAIN, y, h: 65.0, label: lt("#Settings_Internet_Custom_Network_Button", "Другая сеть…"), ..Default::default() });
        y += 65.0;
    } else {
        y -= GAP;
    }
    section(&mut rows, &mut y, &lt("#Settings_Internet_Advanced_Settings", "Дополнительные настройки"));
    if d.offline {
        rows.push(Row { key: Some(Key::Offline), kind: rk::SETTING, y, h: 95.0, label: lt("#Settings_Internet_In_Offline_Mode", "Steam находится в автономном режиме."), btn: lt("#GoOnline", "Войти в сеть"), btn_w: 240.0, warn: true, ..Default::default() });
        y += 95.0 + GAP;
    } else {
        let desc = if d.has_network { String::new() } else { lt("#UserManagement_OfflineMode_NoNetwork", "Нет подключения к сети") };
        rows.push(Row { key: d.has_network.then_some(Key::Offline), kind: rk::SETTING, y, h: 125.0, label: lt("#Settings_Internet_Go_Offline", "Автономный режим"), desc, btn: lt("#GoOffline", "Перейти в автономный режим"), btn_w: 406.0, on: i32::from(!d.has_network), ..Default::default() });
        y += 125.0 + GAP;
    }
    rows.push(Row { key: Some(Key::Proxy), kind: rk::SETTING, y, h: 95.0, label: lt("#Settings_Internet_HTTPProxy", "HTTP-прокси"), btn: lt("#Settings_Internet_ConfigureProxy", "Настроить"), btn_w: 240.0, ..Default::default() });
    y += 95.0 + GAP;
    rows.push(Row { key: Some(Key::Delete), kind: rk::SETTING, y, h: 96.0, label: lt("#Settings_Internet_WebBrowserDataDelete", "Удалить данные браузера"), btn: lt("#Settings_Internet_WebBrowserDataDeleteButton", "Удалить"), btn_w: 240.0, ..Default::default() });
    rows
}

pub fn content_height(rows: &[Row]) -> f32 { rows.iter().map(|r| r.y + r.h).fold(0.0, f32::max) + 40.0 }

// ---------------------------------------------------------------- dialogs

/// Focus items of the connect form, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum F { Ssid, Security, Pass, AdvShow, Ip, Mask, Gw, Dns1, Dns2, Mac, Connect, Cancel }

#[derive(Clone, Debug)]
pub struct Form {
    /// «Другая сеть…»: SSID and security are typed in
    pub custom: bool,
    pub ssid: String,
    pub security: Security,
    pub pass: String,
    pub show_pass: bool,
    pub adv: bool,
    pub ip: StaticIp,
    /// the previous attempt failed: red «Не удалось подключиться», «Повторно введите…»
    pub failed: bool,
    pub focus: F,
    pub mac: String,
}

impl Form {
    pub fn new(ssid: &str, security: Security, custom: bool, mac: &str) -> Form {
        let focus = if custom { F::Ssid } else if security.needs_password() { F::Pass } else { F::Connect };
        Form { custom, ssid: ssid.into(), security: if custom { Security::Wpa2 } else { security }, pass: String::new(), show_pass: false, adv: false, ip: StaticIp::default(), failed: false, focus, mac: mac.into() }
    }
    pub fn items(&self) -> Vec<F> {
        let mut v = Vec::new();
        if self.custom { v.extend([F::Ssid, F::Security]); }
        if self.security.needs_password() { v.push(F::Pass); }
        if self.adv { v.extend([F::Ip, F::Mask, F::Gw, F::Dns1, F::Dns2, F::Mac]); } else { v.push(F::AdvShow); }
        v.extend([F::Connect, F::Cancel]);
        v
    }
    pub fn can_connect(&self) -> bool { !self.custom || !self.ssid.trim().is_empty() }
    fn text_mut(&mut self, f: F) -> Option<&mut String> {
        Some(match f { F::Ssid => &mut self.ssid, F::Pass => &mut self.pass, F::Ip => &mut self.ip.ip, F::Mask => &mut self.ip.netmask, F::Gw => &mut self.ip.gateway, F::Dns1 => &mut self.ip.dns1, F::Dns2 => &mut self.ip.dns2, _ => return None })
    }
    /// The request, or the field to focus and what is wrong with it.
    pub fn request(&self) -> Result<ConnectRequest, (F, String)> {
        let ip = if self.adv && !self.ip.is_empty() {
            wifi::validate_static(&self.ip).map_err(|e| match e {
                wifi::IpError::Missing(n) => (field_of(n), format!("Заполните поле «{n}»")),
                wifi::IpError::Invalid(n) => (field_of(n), format!("Неверное значение в поле «{n}»")),
            })?;
            Some(self.ip.clone())
        } else { None };
        Ok(ConnectRequest { ssid: self.ssid.trim().to_string(), security: self.security, username: String::new(), password: if self.security.needs_password() { self.pass.clone() } else { String::new() }, hidden: self.custom, ip })
    }
}

fn field_of(name: &str) -> F {
    match name { "IP" => F::Ip, "Маска сети" => F::Mask, "Шлюз" => F::Gw, "Первичный DNS" => F::Dns1, _ => F::Dns2 }
}

#[derive(Clone, Debug)]
pub struct Info { pub ssid: String, pub focus: usize, pub details: Option<Details>, pub saved: Option<Profile>, pub connected: bool, pub connectable: bool, pub present: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage { Connecting, Checking, Success, Failed, NoInternet, Portal }

#[derive(Clone, Debug)]
pub struct Prog { pub ssid: String, pub stage: Stage, pub back: Option<Form>, pub secured: bool, pub focus: usize }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirm { GoOffline, GoOnline, DeleteBrowser }

#[derive(Clone, Debug)]
pub enum AlertKind { NotFound(String), MustCloseGames, NoNetwork, Unsupported }

#[derive(Clone, Debug)]
pub enum Dialog {
    Form(Form),
    /// drop-down of security protocols over the form (index into `CUSTOM_SECURITY`)
    Pick(Form, usize),
    Info(Info),
    Progress(Prog),
    Confirm(Confirm, usize),
    Alert(AlertKind),
    Proxy(ProxyForm),
}

/// One painted row of a dialog (box-relative px).
#[derive(Clone, Debug, Default)]
pub struct DRow {
    pub kind: i32,
    pub y: f32,
    pub h: f32,
    pub label: String,
    pub value: String,
    pub hint: String,
    pub on: i32,
    /// 0 none, 1 focused, 2 right half of a pair focused
    pub focus: i32,
    pub a: i32,
    pub b: i32,
    pub w2: f32,
    /// the pad focus index this row belongs to (for scrolling and touch)
    pub item: i32,
}

/// Dialog row kinds (match `ui/screens/internet.slint`).
pub mod dk {
    pub const TITLE: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const FIELD: i32 = 2;
    pub const DROPDOWN: i32 = 3;
    pub const BUTTON_ROW: i32 = 4;
    pub const TOGGLE: i32 = 5;
    pub const INFO: i32 = 6;
    pub const SECTION: i32 = 7;
    pub const PAIR: i32 = 8;
    pub const SINGLE: i32 = 9;
    pub const BODY: i32 = 10;
    pub const ICONS: i32 = 11;
    pub const STATUS: i32 = 12;
    pub const STRIP: i32 = 13;
    pub const OPTION: i32 = 14;
}

#[derive(Clone, Debug, Default)]
pub struct DialogView {
    pub rows: Vec<DRow>,
    /// content height (title included) and the visible box
    pub content_h: f32,
    pub box_y: f32,
    pub box_h: f32,
    pub scroll: f32,
    /// pad focus items count and the focused one
    pub items: usize,
    pub focus: usize,
}

pub(crate) struct B { pub rows: Vec<DRow>, pub y: f32, pub last_bottom: f32 }
impl B {
    pub(crate) fn new() -> B { B { rows: Vec::new(), y: 0.0, last_bottom: 0.0 } }
    pub(crate) fn push(&mut self, mut r: DRow, pitch: f32) {
        r.y = self.y;
        self.last_bottom = r.y + r.h;
        self.y += pitch;
        self.rows.push(r);
    }
}

/// Title (bold, 32 px) with an optional red error line above it; the first control starts at y 94 (124 with the error).
pub(crate) fn title_block(b: &mut B, title: &str, error: Option<&str>) {
    let dy = if error.is_some() { 30.0 } else { 0.0 };
    if let Some(e) = error {
        b.rows.push(DRow { kind: dk::ERROR, y: 10.0, h: 34.0, label: e.into(), item: -1, ..Default::default() });
    }
    b.rows.push(DRow { kind: dk::TITLE, y: 36.0 + dy, h: 46.0, label: title.into(), item: -1, ..Default::default() });
    b.y = 94.0 + dy;
}

/// Number of text lines the stock body text takes at 24 px in a 913 px column (conservative, ≥ the real wrap).
fn body_lines(text: &str) -> usize { text.chars().count().div_ceil(58).max(1) }

fn mask(s: &str) -> String { "•".repeat(s.chars().count()) }

fn state_label(info: &Info) -> String {
    if info.connected { lt("#Internet_Network_State_Connected", "Подключено") }
    else if !info.present && info.saved.is_some() { lt("#Internet_Network_State_NotFound", "Не найдено") }
    else { lt("#Internet_Network_State_Disconnected", "Отключено") }
}

/// Box placement and scrolling. `focus_row` is the row to keep visible; the first control shows the title, the last one the padding.
pub(crate) fn finish(b: B, items: usize, focus: usize, focus_row: Option<usize>, osk: bool, pad_bottom: f32, scroll: f32) -> DialogView {
    let content_h = b.y.max(b.last_bottom) + pad_bottom;
    let box_h = content_h.min(DLG_MAX_H);
    let top = if content_h >= DLG_MAX_H - 40.0 { VIEW_TOP + (DLG_MAX_H - box_h) / 2.0 } else if content_h >= 600.0 { 411.0 - box_h / 2.0 } else { 399.0 - box_h / 2.0 };
    let (box_y, box_h) = if osk { (VIEW_TOP, box_h.min(330.0)) } else { (top, box_h) };
    let max = (content_h - box_h).max(0.0);
    let mut scroll = scroll.clamp(0.0, max);
    if let Some(r) = focus_row.and_then(|i| b.rows.get(i)) {
        let (top, bottom) = (r.y, r.y + r.h);
        if top < scroll + 12.0 { scroll = top - 12.0; }
        if bottom > scroll + box_h - 12.0 { scroll = bottom - box_h + 12.0; }
        if top < 200.0 { scroll = 0.0; }
        if bottom + pad_bottom >= content_h - 0.5 { scroll = max; }
        scroll = scroll.clamp(0.0, max);
    }
    DialogView { rows: b.rows, content_h, box_y, box_h, scroll, items, focus }
}

fn field_row(b: &mut B, label: &str, value: String, hint: &str, focus: bool, item: usize, eye: Option<bool>, on: bool) {
    b.push(DRow { kind: dk::FIELD, h: DLG_ROW_H, label: label.into(), value, hint: hint.into(), focus: i32::from(focus), a: match eye { Some(true) => 2, Some(false) => 1, None => 0 }, on: i32::from(on), item: item as i32, ..Default::default() }, DLG_PITCH);
}

/// Rows of the connect/password dialog.
fn form_view(f: &Form, st_osk: bool, scroll: f32) -> DialogView {
    let mut b = B::new();
    let title = match (f.custom, f.failed) {
        (true, true) => lt("#Settings_Internet_Reenter_Network_Credentials", "Введите данные вашей сети ещё раз"),
        (true, false) => lt("#Settings_Internet_Enter_Network_Credentials", "Введите данные вашей сети"),
        (false, true) => lf("#Settings_Internet_Reenter_The_Password_For_Network", "Повторно введите пароль для %1$s", &f.ssid),
        (false, false) => lf("#Settings_Internet_Enter_The_Password_For_Network", "Введите пароль для %1$s", &f.ssid),
    };
    let error = f.failed.then(|| lt("#Settings_Internet_Failed_To_Connect", "Не удалось подключиться"));
    title_block(&mut b, &title, error.as_deref());
    let items = f.items();
    let mut focus_row = None;
    for (i, it) in items.iter().enumerate() {
        let foc = f.focus == *it;
        let row0 = b.rows.len();
        match it {
            F::Ssid => field_row(&mut b, &lt("#Settings_Internet_Enter_NetworkName", "Имя сети"), f.ssid.clone(), "", foc, i, None, false),
            F::Security => b.push(DRow { kind: dk::DROPDOWN, h: DLG_ROW_H, label: lt("#Settings_Internet_Security_Type", "Протокол безопасности"), value: f.security.label().into(), focus: i32::from(foc), item: i as i32, ..Default::default() }, DLG_PITCH),
            F::Pass => field_row(&mut b, &lt("#Settings_Internet_Enter_Password", "Пароль"), if f.show_pass { f.pass.clone() } else { mask(&f.pass) }, "", foc, i, Some(f.show_pass), false),
            F::AdvShow => b.push(DRow { kind: dk::BUTTON_ROW, h: DLG_ROW_H, label: lt("#Settings_Internet_Connect_Advanced_Enable_Label", "Расширенные настройки…"), value: lt("#Settings_Internet_Connect_Advanced_Enable", "Показать"), w2: 240.0, focus: i32::from(foc), item: i as i32, ..Default::default() }, DLG_PITCH),
            F::Ip => field_row(&mut b, &lt("#Settings_Internet_IP", "IP"), f.ip.ip.clone(), " .  .  . ", foc, i, None, true),
            F::Mask => field_row(&mut b, &lt("#Settings_Internet_Netmask", "Маска сети"), f.ip.netmask.clone(), " .  .  . ", foc, i, None, true),
            F::Gw => field_row(&mut b, &lt("#Settings_Internet_Gateway", "Шлюз"), f.ip.gateway.clone(), " .  .  . ", foc, i, None, true),
            F::Dns1 => field_row(&mut b, &lt("#Settings_Internet_Primary_DNS", "Первичный DNS"), f.ip.dns1.clone(), " .  .  . ", foc, i, None, true),
            F::Dns2 => field_row(&mut b, &lt("#Settings_Internet_Secondary_DNS", "Вторичный DNS"), f.ip.dns2.clone(), " .  .  . ", foc, i, None, true),
            F::Mac => b.push(DRow { kind: dk::INFO, h: 70.0, label: lt("#Settings_Internet_MAC_Address", "MAC-адрес"), value: f.mac.clone(), focus: i32::from(foc), item: i as i32, ..Default::default() }, 70.0),
            F::Connect | F::Cancel => {
                if *it == F::Connect {
                    let pf = if f.focus == F::Connect { 1 } else if f.focus == F::Cancel { 2 } else { 0 };
                    b.y = b.last_bottom + 24.0;
                    b.push(DRow { kind: dk::PAIR, h: DLG_BTN_H, label: lt("#Button_Cancel", "Отмена"), value: lt("#Settings_Internet_Connect", "Подключиться"), on: i32::from(f.can_connect()), focus: pf, item: i as i32, ..Default::default() }, DLG_BTN_H);
                }
                if foc { focus_row = Some(b.rows.len() - 1); }
                continue;
            }
        }
        if foc { focus_row = Some(row0); }
    }
    let focus = items.iter().position(|i| *i == f.focus).unwrap_or(0);
    finish(b, items.len(), focus, focus_row, st_osk, 37.0, scroll)
}

fn pick_view(cur: usize, sel: usize) -> DialogView {
    let mut b = B::new();
    title_block(&mut b, &lt("#Settings_Internet_Security_Type", "Протокол безопасности"), None);
    b.y = 80.0;
    for (i, s) in wifi::CUSTOM_SECURITY.iter().enumerate() {
        b.push(DRow { kind: dk::OPTION, h: 72.0, label: s.label().into(), focus: i32::from(i == sel), on: i32::from(i == cur), item: i as i32, ..Default::default() }, 72.0);
    }
    let n = wifi::CUSTOM_SECURITY.len();
    finish(b, n, sel, None, false, 24.0, 0.0)
}

fn info_view(i: &Info, scroll: f32) -> DialogView {
    let mut b = B::new();
    title_block(&mut b, &i.ssid, None);
    let mut items = 0usize;
    let mut focus_row = None;
    let mut next = |b: &mut B, mut r: DRow, pitch: f32, items: &mut usize| {
        r.item = *items as i32;
        r.focus = i32::from(i.focus == *items);
        if r.focus == 1 { focus_row = Some(b.rows.len()); }
        *items += 1;
        b.push(r, pitch);
    };
    let btn = |label: String| DRow { kind: dk::BUTTON_ROW, h: DLG_ROW_H, value: label, w2: CTRL_W, ..Default::default() };
    if i.saved.is_some() { next(&mut b, btn(lt("#Settings_Internet_Forget", "Забыть")), DLG_PITCH, &mut items); }
    if i.connectable { next(&mut b, btn(lt("#Settings_Internet_Connect", "Подключиться")), DLG_PITCH, &mut items); }
    if i.connected { next(&mut b, btn(lt("#Settings_Internet_Disconnect", "Отключиться")), DLG_PITCH, &mut items); }
    let saved = i.saved.as_ref();
    let auto = saved.map(|p| i32::from(p.autoconnect)).unwrap_or(-1);
    // the toggle is focusable even when disabled (stock `disabled: !isSaved`)
    next(&mut b, DRow { kind: dk::TOGGLE, h: 68.0, label: lt("#Settings_Internet_Autoconnect_Toggle", "Автоподключение к этой сети"), on: auto, ..Default::default() }, 78.0, &mut items);
    b.push(DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_Status", "Состояние"), value: state_label(i), item: -1, ..Default::default() }, 80.0);
    let d = i.details.clone().unwrap_or_default();
    b.push(DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_MAC_Address", "MAC-адрес"), value: d.mac.clone(), item: -1, ..Default::default() }, 80.0);
    if i.connected && !d.ipv4.is_empty() {
        b.y += 16.0;
        b.push(DRow { kind: dk::SECTION, h: 54.0, label: lt("#Settings_Internet_IPv4_Address", "IPv4-адрес").to_uppercase(), item: -1, ..Default::default() }, 70.0);
        next(&mut b, DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_IP_Address_Assignment", "Назначение IP-адреса"), value: if d.dhcp { lt("#Settings_Internet_IP_Address_Assignment_Automatic", "Автоматически") } else { lt("#Settings_Internet_IP_Address_Assignment_Manual", "Вручную") }, ..Default::default() }, 80.0, &mut items);
        for (ip, mask) in &d.ipv4 {
            b.push(DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_IP_Address", "IP-адрес"), value: ip.clone(), item: -1, ..Default::default() }, 80.0);
            b.push(DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_Subnet_Mask", "Маска подсети"), value: mask.clone(), item: -1, ..Default::default() }, 80.0);
        }
        if !d.gateway.is_empty() { b.push(DRow { kind: dk::INFO, h: ROW_H, label: lt("#Settings_Internet_Gateway", "Шлюз"), value: d.gateway.clone(), item: -1, ..Default::default() }, 80.0); }
        for (n, dns) in d.dns.iter().enumerate() {
            let label = if n == 0 { lt("#Settings_Internet_Primary_DNS", "Первичный DNS") } else { lt("#Settings_Internet_Secondary_DNS", "Вторичный DNS") };
            b.push(DRow { kind: dk::INFO, h: ROW_H, label, value: dns.clone(), item: -1, ..Default::default() }, 80.0);
        }
    }
    b.y += 3.0;
    next(&mut b, DRow { kind: dk::SINGLE, h: DLG_BTN_H, value: lt("#Button_OK", "OK"), on: 1, ..Default::default() }, DLG_BTN_H, &mut items);
    let focus = i.focus.min(items.saturating_sub(1));
    finish(b, items, focus, focus_row, false, 55.0, scroll)
}

fn progress_view(p: &Prog) -> DialogView {
    let mut b = B::new();
    let (l1, l2, f1, f2, text, text_err) = match p.stage {
        Stage::Connecting => (1, 0, false, false, lf("#Settings_Internet_Connecting_To_Network", "Подключение к %1$s…", &p.ssid), false),
        Stage::Checking => (2, 1, false, false, lt("#Settings_Internet_Connecting_To_Internet", "Подключение к интернету…"), false),
        Stage::Success => (2, 2, false, false, lt("#Settings_Internet_Connection_Successful", "Подключение установлено!"), false),
        Stage::Failed => (3, 0, true, false, lt("#Settings_Internet_Failed_To_Connect_To_Network", "Не удалось подключиться к сети."), true),
        Stage::NoInternet => (2, 3, false, true, lt("#Settings_Internet_Failed_To_Connect_To_Internet", "Не удалось подключиться к интернету."), true),
        Stage::Portal => (2, 3, false, true, lt("#Settings_Internet_Captive_Portal_Detected", "Для выхода в интернет требуется дополнительная информация."), false),
    };
    b.push(DRow { kind: dk::ICONS, h: 150.0, a: l1, b: l2, on: i32::from(f1) | (i32::from(f2) << 1), item: -1, ..Default::default() }, 150.0);
    b.push(DRow { kind: dk::STATUS, h: 60.0, label: text, on: i32::from(text_err), item: -1, ..Default::default() }, 74.0);
    b.y -= 0.0;
    let two = matches!(p.stage, Stage::NoInternet | Stage::Portal);
    let items = if two { 2 } else { 1 };
    if two {
        b.push(DRow { kind: dk::PAIR, h: DLG_BTN_H, label: lt("#Settings_Internet_Try_Another_Network", "Попробовать другую сеть"), value: lt("#Settings_Internet_Use_Network_Anyway", "Не подключаться к интернету"), on: 1, focus: if p.focus == 0 { 1 } else { 2 }, item: 0, ..Default::default() }, DLG_BTN_H);
    } else {
        let label = match p.stage { Stage::Success => lt("#Login_Continue", "Продолжить"), _ => lt("#Button_Cancel", "Отмена") };
        b.push(DRow { kind: dk::STRIP, h: 96.0, value: label, focus: 1, item: 0, ..Default::default() }, 96.0);
    }
    finish(b, items, p.focus.min(items - 1), None, false, 36.0 + if two { 0.0 } else { 0.0 }, 0.0)
}

fn confirm_text(c: Confirm, game_running: bool, has_network: bool) -> (String, String, bool) {
    // (title, body, has_cancel)
    match c {
        Confirm::GoOffline if game_running => (lt("#GoOffline", "Перейти в автономный режим"), lt("#UserManagement_OfflineMode_MustCloseGames", "Все запущенные игры должны быть закрыты до перехода в автономный режим."), false),
        Confirm::GoOffline => (lt("#GoOffline", "Перейти в автономный режим"), lt("#UserManagement_OfflineMode_Desc", "Автономный режим может пригодиться, когда вы хотите играть, но подключение к сети Steam нестабильно. Вы также будете отключены от сети друзей и чатов."), true),
        Confirm::GoOnline if !has_network => (lt("#GoOnline", "Войти в сеть"), lt("#UserManagement_GoOnline_NoNetwork", "Для входа в сеть необходимо подключение к интернету."), false),
        Confirm::GoOnline => (lt("#GoOnline", "Войти в сеть"), lt("#UserManagement_GoOnline_Desc", "Вы снова подключитесь к сети Steam, друзьям и чатам."), true),
        Confirm::DeleteBrowser => (lt("#Settings_Internet_WebBrowserDataDelete", "Удалить данные браузера"), lt("#Settings_Internet_WebBrowserDataDelete_Desc", "Удалить все файлы из кэша, файлы cookie и историю браузера?"), true),
    }
}

pub(crate) fn message_view(title: &str, body: &str, ok: &str, cancel: Option<&str>, focus: usize) -> DialogView {
    let mut b = B::new();
    title_block(&mut b, title, None);
    b.y = 111.0 - 0.0;
    let lines = body_lines(body);
    let h = lines as f32 * 30.0;
    b.push(DRow { kind: dk::BODY, h, label: body.into(), item: -1, ..Default::default() }, h + 42.0);
    let items;
    match cancel {
        Some(c) => { items = 2; b.push(DRow { kind: dk::PAIR, h: DLG_BTN_H, value: ok.into(), label: c.into(), on: 1, focus: if focus == 0 { 1 } else { 2 }, a: 1, item: 0, ..Default::default() }, DLG_BTN_H); }
        None => { items = 1; b.push(DRow { kind: dk::SINGLE, h: DLG_BTN_H, value: ok.into(), on: 1, focus: 1, item: 0, ..Default::default() }, DLG_BTN_H); }
    }
    finish(b, items, focus.min(items - 1), None, false, 36.0, 0.0)
}

// ---------------------------------------------------------------- state

#[derive(Default)]
pub struct State {
    pub open: bool,
    pub radio: Option<bool>,
    pub aps: Vec<AccessPoint>,
    pub profiles: Vec<Profile>,
    pub connectivity: Option<Connectivity>,
    /// first-seen signal per SSID: rows keep their order while the scan jitters (stock `initialRawSignalStrength`)
    order: HashMap<String, u8>,
    pub focus: Option<Key>,
    pub vy: f32,
    pub dialog: Option<Dialog>,
    pub dscroll: f32,
    generation: u64,
    radio_pending: bool,
    cancel: Option<Arc<AtomicBool>>,
    busy: Option<String>,
    offline_override: Option<(bool, Instant)>,
    pub game_running: bool,
    pub live_offline: bool,
    pub osk_open: bool,
    mac: String,
    /// last applied HTTP proxy
    pub proxy: ProxyCfg,
    /// optimistic Forget/Disconnect not yet confirmed by the backend: re-applied over every scan snapshot so a stale poll cannot undo them
    pending: Vec<Op>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op { Forget(String), Disconnect }

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
    static TIMER: slint::Timer = slint::Timer::default();
}

pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> R { ST.with(|s| f(&mut s.borrow_mut())) }

static BACKEND: Mutex<Option<Arc<dyn WifiBackend>>> = Mutex::new(None);
static RADIO_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(super) fn radio_epoch() -> u64 {
    RADIO_EPOCH.load(Ordering::SeqCst)
}

pub(super) fn radio_pending() -> bool {
    with(|s| s.radio_pending)
}

#[cfg(test)]
thread_local! {
    static TEST_BACKEND: RefCell<Option<Arc<dyn WifiBackend>>> = RefCell::new(None);
}

/// The Wi-Fi backend: the real NetworkManager only outside tests, dry runs and mock mode.
pub fn backend() -> Arc<dyn WifiBackend> {
    #[cfg(test)]
    if let Some(backend) = TEST_BACKEND.with(|slot| slot.borrow().clone()) {
        return backend;
    }
    let mut g = BACKEND.lock().unwrap();
    g.get_or_insert_with(|| {
        let mock = cfg!(test) || crate::actions::dryrun() || std::env::var("DECKSHELL_MOCK").map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
        if mock { Arc::new(MockWifi::demo()) as Arc<dyn WifiBackend> } else { Arc::new(Nmcli::default()) }
    }).clone()
}
/// Tests and fixtures install their own network.
#[cfg(test)]
pub fn set_backend(b: Arc<dyn WifiBackend>) {
    TEST_BACKEND.with(|slot| slot.replace(Some(b)));
}

pub(crate) fn header_offline(live: bool) -> bool {
    with(|state| {
        state.live_offline = live;
        state.offline()
    })
}

pub fn sig() -> u64 {
    with(|s| {
        let d = match &s.dialog {
            None => 0u64,
            Some(Dialog::Form(f)) => 1 + (f.focus as u64) * 16 + (f.adv as u64) * 4096,
            Some(Dialog::Pick(_, i)) => 2 + (*i as u64) * 16,
            Some(Dialog::Info(i)) => 3 + (i.focus as u64) * 16,
            Some(Dialog::Progress(p)) => 4 + (p.stage as u64) * 16 + (p.focus as u64) * 256,
            Some(Dialog::Confirm(_, f)) => 5 + (*f as u64) * 16,
            Some(Dialog::Alert(_)) => 6,
            Some(Dialog::Proxy(p)) => proxy::sig(p),
        };
        d ^ (s.focus.as_ref().map(|k| k_idx(k) as u64 + 1).unwrap_or(0) << 20)
    })
}

fn k_idx(k: &Key) -> usize { match k { Key::Toggle => 0, Key::Other => 1, Key::Offline => 2, Key::Proxy => 3, Key::Delete => 4, Key::Ap(s) => 5 + s.len() + s.bytes().map(|b| b as usize).sum::<usize>(), Key::Tag(t) => 5 + *t as usize } }

impl State {
    /// Applies the unconfirmed optimistic operations to the model (aps/profiles).
    fn overlay(&mut self) {
        for op in self.pending.clone() {
            match op {
                Op::Forget(ssid) => {
                    while let Some(u) = wifi::profile_for(&self.profiles, &ssid).map(|p| p.uuid.clone()) { self.profiles.retain(|p| p.uuid != u); }
                    for ap in self.aps.iter_mut().filter(|a| a.ssid == ssid) { ap.in_use = false; }
                }
                Op::Disconnect => {
                    let active: Vec<String> = self.aps.iter().filter(|a| a.in_use).map(|a| a.ssid.clone()).collect();
                    for ap in self.aps.iter_mut() { ap.in_use = false; }
                    for p in self.profiles.iter_mut() { if active.iter().any(|s| wifi::profile_for(std::slice::from_ref(p), s).is_some()) { p.active = false; } }
                }
            }
        }
        self.connectivity = if self.aps.iter().any(|a| a.in_use) { self.connectivity } else { None };
    }
    /// Instant UI update + drop in-flight stale snapshots (generation bump).
    fn begin_op(&mut self, op: Op) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.push(op);
        self.overlay();
        self.fix_focus();
    }
    /// Connect succeeded: the new network is the only connected one.
    fn mark_connected(&mut self, ssid: &str) {
        for ap in self.aps.iter_mut() { ap.in_use = ap.ssid == ssid; }
    }
    fn offline(&self) -> bool {
        match self.offline_override { Some((v, t)) if t.elapsed() < Duration::from_secs(4) || cfg!(test) || backend().is_mock() => v, _ => self.live_offline }
    }
    fn net(&self, ap: &AccessPoint) -> Net { Net { ssid: ap.ssid.clone(), bars: wifi::bars(ap.signal), lock: ap.security.locked(), connected: ap.in_use, busy: self.busy.as_deref() == Some(ap.ssid.as_str()) } }

    pub fn page_data(&self) -> PageData {
        let on = self.radio == Some(true);
        let mut connected = Vec::new();
        let mut found: Vec<&AccessPoint> = Vec::new();
        if on {
            for ap in &self.aps { if ap.in_use { connected.push(self.net(ap)); } else { found.push(ap); } }
        }
        found.sort_by(|a, b| {
            let (sa, sb) = (self.order.get(&a.ssid).copied().unwrap_or(a.signal), self.order.get(&b.ssid).copied().unwrap_or(b.signal));
            sb.cmp(&sa).then(a.ssid.cmp(&b.ssid))
        });
        PageData { radio: self.radio, has_network: !connected.is_empty() || self.aps.iter().any(|a| a.in_use), connected, found: found.into_iter().map(|a| self.net(a)).collect(), offline: self.offline(), connectivity: self.connectivity }
    }
    pub fn rows(&self) -> Vec<Row> { page_rows(&self.page_data()) }
    fn focusable(&self) -> Vec<Key> { self.rows().into_iter().filter_map(|r| r.key).collect() }
    fn ap(&self, ssid: &str) -> Option<&AccessPoint> { self.aps.iter().find(|a| a.ssid == ssid) }
    fn profile(&self, ssid: &str) -> Option<&Profile> { wifi::profile_for(&self.profiles, ssid) }
    fn remember_order(&mut self) {
        for ap in &self.aps { self.order.entry(ap.ssid.clone()).or_insert(ap.signal); }
        let live: Vec<String> = self.aps.iter().map(|a| a.ssid.clone()).collect();
        self.order.retain(|k, _| live.contains(k));
    }
    /// Keeps the focused row across rebuilds; falls back to the toggle.
    fn fix_focus(&mut self) {
        let keys = self.focusable();
        if self.focus.as_ref().is_none_or(|k| !keys.contains(k)) { self.focus = keys.first().cloned(); }
    }
    fn dialog_view(&self) -> Option<DialogView> {
        let osk = self.osk_open;
        Some(match self.dialog.as_ref()? {
            Dialog::Form(f) => form_view(f, osk, self.dscroll),
            Dialog::Pick(f, sel) => pick_view(wifi::CUSTOM_SECURITY.iter().position(|s| *s == f.security).unwrap_or(0), *sel),
            Dialog::Info(i) => info_view(i, self.dscroll),
            Dialog::Proxy(p) => proxy::view(p, osk, self.dscroll),
            Dialog::Progress(p) => progress_view(p),
            Dialog::Confirm(c, f) => { let (t, b, cancel) = confirm_text(*c, self.game_running, self.page_data().has_network); message_view(&t, &b, &lt(if cancel { "#Button_Confirm" } else { "#Button_OK" }, if cancel { "Подтвердить" } else { "OK" }), cancel.then(|| lt("#Button_Cancel", "Отмена")).as_deref(), *f) }
            Dialog::Alert(k) => match k {
                AlertKind::NotFound(s) => message_view(&lf("#Settings_Internet_Network_Not_Found", "Сеть не найдена: %1$s", s), "", &lt("#Button_Close", "Закрыть"), None, 0),
                AlertKind::MustCloseGames => { let (t, b, _) = confirm_text(Confirm::GoOffline, true, true); message_view(&t, &b, &lt("#Button_OK", "OK"), None, 0) }
                AlertKind::NoNetwork => { let (t, b, _) = confirm_text(Confirm::GoOnline, false, false); message_view(&t, &b, &lt("#Button_OK", "OK"), None, 0) }
                AlertKind::Unsupported => message_view(&lt("#Settings_Internet_AP_Security_Unsupported", "Этот тип защиты сети не поддерживается"), "", &lt("#Button_Close", "Закрыть"), None, 0),
            },
        })
    }
}

// ---------------------------------------------------------------- scanning (worker thread → UI)

fn refresh(rescan: bool) {
    let generation = with(|s| s.generation);
    let b = backend();
    std::thread::spawn(move || {
        let radio = b.radio();
        let aps = if radio == Some(true) { b.scan(rescan) } else { Vec::new() };
        let profiles = b.profiles();
        let conn = if radio == Some(true) && aps.iter().any(|a| a.in_use) { Some(b.connectivity()) } else { None };
        let mac = b.details("").mac;
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| apply_scan(a, generation, radio, aps, profiles, conn, mac)); });
    });
}

pub fn apply_scan(a: &mut App, generation: u64, radio: Option<bool>, aps: Vec<AccessPoint>, profiles: Vec<Profile>, conn: Option<Connectivity>, mac: String) {
    let live = with(|s| {
        if !s.open || s.generation != generation || s.radio_pending { return false; }
        s.radio = radio;
        s.aps = aps;
        s.profiles = profiles;
        s.connectivity = conn;
        s.mac = mac;
        s.overlay();
        s.remember_order();
        s.fix_focus();
        true
    });
    if live { push(a); }
}

fn start_polling() {
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(6), || {
        if with(|s| s.open && (s.dialog.is_none() || matches!(s.dialog, Some(Dialog::Info(_))))) { refresh(true); }
    }));
}

// ---------------------------------------------------------------- entering / leaving

pub fn open(a: &mut App) {
    with(|s| {
        s.open = true;
        s.generation = s.generation.wrapping_add(1);
        s.dialog = None;
        s.vy = 0.0;
        s.focus = None;
        s.aps.clear();
        s.radio = backend().radio();
        s.profiles = backend().profiles();
    });
    refresh(false);
    refresh(true);
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
        close_dialog(a);
        TIMER.with(|t| t.stop());
        with(|s| { s.generation = s.generation.wrapping_add(1); s.dialog = None; });
    }
}

/// Rail → content (Right / A on the rail): focus the first row.
pub fn enter(a: &mut App) {
    with(|s| s.fix_focus());
    a.se.zone = 1;
    scroll_to_focus();
    push(a);
}

// ---------------------------------------------------------------- input

/// Actions the page handles itself while its content has focus (Y shows the password, Menu opens network info).
pub fn owns(a: Action) -> bool { !matches!(a, Action::B | Action::Search | Action::View) }

fn dialog_open() -> bool { with(|s| s.dialog.is_some()) }

pub fn hints() -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    let a_sel = ("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A);
    let (dlg, focus, osk) = with(|s| (s.dialog.clone(), s.focus.clone(), s.osk_open));
    if osk { return vec![("a", "Ввод", 1, "Enter", Action::A), ("b", "Готово", 1, "Esc", Action::B)]; }
    match dlg {
        Some(Dialog::Progress(p)) => match p.stage {
            Stage::Success => vec![("a", "Готово", 1, "Enter", Action::A)],
            Stage::NoInternet | Stage::Portal => vec![a_sel, ("b", "Отмена", 1, "Esc", Action::B)],
            _ => vec![("b", "Отмена", 1, "Esc", Action::B)],
        },
        Some(Dialog::Proxy(_)) => vec![a_sel, b],
        Some(Dialog::Form(f)) if f.focus == F::Pass => vec![("y", "Показать пароль", 1, "F4", Action::Y), a_sel, b],
        Some(_) => vec![a_sel, b],
        None => match focus {
            Some(Key::Ap(s)) if with(|st| st.ap(&s).is_some_and(|a| !a.in_use)) => vec![("menu", "Информация", 2, "F9", Action::Menu), ("a", "Подключиться", 1, "Enter", Action::A), b],
            Some(Key::Ap(_)) => vec![("a", "Информация", 1, "Enter", Action::A), b],
            _ => vec![a_sel, b],
        },
    }
}

fn dir_step(len: usize, cur: usize, a: Action) -> usize {
    match a { Action::Up => cur.saturating_sub(1), Action::Down => (cur + 1).min(len.saturating_sub(1)), _ => cur }
}

pub fn handle(app: &mut App, a: Action) {
    if dialog_open() { return handle_dialog(app, a); }
    let keys = with(|s| s.focusable());
    let cur = with(|s| s.focus.as_ref().and_then(|k| keys.iter().position(|x| x == k))).unwrap_or(0);
    match a {
        Action::Up | Action::Down => {
            let n = dir_step(keys.len(), cur, a);
            if n != cur { with(|s| s.focus = keys.get(n).cloned()); scroll_to_focus(); }
            app.se.cf = Some(n);
        }
        Action::Left => { app.se.zone = 0; app.se.cursor = app.se.focus; app.se.cf = None; }
        Action::A => return activate(app, keys.get(cur).cloned()),
        Action::Menu => {
            if let Some(Key::Ap(s)) = keys.get(cur) { open_info(app, &s.clone()); return; }
        }
        _ => return,
    }
    push(app);
}

/// A / click on a page row.
pub fn activate(app: &mut App, key: Option<Key>) {
    let Some(key) = key else { return };
    with(|s| s.focus = Some(key.clone()));
    match key {
        Key::Toggle => toggle_radio(app),
        Key::Ap(ssid) => {
            let connected = with(|s| s.ap(&ssid).is_some_and(|a| a.in_use));
            if connected { open_info(app, &ssid); } else { start_connect(app, &ssid); }
        }
        Key::Other => {
            let mac = with(|s| s.mac.clone());
            // many fields: select first, the keyboard opens on A
            set_dialog(app, Some(Dialog::Form(Form::new("", Security::Wpa2, true, &mac))));
        }
        Key::Offline => {
            let (offline, has_net) = with(|s| (s.offline(), s.page_data().has_network));
            let running = with(|s| s.game_running);
            if offline { set_dialog(app, Some(if has_net { Dialog::Confirm(Confirm::GoOnline, 0) } else { Dialog::Alert(AlertKind::NoNetwork) })); }
            else if running { set_dialog(app, Some(Dialog::Alert(AlertKind::MustCloseGames))); }
            else { set_dialog(app, Some(Dialog::Confirm(Confirm::GoOffline, 0))); }
        }
        Key::Proxy => proxy::open(app),
        Key::Delete => set_dialog(app, Some(Dialog::Confirm(Confirm::DeleteBrowser, 0))),
        Key::Tag(_) => {}
    }
}

/// Shared by QAM and Settings. Keep the UI thread free while Steam applies
/// the change, then publish the NetworkManager readback on the UI thread.
pub(super) fn request_radio(app: &mut App, on: bool) {
    if radio_pending() {
        return;
    }
    with(|s| {
        s.radio_pending = true;
        s.generation = s.generation.wrapping_add(1);
    });
    RADIO_EPOCH.fetch_add(1, Ordering::SeqCst);
    let backend = backend();
    let mock = backend.is_mock();
    let work = move || {
        let result = write_radio(&*backend, mock, on);
        let observed = if result.is_ok() { Some(on) } else { backend.radio() };
        (observed, result)
    };
    if mock {
        let (observed, result) = work();
        radio_result(app, observed, result);
    } else {
        std::thread::spawn(move || {
            let (observed, result) = work();
            let _ = slint::invoke_from_event_loop(move || {
                crate::with_app(|app| radio_result(app, observed, result));
            });
        });
    }
}

fn write_radio(backend: &dyn WifiBackend, mock: bool, on: bool) -> Result<(), String> {
    if !mock && (cfg!(test) || crate::actions::dryrun()) {
        return Err("Wi-Fi write disabled for this session".into());
    }
    if backend.radio() == Some(on) {
        return Ok(());
    }
    let sent_to_steam = !mock && crate::actions::bridge().is_some_and(|bridge| bridge.set_wifi(on).unwrap_or(false));
    if !sent_to_steam {
        backend.set_radio(on)?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if backend.radio() == Some(on) {
            return Ok(());
        }
        if mock || Instant::now() >= deadline {
            return Err("Wi-Fi radio did not confirm the requested state".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn toggle_radio(app: &mut App) {
    if let Some(on) = with(|s| s.radio) {
        request_radio(app, !on);
    }
}

fn radio_result(app: &mut App, observed: Option<bool>, result: Result<(), String>) {
    RADIO_EPOCH.fetch_add(1, Ordering::SeqCst);
    let open = with(|s| {
        s.radio_pending = false;
        s.generation = s.generation.wrapping_add(1);
        s.radio = observed;
        if observed != Some(true) {
            s.aps.clear();
            s.connectivity = None;
        }
        s.fix_focus();
        s.open
    });
    match result {
        Ok(()) => {
            if let Some(on) = observed {
                crate::sound::play(if on { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
            }
        }
        Err(err) => {
            eprintln!("[deckshell] Wi-Fi: {err}");
            app.toast("Не удалось изменить состояние Wi-Fi");
        }
    }
    if open {
        push(app);
        refresh(true);
    }
    app.qam_update_wifi(observed);
}

fn open_info(app: &mut App, ssid: &str) {
    let info = with(|s| {
        let ap = s.ap(ssid).cloned();
        let saved = s.profile(ssid).cloned();
        let connected = ap.as_ref().is_some_and(|a| a.in_use);
        Info { ssid: ssid.into(), focus: 0, details: None, saved, connected, connectable: ap.is_some() && !connected, present: ap.is_some() }
    });
    set_dialog(app, Some(Dialog::Info(info)));
    let generation = with(|s| s.generation);
    let ssid = ssid.to_string();
    let b = backend();
    std::thread::spawn(move || {
        let d = b.details(&ssid);
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| {
            with(|s| if s.generation == generation { if let Some(Dialog::Info(i)) = &mut s.dialog { if i.ssid == ssid { i.details = Some(d); } } });
            push(a);
        }); });
    });
}

fn set_dialog(app: &mut App, d: Option<Dialog>) {
    with(|s| { s.dialog = d; s.dscroll = 0.0; });
    sync_osk_flag();
    push(app);
}

fn sync_osk_flag() { /* the keyboard state is read from the app in `push` */ }

pub fn close_dialog(app: &mut App) {
    detach_all(app);
    with(|s| { s.dialog = None; s.dscroll = 0.0; });
}

// --- connect

fn start_connect(app: &mut App, ssid: &str) {
    let (ap, saved, mac) = with(|s| (s.ap(ssid).cloned(), s.profile(ssid).cloned(), s.mac.clone()));
    let Some(ap) = ap else { return set_dialog(app, Some(Dialog::Alert(AlertKind::NotFound(ssid.into())))) };
    if !ap.security.supported() { return set_dialog(app, Some(Dialog::Alert(AlertKind::Unsupported))); }
    // stock: a secured network without a stored key asks for it first
    if ap.security.needs_password() && saved.is_none() {
        set_dialog(app, Some(Dialog::Form(Form::new(ssid, ap.security, false, &mac))));
        return attach_field(app);
    }
    let req = ConnectRequest { ssid: ssid.into(), security: ap.security, username: String::new(), password: String::new(), hidden: false, ip: None };
    begin_connect(app, req, None);
}

/// Starts the attempt on a worker thread and shows the progress dialog.
fn begin_connect(app: &mut App, req: ConnectRequest, back: Option<Form>) {
    detach_all(app);
    let cancel = Arc::new(AtomicBool::new(false));
    let generation = with(|s| {
        s.generation = s.generation.wrapping_add(1);
        s.cancel = Some(cancel.clone());
        s.busy = Some(req.ssid.clone());
        s.generation
    });
    let secured = req.security.needs_password();
    set_dialog(app, Some(Dialog::Progress(Prog { ssid: req.ssid.clone(), stage: Stage::Connecting, back, secured, focus: 0 })));
    let b = backend();
    std::thread::spawn(move || {
        let result = b.connect(&req, &cancel);
        let mut conn = Connectivity::Unknown;
        if result.is_ok() {
            // Steam tests internet access for a few seconds before it reports success
            for _ in 0..6 {
                conn = b.connectivity();
                if conn != Connectivity::Unknown && conn != Connectivity::None || cancel.load(Ordering::SeqCst) { break; }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| apply_connect(a, generation, result, conn)); });
    });
}

pub fn apply_connect(a: &mut App, generation: u64, result: Result<(), ConnectError>, conn: Connectivity) {
    let ok = with(|s| {
        if !s.open || s.generation != generation { return false; }
        s.busy = None;
        s.cancel = None;
        true
    });
    if !ok { return; }
    let prog = with(|s| match &s.dialog { Some(Dialog::Progress(p)) => Some(p.clone()), _ => None });
    let Some(mut p) = prog else { refresh(true); return };
    match result {
        Ok(()) => {
            with(|s| { s.mark_connected(&p.ssid); s.fix_focus(); });
            p.stage = match conn { Connectivity::Full | Connectivity::Unknown => Stage::Success, Connectivity::Portal => Stage::Portal, Connectivity::Limited | Connectivity::None => Stage::NoInternet };
            with(|s| s.dialog = Some(Dialog::Progress(p)));
        }
        Err(ConnectError::Cancelled) => { close_dialog(a); }
        Err(ConnectError::NotFound) => { with(|s| s.dialog = Some(Dialog::Alert(AlertKind::NotFound(p.ssid.clone())))); }
        Err(ConnectError::Unsupported) => { with(|s| s.dialog = Some(Dialog::Alert(AlertKind::Unsupported))); }
        Err(_) if p.secured => {
            // stock: back to the password prompt, red «Не удалось подключиться»
            let mut f = p.back.clone().unwrap_or_else(|| Form::new(&p.ssid, Security::Wpa2, false, &with(|s| s.mac.clone())));
            f.failed = true;
            f.focus = if f.security.needs_password() { F::Pass } else { F::Connect };
            with(|s| s.dialog = Some(Dialog::Form(f)));
            push(a);
            refresh(true);
            return attach_field(a);
        }
        Err(_) => { p.stage = Stage::Failed; with(|s| s.dialog = Some(Dialog::Progress(p))); }
    }
    refresh(true);
    push(a);
}

// --- dialog input

fn attach_field(app: &mut App) {
    let f = with(|s| match &s.dialog { Some(Dialog::Form(f)) => Some(f.focus), _ => None });
    match f {
        Some(F::Ssid) | Some(F::Pass) => osk::attach(app, if f == Some(F::Pass) && !with(|s| matches!(&s.dialog, Some(Dialog::Form(f)) if f.show_pass)) { PASS_SINK } else { TEXT_SINK }),
        Some(F::Ip) | Some(F::Mask) | Some(F::Gw) | Some(F::Dns1) | Some(F::Dns2) => osk::attach(app, NUM_SINK),
        _ => {}
    }
}

/// Focus moved between fields of a multi-field form (D-pad, Y): an open keyboard follows the focus (its target switches),
/// a closed one stays closed — it opens only on A / touch on a field (`attach_field`).
fn follow_field(app: &mut App) {
    if app.osk_is_open() { attach_field(app); }
}

fn detach_all(app: &mut App) {
    for s in [TEXT_SINK, PASS_SINK, NUM_SINK, proxy::HOST_SINK, proxy::PORT_SINK] { osk::detach(app, s); }
}

const TEXT_SINK: ExtSink = ExtSink { id: 11, mode: InputMode::Normal, typed: typed, enter: enter_key, cancel: kb_closed };
const PASS_SINK: ExtSink = ExtSink { id: 12, mode: InputMode::Password, typed: typed, enter: enter_key, cancel: kb_closed };
const NUM_SINK: ExtSink = ExtSink { id: 13, mode: InputMode::Numeric, typed: typed_num, enter: enter_key, cancel: kb_closed };

fn typed(a: &mut App, c: Option<char>) { edit(a, c, false); }
fn typed_num(a: &mut App, c: Option<char>) { edit(a, c.map(|c| if c == ',' { '.' } else { c }).filter(|c| c.is_ascii_digit() || *c == '.').or(None).filter(|_| c.is_some()), true); }

fn edit(a: &mut App, c: Option<char>, numeric: bool) {
    if numeric && c.is_none() && false { return; }
    with(|s| if let Some(Dialog::Form(f)) = &mut s.dialog {
        let foc = f.focus;
        if let Some(t) = f.text_mut(foc) {
            match c { Some(c) => if t.chars().count() < 64 { t.push(c) }, None => { t.pop(); } }
        }
    });
    push(a);
}

fn kb_closed(a: &mut App) { push(a); }

/// Enter on the keyboard: next field, or connect from the last text field (stock `onOK`).
fn enter_key(a: &mut App) {
    a.osk_set_open(false);
    let go = with(|s| match &mut s.dialog {
        Some(Dialog::Form(f)) => {
            let items = f.items();
            let i = items.iter().position(|x| *x == f.focus).unwrap_or(0);
            if f.focus == F::Ssid { f.focus = items[(i + 1).min(items.len() - 1)]; false } else { true }
        }
        _ => false,
    });
    if go { form_submit(a); } else { attach_field(a); push(a); }
}

fn form_submit(a: &mut App) {
    let f = with(|s| match &s.dialog { Some(Dialog::Form(f)) => Some(f.clone()), _ => None });
    let Some(f) = f else { return };
    if !f.can_connect() { return; }
    match f.request() {
        Ok(req) => begin_connect(a, req, Some(f)),
        Err((field, msg)) => {
            with(|s| if let Some(Dialog::Form(f)) = &mut s.dialog { f.focus = field; });
            a.toast(&msg);
            push(a);
        }
    }
}

fn handle_dialog(app: &mut App, a: Action) {
    let dlg = with(|s| s.dialog.clone()).unwrap();
    match dlg {
        Dialog::Form(f) => form_input(app, f, a),
        Dialog::Pick(f, sel) => match a {
            Action::Up | Action::Down => { with(|s| s.dialog = Some(Dialog::Pick(f, dir_step(wifi::CUSTOM_SECURITY.len(), sel, a)))); push(app); }
            Action::A => {
                let mut f = f;
                f.security = wifi::CUSTOM_SECURITY[sel];
                f.focus = F::Security;
                set_dialog(app, Some(Dialog::Form(f)));
            }
            Action::B => set_dialog(app, Some(Dialog::Form(f))),
            _ => {}
        },
        Dialog::Info(i) => info_input(app, i, a),
        Dialog::Proxy(p) => proxy::input(app, p, a),
        Dialog::Progress(p) => progress_input(app, p, a),
        Dialog::Confirm(c, foc) => match a {
            Action::Left | Action::Right | Action::Up | Action::Down => {
                let two = !matches!(confirm_text(c, with(|s| s.game_running), with(|s| s.page_data().has_network)), (_, _, false));
                if two { with(|s| s.dialog = Some(Dialog::Confirm(c, if matches!(a, Action::Left | Action::Up) { 0 } else { 1 }))); push(app); }
            }
            Action::A if foc == 0 => confirm_ok(app, c),
            Action::A | Action::B => close_and_refocus(app),
            _ => {}
        },
        Dialog::Alert(_) => if matches!(a, Action::A | Action::B) { close_and_refocus(app) },
    }
}

fn close_and_refocus(app: &mut App) { close_dialog(app); push(app); }

fn confirm_ok(app: &mut App, c: Confirm) {
    let running = with(|s| s.game_running);
    match c {
        Confirm::GoOffline if running => {}
        Confirm::GoOffline => { crate::setters::write("go-offline", "SteamClient.User.GoOffline()".into()); with(|s| s.offline_override = Some((true, Instant::now()))); }
        Confirm::GoOnline => { crate::setters::write("go-online", "SteamClient.User.GoOnline()".into()); with(|s| s.offline_override = Some((false, Instant::now()))); }
        Confirm::DeleteBrowser => crate::setters::write("clear-browser-data", "SteamClient.Browser.ClearAllBrowsingData()".into()),
    }
    close_and_refocus(app);
}

fn form_input(app: &mut App, f: Form, a: Action) {
    let items = f.items();
    let i = items.iter().position(|x| *x == f.focus).unwrap_or(0);
    let mut f = f;
    match a {
        Action::Up | Action::Down => {
            let mut n = dir_step(items.len(), i, a);
            // the button pair is one row: Up from Cancel is the row above Connect, not Connect
            if a == Action::Up && f.focus == F::Cancel { n = i.saturating_sub(2); }
            if a == Action::Down && f.focus == F::Connect { n = i; }
            f.focus = items[n];
            with(|s| s.dialog = Some(Dialog::Form(f)));
            follow_field(app);
            push(app);
        }
        Action::Left | Action::Right => {
            if matches!(f.focus, F::Connect | F::Cancel) {
                f.focus = if a == Action::Left { F::Connect } else { F::Cancel };
                with(|s| s.dialog = Some(Dialog::Form(f)));
                push(app);
            }
        }
        Action::Y if f.focus == F::Pass => {
            f.show_pass = !f.show_pass;
            with(|s| s.dialog = Some(Dialog::Form(f)));
            // the keyboard mode follows the visibility (only if it is open)
            let open = app.osk_is_open();
            detach_all(app);
            if open { attach_field(app); }
            push(app);
        }
        Action::A => match f.focus {
            F::Ssid | F::Pass | F::Ip | F::Mask | F::Gw | F::Dns1 | F::Dns2 => attach_field(app),
            F::Security => { let cur = wifi::CUSTOM_SECURITY.iter().position(|s| *s == f.security).unwrap_or(0); set_dialog(app, Some(Dialog::Pick(f, cur))); }
            F::AdvShow => { f.adv = true; f.focus = F::Ip; with(|s| s.dialog = Some(Dialog::Form(f))); push(app); }
            F::Mac => {}
            F::Connect => form_submit(app),
            F::Cancel => close_and_refocus(app),
        },
        Action::B => close_and_refocus(app),
        _ => {}
    }
}

fn info_input(app: &mut App, i: Info, a: Action) {
    let view = with(|s| s.dialog_view()).unwrap();
    let n = view.items;
    match a {
        Action::Up | Action::Down => { let mut i = i; i.focus = dir_step(n, i.focus, a); with(|s| s.dialog = Some(Dialog::Info(i))); push(app); }
        Action::B | Action::Menu => close_and_refocus(app),
        Action::A => info_activate(app, i),
        _ => {}
    }
}

fn info_activate(app: &mut App, i: Info) {
    // which row is the focused item: rebuild the item list the same way `info_view` does
    let view = info_view(&i, 0.0);
    let row = view.rows.iter().find(|r| r.item == i.focus as i32).cloned();
    let Some(row) = row else { return };
    let ssid = i.ssid.clone();
    let generation = with(|s| s.generation);
    let run = move |op: Option<Op>, f: Box<dyn FnOnce(&dyn WifiBackend) + Send>| {
        let _ = generation;
        if let Some(op) = &op { with(|s| s.begin_op(op.clone())); }
        let b = backend();
        std::thread::spawn(move || {
            f(&*b);
            let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| {
                if let Some(op) = &op { with(|s| if let Some(i) = s.pending.iter().position(|p| p == op) { s.pending.remove(i); }); }
                refresh(true);
                push(a);
            }); });
        });
    };
    match (row.kind, row.value.as_str(), row.label.as_str()) {
        (dk::SINGLE, ..) => close_and_refocus(app),
        (dk::TOGGLE, ..) => {
            let Some(p) = i.saved.clone() else { return app.toast("Сначала подключитесь к сети") };
            let on = !p.autoconnect;
            with(|s| if let Some(Dialog::Info(inf)) = &mut s.dialog { if let Some(sp) = &mut inf.saved { sp.autoconnect = on; } });
            crate::sound::play(if on { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
            push(app);
            run(None, Box::new(move |b| { let _ = b.set_autoconnect(&ssid, on); }));
        }
        (dk::BUTTON_ROW, v, _) => {
            if v == lt("#Settings_Internet_Forget", "Забыть") {
                close_dialog(app);
                run(Some(Op::Forget(ssid.clone())), Box::new(move |b| { let _ = b.forget(&ssid); }));
                push(app);
            } else if v == lt("#Settings_Internet_Disconnect", "Отключиться") {
                close_dialog(app);
                run(Some(Op::Disconnect), Box::new(move |b| { let _ = b.disconnect(); }));
                push(app);
            } else {
                // Подключиться from the info card: the stock dialog closes the card on success
                close_dialog(app);
                start_connect(app, &ssid);
            }
        }
        _ => {}
    }
}

fn progress_input(app: &mut App, p: Prog, a: Action) {
    match (p.stage, a) {
        (Stage::Success, Action::A) | (Stage::Success, Action::B) => close_and_refocus(app),
        (Stage::Connecting | Stage::Checking, Action::A) => {}
        (Stage::Connecting | Stage::Checking | Stage::Failed, Action::B) | (Stage::Failed, Action::A) => cancel_connect(app, p),
        (Stage::NoInternet | Stage::Portal, Action::Left | Action::Right | Action::Up | Action::Down) => {
            let mut p = p;
            p.focus = if matches!(a, Action::Left | Action::Up) { 0 } else { 1 };
            with(|s| s.dialog = Some(Dialog::Progress(p)));
            push(app);
        }
        // «Попробовать другую сеть» (left) forgets the new profile and returns; «Не подключаться к интернету» keeps it
        (Stage::NoInternet | Stage::Portal, Action::A) => if p.focus == 0 { forget_and_close(app, &p.ssid) } else { close_and_refocus(app) },
        (Stage::NoInternet | Stage::Portal, Action::B) => forget_and_close(app, &p.ssid),
        _ => {}
    }
}

fn forget_and_close(app: &mut App, ssid: &str) {
    let ssid = ssid.to_string();
    close_dialog(app);
    let ops = [Op::Disconnect, Op::Forget(ssid.clone())];
    for op in &ops { with(|s| s.begin_op(op.clone())); }
    push(app);
    std::thread::spawn(move || {
        let _ = backend().disconnect();
        let _ = backend().forget(&ssid);
        let _ = slint::invoke_from_event_loop(move || { let _ = crate::with_app(|a| {
            with(|s| s.pending.retain(|p| !ops.contains(p)));
            refresh(true);
            push(a);
        }); });
    });
}

/// B / «Отмена» while connecting: abort the worker; a secured network goes back to the prompt, an open one closes.
fn cancel_connect(app: &mut App, p: Prog) {
    if let Some(c) = with(|s| s.cancel.take()) { c.store(true, Ordering::SeqCst); }
    with(|s| { s.generation = s.generation.wrapping_add(1); s.busy = None; });
    match p.back {
        Some(mut f) if p.secured => { f.failed = false; f.focus = F::Pass; set_dialog(app, Some(Dialog::Form(f))); attach_field(app); }
        _ => close_and_refocus(app),
    }
    refresh(true);
}

/// B on the page when no dialog is open is the settings rail's job.
pub fn back(app: &mut App) -> bool {
    if dialog_open() {
        let dlg = with(|s| s.dialog.clone()).unwrap();
        // B on the keyboard is consumed by the keyboard; here it is the dialog's own cancel
        match dlg {
            Dialog::Progress(p) => progress_input(app, p, Action::B),
            _ => handle_dialog(app, Action::B),
        }
        return true;
    }
    false
}

// ---------------------------------------------------------------- scrolling

fn scroll_to_focus() {
    with(|s| {
        let rows = s.rows();
        let Some(r) = rows.iter().find(|r| r.key.is_some() && r.key == s.focus) else { return };
        let (top, bottom) = (r.y - s.vy, r.y + r.h - s.vy);
        if top < VIEW_TOP + 20.0 { s.vy = (r.y - (VIEW_TOP + 20.0)).max(0.0); }
        else if bottom > VIEW_BOTTOM - 25.0 { s.vy = r.y + r.h - (VIEW_BOTTOM - 25.0); }
        let max = (content_height(&rows) - VIEW_BOTTOM).max(0.0);
        s.vy = s.vy.clamp(0.0, max);
        // the first row shows the section above it
        if rows.first().is_some_and(|f| f.key == s.focus) { s.vy = 0.0; }
    });
}

// ---------------------------------------------------------------- painting

pub(crate) fn to_wrow(r: &Row, focused: bool) -> WifiRow {
    WifiRow { kind: r.kind, y: r.y, h: r.h, label: r.label.as_str().into(), desc: r.desc.as_str().into(), btn: r.btn.as_str().into(), btn_w: r.btn_w, on: r.on, bars: r.bars, lock: r.lock, mark: r.mark, warn: r.warn, focused, ptr: r.key.is_some() }
}

pub(crate) fn to_drow(r: &DRow) -> WifiDRow {
    WifiDRow { kind: r.kind, y: r.y, h: r.h, label: r.label.as_str().into(), value: r.value.as_str().into(), hint: r.hint.as_str().into(), on: r.on, focus: r.focus, a: r.a, b: r.b, w2: r.w2, item: r.item }
}

/// Pushes the whole page and the dialog to Slint.
pub fn push(app: &mut App) {
    let osk = app.osk_is_open();
    let live_offline = app.live.system.as_ref().is_some_and(|s| s.network.offline);
    let running = !app.live.running.is_empty();
    let (rows, dv, vy, focus_idx, offline) = with(|s| {
        s.osk_open = osk;
        s.live_offline = live_offline;
        s.game_running = running;
        s.fix_focus();
        let rows = s.rows();
        let mut dv = s.dialog_view();
        if let Some(v) = &mut dv { s.dscroll = v.scroll; }
        let focus_idx = s.focus.as_ref().and_then(|k| rows.iter().position(|r| r.key.as_ref() == Some(k)));
        (rows, dv, s.vy, focus_idx, s.offline())
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
    let model: Vec<WifiRow> = rows.iter().enumerate().map(|(i, r)| to_wrow(r, show_focus && focus_idx == Some(i))).collect();
    let is = ui.global::<InternetState>();
    is.set_rows(ModelRc::new(VecModel::from(model)));
    is.set_offline(offline);
    match dv {
        Some(v) => {
            is.set_dialog_open(true);
            is.set_box_y(v.box_y);
            is.set_box_h(v.box_h);
            is.set_content_h(v.content_h);
            is.set_scroll(v.scroll);
            is.set_drows(ModelRc::new(VecModel::from(v.rows.iter().map(to_drow).collect::<Vec<_>>())));
        }
        None => { is.set_dialog_open(false); is.set_drows(ModelRc::new(VecModel::from(Vec::<WifiDRow>::new()))); }
    }
    app.update_hint();
    ui.global::<crate::TopBarState>().set_wifi_offline(offline);
}

pub fn wire(ui: &crate::AppWindow) {
    let is = ui.global::<InternetState>();
    is.on_row_clicked(|i| crate::screens::defer(move |a| {
        a.se.zone = 1;
        if a.se.page_id() == crate::screens::bluetooth::PAGE || crate::screens::settings::controller_devices::bluetooth(a) { return crate::screens::bluetooth::row_clicked(a, i); }
        let key = with(|s| s.rows().get(i as usize).and_then(|r| r.key.clone()));
        activate(a, key);
    }));
    is.on_dialog_clicked(|item, side| crate::screens::defer(move |a| {
        if a.se.page_id() == crate::screens::bluetooth::PAGE || crate::screens::settings::controller_devices::bluetooth(a) { return crate::screens::bluetooth::dialog_clicked(a, item, side); }
        dialog_click(a, item, side)
    }));
    is.on_backdrop_clicked(|| crate::screens::defer(|a| {
        if crate::screens::settings::controller_devices::bluetooth(a) { crate::screens::settings::controller_devices::back(a); } else if a.se.page_id() == crate::screens::bluetooth::PAGE { crate::screens::bluetooth::back(a); } else { back(a); }
    }));
}

/// Touch / mouse on a dialog control: focus it and press A (a pair's right half is `side == 1`).
fn dialog_click(app: &mut App, item: i32, side: i32) {
    if item < 0 { return; }
    let item = item as usize;
    with(|s| match &mut s.dialog {
        Some(Dialog::Form(f)) => { let it = f.items(); if let Some(x) = it.get(item) { f.focus = if side == 1 && *x == F::Connect { F::Cancel } else { *x }; } }
        Some(Dialog::Info(i)) => i.focus = item,
        Some(Dialog::Proxy(p)) => proxy::click(p, item, side),
        Some(Dialog::Pick(f, sel)) => { let _ = f; *sel = item; }
        Some(Dialog::Progress(p)) => p.focus = if side == 1 { 1 } else { 0 }.min(if matches!(p.stage, Stage::NoInternet | Stage::Portal) { 1 } else { 0 }),
        Some(Dialog::Confirm(_, f)) => *f = side as usize,
        _ => {}
    });
    handle_dialog(app, Action::A);
}

#[cfg(test)]
mod tests;
