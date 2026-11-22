//! /routes/settings/<page> - rail of pages (docs/engine/routes.json `settings_pages`) + generated page content.
//! Rail navigation moves the cursor; tapping or pressing A on a category replaces the route. Content rows come from
//! `ui/generated/settings/pages.json` (tools/compile-ui/pages.mjs: rect/role/handlers of every element of the captured Steam page);
//! current values are read (read-only) from `settingsStore` through the bridge; setters only log under DECKSHELL_DRYRUN.
use steam_assets::i18n;
#[path = "storage.rs"]
mod storage;
#[path = "recording_settings.rs"]
mod recording_settings;
#[path = "power_page.rs"]
pub(crate) mod power_page;
#[path = "text_wrap.rs"]
pub(super) mod text_wrap;
#[path = "controller_devices.rs"]
pub(crate) mod controller_devices;

use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::assets::{t, t_or};
use crate::input::Action;
use crate::loader::Kind;
use crate::router::Route;
use crate::{AppWindow, GPageRow, ListEntry, SettingsState};

const ROUTES_JSON: &str = include_str!("../../../docs/engine/routes.json");
const PAGES_JSON: &str = include_str!("../../ui/generated/settings/pages.json");

fn system_report_unavailable() -> &'static str {
    i18n::tr_ref("settings.system_reporting_not_available_on_deckshell")
}

/// Steam css px -> device px (window 1280 wide for the 1068 css px BP window).
pub const SC: f32 = 1280.0 / 1068.0;

#[derive(Clone, Debug)]
pub struct Page {
    pub id: String,
    pub label: String,
}

/// One generated element (css px geometry).
#[derive(Clone, Debug, Default)]
pub struct PRow {
    pub k: i32,
    pub role: String,
    pub tok: String,
    pub lab: String,
    pub desc: String,
    pub full: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub pad: String,
    pub f: bool,
    /// id captured from Steam (`text-field` rows)
    pub id: String,
}

/// Тестовые поля страницы «Клавиатура»: фокусируются, A / касание открывают клавиатуру.
fn is_test_field(r: &Value) -> bool {
    r["k"].as_i64() == Some(5) && r["pad"].as_str() == Some("keyboard") && !r["id"].as_str().unwrap_or("").is_empty()
}

thread_local! {
    static FIELD_TEXT: std::cell::RefCell<std::collections::HashMap<String, String>> = Default::default();
    static FIELD_ACTIVE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}
const FIELD_SINK: super::osk::ExtSink = super::osk::ExtSink { id: 5, mode: super::osk::InputMode::Normal, typed: field_typed, enter: field_done, cancel: field_done };
fn field_typed(a: &mut App, c: Option<char>) {
    let key = FIELD_ACTIVE.with(|k| k.borrow().clone());
    FIELD_TEXT.with(|m| {
        let mut m = m.borrow_mut();
        let t = m.entry(key).or_default();
        match c { Some(c) => t.push(c), None => { t.pop(); } }
    });
    a.push_settings();
}
fn field_done(_: &mut App) {}
fn field_text(id: &str) -> String { FIELD_TEXT.with(|m| m.borrow().get(id).cloned().unwrap_or_default()) }

pub fn parse_rows(v: &Value) -> Vec<PRow> {
    let s = |r: &Value, k: &str| i18n::captured_text(r[k].as_str().unwrap_or_default()).to_owned();
    let f = |r: &Value, k: &str| r[k].as_f64().unwrap_or(0.0) as f32;
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|r| PRow { k: r["k"].as_i64().unwrap_or(0) as i32, role: s(r, "role"), tok: s(r, "tok"), lab: s(r, "lab"), desc: s(r, "desc"), full: s(r, "full"), x: f(r, "x"), y: f(r, "y"), w: f(r, "w"), h: f(r, "h"), pad: s(r, "pad"), f: r["f"].as_i64().unwrap_or(0) == 1 || is_test_field(r), id: s(r, "id") })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve capture text from the catalog and prefer Steam tokens in English. The recorded
/// Russian copy also carries the description suffix used by token-backed rows.
pub(super) fn parse_captured_rows(v: &Value) -> Vec<PRow> {
    let mut rows = parse_rows(v);
    if i18n::current().code() == "ru" {
        return rows;
    }
    let translate = |raw: &Value, key: &str, token: &str, source_token: &str| {
        let fallback = raw[key].as_str().map(i18n::tr_ref).unwrap_or_default();
        let token = raw[token].as_str().unwrap_or(source_token);
        if token.is_empty() { fallback.to_owned() } else { t_or(token, fallback) }
    };
    if let Some(capture) = v.as_array() {
        for (row, raw) in rows.iter_mut().zip(capture) {
            row.full.clear();
            if row.k == 0 {
                row.lab.clear();
                row.desc.clear();
                continue;
            }
            if raw["lab_key"].is_string() {
                // Slint's search pills use captured uppercase labels.
                let source_token = if row.k == 9 { "" } else { &row.tok };
                row.lab = translate(raw, "lab_key", "lab_token", source_token);
            }
            if raw["desc_key"].is_string() {
                row.desc = translate(raw, "desc_key", "desc_token", "");
            }
        }
    }
    rows
}

/// A captured label row whose token the running Steam does not localize and that has no captured text: showing the
/// bare token would be wrong (the Deck's Steam no longer has it). Such rows are hidden, not shown as raw tokens.
pub fn unlocalized_label(r: &PRow, has_loc: impl Fn(&str) -> bool) -> bool {
    r.k == 1 && !r.tok.is_empty() && r.lab.is_empty() && !has_loc(&r.tok)
}

/// Removes unlocalized label rows and the toggles/checkboxes (k 2|3) whose centre lies inside one of them, so no
/// control is left without its label.
pub fn drop_unlocalized(rows: Vec<PRow>, has_loc: impl Fn(&str) -> bool) -> Vec<PRow> {
    let gone: Vec<(f32, f32, f32, f32)> = rows.iter().filter(|r| unlocalized_label(r, &has_loc)).map(|r| (r.x, r.y, r.w, r.h)).collect();
    rows.into_iter()
        .filter(|r| {
            if unlocalized_label(r, &has_loc) {
                return false;
            }
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            !(matches!(r.k, 2 | 3) && gone.iter().any(|g| cx >= g.0 && cx <= g.0 + g.2 && cy >= g.1 && cy <= g.1 + g.3))
        })
        .collect()
}

/// Label/description of a label row: loc token (RU at runtime) + the rest of the captured text.
pub fn texts(r: &PRow) -> (String, String) {
    if !r.tok.is_empty() {
        let l = t(&r.tok);
        let l = if l.is_empty() || l == r.tok { r.lab.clone() } else { l };
        let rest = r.full.strip_prefix(l.as_str()).map(|x| x.trim().to_string()).unwrap_or_else(|| r.desc.clone());
        return (l, rest);
    }
    (r.lab.clone(), r.desc.clone())
}

pub fn to_grow(r: &PRow, on: i32) -> GPageRow {
    let (label, desc) = match r.k {
        1 => texts(r),
        24 => (r.lab.clone(), r.desc.clone()),
        4 | 5 | 9 if r.lab.is_empty() && !r.tok.is_empty() => (t_or(&r.tok, ""), String::new()),
        _ => (r.lab.clone(), String::new()),
    };
    GPageRow {
        kind: r.k,
        role: r.role.as_str().into(),
        token: r.tok.as_str().into(),
        label: label.into(),
        desc: desc.into(),
        x: r.x * SC,
        y: r.y * SC,
        w: r.w * SC,
        h: r.h * SC,
        pad: r.pad.as_str().into(),
        on,
        img: Default::default(),
        has_img: false,
        sub: r.full.as_str().into(),
        shift: "".into(),
        ptr: r.pad.contains("touch") || r.pad.contains("mouse"),
        fcs: r.pad.contains("FOCUS"),
    }
}

/// Storage sort popup: Up/Down move one option and clamp at both ends (Steam's four-row dialog, task 123).
/// Returns the option index to focus; `pos` and the result index the option list, not the page rows.
pub fn sort_menu_step(pos: usize, len: usize, down: bool) -> usize {
    if down { (pos + 1).min(len.saturating_sub(1)) } else { pos.saturating_sub(1) }
}

/// Spatial focus move among focusable rows (dir: 0 up, 1 down, 2 left, 3 right).
pub fn spatial(rows: &[PRow], from: usize, dir: u8) -> Option<usize> {
    let c = &rows[from];
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let mut best: Option<(f32, usize)> = None;
    for (i, r) in rows.iter().enumerate() {
        if i == from || !r.f {
            continue;
        }
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let (dx, dy) = (x - cx, y - cy);
        let ok = match dir {
            0 => dy < -2.0,
            1 => dy > 2.0,
            2 => dx < -2.0 && dy.abs() < c.h.max(r.h) * 0.6,
            _ => dx > 2.0 && dy.abs() < c.h.max(r.h) * 0.6,
        };
        if !ok {
            continue;
        }
        let d = if dir < 2 { dy.abs() * 10.0 + dx.abs() } else { dx.abs() * 10.0 + dy.abs() };
        if best.map(|b| d < b.0).unwrap_or(true) {
            best = Some((d, i));
        }
    }
    best.map(|b| b.1)
}

/// Rail (page list) behaviour, from Steam's PagedSettings page list (steamui-snapshot sp.js, module 91371 `O`/`v`):
/// Up/Down move the list focus only; the page changes on click (A on the focused item) and the content column is
/// the focus target that A on the list enters. Right from the list moves focus into the shown page without a page change.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RailStep {
    /// List focus moves to this index; the shown page is unchanged.
    Move(usize),
    /// Focus enters the content column; `commit` = the focused item becomes the shown page first.
    Enter { commit: bool },
    Nothing,
}

pub fn rail_step(cursor: usize, focus: usize, n: usize, a: Action) -> RailStep {
    match a {
        Action::Up => RailStep::Move(cursor.saturating_sub(1)),
        Action::Down => RailStep::Move((cursor + 1).min(n.saturating_sub(1))),
        Action::A => RailStep::Enter { commit: cursor != focus },
        Action::Right => RailStep::Enter { commit: false },
        _ => RailStep::Nothing,
    }
}

#[derive(Default)]
pub struct State {
    pub(crate) devices: controller_devices::State,
    pub pages: Vec<Page>,
    /// the shown page (route)
    pub focus: usize,
    /// list focus; equals `focus` except while the user moves through the list without pressing A
    pub cursor: usize,
    /// 0 rail, 1 content
    pub zone: u8,
    pub rows: Vec<PRow>,
    pub on: Vec<i32>,
    pub cf: Option<usize>,
    pub vy: f32,
    pub snapshot: Value,
    pub pending: Option<String>,
    storage: Option<Result<Vec<storage::Drive>, ()>>,
    storage_selected: Option<i64>,
    storage_sort: usize,
    pub(crate) storage_sort_menu: bool,
    pub(crate) recording_menu: Option<(String, Vec<(String, Value)>)>,
    /// device-px y of the centre of the drop-down that opened the power menu (the stock popup is anchored beside it)
    pub(crate) power_menu_y: f32,
    recording_write: Option<&'static str>,
    recording_revision: u64,
    storage_generation: u64,
}

