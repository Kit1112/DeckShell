//! Typed views over the trimmed JSON returned by the `js` expressions.
use crate::error::{Error, Result};
use serde_json::Value;

pub const APP_TYPE_GAME: u32 = 1;
pub const APP_TYPE_APPLICATION: u32 = 2;
pub const APP_TYPE_TOOL: u32 = 4;
pub const APP_TYPE_DEMO: u32 = 8;
pub const APP_TYPE_SHORTCUT: u32 = 1_073_741_824;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct App {
    pub appid: u32,
    pub display_name: String,
    pub sort_as: String,
    pub installed: bool,
    pub size_on_disk: u64,
    /// Total playtime, minutes.
    pub playtime_minutes: u32,
    pub playtime_2weeks_minutes: u32,
    /// Unix seconds, 0 = never.
    pub last_played: u64,
    pub app_type: u32,
    /// local_per_client_data.display_status (11 = installed/ready; see Steam EAppDisplayStatus).
    pub display_status: u32,
    /// local_per_client_data.status_percentage.
    pub status_percentage: u32,
    pub is_shortcut: bool,
    /// 64-bit game id as string (differs from appid for non-Steam shortcuts).
    pub game_id: String,
    pub visible: bool,
    pub deck_compat_packed: u32,
    pub review_percentage: u32,
    pub release_date: u64,
    pub recent_activity_time: u64,
}