impl State {
    pub fn page_id(&self) -> String {
        self.pages.get(self.focus).map(|p| p.id.to_lowercase()).unwrap_or_else(|| "system".into())
    }
}

fn load_pages() -> Vec<Page> {
    let v: serde_json::Value = serde_json::from_str(ROUTES_JSON).unwrap_or_default();
    let mut out: Vec<Page> = v["settings_pages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let id = p["id"].as_str()?.to_string();
                    let label = if id.eq_ignore_ascii_case("gamerecording") {
                        recording_settings::steam_text("Settings_Page_GameRecording")
                    } else if id.eq_ignore_ascii_case(super::anim_page::PAGE) {
                        i18n::tr(super::anim_page::LABEL)
                    } else if id.eq_ignore_ascii_case(power_page::PAGE) {
                        power_page::rail_label()
                    } else if id.eq_ignore_ascii_case(controller_devices::PAGE) {
                        t_or("#Settings_Page_Controller", steam_assets::i18n::tr_ref("controller.controller"))
                    } else {
                        t_or(&format!("#Settings_Page_{id}"), i18n::tr_ref(&format!("settings.rail.{}", id.to_lowercase())))
                    };
                    Some(Page { id, label })
                })
                // stock `Ii`: «Bluetooth» is on the rail only while the Bluetooth service answers
                .filter(|p| !p.id.eq_ignore_ascii_case(super::bluetooth::PAGE) || super::bluetooth::service_available())
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty() {
        out.push(Page { id: "System".into(), label: t_or("#Settings_Page_System", i18n::tr_ref("settings.system")) });
    }
    out
}

/// Page group of the stock rail (`no` array in sp.js:555242): the groups are divided by `l.I0` separators —
/// System…Bluetooth | Display…Security | Notifications…Game recording | Home…
fn rail_group(id: &str) -> u8 {
    match id.to_ascii_lowercase().as_str() {
        "system" | "internet" | "storage" | "bluetooth" => 0,
        "display" | "power" | "audio" | "controller" | "keyboard" | "customization" | "accessibility" | "security" => 1,
        "notifications" | "friends" | "downloads" | "cloud" | "ingame" | "compatibility" | "family" | "remoteplay" | "gamerecording" => 2,
        _ => 3,
    }
}

/// Extra css px above each rail row: 18 for every separator passed (measured on the 1280x800 stock capture: 90 px between
/// Bluetooth and Display against the 63 px row pitch). A rise between neighbours is a separator line.
pub fn rail_extra(ids: &[&str]) -> Vec<f32> {
    let mut extra = 0.0;
    ids.iter().enumerate().map(|(i, id)| {
        if i > 0 && rail_group(id) != rail_group(ids[i - 1]) { extra += 18.0; }
        extra
    }).collect()
}

pub fn wire(ui: &AppWindow) {
    ui.global::<SettingsState>().on_activate(|i| super::defer(move |a| {
        a.se.focus = i as usize;
        a.se.zone = 0;
        let p = a.se.page_id();
        a.router.replace(Route::Settings(p.clone()));
        a.sync_settings(&p);
    }));
    controller_devices::wire(ui);
    super::anim_page::wire(ui);
    super::internet::wire(ui);
    ui.global::<SettingsState>().on_content_activated(|i| super::defer(move |a| {
        a.se.zone = 1;
        a.se.cf = Some(i as usize);
        a.settings_activate(i as usize);
    }));
    // screens that have no wire_all entry (screens/mod.rs is not ours) hook their callbacks here
    super::media::wire(ui);
    super::search::wire(ui);
    super::osk::wire(ui);
}

pub fn init_strings(ui: &AppWindow) {
    let st = ui.global::<SettingsState>();
    st.set_title(t("#Settings_Title").into());
    st.set_empty_text("".into());
}

/// Token -> path into the settingsStore snapshot {c: m_ClientSettings, f: m_FriendSettings, s: settings, w: wifi}.
/// A row with two toggles (toast, sound) uses `path` for the first and `path2` for the second; a leading `!` inverts.
fn binding(tok: &str, nth: usize) -> Option<&'static str> {
    let t = tok.trim_start_matches('#');
    Some(match (t, nth) {
        ("DeckShell_RecordingMic", _) => "c.gamerecording_background_a_m",
        ("DeckShell_RecordingMicMono", _) => "c.gamerecording_force_mic_mono",
        ("DeckShell_RecordingMicAGC", _) => "c.gamerecording_automatic_gain_control",
        ("NotificationsSettings_Notification_UnlockAchievement", 0) => "c.achievement_notification_toast",
        ("NotificationsSettings_Notification_UnlockAchievement", 1) => "c.achievement_notification_sound",
        ("NotificationsSettings_Notification_ControllerConnect", 0) => "c.controllerconnect_notification_toast",
        ("NotificationsSettings_Notification_ControllerConnect", 1) => "c.controllerconnect_notification_sound",
        ("NotificationsSettings_Notification_ControllerLowBattery", 0) => "c.controller_low_battery_notification_toast",
        ("NotificationsSettings_Notification_FriendJoins", 0) => "f.bNotifications_ShowIngame",
        ("NotificationsSettings_Notification_FriendJoins", 1) => "f.bSounds_PlayIngame",
        ("NotificationsSettings_Notification_FriendOnline", 0) => "f.bNotifications_ShowOnline",
        ("NotificationsSettings_Notification_FriendOnline", 1) => "f.bSounds_PlayOnline",
        ("NotificationsSettings_Notification_ChatMessage", 0) => "f.bNotifications_ShowMessage",
        ("NotificationsSettings_Notification_ChatMessage", 1) => "f.bSounds_PlayMessage",
        ("NotificationsSettings_Notification_ChatRoomNotification", 0) => "f.bNotifications_ShowChatRoomNotification",
        ("NotificationsSettings_Notification_ChatRoomNotification", 1) => "f.bSounds_PlayChatRoomNotification",
        ("FriendSettings_CategorizeInGameFriendsByGame", _) => "f.bCategorizeInGameFriendsByGame",
        ("FriendSettings_HideOfflineFriendsInCategories", _) => "f.bHideOfflineFriendsInTagGroups",
        ("FriendSettings_SortFriendsByStatus", _) => "!f.bForceAlphabeticFriendSorting",
        ("FriendSettings_CompactFriendsList", _) => "f.bCompactFriendsList",
        ("FriendSettings_CompactQuickAccess_Client", _) => "f.bCompactQuickAccess",
        ("FriendSettings_AlwaysNewChatWindow", _) => "f.bAlwaysNewChatWindow",
        ("FriendSettings_DisableEmbedInlining", _) => "f.bDisableEmbedInlining",
        ("FriendSettings_DisableSpellcheck", _) => "f.bDisableSpellcheck",
        ("FriendSettings_DisableRoomEffects", _) => "f.bDisableRoomEffects",
        ("AppProperties_EnableOverlay", _) => "c.enable_overlay",
        ("Settings_InGame_Overlay_ToolBar_ListView", _) => "c.overlay_toolbar_list_view",
        ("Settings_InGame_UseGamepadOverlay", _) => "c.always_use_gamepadui_overlay",
        ("Settings_InGame_ScreenshotNotify", _) => "c.enable_screenshot_notification",
        ("Settings_InGame_ScreenshotSound", _) => "c.enable_screenshot_sound",
        ("Settings_InGame_ScreenshotUncompressed", _) => "c.save_uncompressed_screenshots",
        ("Settings_Battery_ChargeLimitEnable", _) => "c.steamos_charge_limit_enabled",
        ("Settings_CEC_Control", _) => "c.steamos_cec_control",
        ("Settings_CEC_SuspendDevice", _) => "c.steamos_cec_suspend_device",
        ("Settings_CEC_WakeTV", _) => "c.steamos_cec_wake_tv",
        ("Settings_CEC_SuspendTV", _) => "c.steamos_cec_suspend_tv",
        ("Settings_System_EnableDeveloperMode", _) => "c.developer_mode_enabled",
        ("Settings_Display_DisplayScaling_Automatically_Scale_UI_Verbose", _) => "s.bDisplayIsUsingAutoScale",
        ("Settings_Internet_Wifi_Enabled", _) => "w",
        ("Audio_Setting_Enable_UI_Sounds", _) => "c.enable_ui_sounds",
        ("HomeSettings_ShowStoreContent", _) => "c.home_show_store_content",
        ("DownloadSettings_DisplayInBitsPerSecond", _) => "c.display_rates_in_bits",
        ("FriendSettings_DockChats", _) => "f.bDockChats",
        ("Settings_System_DesktopModeDefault", _) => "c.desktop_mode_default",
        ("Developer_SLS_Minidump_Enabled", _) => "c.sls_minidump_enabled",
        _ => return None,
    })
}

fn lookup(snap: &Value, path: &str) -> Option<bool> {
    let (inv, p) = path.strip_prefix('!').map(|p| (true, p)).unwrap_or((false, path));
    let v = if p == "w" {
        &snap["w"]
    } else {
        let (g, k) = p.split_once('.')?;
        &snap[g][k]
    };
    v.as_bool().map(|b| b != inv)
}

fn response_is_current(page: &str, generation: u64, active_page: &str, active_generation: u64, route: &Route) -> bool {
    page == active_page && generation == active_generation
        && matches!(route, Route::Settings(id) if id.eq_ignore_ascii_case(page))
}

/// Unknown/missing and unbound controls are unavailable, never a fabricated off.
fn toggle_state(snapshot: &Value, path: Option<&str>) -> i32 {
    path.and_then(|p| lookup(snapshot, p)).map(i32::from).unwrap_or(-1)
}

fn next_toggle_value(current: i32, path: Option<&str>) -> Option<bool> {
    path?;
    match current { 0 => Some(true), 1 => Some(false), _ => None }
}

/// Replace the stale capture with the recording page traced to the Steam source.
fn identify_recording_rows(rows: &mut Vec<PRow>) {
    *rows = recording_settings::rows();
}

/// Global retention is a decimal number of minutes followed by "min".
/// Steam clamps its UI to five minutes minimum; do not guess unknown values.
fn recording_duration(value: &str) -> Option<u32> {
    let digits = value.strip_suffix("min")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) { return None; }
    let minutes = digits.parse::<u32>().ok()?;
    (5..=9999).contains(&minutes).then_some(minutes)
}
#[cfg(test)]
fn next_recording_duration(current: u32) -> u32 {
    [5, 15, 30, 60, 120, 240, 480].into_iter().find(|v| *v > current).unwrap_or(5)
}

#[cfg(test)]
fn next_instant_clip_duration(snapshot: &Value) -> Option<f64> {
    if snapshot["c"]["gamerecording_background_mode"].as_i64() != Some(1) { return None; }
    let current = snapshot["c"]["gamerecording_ic_seconds"].as_f64()?;
    if !current.is_finite() || current <= 0.0 || current > 9999.0 { return None; }
    Some([10.0, 20.0, 30.0, 45.0, 60.0, 120.0, 300.0].into_iter()
        .find(|value| *value > current).unwrap_or(10.0))
}

fn recording_mode(tok: &str) -> Option<i64> {
    match tok {
        "#DeckShell_RecordingMode0" => Some(0),
        "#DeckShell_RecordingMode1" => Some(1),
        "#DeckShell_RecordingMode2" => Some(2),
        _ => None,
    }
}

fn recording_mode_label(label: &str, mode: i64, snapshot: &Value) -> String {
    match snapshot["c"]["gamerecording_background_mode"].as_i64() {
        Some(current @ 0..=2) if current == mode => format!("● {label}"),
        Some(0..=2) => format!("○ {label}"),
        _ => i18n::tr_args("settings.not_available", &[("label", &(label).to_string())]),
    }
}

#[cfg(test)]
fn next_recording_bitrate(current: &str) -> Option<&'static str> {
    let values = ["preset_ultra", "preset_default", "preset_medium", "preset_low"];
    values.iter().position(|v| *v == current).map(|i| values[(i + 1) % values.len()])
}

fn recording_choices(tok: &str) -> Option<(&'static str, &'static [i64])> {
    match tok {
        "#DeckShell_RecordingFps" => Some(("gamerecording_max_fps", &[60, 30])),
        "#DeckShell_RecordingHeight" => Some(("gamerecording_video_maxheight", &[0, 1440, 1200, 1080, 800, 720, 480])),
        _ => None,
    }
}

#[cfg(test)]
fn next_recording_choice(tok: &str, current: i64) -> Option<(&'static str, i64)> {
    let (key, values) = recording_choices(tok)?;
    let next = values.iter().position(|v| *v == current).map(|i| values[(i + 1) % values.len()]).unwrap_or(values[0]);
    Some((key, next))
}

/// Retain the last known value on timeout, never claiming an unobserved success.
fn recording_readback(requested: &Value, attempts: usize, mut read: impl FnMut() -> Option<Value>, mut wait: impl FnMut()) -> Option<Value> {
    let mut last = None;
    for n in 0..attempts {
        if n > 0 { wait(); }
        if let Some(value) = read().filter(|v| !v.is_null()) {
            if &value == requested || value.as_f64().zip(requested.as_f64()).is_some_and(|(a, b)| a == b) { return Some(value); }
            last = Some(value);
        }
    }
    last
}

fn recording_execute(requested: &Value, mut write: impl FnMut(), read: impl FnMut() -> Option<Value>, wait: impl FnMut()) -> Option<Value> {
    write();
    recording_readback(requested, 8, read, wait)
}

// A page can be reopened before its previous asynchronous write finishes.
// Register the write before spawning it, so the new snapshot cannot race it.
static RECORDING_WRITES: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());
struct RecordingWriteGuard;
impl RecordingWriteGuard {
    fn begin() -> Self {
        *RECORDING_WRITES.0.lock().unwrap() += 1;
        Self
    }
}
impl Drop for RecordingWriteGuard {
    fn drop(&mut self) {
        let mut count = RECORDING_WRITES.0.lock().unwrap();
        *count -= 1;
        RECORDING_WRITES.1.notify_all();
    }
}
fn wait_for_recording_writes() {
    let mut count = RECORDING_WRITES.0.lock().unwrap();
    while *count != 0 { count = RECORDING_WRITES.1.wait(count).unwrap(); }
}

fn recording_snapshot_current(page: &str, captured: u64, current: u64) -> bool {
    page != "gamerecording" || captured == current
}

#[cfg(test)]
#[test]
fn reopened_recording_snapshot_waits_for_inflight_write() {
    let guard = RecordingWriteGuard::begin();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || { wait_for_recording_writes(); tx.send(()).unwrap(); });
    assert!(rx.recv_timeout(std::time::Duration::from_millis(30)).is_err());
    drop(guard);
    rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    reader.join().unwrap();
}

#[cfg(test)]
#[test]
fn recording_snapshot_started_before_write_cannot_overwrite_readback() {
    assert!(recording_snapshot_current("gamerecording", 4, 4));
    assert!(!recording_snapshot_current("gamerecording", 4, 5));
    assert!(recording_snapshot_current("system", 4, 5));
}

/// SteamOSManager limits (charge limit range, CEC wake support) are static for a session (stock `staleTime: Infinity`), so a successful
/// read is kept and the RPC is not repeated on every snapshot; a failed read is retried next time and the page uses its fallbacks.
fn merge_power_limits(b: &steam_bridge::Bridge, snapshot: &mut Value) {
    static CACHE: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.is_none() {
        match b.power_limits() {
            Ok(v) if v.is_object() => *cache = Some(v),
            Ok(_) => {}
            Err(error) => eprintln!("[deckshell] power limits: {error}"),
        }
    }
    let (Some(limits), Some(power)) = (cache.as_ref().and_then(Value::as_object), snapshot["power"].as_object_mut()) else { return };
    for (k, v) in limits { if !v.is_null() { power.insert(k.clone(), v.clone()); } }
}

/// Read-only snapshot of the live settings (SharedJSContext `settingsStore`); null when the bridge is down.
fn fetch_snapshot(recording: bool) -> Value {
    if recording { wait_for_recording_writes(); }
    let Some(b) = crate::actions::bridge() else { return Value::Null };
    let js = include_str!("settings_snapshot.js");
    let mut snapshot = Value::Null;
    for attempt in 0..if recording { 3 } else { 1 } {
        match b.eval_json(if recording { include_str!("recording_snapshot.js") } else { js }) {
            Ok(value) => snapshot = value,
            Err(error) => eprintln!("[deckshell] settings snapshot: {error}"),
        }
        if !recording || snapshot["c"]["gamerecording_background_mode"].as_i64().is_some() { break; }
        if attempt < 2 { std::thread::sleep(std::time::Duration::from_millis(250)); }
    }
    if !recording { merge_power_limits(&b, &mut snapshot); }
    // stock BatteryPreferences (localStorage CachedBatteryPreferences): the flag is decided by the gated battery-logic port
    if let Some(power) = snapshot["power"].as_object_mut() {
        let flag = battery_logic::pref_show(power.get("pref").unwrap_or(&Value::Null));
        power.insert("percent".into(), flag.map_or(Value::Null, Value::Bool));
    }
    if recording {
        let missing = ["gamerecording_background_mode", "gamerecording_ic_seconds", "gamerecording_background_max_keep",
            "gamerecording_video_bitrate", "gamerecording_max_fps", "gamerecording_video_maxheight",
            "gamerecording_background_a_m", "gamerecording_force_mic_mono", "gamerecording_automatic_gain_control"].into_iter().filter(|key| snapshot["c"][*key].is_null()).collect::<Vec<_>>();
        eprintln!("[deckshell] recording snapshot: missing={missing:?}");
    }
    snapshot
}

impl App {
    pub fn build_settings_rail(&mut self) {
        self.se.pages = load_pages();
        let rail: Vec<ListEntry> = self.se.pages.iter().map(|p| {
            // Steam's rail icons identify the category; missing assets stay absent.
            let name = crate::assets::settings_rail_icon(&p.id).unwrap_or("");
            let icon = (!name.is_empty()).then(|| crate::assets::try_icon_image(name)).flatten();
            ListEntry { label: p.label.as_str().into(), value: "".into(), has_icon: icon.is_some(), icon: icon.unwrap_or_default() }
        }).collect();
        let ids: Vec<&str> = self.se.pages.iter().map(|p| p.id.as_str()).collect();
        self.ui().global::<SettingsState>().set_rail(ModelRc::new(VecModel::from(rail)));
        self.ui().global::<SettingsState>().set_rail_extra(ModelRc::new(VecModel::from(rail_extra(&ids))));
        if let Ok(v) = serde_json::from_str::<Value>(PAGES_JSON) {
            let w = &v["_wrap"];
            let st = self.ui();
            let st = st.global::<SettingsState>();
            st.set_rail_pad(w["rail"].as_str().unwrap_or("A,FOCUS").into());
            st.set_content_pad(w["content"].as_str().unwrap_or("B").into());
        }
    }