impl App {
    pub fn is_game(&self) -> bool {
        self.app_type == APP_TYPE_GAME || self.is_shortcut
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub apps: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DownloadOverview {
    pub state: String,
    pub appid: u32,
    pub net_bytes_per_sec: u64,
    /// update_peak_network_bytes_per_second.
    pub peak_net_bytes_per_sec: u64,
    pub disc_bytes_per_sec: u64,
    pub is_upload: bool,
    /// `paused` of the overview: all downloads disabled (EnableAllDownloads(false)). None = Steam did not report it.
    pub paused: Option<bool>,
    /// Bandwidth limit temporarily overridden (SuspendDownloadThrottling).
    pub throttling_suspended: Option<bool>,
    pub lan_peer_hostname: Option<String>,
    /// overall_percent_complete / overall_estimated_time_remaining_sec of the app in `appid`.
    pub percent: Option<f64>,
    pub eta_sec: Option<i64>,
    /// progress[k_EAppUpdateProgress_Download] of the active update.
    pub dl_bytes: Option<u64>,
    pub dl_total: Option<u64>,
    /// update_is_shader / update_is_workshop: the active update is a shader cache / workshop content.
    pub is_shader: bool,
    pub is_workshop: bool,
}

/// One DownloadHistory sample (one per second): rt_sample_time (Unix s), network and disc bytes/s.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistorySample {
    pub time: f64,
    pub net: u64,
    pub disc: u64,
    pub upload: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DownloadItem {
    pub appid: u32,
    pub active: bool,
    pub paused: bool,
    pub completed: bool,
    pub queue_index: i32,
    pub percent: f64,
    pub eta_sec: i64,
    /// Stock H(): sum of progress[k_EAppUpdateProgress_Download = 2] over update_type_info (0 when Steam sent no table).
    pub bytes: u64,
    pub total: u64,
    pub launch_on_completion: bool,
    /// Steam's display name (appStore overview); None when the store does not know the app.
    pub name: Option<String>,
    /// Unix time of a scheduled update, 0 = not scheduled.
    pub deferred_time: i64,
    pub completed_time: i64,
    /// EAppUpdateError; None = not reported. 0, 2 and 4 are not errors in the stock list (sp.js Re()).
    pub update_result: Option<i64>,
    pub update_error: Option<String>,
    /// appDetailsStore eAutoUpdateValue (1 = only update on launch); None = details not loaded.
    pub auto_update: Option<i64>,
    /// local_per_client_data.update_available_but_disabled_by_app; None = not reported.
    pub disabled_by_app: Option<bool>,
    /// Sum of progress[k_EAppUpdateProgress_Download] over update_type_info (stock H()); None = no progress table.
    pub dl_bytes: Option<u64>,
    pub dl_total: Option<u64>,
    /// update_type_info entries with has_update: (content type 0 content / 1 workshop / 2 shader, completed).
    pub types: Vec<(u8, bool)>,
}

/// Steam download settings (settingsStore.m_ClientSettings / m_Settings). Every value is None when Steam did not report it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DownloadSettings {
    /// download_throttle_rate, kbit/s; 0 = no limit.
    pub throttle_rate: Option<i64>,
    /// download_rate_bits_per_s: show rates in bits.
    pub rate_bits: Option<bool>,
    pub restrict_auto_updates: Option<bool>,
    pub restrict_start: Option<i64>,
    pub restrict_end: Option<i64>,
    pub while_app_running: Option<bool>,
    pub throttle_while_streaming: Option<bool>,
    /// default_app_update_behavior: 3 keep up to date, 1 update on launch.
    pub update_behavior: Option<i64>,
    /// download_peer_content: 0 off, 1 same user, 2 friends, 3 anyone.
    pub peer_content: Option<i64>,
    pub region: Option<i64>,
    /// vecValidAutoUpdateRestrictHours: (nHour, strDisplay).
    pub hours: Vec<(i64, String)>,
    /// vecValidDownloadRegions: (nRegionID, strRegionName).
    pub regions: Vec<(i64, String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Downloads {
    pub overview: DownloadOverview,
    /// downloadsStore.DownloadHistory, oldest first (empty when Steam sent none).
    pub history: Vec<HistorySample>,
    pub items: Vec<DownloadItem>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notification {
    pub id: u64,
    pub kind: i64,
    pub source: i64,
    pub is_new: bool,
    pub read: bool,
    pub count: u32,
    /// Truncated JSON of the notification payload (title/body live inside, shape depends on type).
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notifications {
    pub unviewed: u32,
    pub items: Vec<Notification>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Network {
    pub connected: bool,
    pub internet: bool,
    pub steam: bool,
    pub wifi_enabled: bool,
    /// Steam EWirelessStrength for the connected access point (0 through 4).
    pub wifi_strength: Option<u8>,
    pub ssid: Option<String>,
    /// Steam is in offline mode (`friendStore.BIsOfflineMode()`).
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Battery {
    pub present: bool,
    /// 0..1
    pub level: f64,
    pub ac_state: i64,
    pub state: i64,
    pub seconds_remaining: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct User {
    pub persona_name: Option<String>,
    pub account_name: Option<String>,
    /// As reported by Steam, e.g. "/avatarcache/<steamid>.png".
    pub avatar_url: Option<String>,
    /// Absolute path under the Steam config dir if the file exists.
    pub avatar_path: Option<String>,
    pub steamid: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SystemInfo {
    pub network: Network,
    pub battery: Battery,
    pub user: User,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioDevice {
    pub id: i64,
    pub name: String,
    pub volume: f64,
    pub is_default: bool,
}

fn bad(what: &str) -> Error {
    Error::Protocol(format!("unexpected shape: {what}"))
}
fn u(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)).or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0)
}
fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub fn parse_apps(v: &Value) -> Result<Vec<App>> {
    v.as_array()
        .ok_or_else(|| bad("apps"))?
        .iter()
        .map(|r| {
            let r = r.as_array().ok_or_else(|| bad("app row"))?;
            if r.len() < 18 {
                return Err(bad("app row length"));
            }
            Ok(App {
                appid: u(&r[0]) as u32,
                display_name: s(&r[1]),
                sort_as: s(&r[2]),
                installed: u(&r[3]) != 0,
                size_on_disk: u(&r[4]),
                playtime_minutes: u(&r[5]) as u32,
                playtime_2weeks_minutes: u(&r[6]) as u32,
                last_played: u(&r[7]),
                app_type: u(&r[8]) as u32,
                display_status: u(&r[9]) as u32,
                status_percentage: u(&r[10]) as u32,
                is_shortcut: u(&r[11]) != 0,
                game_id: s(&r[12]),
                visible: u(&r[13]) != 0,
                deck_compat_packed: u(&r[14]) as u32,
                review_percentage: u(&r[15]) as u32,
                release_date: u(&r[16]),
                recent_activity_time: u(&r[17]),
            })
        })
        .collect()
}

pub fn parse_collections(v: &Value) -> Result<Vec<Collection>> {
    v.as_array()
        .ok_or_else(|| bad("collections"))?
        .iter()
        .map(|r| {
            Ok(Collection {
                id: s(&r[0]),
                name: s(&r[1]),
                apps: r[2].as_array().ok_or_else(|| bad("collection apps"))?.iter().map(|a| u(a) as u32).collect(),
            })
        })
        .collect()
}

pub fn parse_ids(v: &Value) -> Result<Vec<u32>> {
    Ok(v.as_array().ok_or_else(|| bad("id list"))?.iter().map(|a| u(a) as u32).collect())
}

pub fn parse_downloads(v: &Value) -> Result<Downloads> {
    let o = &v["overview"];
    let items = v["items"]
        .as_array()
        .ok_or_else(|| bad("download items"))?
        .iter()
        .map(|i| DownloadItem {
            appid: u(&i["appid"]) as u32,
            active: i["active"].as_bool().unwrap_or(false),
            paused: i["paused"].as_bool().unwrap_or(false),
            completed: i["completed"].as_bool().unwrap_or(false),
            queue_index: i["queue_index"].as_i64().unwrap_or(-1) as i32,
            percent: i["percent"].as_f64().unwrap_or(0.0),
            eta_sec: i["eta"].as_i64().unwrap_or(-1),
            bytes: u(&i["bytes"]),
            total: u(&i["total"]),
            launch_on_completion: i["launch_on_completion"].as_bool().unwrap_or(false),
            name: i["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string),
            deferred_time: i["deferred_time"].as_i64().unwrap_or(0),
            completed_time: i["completed_time"].as_i64().unwrap_or(0),
            update_result: i["update_result"].as_i64(),
            update_error: i["update_error"].as_str().filter(|e| !e.is_empty()).map(str::to_string),
            auto_update: i["auto_update"].as_i64(),
            disabled_by_app: i["disabled_by_app"].as_bool(),
            dl_bytes: i["dl_bytes"].as_u64(),
            dl_total: i["dl_total"].as_u64(),
            types: i["types"].as_array().map(|a| a.iter().filter_map(|p| Some((p[0].as_u64()? as u8, p[1].as_i64()? != 0))).collect()).unwrap_or_default(),
        })
        .collect();
    Ok(Downloads {
        overview: DownloadOverview {
            state: s(&o["state"]),
            appid: u(&o["appid"]) as u32,
            net_bytes_per_sec: u(&o["net"]),
            peak_net_bytes_per_sec: u(&o["peak"]),
            disc_bytes_per_sec: u(&o["disc"]),
            is_upload: o["upload"].as_bool().unwrap_or(false),
            paused: o["paused"].as_bool(),
            throttling_suspended: o["throttling_suspended"].as_bool(),
            lan_peer_hostname: o["lan_peer_hostname"].as_str().map(str::to_string),
            percent: o["percent"].as_f64(),
            eta_sec: o["eta"].as_i64(),
            dl_bytes: o["dl_bytes"].as_u64(),
            dl_total: o["dl_total"].as_u64(),
            is_shader: o["is_shader"].as_bool().unwrap_or(false),
            is_workshop: o["is_workshop"].as_bool().unwrap_or(false),
        },
        history: v["history"].as_array().map(|a| a.iter().map(|h| HistorySample { time: h[0].as_f64().unwrap_or(0.0), net: u(&h[1]), disc: u(&h[2]), upload: h[3].as_i64().unwrap_or(0) != 0 }).collect()).unwrap_or_default(),
        items,
    })
}

pub fn parse_download_settings(v: &Value) -> Result<DownloadSettings> {
    if !v.is_object() {
        return Err(bad("download settings"));
    }
    let pairs = |k: &str| -> Vec<(i64, String)> {
        v[k].as_array().map(|a| a.iter().filter_map(|p| Some((p[0].as_i64()?, p[1].as_str()?.to_string()))).collect()).unwrap_or_default()
    };
    Ok(DownloadSettings {
        throttle_rate: v["throttle_rate"].as_i64(),
        rate_bits: v["rate_bits"].as_bool(),
        restrict_auto_updates: v["restrict"].as_bool(),
        restrict_start: v["restrict_start"].as_i64(),
        restrict_end: v["restrict_end"].as_i64(),
        while_app_running: v["while_running"].as_bool(),
        throttle_while_streaming: v["throttle_streaming"].as_bool(),
        update_behavior: v["update_behavior"].as_i64(),
        peer_content: v["peer_content"].as_i64(),
        region: v["region"].as_i64(),
        hours: pairs("hours"),
        regions: pairs("regions"),
    })
}

pub fn parse_notifications(v: &Value) -> Result<Notifications> {
    let items = v["items"]
        .as_array()
        .ok_or_else(|| bad("notifications"))?
        .iter()
        .map(|n| Notification {
            id: u(&n["id"]),
            kind: n["type"].as_i64().unwrap_or(-1),
            source: n["source"].as_i64().unwrap_or(-1),
            is_new: n["is_new"].as_bool().unwrap_or(false),
            read: n["read"].as_bool().unwrap_or(false),
            count: u(&n["count"]) as u32,
            data: s(&n["data"]),
        })
        .collect();
    Ok(Notifications { unviewed: u(&v["unviewed"]) as u32, items })
}

pub fn parse_system(v: &Value) -> Result<SystemInfo> {
    if v.is_null() { return Err(bad("system unavailable")); }
    let n = &v["network"];
    let b = &v["battery"];
    let us = &v["user"];
    let opt = |x: &Value| x.as_str().map(str::to_owned);
    let avatar_url = opt(&us["avatar_url"]);
    Ok(SystemInfo {
        network: Network {
            connected: n["connected"].as_bool().unwrap_or(false),
            internet: n["internet"].as_bool().unwrap_or(false),
            steam: n["steam"].as_bool().unwrap_or(false),
            wifi_enabled: n["wifi_enabled"].as_bool().unwrap_or(false),
            wifi_strength: n["wifi_strength"].as_u64().filter(|&level| level <= 4).map(|level| level as u8),
            ssid: opt(&n["ssid"]),
            offline: n["offline"].as_bool().unwrap_or(false),
        },
        battery: Battery {
            present: b["present"].as_bool().unwrap_or(false),
            level: b["level"].as_f64().unwrap_or(0.0),
            ac_state: b["ac_state"].as_i64().unwrap_or(0),
            state: b["state"].as_i64().unwrap_or(0),
            seconds_remaining: b["seconds_remaining"].as_i64().unwrap_or(-1),
        },
        user: User {
            persona_name: opt(&us["persona_name"]),
            account_name: opt(&us["account_name"]),
            avatar_path: avatar_url.as_deref().and_then(avatar_path),
            avatar_url,
            steamid: s(&us["steamid"]),
        },
    })
}

fn avatar_path(url: &str) -> Option<String> {
    let rel = url.strip_prefix('/')?;
    let home = std::env::var("HOME").ok()?;
    for base in [".local/share/Steam/config", ".steam/steam/config"] {
        let p = format!("{home}/{base}/{rel}");
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

pub fn parse_audio(v: &Value) -> Result<Vec<AudioDevice>> {
    Ok(v.as_array()
        .ok_or_else(|| bad("audio"))?
        .iter()
        .map(|d| AudioDevice {
            id: d["id"].as_i64().unwrap_or(-1),
            name: s(&d["name"]),
            volume: d["volume"].as_f64().unwrap_or(0.0),
            is_default: d["default"].as_bool().unwrap_or(false),
        })
        .collect())
}