    pub fn sync_settings(&mut self, page: &str) {
        if let Some(p) = self.se.pages.iter().position(|p| p.id.to_lowercase() == page) {
            self.se.focus = p;
        }
        self.se.cursor = self.se.focus;
        let page_id = self.se.page_id();
        if page_id == controller_devices::PAGE {
            super::anim_page::leave();
            super::internet::leave(self);
            super::bluetooth::leave(self);
            controller_devices::open(self);
            return;
        }
        controller_devices::leave(self);
        self.se.pending = None;
        self.se.storage_generation = self.se.storage_generation.wrapping_add(1);
        let generation = self.se.storage_generation;
        let recording_revision = self.se.recording_revision;
        if page_id == super::anim_page::PAGE {
            // native page (task 412): replaces the generated stock «Кастомизация» rows
            self.se.rows.clear();
            self.se.on.clear();
            self.se.cf = None;
            self.se.zone = 0;
            super::anim_page::open(self);
            self.render_settings();
            return;
        }
        if page_id == super::internet::PAGE {
            // native page (task 451): the stock Wi-Fi page, not the generated rows
            super::anim_page::leave();
            super::internet::open(self);
            self.render_settings();
            return;
        }
        if page_id == super::bluetooth::PAGE {
            // native page (task 488): the stock Bluetooth page on the in-memory / BlueZ backend
            super::internet::leave(self);
            super::anim_page::leave();
            super::bluetooth::open(self);
            self.render_settings();
            return;
        }
        super::internet::leave(self);
        super::bluetooth::leave(self);
        super::anim_page::leave();
        if page_id == "storage" {
            self.se.storage = None;
            self.se.storage_selected = None;
            self.se.storage_sort_menu = false;
            self.se.zone = 0;
            self.show_storage();
            std::thread::spawn(move || {
                let data = storage::fetch();
                let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| {
                    if response_is_current("storage", generation, &a.se.page_id(), a.se.storage_generation, a.router.current()) {
                        a.se.storage = Some(data); a.show_storage();
                    }
                }); });
            });
            return;
        }
        self.se.recording_menu = None;
        self.se.recording_write = None;
        let rows: Vec<PRow> = serde_json::from_str::<Value>(PAGES_JSON).map(|v| parse_captured_rows(&v[page_id.as_str()])).unwrap_or_default();
        let mut rows = drop_unlocalized(rows, steam_assets::has_loc);
        if page_id == power_page::PAGE {
            // task 466: the stock page is rebuilt natively; controls stay unavailable until the live snapshot arrives
            self.se.snapshot = Value::Null;
            rows = power_page::rows_for(&self.se.snapshot);
        }
        if page_id == "gamerecording" {
            self.se.snapshot = serde_json::json!({"recording_loading": true});
            identify_recording_rows(&mut rows);
        }
        self.se.on = rows.iter().map(|r| if matches!(r.k, 2 | 3) { -1 } else { 0 }).collect();
        self.se.rows = rows;
        self.se.cf = None;
        self.se.vy = 0.0;
        self.apply_values();
        self.push_settings();
        // refresh the live values off the UI thread
        std::thread::spawn(move || {
            let snap = fetch_snapshot(page_id == "gamerecording");
            let _ = slint::invoke_from_event_loop(move || {
                crate::with_app(|a| {
                    if response_is_current(&page_id, generation, &a.se.page_id(), a.se.storage_generation, a.router.current())
                        && recording_snapshot_current(&page_id, recording_revision, a.se.recording_revision) {
                        a.se.snapshot = snap;
                        a.apply_values();
                        a.push_settings();
                    }
                });
            });
        });
    }

    fn show_storage(&mut self) {
        let (mut rows, selected) = match &self.se.storage {
            None => (vec![PRow { k: 12, lab: i18n::tr_ref("settings.loading_the_vault").into(), x: 270., y: 125., w: 650., h: 32., ..Default::default() }], None),
            Some(Err(())) => (vec![PRow { k: 12, lab: i18n::tr_ref("settings.unable_to_obtain_steam_storage_data").into(), x: 270., y: 125., w: 650., h: 32., ..Default::default() }], None),
            Some(Ok(d)) => storage::rows(d, self.se.storage_selected, self.se.storage_sort, self.se.storage_sort_menu),
        };
        // Storage uses the wider Steam settings rail; map the captured content
        // range into the remaining viewport without changing its vertical flow.
        for r in &mut rows {
            r.x = 330. + (r.x - 270.) * 0.90;
            r.w *= 0.90;
        }
        self.se.storage_selected = selected;
        self.se.on = rows.iter().map(|r| (r.tok == format!("storage:{}", selected.unwrap_or(-1))) as i32).collect();
        self.se.rows = rows;
        self.se.vy = 0.;
        self.push_settings();
    }

    /// Toggle states from the snapshot.
    fn apply_values(&mut self) {
        let mut nth = 0usize;
        let mut cur = String::new();
        let mut last_tok = String::new();
        for (i, r) in self.se.rows.iter().enumerate() {
            if r.k == 1 && !r.tok.is_empty() {
                last_tok = r.tok.clone();
                nth = 0;
            }
            if matches!(r.k, 2 | 3) {
                if cur != last_tok {
                    cur = last_tok.clone();
                }
                self.se.on[i] = toggle_state(&self.se.snapshot, binding(&last_tok, nth));
                nth += 1;
            }
        }
    }

    /// Input-driven: a settings change or scroll. Requests artwork for the storage rows now on screen, then renders.
    pub fn push_settings(&mut self) {
        if self.se.page_id() == "gamerecording" && self.se.recording_menu.is_none() {
            let focused = self.se.cf.and_then(|i| self.se.rows.get(i)).map(|r| r.tok.clone());
            self.se.rows = recording_settings::rows_for(&self.se.snapshot);
            self.se.on = self.se.rows.iter().map(|r| if matches!(r.k, 2 | 3) { -1 } else { 0 }).collect();
            self.apply_values();
            self.se.cf = focused.and_then(|t| self.se.rows.iter().position(|r| r.f && r.tok == t));
        }
        if self.se.page_id() == power_page::PAGE && self.se.recording_menu.is_none() {
            let old = self.se.cf.and_then(|i| self.se.rows.get(i)).map(|r| (r.tok.clone(), r.y));
            self.se.rows = power_page::rows_for(&self.se.snapshot);
            self.se.on = vec![0; self.se.rows.len()];
            if self.se.cf.is_some() || self.se.zone == 1 {
                self.se.cf = power_page::refocus(&self.se.rows, old.as_ref().map(|o| o.0.as_str()), old.as_ref().map_or(0., |o| o.1));
            }
            // rows that appeared or went away move everything below them: keep the scroll inside the new content and the focus on screen
            let bottom = 40.0 + self.ui().global::<SettingsState>().get_viewport_h() / SC;
            let maxv = (self.se.rows.iter().map(|r| r.y + r.h).fold(0.0, f32::max) + 40.0 - bottom).max(0.0);
            self.se.vy = self.se.vy.clamp(0.0, maxv);
            self.settings_scroll_to_focus();
        }
        self.request_storage_art();
        self.render_settings();
    }

    /// Requests the artwork of the visible storage rows. Only input may call this (through `push_settings`): a finished
    /// load that started a pass here would evict and reload its own working set without end.
    fn request_storage_art(&mut self) {
        let visible_ids: Vec<u32> = self.se.rows.iter().filter(|r| r.y + r.h >= self.se.vy && r.y <= self.se.vy + 600.)
            .filter_map(|r| r.tok.strip_prefix("storage-art:")?.parse::<u32>().ok()).collect();
        // Apps in the library model use its art; an installed app the bridge hides from the library (task 143: Euro Truck
        // Simulator 2 showed a blank capsule) loads its librarycache header directly.
        let mut visible_art = Vec::new();
        let cache = self.lib.steam_root.join("appcache/librarycache");
        for id in visible_ids {
            match self.gidx.get(&id) {
                Some(&gi) => visible_art.push(gi),
                None => {
                    let art = steam_data::art::librarycache(&cache, &id.to_string());
                    if let Some(path) = art.header.or(art.hero) { self.request_path(id, Kind::Header, path); }
                }
            }
        }
        if crate::diag::enabled() && self.se.page_id() == "storage" {
            let art: Vec<(String, f32)> = self.se.rows.iter().filter(|r| r.tok.starts_with("storage-art:")).map(|r| (r.tok.clone(), r.y)).collect();
            crate::diag::log("storage.art", serde_json::json!({"rows": art.len(), "visible": visible_art.len(), "vy": self.se.vy, "first": art.iter().take(4).collect::<Vec<_>>(), "gidx_known": art.iter().take(4).map(|(t, _)| t.strip_prefix("storage-art:").and_then(|v| v.parse::<u32>().ok()).map(|id| self.gidx.contains_key(&id))).collect::<Vec<_>>()}));
        }
        for gi in visible_art { self.request(gi, Kind::Header); }
    }

    /// Rebuilds the page from the current rows and the images already resident. Requests nothing.
    pub fn render_settings(&mut self) {
        if self.se.page_id() == controller_devices::PAGE && controller_devices::active(self) {
            controller_devices::push(self);
            return;
        }
        if self.se.page_id() == super::internet::PAGE {
            let ui = self.ui();
            ui.global::<SettingsState>().set_is_anim(false);
            recording_settings::render_menu(&self.ui(), &self.se);
            super::internet::push(self);
            return;
        }
        if self.se.page_id() == super::bluetooth::PAGE {
            let ui = self.ui();
            ui.global::<SettingsState>().set_is_anim(false);
            recording_settings::render_menu(&self.ui(), &self.se);
            super::bluetooth::push(self);
            return;
        }
        ui_not_internet(self);
        if self.se.page_id() == super::anim_page::PAGE {
            let ui = self.ui();
            let st = ui.global::<SettingsState>();
            st.set_focus(self.se.focus as i32);
            st.set_cursor(self.se.cursor as i32);
            st.set_zone(self.se.zone as i32);
            st.set_page_title(self.se.pages.get(self.se.focus).map(|p| p.label.as_str()).unwrap_or("").into());
            recording_settings::render_menu(&self.ui(), &self.se);
            super::anim_page::push(self);
            return;
        }
        let mut rows: Vec<GPageRow> = self.se.rows.iter().enumerate().map(|(i, r)| {
            let mut row = to_grow(r, self.se.on.get(i).copied().unwrap_or(if matches!(r.k, 2 | 3) { -1 } else { 0 }));
            if r.k == 5 && r.f { row.label = field_text(&r.id).into(); }
            if self.se.page_id() == "gamerecording" {
                if r.k == 1 { row.label = r.lab.as_str().into(); }
                row.desc = r.desc.as_str().into();
                row.on = recording_settings::available_on(r, &self.se.snapshot, row.on);
                if let Some(mode) = recording_mode(&r.tok) {
                    row.label = recording_mode_label(&r.lab, mode, &self.se.snapshot).into();
                    row.on = if self.se.snapshot["c"]["gamerecording_background_mode"].as_i64() == Some(mode) { 1 } else { row.on };
                }
                if r.tok == "#DeckShell_InstantClipDuration" {
                    row.label = if self.se.snapshot["c"]["gamerecording_background_mode"].as_i64() == Some(1) {
                        self.se.snapshot["c"]["gamerecording_ic_seconds"].as_f64()
                            .filter(|v| v.is_finite() && *v > 0.0 && *v <= 9999.0)
                            .map(|v| i18n::tr_args("settings.sec", &[("value", &(v).to_string())])).unwrap_or_else(|| i18n::tr_ref("settings.unavailable").into())
                    } else { i18n::tr_ref("settings.for_background_recording").into() }.into();
                }
                if r.tok == "#DeckShell_RecordingBitrate" {
                    row.label = match self.se.snapshot["c"]["gamerecording_video_bitrate"].as_str() {
                        Some("preset_ultra") => i18n::tr_ref("settings.ultra"),
                        Some("preset_default") => i18n::tr_ref("settings.high_by_default"),
                        Some("preset_medium") => i18n::tr_ref("settings.medium"),
                        Some("preset_low") => i18n::tr_ref("settings.low"),
                        Some(_) => i18n::tr_ref("settings.more"),
                        None => i18n::tr_ref("settings.unavailable"),
                    }.into();
                }
                if r.tok == "#DeckShell_RecordingDuration" {
                    row.label = self.se.snapshot["c"]["gamerecording_background_max_keep"].as_str()
                        .and_then(recording_duration).map(|minutes| i18n::tr_args("settings.min", &[("minutes", &(minutes).to_string())]))
                        .unwrap_or_else(|| i18n::tr_ref("settings.unavailable").into()).into();
                }
                let key = match r.tok.as_str() {
                    "#DeckShell_RecordingFps" => Some("gamerecording_max_fps"),
                    "#DeckShell_RecordingHeight" => Some("gamerecording_video_maxheight"),
                    _ => None,
                };
                if let Some(key) = key {
                    row.label = self.se.snapshot["c"][key].as_i64().map(|v| {
                        if key.ends_with("maxheight") && v == 0 { i18n::tr_ref("settings.unlimited").to_string() }
                        else if key.ends_with("maxheight") { i18n::tr_args("settings.px", &[("value", &(v).to_string())]) }
                        else { v.to_string() }
                    }).unwrap_or_else(|| i18n::tr_ref("settings.unavailable").into()).into();
                }
            }
            if self.se.page_id() == power_page::PAGE {
                row.desc = r.desc.as_str().into();
                if r.k == 1 { row.label = r.lab.as_str().into(); row.sub = "tight".into(); }
                row.on = power_page::available_on(r, &self.se.snapshot);
                if matches!(r.k, 29 | 30) { let (label, desc) = power_page::card_texts(r); row.shift = label.into(); if r.k == 29 { row.desc = desc.into(); } }
                if r.k == 27 { row.sub = power_page::slider_fill(&self.se.snapshot).into(); row.shift = power_page::slider_tick(&self.se.snapshot).into(); }
            }
            if self.se.page_id() == "gamerecording" && self.se.recording_write.is_some() {
                if matches!(r.k, 2 | 4) { row.on = -1; }
                if r.tok == recording_settings::SAVE_STATUS { row.label = i18n::tr_ref("settings.save_the_settings_on_steam").into(); }
            }
            if self.se.page_id() != "storage" && self.se.page_id() != power_page::PAGE {
                // Reflow the legacy 1.2x content into the space beside the 360px rail.
                row.x = (395.5 + (r.x * SC - 323.6) * 0.93).into();
                row.w = (r.w * SC * 0.93).into();
            }
            if let Some(img) = r.tok.strip_prefix("storage-art:").and_then(|s| s.parse::<u32>().ok()).and_then(|id| self.imgs.get(&(id, Kind::Header))) {
                row.img = img.clone(); row.has_img = true;
            }
            row
        }).collect();
        let option_start = self.se.rows.iter().position(|r| r.tok.starts_with("storage-sort-option:"));
        let mut modal = Vec::new();
        if self.se.storage_sort_menu {
            if let Some(start) = option_start {
                for (offset, row) in rows.iter_mut().skip(start).take(4).enumerate() {
                    let mut item = row.clone();
                    item.kind = 26;
                    item.x = 0.; item.y = (offset as f32 * 72.).into(); item.w = 420.; item.h = 72.;
                    modal.push(item);
                    row.kind = 0;
                }
            }
        }
        let h = self.se.rows.iter().map(|r| r.y + r.h).fold(0.0, f32::max) + 40.0;
        let ui = self.ui();
        let st = ui.global::<SettingsState>();
        recording_settings::render_menu(&self.ui(), &self.se);
        st.set_is_anim(false);
        st.set_is_internet(false);
        st.set_is_recording(matches!(self.se.page_id().as_str(), "gamerecording" | power_page::PAGE));
        st.set_is_storage(self.se.page_id() == "storage");
        st.set_storage_modal(self.se.storage_sort_menu);
        st.set_storage_modal_start(option_start.unwrap_or(0) as i32);
        st.set_storage_modal_focus(self.se.cf.and_then(|i| option_start.and_then(|start| i.checked_sub(start))).map(|i| i as i32).unwrap_or(-1));
        st.set_storage_modal_rows(ModelRc::new(VecModel::from(modal)));
        st.set_focus(self.se.focus as i32);
        st.set_cursor(self.se.cursor as i32);
        st.set_zone(self.se.zone as i32);
        st.set_page_title(self.se.pages.get(self.se.focus).map(|p| p.label.as_str()).unwrap_or("").into());
        st.set_content(ModelRc::new(VecModel::from(rows)));
        st.set_content_h(((h - 40.0) * SC).max(586.0 * SC).into());
        st.set_cfocus(self.se.cf.map(|c| c as i32).unwrap_or(-1));
        st.set_vy((-self.se.vy * SC).into());
    }

    /// Binding path (`c.key` client setting, `f.key` friend setting, `s.key` store setting, `w` wifi) of toggle row `i` and its
    /// current value, using the same token/nth bookkeeping as `apply_values`.
    fn toggle_binding(&self, i: usize) -> Option<(&'static str, usize)> {
        let (mut nth, mut last_tok) = (0usize, String::new());
        for (k, r) in self.se.rows.iter().enumerate() {
            if r.k == 1 && !r.tok.is_empty() {
                last_tok = r.tok.clone();
                nth = 0;
            }
            if matches!(r.k, 2 | 3) {
                if k == i {
                    return binding(&last_tok, nth).map(|p| (p, nth));
                }
                nth += 1;
            }
        }
        None
    }

    /// A / click on a content element. Toggles write through the real setters (settingsStore / SteamClient.Settings, DRYRUN-gated)
    /// and the snapshot is updated so the page keeps the value; buttons run their action.
    pub fn settings_activate(&mut self, i: usize) {
        let Some(r) = self.se.rows.get(i).cloned() else { return };
        if self.se.page_id() == "gamerecording" && self.se.recording_write.is_some() { self.toast(i18n::tr_ref("settings.waiting_to_save_settings_on_steam")); return; }
        if self.se.page_id() == "gamerecording" && self.recording_dropdown(&r) { return; }
        if self.se.page_id() == power_page::PAGE && self.power_activate(&r) { return; }
        if r.tok != "#DeckShell_RecordingDuration" { self.se.pending = None; }
        if self.se.page_id() == "storage" {
            if r.tok == "storage-sort" { self.se.storage_sort_menu = true; self.show_storage(); self.se.cf = self.se.rows.iter().position(|r| r.tok == format!("storage-sort-option:{}", self.se.storage_sort % 3)); self.push_settings(); return; }
            if r.tok == "storage-direction" { self.se.storage_sort = (self.se.storage_sort + 3) % 6; self.show_storage(); return; }
            if let Some(mode) = r.tok.strip_prefix("storage-sort-option:").and_then(|v| v.parse::<usize>().ok()) {
                if mode < 3 { self.se.storage_sort = if mode == 2 { mode + 3 } else { mode }; } self.se.storage_sort_menu = false; self.show_storage(); self.se.cf = self.se.rows.iter().position(|r| r.tok == "storage-sort"); self.push_settings(); return;
            }
            if let Some(id) = r.tok.strip_prefix("storage:").and_then(|v| v.parse().ok()) {
                self.se.storage_selected = Some(id);
                self.show_storage();
            }
            return;
        }
        let dry = if crate::actions::dryrun() { "DRYRUN " } else { "" };
        match r.k {
            2 | 3 => {
                if self.se.page_id() == "gamerecording" && recording_settings::available_on(&r, &self.se.snapshot, self.se.on.get(i).copied().unwrap_or(-1)) < 0 {
                    self.toast(i18n::tr_ref("settings.setup_is_not_available_in_the_current")); return;
                }
                let binding = self.toggle_binding(i);
                let Some(on) = next_toggle_value(self.se.on.get(i).copied().unwrap_or(-1), binding.map(|b| b.0)) else {
                    self.toast(&t_or("#DeckShell_SettingUnavailable", i18n::tr_ref("settings.setup_unavailable_no_confirmed_state_or_processor")));
                    return;
                };
                if self.se.page_id() == "gamerecording" {
                    if let Some((path, _)) = binding {
                        self.recording_toggle(i, path, on);
                    }
                    return;
                }
                self.se.on[i] = i32::from(on);
                match binding {
                    Some((path, _)) => {
                        let (inv, p) = path.strip_prefix('!').map(|p| (true, p)).unwrap_or((false, path));
                        let val = serde_json::Value::Bool(on != inv);
                        if p == "w" {
                            self.act(crate::actions::Act::SetWifi(on));
                            self.se.snapshot["w"] = val;
                        } else if let Some((g, k)) = p.split_once('.') {
                            match g {
                                "c" => {
                                    crate::setters::client_setting(k, &val);
                                }
                                "f" => crate::setters::friend_setting(k, &val),
                                "s" => crate::setters::write("store-setting", format!("settingsStore.settings.{k}={val}")),
                                _ => {}
                            }
                            if self.se.snapshot.get(g).map(|x| x.is_object()).unwrap_or(false) {
                                self.se.snapshot[g][k] = val;
                            }
                        }
                        crate::sound::play(if on { crate::sound::Snd::ToggleOn } else { crate::sound::Snd::ToggleOff });
                    }
                    None => unreachable!("next_toggle_value requires a binding"),
                }
            }
            4 if self.se.page_id() == "gamerecording" && recording_mode(&r.tok).is_some() => {
                let target = recording_mode(&r.tok).unwrap();
                let Some(current @ 0..=2) = self.se.snapshot["c"]["gamerecording_background_mode"].as_i64() else {
                    self.toast(i18n::tr_ref("settings.recording_mode_unavailable")); return;
                };
                if current != target { self.recording_setting("gamerecording_background_mode", Value::from(target)); }
                return;
            }

            4 => self.settings_button(&r),
            5 if r.f => {
                FIELD_ACTIVE.with(|k| *k.borrow_mut() = r.id.clone());
                super::osk::attach(self, FIELD_SINK);
            }
            _ => eprintln!("[deckshell] {dry}settings activate {} '{}' {}", r.role, r.lab, r.tok),
        }
        self.push_settings();
    }

    /// Write once and show only the value read back from Steam, not an optimistic success.
    fn recording_toggle(&mut self, _i: usize, path: &'static str, on: bool) {
        let Some(key) = path.strip_prefix("c.") else { return };
        self.recording_setting(key, Value::Bool(on));
    }

    fn recording_setting(&mut self, key: &'static str, requested: Value) {
        if crate::actions::dryrun() {
            self.toast(i18n::tr_ref("settings.verification_mode_the_setting_is_not_changed"));
            return;
        }
        let Some(encoded) = crate::setters::encode_client_setting(key, &requested) else { return };
        let generation = self.se.storage_generation;
        self.se.recording_revision = self.se.recording_revision.wrapping_add(1);
        self.se.recording_write = Some(key);
        self.apply_values();
        self.push_settings();
        let write_guard = RecordingWriteGuard::begin();
        std::thread::spawn(move || {
            let result = crate::actions::bridge().and_then(|bridge| {
                let write = format!("(async()=>{{await SteamClient.Settings.SetSetting({});return true}})()", crate::setters::js_str(&encoded));
                // A lost reply does not imply that the write failed: issue it only once,
                // then use bounded reads to observe Steam's asynchronous store update.
                let read = format!("({})?.c?.{} ?? null", include_str!("recording_snapshot.js"), key);
                recording_execute(&requested, || {
                    if bridge.eval_json_once(&write).is_err() {
                        eprintln!("[deckshell] recording setting {key}: setter error or reply lost; checking readback");
                    }
                }, || bridge.eval_json(&read).ok(), || std::thread::sleep(std::time::Duration::from_millis(250)))
            });
            eprintln!("[deckshell] recording setting {key}: {}", if result.as_ref().is_some_and(|v| v == &requested || v.as_f64().zip(requested.as_f64()).is_some_and(|(a, b)| a == b)) { "readback confirmed" } else if result.is_some() { "readback differs" } else { "readback unavailable" });
            drop(write_guard);
            let _ = slint::invoke_from_event_loop(move || { crate::with_app(|a| {
                if !response_is_current("gamerecording", generation, &a.se.page_id(), a.se.storage_generation, a.router.current()) { return; }
                a.se.recording_write = None;
                if !result.as_ref().is_some_and(|v| v == &requested || v.as_f64().zip(requested.as_f64()).is_some_and(|(a, b)| a == b)) {
                    a.toast(if result.is_some() { i18n::tr_ref("settings.steam_has_not_confirmed_the_selected_value") } else { i18n::tr_ref("settings.i_couldn_t_read_the_steam_recording") });
                }
                a.se.snapshot["c"][key] = result.unwrap_or(Value::Null);
                a.apply_values();
                a.push_settings();
            }); });
        });
    }

    /// Buttons with a known Steam call. Destructive ones need a second press.
    fn settings_button(&mut self, r: &PRow) {
        let key = if r.tok.is_empty() { r.lab.clone() } else { r.tok.clone() };
        let confirm = |a: &mut App, what: &str| -> bool {
            if a.se.pending.as_deref() == Some(what) {
                a.se.pending = None;
                true
            } else {
                a.se.pending = Some(what.to_string());
                a.toast(i18n::tr_ref("settings.click_again_for_confirmation"));
                false
            }
        };
        let lab = if i18n::current().code() == "ru" { &r.full } else { &r.lab }.to_lowercase();
        let russian = i18n::by_code("ru").expect("Russian locale");
        let contains_label = |key| lab.contains(i18n::tr_ref(key)) || lab.contains(&russian.tr(key));
        if key.contains("GoOffline") || contains_label("settings.offline_mode") {
            if confirm(self, "offline") {
                crate::setters::write("go-offline", "SteamClient.User.GoOffline()".into());
            }
        } else if contains_label("settings.delete_browser_data") || key.contains("ClearBrowser") {
            if confirm(self, "http-cache") {
                crate::setters::write("clear-http-cache", "SteamClient.Settings.ClearAllHTTPCaches()".into());
            }
        } else if key.contains("ClearDownloadCache") {
            if confirm(self, "dl-cache") {
                crate::setters::write("clear-download-cache", "SteamClient.Settings.ClearDownloadCache()".into());
            }
        } else if key.contains("Settings_Internet_Custom_Network") {
            self.act(crate::actions::Act::Open(steam_control::SteamPage::Settings));
        } else if key.contains("CheckForUpdates") || contains_label("settings.check_out") {
            crate::setters::write("check-updates", "SteamClient.System.CheckForUpdates()".into());
            self.toast(i18n::tr_ref("settings.checking_for_updates"));
        } else if key.contains("Settings_Keyboard_Preview") || contains_label("settings.keyboard") {
            // предпросмотр клавиатуры: ввод идёт в первое тестовое поле страницы
            FIELD_ACTIVE.with(|k| *k.borrow_mut() = "1".into());
            super::osk::attach(self, FIELD_SINK);
        } else if key.contains("Settings_Keyboard_PointsShop_Visit") || contains_label("settings.point_shop") {
            self.act(crate::actions::Act::Open(steam_control::SteamPage::Store));
        } else if key.contains("ThirdPartyLicenses") || contains_label("settings.third_party_license") || key.contains("Settings_SRSI_Run") {
            crate::setters::write("open-runtimeinfo", "SteamClient.System.OpenRuntimeInfo()".into());
        } else if key.contains("Settings_SystemReport_Start") || contains_label("settings.report") {
            // Steam's sp.js:464507 checks the native result, decodes the reply,
            // and fetches the report text. We cannot present that flow yet.
            self.se.pending = None;
            self.toast(system_report_unavailable());
        } else if key.contains("Settings_System_Factory_Reset") || contains_label("settings.factory_setting") {
            // Native sp.js:349172 uses the service request with explicit reset_os
            // and reset_user_data options. A guessed no-argument client method
            // plus "press twice" is not that flow. Keep it unavailable.
            self.se.pending = None;
            self.toast(&t_or("#DeckShell_ActionUnavailable", i18n::tr_ref("settings.the_team_is_not_yet_available_on")));
        } else {
            self.se.pending = None;
            eprintln!("[deckshell] {}settings button '{}' ({}) has no native equivalent wired", if crate::actions::dryrun() { "DRYRUN " } else { "" }, r.lab, r.tok);
            self.toast(&t_or("#DeckShell_ActionUnavailable", i18n::tr_ref("settings.the_team_is_not_yet_available_on")));
        }
    }

    /// B inside page content returns to the rail first (sem Settings), then leaves.
    pub fn settings_back(&mut self) -> bool {
        if controller_devices::active(self) && controller_devices::back(self) { return true; }
        self.se.pending = None;
        if self.se.page_id() == power_page::PAGE { self.power_flush(); }
        if self.se.recording_menu.is_some() { self.close_recording_menu(); return true; }
        if self.se.zone == 1 && self.se.page_id() == super::anim_page::PAGE && super::anim_page::back(self) { return true; }
        if self.se.zone == 1 && self.se.page_id() == super::internet::PAGE && super::internet::back(self) { return true; }
        if self.se.zone == 1 && self.se.page_id() == super::bluetooth::PAGE && super::bluetooth::back(self) { return true; }
        if self.se.zone == 1 && self.se.storage_sort_menu {
            self.se.storage_sort_menu = false; self.show_storage(); self.se.cf = self.se.rows.iter().position(|r| r.tok == "storage-sort"); self.push_settings(); return true;
        }
        if self.se.zone == 1 {
            self.se.zone = 0;
            self.se.cursor = self.se.focus;
            self.se.cf = None;
            self.push_settings();
            return true;
        }
        false
    }

    fn settings_scroll_to_focus(&mut self) {
        if let Some(c) = self.se.cf.and_then(|c| self.se.rows.get(c)) {
            // Match the clipped viewport and retain the existing scroll margins.
            let bottom = 40.0 + self.ui().global::<SettingsState>().get_viewport_h() / SC;
            let top = c.y - self.se.vy;
            let bot = c.y + c.h - self.se.vy;
            if top < 40.0 + 20.0 {
                self.se.vy = (c.y - 60.0).max(0.0);
            } else if bot > bottom - 55.0 {
                self.se.vy = c.y + c.h - (bottom - 55.0);
            }
            let maxv = (self.se.rows.iter().map(|r| r.y + r.h).fold(0.0, f32::max) + 40.0 - bottom).max(0.0);
            self.se.vy = self.se.vy.clamp(0.0, maxv);
        }
    }

    pub fn handle_settings(&mut self, a: Action) {
        if a != Action::A && self.se.recording_menu.is_none() { self.se.pending = None; }
        let n = self.se.pages.len();
        if self.se.zone == 0 {
            match rail_step(self.se.cursor, self.se.focus, n, a) {
                RailStep::Move(c) => {
                    self.se.cursor = c;
                    self.push_settings();
                }
                RailStep::Enter { commit } => {
                    if commit {
                        let p = self.se.pages.get(self.se.cursor).map(|p| p.id.to_lowercase()).unwrap_or_default();
                        self.se.focus = self.se.cursor;
                        self.router.replace(Route::Settings(p.clone()));
                        self.sync_settings(&p);
                    }
                    if controller_devices::active(self) {
                        controller_devices::enter(self);
                        return;
                    }
                    if self.se.page_id() == super::internet::PAGE {
                        super::internet::enter(self);
                        self.render_settings();
                        return;
                    }
                    if self.se.page_id() == super::bluetooth::PAGE {
                        super::bluetooth::enter(self);
                        self.render_settings();
                        return;
                    }
                    if self.se.page_id() == super::anim_page::PAGE {
                        super::anim_page::enter(self);
                        self.render_settings();
                        return;
                    }
                    // into the page: first focusable control (storage fills its rows asynchronously)
                    if let Some(i) = self.se.rows.iter().position(|r| r.f) {
                        self.se.zone = 1;
                        self.se.cf = Some(i);
                        self.settings_scroll_to_focus();
                        self.push_settings();
                    }
                }
                RailStep::Nothing => {}
            }
            return;
        }
        if controller_devices::active(self) { return controller_devices::handle(self, a); }
        if self.se.page_id() == super::internet::PAGE {
            return super::internet::handle(self, a);
        }
        if self.se.page_id() == super::bluetooth::PAGE {
            return super::bluetooth::handle(self, a);
        }
        if self.se.page_id() == super::anim_page::PAGE {
            return super::anim_page::handle(self, a);
        }
        let cur = self.se.cf.unwrap_or(0);
        if self.se.recording_menu.is_some() {
            let start = self.se.rows.iter().position(|r| r.tok.starts_with("recording-option:")).unwrap_or(cur);
            let len = self.se.rows.len() - start;
            match a {
                Action::Up | Action::Down => self.se.cf = Some(start + sort_menu_step(cur.saturating_sub(start), len, a == Action::Down)),
                Action::A => return self.settings_activate(cur),
                _ => return,
            }
            self.push_settings(); return;
        }
        if self.se.storage_sort_menu {
            let options: Vec<usize> = self.se.rows.iter().enumerate().filter_map(|(i, r)| r.tok.starts_with("storage-sort-option:").then_some(i)).collect();
            let pos = options.iter().position(|i| *i == cur).unwrap_or(0);
            match a {
                Action::Up => self.se.cf = options.get(sort_menu_step(pos, options.len(), false)).copied(),
                Action::Down => self.se.cf = options.get(sort_menu_step(pos, options.len(), true)).copied(),
                Action::A => return self.settings_activate(cur),
                _ => return,
            }
            self.push_settings(); return;
        }
        if self.se.page_id() == power_page::PAGE {
            if self.power_slider_step(a) { return; }
            self.power_flush();
        }
        let dir = match a {
            Action::Up => 0,
            Action::Down => 1,
            Action::Left => 2,
            Action::Right => 3,
            Action::A => return self.settings_activate(cur),
            _ => return,
        };
        match spatial(&self.se.rows, cur, dir) {
            Some(n) => self.se.cf = Some(n),
            None if dir == 2 => self.se.zone = 0,
            None => return,
        }
        self.settings_scroll_to_focus();
        self.push_settings();
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_touch_stays_in_the_column_where_it_started() {
        use std::{cell::Cell, rc::Rc, time::Duration};

        use i_slint_backend_testing::{mock_elapsed_time, TestingBackend, TestingBackendOptions};
        use i_slint_core::{input::TouchPhase, lengths::LogicalPoint};
        use slint::platform::{InternalEvent, WindowEvent};

        slint::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            ..Default::default()
        }))).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        ui.global::<crate::Shell>().set_page(4);
        let settings = ui.global::<SettingsState>();
        settings.set_rail(ModelRc::new(VecModel::from((0..22).map(|i| ListEntry {
            label: format!("Category {i}").into(),
            ..Default::default()
        }).collect::<Vec<_>>())));
        settings.set_content_h(1500.0);
        settings.set_content(ModelRc::new(VecModel::from(vec![GPageRow {
            kind: 4, x: 396.0, y: 132.0, w: 200.0, h: 60.0, ptr: true, fcs: true,
            label: "Content button".into(),
            ..Default::default()
        }])));

        let category = Rc::new(Cell::new(None));
        let content_click = Rc::new(Cell::new(None));
        settings.on_activate({
            let category = category.clone();
            move |i| category.set(Some(i))
        });
        settings.on_content_activated({
            let content_click = content_click.clone();
            move |i| content_click.set(Some(i))
        });

        let settle = |ms| {
            mock_elapsed_time(Duration::from_millis(ms));
            slint::platform::update_timers_and_animations();
            ui.window().take_snapshot().unwrap();
        };
        settle(0);
        settle(1000);
        let touch = |phase, x, y| {
            ui.window().dispatch_event(WindowEvent::internal(InternalEvent::Touch {
                id: 1, position: LogicalPoint::new(x, y), phase,
            }));
        };
        let tap = |x, y| {
            touch(TouchPhase::Started, x, y);
            touch(TouchPhase::Ended, x, y);
        };
        let swipe_up = |start_x, end_x| {
            touch(TouchPhase::Started, start_x, 680.0);
            settle(16);
            touch(TouchPhase::Moved, start_x, 600.0);
            settle(16);
            touch(TouchPhase::Moved, end_x, 120.0);
            settle(120);
            touch(TouchPhase::Ended, end_x, 120.0);
            settle(250);
        };

        swipe_up(350.0, 370.0);
        assert_eq!(settings.get_vy(), 0.0, "a swipe from the rail moved the content");
        swipe_up(350.0, 350.0);
        assert_eq!(category.get(), None, "a swipe activated a category");
        tap(350.0, 710.0);
        assert_eq!(category.take(), Some(21), "the last category is reachable by touch");

        tap(410.0, 150.0);
        assert_eq!(content_click.take(), Some(0), "the content button kept its screen coordinates");
        swipe_up(370.0, 350.0);
        assert!(settings.get_vy() < 0.0, "a swipe from the content did not scroll it");
        assert_eq!(category.get(), None, "a swipe from the content activated the rail");
        tap(350.0, 710.0);
        assert_eq!(category.take(), Some(21), "a swipe from the content moved the rail");

        settings.set_cursor(1);
        settle(0);
        tap(350.0, 90.0);
        assert_eq!(category.take(), Some(1), "the rail did not reveal the gamepad cursor");
    }

    #[test]
    fn system_report_outcomes() {
        // This action has no dispatch or success/error outcome until the full
        // report flow is implemented; every activation reports unavailability.
        assert_eq!(system_report_unavailable(), i18n::tr_ref("settings.system_reporting_not_available_on_deckshell"));
    }

    #[test]
    fn latest_request_wins_even_after_returning_to_same_page() {
        let mut applied = json!({"value": "current"});
        // A(1) -> B(2) -> A(3). A(1) has the same page name but an old epoch.
        for (page, generation, value) in [("audio", 3, "latest"), ("display", 2, "stale-b"), ("audio", 1, "stale-a")] {
            if response_is_current(page, generation, "audio", 3, &Route::Settings("audio".into())) {
                applied = json!({"value": value});
            }
        }
        assert_eq!(applied["value"], "latest");
        assert!(!response_is_current("audio", 3, "audio", 3, &Route::Home));
        assert!(!response_is_current("storage", 3, "storage", 3, &Route::App(42)));
        assert!(response_is_current("storage", 3, "storage", 3, &Route::Settings("storage".into())));
    }

    #[test]
    fn missing_or_unbound_toggle_never_becomes_fake_off_or_a_local_write() {
        let live = json!({"c": {"known": false, "enabled": true, "wrong_type": 0}});
        assert_eq!(toggle_state(&live, Some("c.known")), 0);
        assert_eq!(toggle_state(&live, Some("!c.known")), 1);
        assert_eq!(toggle_state(&live, Some("c.enabled")), 1);
        for (snapshot, path) in [(&live, None), (&live, Some("c.missing")), (&live, Some("c.wrong_type")), (&Value::Null, Some("c.known"))] {
            let state = toggle_state(snapshot, path);
            assert_eq!(state, -1);
            assert_eq!(next_toggle_value(state, path), None);
        }
        assert_eq!(next_toggle_value(0, None), None);
        assert_eq!(next_toggle_value(0, Some("c.known")), Some(true));
        assert_eq!(next_toggle_value(1, Some("c.known")), Some(false));
    }
}

#[cfg(test)]
mod rail_focus_tests {
    use super::*;

    /// Focus table for the page list (cursor, shown page, page count, button) -> step. Source: Steam PagedSettings
    /// (steamui-snapshot sp.js module 91371, `O` and `v`): Up/Down move the list, A clicks the item (page change) and
    /// enters the content column, Right enters the shown page, B is not consumed by the list (app back / content B returns).
    #[test]
    fn rail_focus_table() {
        use RailStep::*;
        let table = [
            (0, 0, 5, Action::Up, Move(0)),
            (0, 0, 5, Action::Down, Move(1)),
            (2, 0, 5, Action::Up, Move(1)),
            (4, 0, 5, Action::Down, Move(4)),
            (1, 0, 5, Action::Down, Move(2)),
            (0, 0, 5, Action::A, Enter { commit: false }),
            (3, 0, 5, Action::A, Enter { commit: true }),
            (3, 3, 5, Action::A, Enter { commit: false }),
            (3, 0, 5, Action::Right, Enter { commit: false }),
            (3, 0, 5, Action::B, Nothing),
            (3, 0, 5, Action::Left, Nothing),
            (0, 0, 0, Action::Down, Move(0)),
        ];
        for (cursor, focus, n, a, want) in table {
            let got = rail_step(cursor, focus, n, a);
            assert_eq!(got, want, "cursor={cursor} focus={focus} n={n}");
        }
    }

    /// Layout lint over the generated page geometry (pages.json, css px). Checks: every row has a positive size and
    /// lies inside the content column (x 240..1068 css); focusable rows never partially overlap another focusable row
    /// (containment is allowed: Steam nests toggles and child buttons inside container rows). The storage page is excluded.
    #[test]
    fn unlocalized_label_rows_are_hidden_not_shown_as_tokens() {
        let row = |k: i32, tok: &str, lab: &str| PRow { k, tok: tok.into(), lab: lab.into(), ..Default::default() };
        let known = |t: &str| t == "#Settings_Updates_CheckForUpdates";
        // the stale SteamRT row: a label token that the running Steam does not localize, no captured text
        assert!(unlocalized_label(&row(1, "#Settings_Interface_SteamRTClient", ""), known));
        // a localized token, a captured label, and non-label rows stay
        assert!(!unlocalized_label(&row(1, "#Settings_Updates_CheckForUpdates", ""), known));
        assert!(!unlocalized_label(&row(1, "#Settings_Interface_SteamRTClient", "Captured text"), known));
        assert!(!unlocalized_label(&row(4, "#Settings_Interface_SteamRTClient", ""), known));
        assert!(!unlocalized_label(&row(1, "", ""), known));
        // the toggle that sits inside the hidden label row goes with it; a toggle elsewhere stays
        let mut toggle = row(2, "", "");
        toggle.x = 984.0; toggle.y = 432.0; toggle.w = 38.0; toggle.h = 22.0;
        let mut label = row(1, "#Settings_Interface_SteamRTClient", "");
        label.x = 270.0; label.y = 420.0; label.w = 768.0; label.h = 46.0;
        let mut other = toggle.clone();
        other.y = 600.0;
        let kept = drop_unlocalized(vec![label, toggle, other.clone()], known);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].y, other.y);
    }

    /// Ratchet for a known layout defect (task 135, open): a k=4 container with a text label that also holds a nested
    /// control renders its label centered across the whole row (rows.slint), so the text runs under the control.
    /// Stock lays out title/description left and the control right. Fixing it needs the generator to split the label
    /// (tools/compile-ui/pages.mjs). Until then the count may not grow.
    #[test]
    fn nested_label_containers_do_not_grow() {
        const KNOWN_NESTED_LABEL_CONTAINERS: usize = 26;
        let v: Value = serde_json::from_str(PAGES_JSON).expect("pages.json");
        let mut hits = Vec::new();
        for (page, arr) in v.as_object().expect("object") {
            if page.starts_with('_') || page == "storage" {
                continue;
            }
            let rows = parse_rows(arr);
            for r in rows.iter().filter(|r| r.k == 4 && r.tok.is_empty() && !r.lab.is_empty()) {
                let nested = |c: &PRow| c.x >= r.x && c.y >= r.y && c.x + c.w <= r.x + r.w && c.y + c.h <= r.y + r.h && c.w * c.h < r.w * r.h;
                if rows.iter().any(nested) {
                    hits.push(format!("{page}: {:?}", r.lab.chars().take(40).collect::<String>()));
                }
            }
        }
        assert!(hits.len() <= KNOWN_NESTED_LABEL_CONTAINERS, "new nested-label container(s); {} found:\n{}", hits.len(), hits.join("\n"));
    }

    #[test]
    fn generated_page_layout_lint() {
        let v: Value = serde_json::from_str(PAGES_JSON).expect("pages.json");
        let mut problems = Vec::new();
        for (page, arr) in v.as_object().expect("object") {
            // `storage` is built at runtime by storage.rs (sync_settings returns before reading these rows), so its captured
            // geometry is not what the user sees; it is linted with the Storage screen instead.
            if page.starts_with('_') || page == "storage" {
                continue;
            }
            let rows = parse_rows(arr);
            for r in &rows {
                if !(r.w > 0.0 && r.h > 0.0 && r.x.is_finite() && r.y.is_finite()) {
                    problems.push(format!("{page}: non-positive or non-finite row {r:?}"));
                }
                if r.x < 240.0 - 0.5 || r.x + r.w > 1068.0 + 0.5 {
                    problems.push(format!("{page}: row outside content column x={} w={} tok={}", r.x, r.w, r.tok));
                }
            }
            let fo: Vec<&PRow> = rows.iter().filter(|r| r.f).collect();
            for (i, a) in fo.iter().enumerate() {
                for b in &fo[i + 1..] {
                    let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                    let nested = |p: &PRow, q: &PRow| p.x <= q.x && p.y <= q.y && q.x + q.w <= p.x + p.w && q.y + q.h <= p.y + p.h;
                    if overlap && !nested(a, b) && !nested(b, a) {
                        problems.push(format!("{page}: focusable rows partially overlap: {:?} / {:?}", a.lab, b.lab));
                    }
                }
            }
        }
        assert!(problems.is_empty(), "layout lint: {} problem(s):\n{}", problems.len(), problems.join("\n"));
    }

    #[test]
    fn english_captured_settings_use_localized_display_text() {
        if i18n::current().code() != "en" { return; }
        let pages: Value = serde_json::from_str(PAGES_JSON).unwrap();
        for (page, rows) in pages.as_object().unwrap() {
            if matches!(page.as_str(), "internet" | "storage" | "power" | "gamerecording" | "controller") {
                continue;
            }
            for (index, row) in parse_captured_rows(rows).iter().enumerate() {
                let row = to_grow(row, 0);
                for text in [&row.label, &row.desc, &row.sub] {
                    assert!(!text.chars().any(|c| ('\u{0400}'..='\u{052f}').contains(&c)),
                        "{page} row {index} contains an untranslated label");
                }
            }
        }
    }

    /// Task 143: on Система the status label «Обновления ПО / Все обновления установлены…» sat centred across the whole
    /// row and ran under «Проверить обновления» (Deck build a7777085, g04-rail). The label must end before the control's
    /// left edge, and the two must not share a vertical band.
    #[test]
    fn system_status_label_ends_before_update_button() {
        let v: Value = serde_json::from_str(PAGES_JSON).expect("pages.json");
        let rows = parse_rows(&v["system"]);
        let label = rows.iter().find(|r| r.k == 1 && r.lab == i18n::tr_ref("settings.capture.system.3.label")).expect("status label row");
        let button = rows.iter().find(|r| r.k == 4 && r.tok == "#Settings_Updates_CheckForUpdates").expect("check-updates button");
        assert!(label.x + label.w <= button.x, "label ends at {} but the button starts at {}", label.x + label.w, button.x);
        // the label stays on the page's column and keeps its description
        assert!(label.x >= 240.0 && label.w > 300.0 && !label.desc.is_empty(), "label geometry {label:?}");
        // no focusable row on the page is partly covered by another row (the generic lint, scoped to this page)
        let fo: Vec<&PRow> = rows.iter().filter(|r| r.f).collect();
        for a in &fo {
            for b in &fo {
                if std::ptr::eq(*a, *b) { continue; }
                let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                let nested = |p: &PRow, q: &PRow| p.x <= q.x && p.y <= q.y && q.x + q.w <= p.x + p.w && q.y + q.h <= p.y + p.h;
                assert!(!overlap || nested(a, b) || nested(b, a), "focusable rows overlap: {:?} / {:?}", a.lab, b.lab);
            }
        }
    }
}

#[cfg(test)]
mod sort_menu_focus_tests {
    use super::sort_menu_step;

    /// Focus table for the storage sort dialog (option count 4: alphabetical, last played, disk size, cancel).
    /// Up/Down move one option and clamp at the first and last; the dialog is a list, so no column moves apply.
    #[test]
    fn sort_menu_focus_table() {
        // (pos, len, direction, expected option index)
        let table = [
            (0, 4, false, 0), // Up on the first option stays
            (0, 4, true, 1),
            (1, 4, false, 0),
            (2, 4, true, 3),  // into Cancel
            (3, 4, true, 3),  // Down on Cancel stays
            (3, 4, false, 2),
            (0, 1, true, 0),  // single option never moves
            (0, 1, false, 0),
        ];
        for (pos, len, down, want) in table {
            assert_eq!(sort_menu_step(pos, len, down), want, "pos {pos} len {len} down {down}");
        }
    }
}

#[cfg(test)]
mod recording_setting_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recording_duration_uses_steam_minute_format_and_safe_bounds() {
        assert_eq!(recording_duration("5min"), Some(5));
        assert_eq!(recording_duration("120min"), Some(120));
        for invalid in ["", "4min", "10000min", "infinite", "5minxyz", "-5min"] {
            assert_eq!(recording_duration(invalid), None, "{invalid}");
        }
        assert_eq!(next_recording_duration(60), 120);
        assert_eq!(next_recording_duration(47), 60);
        assert_eq!(next_recording_duration(480), 5);
        assert!(crate::setters::encode_client_setting("gamerecording_background_max_keep", &json!("120min")).is_some());
    }

    #[test]
    fn captured_rows_connect_each_control_to_the_corresponding_steam_field() {
        let pages: Value = serde_json::from_str(PAGES_JSON).unwrap();
        let mut rows = parse_rows(&pages["gamerecording"]);
        identify_recording_rows(&mut rows);
        for (label, expected) in [
            (recording_settings::steam_text("Settings_GameRecording_AudioRecording_Mic"), "c.gamerecording_background_a_m"),
            (recording_settings::steam_text("Settings_GameRecording_AudioRecording_MicMono"), "c.gamerecording_force_mic_mono"),
            (recording_settings::steam_text("Settings_GameRecording_AudioRecording_AGC"), "c.gamerecording_automatic_gain_control"),
        ] {
            let index = rows.iter().position(|r| r.lab.starts_with(&label)).unwrap();
            assert!(matches!(rows[index + 1].k, 2 | 3));
            assert_eq!(binding(&rows[index].tok, 0), Some(expected));
        }
        assert_eq!(rows.iter().filter(|r| r.tok == "#DeckShell_RecordingAudio").count(), 0);
        let bitrate = rows.iter().find(|r| r.tok == "#DeckShell_RecordingBitrate").unwrap();
        assert!(bitrate.f);
        assert_eq!(rows.iter().filter(|r| r.tok == "#DeckShell_RecordingFps").count(), 1);
        assert_eq!(rows.iter().filter(|r| r.tok == "#DeckShell_RecordingHeight").count(), 1);
    }

    #[test]
    fn recording_modes_use_steam_values_and_render_only_confirmed_selection() {
        let pages: Value = serde_json::from_str(PAGES_JSON).unwrap();
        let mut rows = parse_rows(&pages["gamerecording"]);
        identify_recording_rows(&mut rows);
        for mode in 0..=2 {
            let token = format!("#DeckShell_RecordingMode{mode}");
            let row = rows.iter().find(|r| r.tok == token).unwrap();
            assert!(row.f);
            assert_eq!(recording_mode(&row.tok), Some(mode));
            assert!(recording_mode_label(&row.lab, mode, &json!({"c":{"gamerecording_background_mode": mode}})).starts_with('●'));
            assert!(recording_mode_label(&row.lab, mode, &Value::Null).contains(i18n::tr_ref("settings.unavailable_2")));
            assert!(crate::setters::encode_client_setting("gamerecording_background_mode", &json!(mode)).is_some());
        }
        let manual = rows.iter().find(|r| r.tok == "#DeckShell_RecordingMode2").unwrap();
        let faq = rows.iter().find(|r| r.lab == i18n::tr_ref("recording_settings.on_the_recording_settings")).unwrap();
        assert!(manual.y + manual.h <= faq.y);
    }

    #[test]
    fn instant_clip_duration_requires_background_mode_and_uses_steam_float() {
        let mut snapshot = json!({"c":{"gamerecording_background_mode":1,"gamerecording_ic_seconds":20}});
        assert_eq!(next_instant_clip_duration(&snapshot), Some(30.0));
        assert!(crate::setters::encode_client_setting("gamerecording_ic_seconds", &json!(30.0)).is_some());
        snapshot["c"]["gamerecording_ic_seconds"] = json!(300);
        assert_eq!(next_instant_clip_duration(&snapshot), Some(10.0));
        snapshot["c"]["gamerecording_background_mode"] = json!(2);
        assert_eq!(next_instant_clip_duration(&snapshot), None);
        assert_eq!(recording_readback(&json!(30.0), 1, || Some(json!(30)), || {}), Some(json!(30)));
        let pages: Value = serde_json::from_str(PAGES_JSON).unwrap();
        let mut rows = parse_rows(&pages["gamerecording"]);
        identify_recording_rows(&mut rows);
        assert!(rows.iter().any(|r| r.tok == "#DeckShell_InstantClipDuration" && r.f));
    }

    #[test]
    fn selecting_video_options_encodes_actual_steam_values() {
        for (token, current, key, value, encoded) in [
            ("#DeckShell_RecordingFps", 60, "gamerecording_max_fps", 30, "sPIIHg=="),
            ("#DeckShell_RecordingHeight", 0, "gamerecording_video_maxheight", 1440, "iPMIoAs="),
        ] {
            let (actual_key, next) = next_recording_choice(token, current).unwrap();
            assert_eq!((actual_key, next), (key, value));
            assert_eq!(crate::setters::encode_client_setting(actual_key, &json!(next)).as_deref(), Some(encoded));
        }
        assert_eq!(next_recording_bitrate("preset_ultra"), Some("preset_default"));
        assert_eq!(next_recording_bitrate("preset_low"), Some("preset_ultra"));
        assert_eq!(crate::setters::encode_client_setting("gamerecording_video_bitrate", &json!(next_recording_bitrate("preset_default").unwrap())).as_deref(), Some("kvIIDXByZXNldF9tZWRpdW0="));
        assert_eq!(next_recording_bitrate("custom_12000"), None);
        assert_eq!(next_recording_bitrate(""), None);
        assert_eq!(next_recording_choice("#DeckShell_RecordingHeight", 480).unwrap().1, 0);
        assert!(next_recording_choice("unknown", 60).is_none());
    }

    #[test]
    fn hidden_audio_source_keeps_steam_wire_contract() {
        assert_eq!(crate::setters::encode_client_setting("gamerecording_background_audio", &json!(0)).as_deref(), Some("qPIIAA=="));
    }

    #[test]
    fn readback_waits_for_update_and_preserves_observed_value_on_timeout() {
        let mut values = [Some(json!(60)), None, Some(json!(30))].into_iter();
        let mut waits = 0;
        let mut writes = 0;
        let result = recording_execute(&json!(30), || writes += 1, || values.next().unwrap(), || waits += 1);
        assert_eq!(writes, 1);
        assert_eq!(result, Some(json!(30)));
        assert_eq!(waits, 2);
        let mut values = [Some(json!(60)), None, Some(Value::Null)].into_iter();
        assert_eq!(recording_readback(&json!(30), 3, || values.next().unwrap(), || {}), Some(json!(60)));
        assert_eq!(recording_readback(&json!(true), 2, || None, || {}), None);
    }
}

#[cfg(test)]
mod field_tests {
    use super::*;
    use crate::input::Action;

    #[test]
    fn keyboard_page_test_field_opens_keyboard_takes_input_and_closes() {
        let (ui, mut a) = crate::screens::osk::field_tests::app();
        let v: Value = serde_json::from_str(PAGES_JSON).unwrap();
        a.se.rows = parse_rows(&v["keyboard"]);
        let fields: Vec<usize> = a.se.rows.iter().enumerate().filter(|(_, r)| r.k == 5 && r.f).map(|(i, _)| i).collect();
        assert_eq!(fields.len(), 8, "Eight test fields of the Keyboard page are focused");
        a.settings_activate(fields[1]);
        assert!(a.osk_is_open(), "A/Tap on the field opens the keyboard");
        a.handle(Action::Char('h'));
        a.handle(Action::Char('i'));
        assert_eq!(field_text("2"), "hi");
        assert_eq!(field_text("1"), "");
        crate::screens::osk::field_tests::shot(&ui, "settings-test-field");
        a.handle(Action::B);
        assert!(!a.osk_is_open());
        a.settings_activate(fields[0]);
        a.handle(Action::Char('x'));
        assert_eq!(field_text("1"), "x", "The other field is your own text");
        assert_eq!(field_text("2"), "hi");
    }
}

/// Leaves the native Wi-Fi page layer when another page is rendered.
fn ui_not_internet(a: &App) { a.ui().global::<SettingsState>().set_is_internet(false); }
