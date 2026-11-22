//! Downloads panel of the left side menu (STEAM → «Загрузки») and its download settings.
//!
//! Source of every rule here: Steam's own gamepad UI (`tools/cdp/build/src/sp.js`, downloads page module and
//! `downloadsStore`), see docs/downloads-menu-2026-11-15.md. Sections, item states and the calls behind each action
//! are ports of that code. A value Steam did not report is shown as unavailable and its action is not offered.
//! Nothing is mirrored optimistically: after a write the panel waits for Steam's next snapshot.
use steam_assets::i18n;
use std::rc::Rc;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, VecModel};
use steam_bridge::{js, DownloadItem, DownloadOverview, DownloadSettings, Downloads};

use crate::app::App;
use crate::input::Action;
use crate::router::Route;
use crate::{AppWindow, DlMenuRow, DownloadsMenuState};

// ---------------------------------------------------------------------------------------------- stock model

/// sp.js `k_EDownloadSection_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Active,
    Queued,
    Scheduled,
    NotQueued,
    Completed,
}

/// sp.js item state enum `Ue` (k_EUnknown … k_ECompleted).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Active,
    ActiveDownloading,
    ActivePaused,
    Queued,
    QueuedNext,
    Scheduled,
    RemovedFromQueue,
    Error,
    DisabledByUser,
    DisabledByApp,
    Completed,
}

/// EAppUpdateError values the stock list does not treat as an error (module 94601: VIA=0, $b2=2, UfC=4).
const RESULT_NOT_ERROR: [i64; 3] = [0, 2, 4];
/// eAutoUpdateValue "only update on launch" (module 71670 `uU`).
const AUTO_UPDATE_ON_LAUNCH: i64 = 1;
/// default_app_update_behavior "keep up to date" (module 71670 `Pm`).
const BEHAVIOR_KEEP_UPDATED: i64 = 3;

/// sp.js `Ve(item)`: the overview applies to an item only while it is active or at the head of the queue.
pub fn item_overview<'a>(i: &DownloadItem, ov: &'a DownloadOverview) -> Option<&'a DownloadOverview> {
    (i.active || i.queue_index == 0).then_some(ov)
}

/// Port of sp.js `Re(item, app, overview)`.
pub fn status(i: &DownloadItem, ov: Option<&DownloadOverview>) -> Status {
    if i.update_result.is_some_and(|r| !RESULT_NOT_ERROR.contains(&r)) {
        Status::Error
    } else if i.queue_index == -1 && i.deferred_time != 0 {
        Status::Scheduled
    } else if i.disabled_by_app == Some(true) {
        Status::DisabledByApp
    } else if i.paused {
        if i.auto_update == Some(AUTO_UPDATE_ON_LAUNCH) { Status::DisabledByUser } else { Status::RemovedFromQueue }
    } else if i.active {
        if ov.is_some_and(|o| o.state == "Downloading") { Status::ActiveDownloading } else { Status::Active }
    } else if i.completed && i.completed_time != 0 {
        Status::Completed
    } else if i.queue_index == 1 {
        Status::QueuedNext
    } else if i.queue_index >= 0 {
        if i.queue_index == 0 && ov.is_some_and(|o| o.paused == Some(true)) { Status::ActivePaused } else { Status::Queued }
    } else if i.auto_update == Some(AUTO_UPDATE_ON_LAUNCH) {
        Status::DisabledByUser
    } else {
        Status::Unknown
    }
}

pub(crate) fn item_name(i: &DownloadItem) -> String {
    i.name.clone().unwrap_or_else(|| format!("App {}", i.appid))
}

/// Port of the downloadsStore getters and the page render: the head of the queue is the active section, the rest of
/// the queue follows in queue order, then scheduled (by time), not queued (by name) and completed (newest first).
/// Steam sorts "not queued" by `sort_as`, which the bridge does not read; the display name is used instead.
/// Steam also hides parentally blocked apps; that filter is not applied here.
pub fn sections(d: &Downloads) -> Vec<(Section, Vec<&DownloadItem>)> {
    let mut queued: Vec<&DownloadItem> = d.items.iter().filter(|i| i.queue_index != -1 && i.deferred_time == 0).collect();
    queued.sort_by_key(|i| i.queue_index);
    let mut scheduled: Vec<&DownloadItem> = d.items.iter().filter(|i| i.queue_index == -1 && i.deferred_time != 0 && !i.completed).collect();
    scheduled.sort_by_key(|i| i.deferred_time);
    let mut unqueued: Vec<&DownloadItem> = d.items.iter().filter(|i| !i.active && !i.completed && i.queue_index == -1 && i.deferred_time == 0).collect();
    unqueued.sort_by_key(|i| item_name(i).to_lowercase());
    let mut completed: Vec<&DownloadItem> = d.items.iter().filter(|i| i.completed).collect();
    completed.sort_by(|a, b| b.completed_time.cmp(&a.completed_time).then_with(|| item_name(a).to_lowercase().cmp(&item_name(b).to_lowercase())));
    let rest = queued.split_off(queued.len().min(1));
    vec![(Section::Active, queued), (Section::Queued, rest), (Section::Scheduled, scheduled), (Section::NotQueued, unqueued), (Section::Completed, completed)]
}

pub(crate) fn find(d: &Downloads, appid: u32) -> Option<(Section, &DownloadItem)> {
    sections(d).into_iter().find_map(|(s, v)| v.into_iter().find(|i| i.appid == appid).map(|i| (s, i)))
}

/// A call into Steam (or a local navigation) behind a row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// EnableAllDownloads(on): pause / resume everything.
    EnableAll(bool),
    /// ResumeAppUpdate: download now / move to top / retry.
    DownloadNow(u32),
    /// PauseAppUpdate: take out of the queue ("not queued").
    Unqueue(u32),
    /// SetQueueIndex(appid, index).
    QueueIndex(u32, u32),
    Remove { appid: u32, reenable: bool },
    LaunchOnComplete { appid: u32, on: bool },
    SuspendThrottle(bool),
    /// Local: open the game page.
    ViewInLibrary(u32),
}

impl Op {
    /// The JavaScript evaluated in Steam; None for local navigation.
    pub fn js(&self) -> Option<String> {
        Some(match *self {
            Op::EnableAll(on) => js::enable_all_downloads(on),
            Op::DownloadNow(id) => js::resume_app_update(id),
            Op::Unqueue(id) => js::pause_app_update(id),
            Op::QueueIndex(id, index) => js::set_queue_index(id, index),
            Op::Remove { appid, reenable } => js::remove_from_download_list(appid, reenable),
            Op::LaunchOnComplete { appid, on } => js::set_launch_on_update_complete(if on { appid } else { 0 }),
            Op::SuspendThrottle(on) => js::suspend_download_throttling(on),
            Op::ViewInLibrary(_) => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemAction {
    pub op: Op,
    pub token: &'static str,
    pub fallback: &'static str,
    /// Some for a checkbox item (current state).
    pub check: Option<bool>,
}

fn act(op: Op, token: &'static str, fallback: &'static str) -> ItemAction {
    ItemAction { op, token, fallback, check: None }
}

/// Port of the OK-button choice in sp.js (`yt`): active item pauses / resumes everything, a queued, scheduled or
/// unqueued item is downloaded now (retry after an error), a completed item opens its game. None when Steam shows
/// no button (updates disabled by the app) or the pause state is unknown.
pub fn primary(section: Section, st: Status, ov_paused: Option<bool>, appid: u32) -> Option<ItemAction> {
    if st == Status::DisabledByApp {
        return None;
    }
    match section {
        Section::Active => match ov_paused? {
            false => Some(act(Op::EnableAll(false), "#Downloads_Pause_Tooltip", i18n::tr_ref("downloads_menu.pause"))),
            true => Some(act(Op::EnableAll(true), "#Downloads_Resume_Tooltip", i18n::tr_ref("downloads_menu.resume"))),
        },
        Section::Queued | Section::Scheduled | Section::NotQueued => Some(if st == Status::Error {
            act(Op::DownloadNow(appid), "#Downloads_Retry_Tooltip", i18n::tr_ref("downloads_menu.retry"))
        } else {
            act(Op::DownloadNow(appid), "#Downloads_MoveToTop_Tooltip", i18n::tr_ref("downloads_menu.download_now"))
        }),
        Section::Completed => Some(act(Op::ViewInLibrary(appid), "#Downloads_GoToGame_Tooltip", i18n::tr_ref("downloads_menu.go_to_the_game"))),
    }
}

/// Actions of one item: the OK action, then the stock context menu (`vt`) and the stock drag gestures
/// (queue order, "not queued") as rows. `throttle_rate` is the download limit setting (None = unknown).
pub fn item_actions(d: &Downloads, appid: u32, throttle_rate: Option<i64>) -> Vec<ItemAction> {
    let Some((section, i)) = find(d, appid) else { return vec![] };
    let ov = item_overview(i, &d.overview);
    let st = status(i, ov);
    let mut v = Vec::new();
    v.extend(primary(section, st, d.overview.paused, appid));
    if i.queue_index == 0 {
        v.push(ItemAction { check: Some(i.launch_on_completion), ..act(Op::LaunchOnComplete { appid, on: !i.launch_on_completion }, "#Downloads_LaunchWhenComplete", i18n::tr_ref("downloads_menu.launch_after_booting")) });
        if let (Some(rate), Some(suspended)) = (throttle_rate, d.overview.throttling_suspended) {
            if rate > 0 {
                v.push(ItemAction { check: Some(suspended), ..act(Op::SuspendThrottle(!suspended), "#Downloads_OverrideDownloadThrottle", i18n::tr_ref("downloads_menu.do_not_limit_the_speed_of_the")) });
            }
        }
    }
    if section == Section::Queued {
        let last = d.items.iter().filter(|x| x.queue_index != -1 && x.deferred_time == 0).map(|x| x.queue_index).max().unwrap_or(0);
        // Index 0 is the active slot: moving there is "download now" above. Only moves inside the waiting queue.
        if i.queue_index >= 2 {
            v.push(act(Op::QueueIndex(appid, i.queue_index as u32 - 1), "", i18n::tr_ref("downloads_menu.up_in_line")));
        }
        if i.queue_index >= 1 && i.queue_index < last {
            v.push(act(Op::QueueIndex(appid, i.queue_index as u32 + 1), "", i18n::tr_ref("downloads_menu.below_in_line")));
        }
    }
    if matches!(section, Section::Queued | Section::Scheduled) {
        v.push(act(Op::Unqueue(appid), "", i18n::tr_ref("downloads_menu.remove_from_the_queue_do_not_download")));
    }
    // sp.js bShowRemoveButton: not in "not queued", not while active, not when disabled by the app.
    if section != Section::NotQueued && !matches!(st, Status::Active | Status::ActiveDownloading | Status::DisabledByApp) {
        let reenable = i.queue_index == 0 && d.overview.paused == Some(true);
        v.push(if i.completed {
            act(Op::Remove { appid, reenable }, "#Downloads_RemoveFromList_Tooltip", i18n::tr_ref("downloads_menu.remove_from_list"))
        } else {
            act(Op::Remove { appid, reenable }, "#Downloads_RemoveFromQueue_Tooltip", i18n::tr_ref("downloads_menu.remove_from_line"))
        });
    }
    if section != Section::Completed {
        v.push(act(Op::ViewInLibrary(appid), "#Generic_ViewInLibrary", i18n::tr_ref("downloads_menu.view_in_library")));
    }
    v
}

// ---------------------------------------------------------------------------------------------- settings

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Limit,
    RateBits,
    UpdateBehavior,
    Restrict,
    RestrictStart,
    RestrictEnd,
    WhileRunning,
    ThrottleStreaming,
    PeerContent,
    Region,
}

impl Setting {
    /// CMsgClientSettings field written through SteamClient.Settings.SetSetting.
    pub fn key(self) -> &'static str {
        match self {
            Setting::Limit => "download_throttle_rate",
            Setting::RateBits => "download_rate_bits_per_s",
            Setting::UpdateBehavior => "default_app_update_behavior",
            Setting::Restrict => "restrict_auto_updates",
            Setting::RestrictStart => "restrict_auto_updates_start",
            Setting::RestrictEnd => "restrict_auto_updates_end",
            Setting::WhileRunning => "download_while_app_running",
            Setting::ThrottleStreaming => "download_throttle_while_streaming",
            Setting::PeerContent => "download_peer_content",
            Setting::Region => "download_region",
        }
    }
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Setting::Limit => ("#DownloadSettings_BandwidthLimit", i18n::tr_ref("downloads_menu.limit_download_speed")),
            Setting::RateBits => ("#DownloadSettings_DisplayInBitsPerSecond", i18n::tr_ref("downloads_menu.speed_in_bits_per_second")),
            Setting::UpdateBehavior => ("#DownloadSettings_GameUpdateTiming", i18n::tr_ref("downloads_menu.update_games")),
            Setting::Restrict => ("#DownloadSettings_ScheduleAutoUpdates", i18n::tr_ref("downloads_menu.schedule_auto_updates")),
            Setting::RestrictStart => ("#DownloadSettings_RestrictUpdatesFrom", i18n::tr_ref("downloads_menu.update_with")),
            Setting::RestrictEnd => ("#DownloadSettings_RestrictUpdatesTo", i18n::tr_ref("downloads_menu.before")),
            Setting::WhileRunning => ("#DownloadSettings_AllowDownloadDuringGameplay", i18n::tr_ref("downloads_menu.allow_downloads_during_gameplay")),
            Setting::ThrottleStreaming => ("#DownloadSettings_ThrottleDuringStreaming", i18n::tr_ref("downloads_menu.throttle_downloads_while_streaming")),
            Setting::PeerContent => ("#DownloadSettings_PeerContent", i18n::tr_ref("downloads_menu.transfer_of_files_over_a_local_network")),
            Setting::Region => ("#DownloadSettings_SelectRegion", i18n::tr_ref("downloads_menu.download_region")),
        }
    }
    fn toggle(self, s: &DownloadSettings) -> Option<Option<bool>> {
        Some(match self {
            Setting::RateBits => s.rate_bits,
            Setting::Restrict => s.restrict_auto_updates,
            Setting::WhileRunning => s.while_app_running,
            Setting::ThrottleStreaming => s.throttle_while_streaming,
            _ => return None,
        })
    }
    fn number(self, s: &DownloadSettings) -> Option<i64> {
        match self {
            Setting::Limit => s.throttle_rate,
            Setting::UpdateBehavior => s.update_behavior,
            Setting::RestrictStart => s.restrict_start,
            Setting::RestrictEnd => s.restrict_end,
            Setting::PeerContent => s.peer_content,
            Setting::Region => s.region,
            _ => None,
        }
    }
}

const UNAVAILABLE: &str = "downloads_menu.unavailable";

/// Stored unit is kbit/s (sp.js: stored = floor(input * (bits ? 1 : 8))); shown in the unit Steam's own field uses.
pub fn limit_label(rate: i64, bits: Option<bool>) -> String {
    if rate <= 0 {
        return i18n::tr_ref("downloads_menu.unlimited").into();
    }
    match bits {
        Some(false) if rate % 8 == 0 => i18n::tr_args("downloads_menu.kb_s", &[("rate", &(rate / 8).to_string())]),
        Some(false) => crate::app::comma(i18n::tr_args("downloads_menu.kb_s", &[("rate", &format!("{:.1}", rate as f64 / 8.0))])),
        _ => i18n::tr_args("downloads_menu.kbit_s", &[("rate", &(rate).to_string())]),
    }
}

/// Options offered for a multi-value setting: (stored value, label). Enum values are the stock ones; the limit
/// steps are DeckShell's own ladder (Steam has a free text field), the current value is always in the list.
pub fn choices(setting: Setting, s: &DownloadSettings) -> Vec<(i64, String)> {
    match setting {
        Setting::Limit => {
            let mut steps: Vec<i64> = if s.rate_bits == Some(false) {
                [256, 512, 1024, 2048, 5120, 10240, 25600, 51200, 102400].iter().map(|kb| kb * 8).collect()
            } else {
                vec![1000, 2000, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000]
            };
            if let Some(cur) = s.throttle_rate.filter(|&c| c > 0) {
                if !steps.contains(&cur) {
                    steps.push(cur);
                    steps.sort_unstable();
                }
            }
            std::iter::once(0).chain(steps).map(|r| (r, limit_label(r, s.rate_bits))).collect()
        }
        Setting::UpdateBehavior => vec![
            (BEHAVIOR_KEEP_UPDATED, i18n::tr_ref("downloads_menu.let_steam_decide_when_to_update").into()),
            (AUTO_UPDATE_ON_LAUNCH, i18n::tr_ref("downloads_menu.update_only_when_the_game_starts").into()),
        ],
        Setting::RestrictStart | Setting::RestrictEnd => s.hours.clone(),
        Setting::PeerContent => vec![
            (0, i18n::tr_ref("downloads_menu.disabled").into()),
            (1, i18n::tr_ref("downloads_menu.just_my_devices").into()),
            (2, i18n::tr_ref("downloads_menu.my_devices_and_friends").into()),
            (3, i18n::tr_ref("downloads_menu.any_user").into()),
        ],
        _ => vec![],
    }
}

fn choice_label(setting: Setting, s: &DownloadSettings) -> String {
    let Some(v) = setting.number(s) else { return i18n::tr_ref(UNAVAILABLE).into() };
    if setting == Setting::Limit {
        return limit_label(v, s.rate_bits);
    }
    let named = if setting == Setting::Region { s.regions.clone() } else { choices(setting, s) };
    named.into_iter().find(|(x, _)| *x == v).map(|(_, l)| l).unwrap_or_else(|| i18n::tr_args("downloads_menu.unknown", &[("value", &(v).to_string())]))
}

/// JS of a settings write: one CMsgClientSettings field through SetSetting (sp.js `K(key, value)`).
pub fn setting_write(setting: Setting, value: &Value) -> Option<String> {
    if setting == Setting::Region {
        return None;
    }
    let b64 = crate::setters::encode_client_setting(setting.key(), value)?;
    Some(format!("SteamClient.Settings.SetSetting({})", crate::setters::js_str(&b64)))
}

// ---------------------------------------------------------------------------------------------- rows

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    None,
    Item(u32),
    Op(Op),
    OpenSettings,
    OpenPage,
    Toggle(Setting, bool),
    Choose(Setting),
    Pick(Setting, i64),
    /// «Своё значение…»: произвольное число с экранной клавиатуры (числовой режим)
    Custom(Setting),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Item,
    Header,
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub label: String,
    pub value: String,
    pub sub: String,
    /// 0..1, or negative when there is no progress to show.
    pub progress: f32,
    /// -1 none, 0 off, 1 on.
    pub check: i32,
    pub dim_bar: bool,
    pub enabled: bool,
    pub target: Target,
}

impl Row {
    fn item(label: impl Into<String>, target: Target) -> Row {
        Row { kind: RowKind::Item, label: label.into(), value: String::new(), sub: String::new(), progress: -1.0, check: -1, dim_bar: false, enabled: true, target }
    }
    fn header(label: impl Into<String>) -> Row {
        Row { kind: RowKind::Header, enabled: false, ..Row::item(label, Target::None) }
    }
    fn note(label: impl Into<String>) -> Row {
        Row { kind: RowKind::Note, enabled: false, ..Row::item(label, Target::None) }
    }
    pub fn focusable(&self) -> bool {
        self.kind == RowKind::Item
    }
    fn height(&self) -> f32 {
        match self.kind {
            RowKind::Header => 44.0,
            RowKind::Note => if self.label.chars().count() > 46 { 72.0 } else { 48.0 },
            RowKind::Item => if self.sub.is_empty() && self.progress < 0.0 { 58.0 } else { 78.0 },
        }
    }
}

/// Text sources, injected so the fixture tests do not depend on the installed localization or the time zone.
pub struct Txt<'a> {
    pub tr: &'a dyn Fn(&str, &str) -> String,
    pub time: &'a dyn Fn(i64) -> String,
    pub size: &'a dyn Fn(u64) -> String,
}

pub(crate) fn speed(bytes_per_sec: u64, bits: Option<bool>) -> String {
    let (v, units) = if bits == Some(true) { (bytes_per_sec as f64 * 8.0, [i18n::tr_ref("downloads_menu.qbit"), i18n::tr_ref("downloads_menu.mbit_s")]) } else { (bytes_per_sec as f64, [i18n::tr_ref("downloads_menu.kb_s_2"), i18n::tr_ref("downloads_menu.mb_s")]) };
    crate::app::comma(if v >= 1e6 { format!("{:.1} {}", v / 1e6, units[1]) } else { format!("{:.0} {}", v / 1e3, units[0]) })
}

pub(crate) fn state_text(state: &str, t: &Txt) -> String {
    let fallback = match state {
        "Downloading" => i18n::tr_ref("downloads_menu.loading"),
        "Starting" => i18n::tr_ref("downloads_menu.launching"),
        "Validating" | "Verifying" => i18n::tr_ref("downloads_menu.validating"),
        "Preallocating" => i18n::tr_ref("downloads_menu.reserving_space"),
        "Updating" | "Patching" => i18n::tr_ref("downloads_menu.updating"),
        "Unpacking" => i18n::tr_ref("downloads_menu.unpacking"),
        "Installing" => i18n::tr_ref("downloads_menu.installing"),
        "Finalizing" => i18n::tr_ref("downloads_menu.finalizing"),
        "Stopping" => i18n::tr_ref("downloads_menu.stopping"),
        other => other,
    };
    (t.tr)(&format!("#Downloads_DetailedState_{state}"), fallback)
}

/// sp.js `E(x, total)`: whole percent, at least 1 once anything is downloaded.
pub(crate) fn percent_of(done: u64, total: u64) -> Option<u32> {
    (total > 0).then(|| ((done as u128 * 100 / total as u128) as u32).clamp(u32::from(done > 0), 100))
}

fn eta_text(sec: i64) -> String {
    match sec {
        s if s < 0 => String::new(),
        s if s <= 60 => i18n::tr_args("downloads_menu.sec_remaining", &[("seconds", &(s).to_string())]),
        s => i18n::tr_args("downloads_menu.min_remaining", &[("seconds", &(s / 60).to_string())]),
    }
}

fn item_row(section: Section, i: &DownloadItem, d: &Downloads, t: &Txt) -> Row {
    let ov = item_overview(i, &d.overview);
    let st = status(i, ov);
    let live = ov.filter(|o| o.appid == i.appid);
    // Stock: an active item with overview download bytes shows the overview numbers, otherwise the item's own.
    let (done, total) = match live.and_then(|o| Some((o.dl_bytes?, o.dl_total?))).filter(|_| i.active) {
        Some(p) if p.0 > 0 => (Some(p.0), Some(p.1)),
        _ => (i.dl_bytes, i.dl_total),
    };
    let pct = match live.and_then(|o| o.percent).filter(|_| section == Section::Active) {
        Some(p) => Some(p.round().clamp(0.0, 100.0) as u32),
        None => done.zip(total).and_then(|(b, n)| percent_of(b, n)),
    };
    let mut row = Row::item(item_name(i), Target::Item(i.appid));
    row.value = match st {
        Status::ActiveDownloading | Status::Active => state_text(&d.overview.state, t),
        Status::ActivePaused => (t.tr)("#Downloads_State_Paused", i18n::tr_ref("downloads_menu.paused")),
        Status::Queued => (t.tr)("#Downloads_State_Queued", i18n::tr_ref("downloads_menu.queued")),
        Status::QueuedNext => (t.tr)("#Downloads_State_UpNext", i18n::tr_ref("downloads_menu.next")),
        Status::Scheduled => (t.tr)("#Downloads_State_Scheduled", i18n::tr_ref("downloads_menu.scheduled")),
        Status::RemovedFromQueue => (t.tr)("#Downloads_State_Paused", i18n::tr_ref("downloads_menu.paused")),
        Status::Error => i18n::tr_ref("downloads_menu.error").into(),
        Status::DisabledByUser | Status::DisabledByApp => i18n::tr_ref("downloads_menu.disconnected").into(),
        Status::Completed => (t.tr)("#Downloads_State_Complete", i18n::tr_ref("downloads_menu.completed")),
        Status::Unknown => i18n::tr_ref("downloads_menu.unknown_2").into(),
    };
    let mut sub: Vec<String> = Vec::new();
    match st {
        Status::Error => {
            let code = i.update_result.unwrap_or_default();
            sub.push((t.tr)(&format!("#Steam_AppUpdateError_{code}"), &i18n::tr_args("downloads_menu.update_error_code", &[("code", &(code).to_string())])));
            sub.extend(i.update_error.clone());
        }
        Status::Scheduled => sub.push(i18n::tr_args("downloads_menu.at", &[("deferred_time", &((t.time)(i.deferred_time)).to_string())])),
        Status::DisabledByUser => sub.push((t.tr)("#Downloads_AutoUpdate_DoNotUpdate", i18n::tr_ref("downloads_menu.updated_only_at_launch"))),
        Status::DisabledByApp => sub.push((t.tr)("#Downloads_State_UpdatesDiabledByApp", i18n::tr_ref("downloads_menu.updates_disabled_by_app"))),
        Status::Completed => sub.push((t.time)(i.completed_time)),
        Status::Unknown => sub.push(i18n::tr_ref("downloads_menu.steam_did_not_report_a_condition").into()),
        _ => {}
    }
    if st != Status::Completed {
        match (done, total) {
            (Some(b), Some(n)) if b > 0 && n > 0 => sub.push(format!("{} / {}", (t.size)(b), (t.size)(n))),
            (_, Some(n)) if n > 0 => sub.push((t.size)(n)),
            _ => {}
        }
        if let Some(p) = pct.filter(|_| done.is_some_and(|b| b > 0) || section == Section::Active) {
            sub.push(format!("{p}%"));
            row.progress = p as f32 / 100.0;
        }
    }
    if matches!(st, Status::ActiveDownloading | Status::Active) {
        if let Some(o) = live {
            if o.net_bytes_per_sec > 0 {
                sub.push(speed(o.net_bytes_per_sec, None));
            }
            sub.extend(o.eta_sec.map(eta_text).filter(|e| !e.is_empty()));
        }
    }
    if i.launch_on_completion && !i.completed {
        sub.push(i18n::tr_ref("downloads_menu.launch").into());
    }
    row.dim_bar = !matches!(st, Status::ActiveDownloading | Status::Active);
    row.sub = sub.join(" · ");
    row
}

fn section_title(s: Section, n: usize, t: &Txt) -> String {
    let (tok, fb) = match s {
        Section::Active => ("#Downloads_Section_Active", i18n::tr_ref("downloads_menu.now")),
        Section::Queued => ("#Downloads_Section_Current", i18n::tr_ref("downloads_menu.up_next")),
        Section::Scheduled => ("#Downloads_Section_Scheduled", i18n::tr_ref("downloads_menu.scheduled")),
        Section::NotQueued => ("#Downloads_Section_NotQueued", i18n::tr_ref("downloads_menu.not_queued")),
        Section::Completed => ("#Downloads_Section_Completed", i18n::tr_ref("downloads_menu.completed")),
    };
    format!("{} ({n})", (t.tr)(tok, fb).to_uppercase())
}

/// One-line state of the whole download system. `None` data = the bridge delivered nothing.
pub fn status_line(d: Option<&Downloads>, s: Option<&DownloadSettings>) -> String {
    let Some(d) = d else { return i18n::tr_ref("downloads_menu.no_data_from_steam").into() };
    let ov = &d.overview;
    let bits = s.and_then(|s| s.rate_bits);
    let mut parts: Vec<String> = Vec::new();
    match ov.paused {
        Some(true) => parts.push(i18n::tr_ref("downloads_menu.downloads_paused").into()),
        None => parts.push(i18n::tr_ref("downloads_menu.pause_unknown").into()),
        Some(false) if ov.appid != 0 && ov.state != "None" => {
            parts.push(if ov.is_upload { i18n::tr_ref("downloads_menu.sending").into() } else { state_text(&ov.state, &Txt { tr: &|_, f| f.to_string(), time: &|_| String::new(), size: &|_| String::new() }) });
            if ov.net_bytes_per_sec > 0 {
                parts.push(speed(ov.net_bytes_per_sec, bits));
            }
        }
        Some(false) => parts.push(i18n::tr_ref("downloads_menu.no_active_loading").into()),
    }
    if let Some(rate) = s.and_then(|s| s.throttle_rate).filter(|&r| r > 0) {
        parts.push(i18n::tr_args("downloads_menu.limit", &[("bits", &(limit_label(rate, bits)).to_string()), ("true", &(if ov.throttling_suspended == Some(true) { i18n::tr_ref("downloads_menu.removed_for_current") } else { "" }).to_string())]));
    }
    if let Some(host) = ov.lan_peer_hostname.as_deref().filter(|h| !h.is_empty()) {
        parts.push(i18n::tr_args("downloads_menu.local_network", &[("host", &(host).to_string())]));
    }
    parts.join(" · ")
}

/// Main view: every stock section that has items, then the global rows.
pub fn list_rows(d: Option<&Downloads>, t: &Txt) -> Vec<Row> {
    let mut rows = Vec::new();
    match d {
        None => rows.push(Row::note(i18n::tr_ref("downloads_menu.steam_did_not_hand_over_the_download"))),
        Some(d) => {
            let secs = sections(d);
            if secs.iter().all(|(_, v)| v.is_empty()) {
                rows.push(Row::note((t.tr)("#Downloads_NoTransfers", i18n::tr_ref("downloads_menu.no_downloads"))));
            }
            for (s, items) in &secs {
                if items.is_empty() {
                    continue;
                }
                rows.push(Row::header(section_title(*s, items.len(), t)));
                rows.extend(items.iter().map(|i| item_row(*s, i, d, t)));
            }
        }
    }
    rows.push(Row::header(i18n::tr_ref("downloads_menu.management")));
    let mut all = match d.and_then(|d| d.overview.paused) {
        Some(true) => Row::item(i18n::tr_ref("downloads_menu.resume_all_downloads"), Target::Op(Op::EnableAll(true))),
        Some(false) => Row::item(i18n::tr_ref("downloads_menu.suspend_all_downloads"), Target::Op(Op::EnableAll(false))),
        None => Row { enabled: false, value: i18n::tr_ref(UNAVAILABLE).into(), ..Row::item(i18n::tr_ref("downloads_menu.suspend_all_downloads"), Target::None) },
    };
    all.check = -1;
    rows.push(all);
    rows.push(Row::item((t.tr)("#DeckShell_DownloadSettings", i18n::tr_ref("downloads_menu.loading_settings")), Target::OpenSettings));
    rows.push(Row::item(i18n::tr_ref("downloads_menu.open_the_download_page"), Target::OpenPage));
    rows
}

/// Action view of one item. Empty item list (the download is gone) leaves only a note.
pub fn action_rows(d: Option<&Downloads>, appid: u32, s: Option<&DownloadSettings>, t: &Txt) -> Vec<Row> {
    let Some(d) = d else { return vec![Row::note(i18n::tr_ref("downloads_menu.steam_didn_t_hand_over_the_download"))] };
    let Some((section, i)) = find(d, appid) else { return vec![Row::note(i18n::tr_ref("downloads_menu.this_download_is_no_longer_on_the"))] };
    let mut info = item_row(section, i, d, t);
    info.label = info.value.clone();
    info.value = String::new();
    info.kind = RowKind::Note;
    info.enabled = false;
    info.target = Target::None;
    if !info.sub.is_empty() {
        info.label = format!("{} · {}", info.label, info.sub);
        info.sub = String::new();
    }
    info.progress = -1.0;
    let mut rows = vec![info];
    let actions = item_actions(d, appid, s.and_then(|s| s.throttle_rate));
    if section == Section::Active && d.overview.paused.is_none() {
        rows.push(Row { enabled: false, value: i18n::tr_ref(UNAVAILABLE).into(), ..Row::item(i18n::tr_ref("downloads_menu.suspend_resume"), Target::None) });
    }
    if i.queue_index == 0 && (s.and_then(|s| s.throttle_rate).is_none() || d.overview.throttling_suspended.is_none()) {
        rows.push(Row { enabled: false, value: i18n::tr_ref(UNAVAILABLE).into(), ..Row::item((t.tr)("#Downloads_OverrideDownloadThrottle", i18n::tr_ref("downloads_menu.do_not_limit_the_speed_of_the")), Target::None) });
    }
    for a in actions {
        let mut r = Row::item(if a.token.is_empty() { a.fallback.to_string() } else { (t.tr)(a.token, a.fallback) }, Target::Op(a.op));
        r.check = a.check.map(i32::from).unwrap_or(-1);
        rows.push(r);
    }
    rows
}

/// Settings view. `None` = Steam's settings were not read: every row is shown as unavailable.
pub fn settings_rows(s: Option<&DownloadSettings>, t: &Txt) -> Vec<Row> {
    let mut rows = Vec::new();
    if s.is_none() {
        rows.push(Row::note(i18n::tr_ref("downloads_menu.steam_has_not_handed_over_download_settings")));
    }
    let mut order = vec![Setting::Limit, Setting::RateBits, Setting::UpdateBehavior];
    // Stock shows the schedule only under "keep up to date", and the hours only while the schedule is on.
    if s.is_none() || s.is_some_and(|s| s.update_behavior == Some(BEHAVIOR_KEEP_UPDATED)) {
        order.push(Setting::Restrict);
        if s.is_some_and(|s| s.restrict_auto_updates == Some(true)) {
            order.extend([Setting::RestrictStart, Setting::RestrictEnd]);
        }
    }
    order.extend([Setting::WhileRunning, Setting::ThrottleStreaming, Setting::PeerContent, Setting::Region]);
    for setting in order {
        let (tok, fb) = setting.label();
        let mut row = Row::item((t.tr)(tok, fb), Target::None);
        match s {
            None => {
                row.enabled = false;
                row.value = i18n::tr_ref(UNAVAILABLE).into();
            }
            Some(s) => match setting.toggle(s) {
                Some(Some(on)) => {
                    row.check = i32::from(on);
                    row.target = Target::Toggle(setting, !on);
                }
                Some(None) => {
                    row.enabled = false;
                    row.value = i18n::tr_ref(UNAVAILABLE).into();
                }
                None => {
                    row.value = choice_label(setting, s);
                    if setting == Setting::Region {
                        // Steam asks for a client restart after a region change; not offered from this panel.
                        row.enabled = false;
                        row.sub = i18n::tr_ref("downloads_menu.changes_on_the_steam_settings_page_customer").into();
                    } else if setting.number(s).is_none() || choices(setting, s).is_empty() {
                        row.enabled = false;
                        row.value = i18n::tr_ref(UNAVAILABLE).into();
                    } else {
                        row.target = Target::Choose(setting);
                    }
                }
            },
        }
        rows.push(row);
    }
    rows
}

pub fn choice_rows(setting: Setting, s: Option<&DownloadSettings>) -> Vec<Row> {
    let Some(s) = s else { return vec![Row::note(i18n::tr_ref("downloads_menu.steam_did_not_hand_over_download_settings"))] };
    let cur = setting.number(s);
    let mut rows: Vec<Row> = choices(setting, s).into_iter().map(|(v, label)| {
        let mut r = Row::item(label, Target::Pick(setting, v));
        r.check = i32::from(cur == Some(v));
        r
    }).collect();
    if setting == Setting::Limit {
        // Steam has a free text field here; the ladder above is DeckShell's own, this row is the field
        let typed = CUSTOM.with(|c| c.borrow().clone());
        let unit = if s.rate_bits == Some(true) { i18n::tr_ref("downloads_menu.qbit") } else { i18n::tr_ref("downloads_menu.kb_s_2") };
        rows.push(Row::item(if typed.is_empty() { i18n::tr_ref("downloads_menu.its_significance").to_string() } else { i18n::tr_args("downloads_menu.custom_value", &[("typed", &(typed).to_string()), ("unit", &(unit).to_string())]) }, Target::Custom(setting)));
    }
    rows
}

/// Тесты: записи в Steam, которые в проверочном режиме не отправляются.
#[cfg(test)]
thread_local! { pub(crate) static WRITES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) }; }
thread_local! { static CUSTOM: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }
pub(crate) const CUSTOM_SINK: super::osk::ExtSink = super::osk::ExtSink { id: 6, mode: super::osk::InputMode::Numeric, typed: custom_typed, enter: custom_enter, cancel: custom_cancel };

fn custom_typed(a: &mut App, c: Option<char>) {
    CUSTOM.with(|t| {
        let mut t = t.borrow_mut();
        match c {
            Some(c) if c == ',' || c == '.' => { if !t.contains('.') && !t.is_empty() { t.push('.'); } }
            Some(c) if c.is_ascii_digit() && t.len() < 9 => t.push(c),
            Some(_) => {}
            None => { t.pop(); }
        }
    });
    a.dl_menu_rebuild(false);
}
fn custom_cancel(a: &mut App) {
    CUSTOM.with(|t| t.borrow_mut().clear());
    a.dl_menu_rebuild(false);
}
/// Stored unit is kbit/s (see `limit_label`): KB/s input is multiplied by 8.
pub fn custom_rate(text: &str, bits: Option<bool>) -> Option<i64> {
    let v: f64 = text.trim_end_matches('.').parse().ok()?;
    let stored = (v * if bits == Some(true) { 1.0 } else { 8.0 }).floor() as i64;
    (stored > 0).then_some(stored)
}
fn custom_enter(a: &mut App) {
    let text = CUSTOM.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let bits = a.dm.settings.clone().flatten().and_then(|s| s.rate_bits);
    match custom_rate(&text, bits) {
        Some(v) => {
            if let Some(js) = setting_write(Setting::Limit, &json!(v)) {
                a.dl_menu_write(Setting::Limit.key(), js);
            }
            a.dl_menu_back();
        }
        None => a.dl_menu_rebuild(false),
    }
}

/// Up/Down inside a view: next focusable row, None at the end (no wrap, as in the game column).
pub fn step(rows: &[Row], focus: usize, a: Action) -> Option<usize> {
    match a {
        Action::Up => (0..focus.min(rows.len())).rev().find(|&i| rows[i].focusable()),
        Action::Down => (focus + 1..rows.len()).find(|&i| rows[i].focusable()),
        _ => None,
    }
}

fn layout(rows: &[Row]) -> (Vec<f32>, f32) {
    let mut y = 0.0;
    let ys = rows.iter().map(|r| { let top = y; y += r.height(); top }).collect();
    (ys, y)
}

// ---------------------------------------------------------------------------------------------- application glue

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Closed,
    List,
    Actions(u32),
    Settings,
    Choice(Setting),
}

#[derive(Default)]
pub struct State {
    pub view: View,
    pub rows: Vec<Row>,
    pub focus: usize,
    scroll: f32,
    /// Focus to restore when a nested view closes: list row, settings row.
    back: Vec<usize>,
    /// None until Steam answered; Some(None) = Steam did not deliver the settings.
    pub settings: Option<Option<DownloadSettings>>,
    generation: u64,
    model: Rc<VecModel<DlMenuRow>>,
}

impl State {
    pub fn is_open(&self) -> bool {
        self.view != View::Closed
    }
}

/// Visible row area of the panel: 680 px column minus the 96 px title block and the 48 px bottom strip.
const VIEWPORT: f32 = 680.0 - 96.0 - 48.0;

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<DownloadsMenuState>();
    st.on_activate(|i| super::defer(move |a| {
        if a.dm.rows.get(i as usize).is_some_and(Row::focusable) {
            a.dm.focus = i as usize;
            a.dl_menu_activate();
        }
    }));
    st.on_hover(|i| super::defer(move |a| {
        if a.dm.rows.get(i as usize).is_some_and(Row::focusable) && a.dm.focus != i as usize {
            a.dm.focus = i as usize;
            a.dl_menu_push();
        }
    }));
}

pub fn wire_models(app: &App, ui: &AppWindow) {
    ui.global::<DownloadsMenuState>().set_rows(ModelRc::from(app.dm.model.clone()));
}

pub(crate) fn local_time(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&chrono::Local).format("%d.%m %H:%M").to_string()).unwrap_or_else(|| i18n::tr_ref("downloads_menu.timeless").into())
}

impl App {
    pub(crate) fn dl_menu_data(&self) -> Option<Downloads> {
        let mut d = self.live.downloads.clone()?;
        // Steam's store name first; the library name only when the store has none.
        for i in d.items.iter_mut().filter(|i| i.name.is_none()) {
            i.name = self.gidx.get(&i.appid).map(|&gi| self.games()[gi].name.clone());
        }
        Some(d)
    }

    /// A on «Загрузки» in the side menu.
    pub fn dl_menu_open(&mut self) {
        self.dm.view = View::List;
        self.dm.back.clear();
        self.dm.focus = 0;
        self.dm.scroll = 0.0;
        self.dl_menu_rebuild(true);
        self.dl_menu_load_settings();
        self.build_menu();
    }

    /// Drop the panel without touching the side menu (the menu itself is closing or reopening).
    pub fn dl_menu_reset(&mut self) {
        if self.dm.is_open() {
            self.dm.view = View::Closed;
            self.ui().global::<DownloadsMenuState>().set_open(false);
        }
    }

    /// New download snapshot from Steam.
    pub fn dl_menu_refresh(&mut self) {
        if self.dm.is_open() {
            self.dl_menu_rebuild(false);
        }
    }

    fn dl_menu_rebuild(&mut self, first_focus: bool) {
        let d = self.dl_menu_data();
        let settings = self.dm.settings.clone().flatten();
        let t = Txt { tr: &crate::assets::t_or, time: &local_time, size: &crate::app::fmt_size };
        let keep = self.dm.rows.get(self.dm.focus).map(|r| r.target.clone());
        let (title, rows) = match self.dm.view {
            View::Closed => return,
            View::List => (crate::assets::t_or("#MainTabsDownload", i18n::tr_ref("downloads_menu.downloads")), list_rows(d.as_ref(), &t)),
            View::Actions(id) => (d.as_ref().and_then(|d| d.items.iter().find(|i| i.appid == id)).map(item_name).unwrap_or_else(|| format!("App {id}")), action_rows(d.as_ref(), id, settings.as_ref(), &t)),
            View::Settings => (i18n::tr_ref("downloads_menu.loading_settings").to_string(), match &self.dm.settings {
                None => vec![Row::note(i18n::tr_ref("downloads_menu.reading_the_steam_settings"))],
                Some(s) => settings_rows(s.as_ref(), &t),
            }),
            View::Choice(setting) => { let (tok, fb) = setting.label(); (crate::assets::t_or(tok, fb), choice_rows(setting, settings.as_ref())) }
        };
        // Keep the cursor on the same thing when Steam's snapshot reorders the rows.
        let same = keep.filter(|k| !first_focus && *k != Target::None).and_then(|k| rows.iter().position(|r| r.target == k));
        let first = rows.iter().position(Row::focusable).unwrap_or(0);
        self.dm.focus = same.unwrap_or_else(|| {
            let f = self.dm.focus.min(rows.len().saturating_sub(1));
            if !first_focus && rows.get(f).is_some_and(Row::focusable) { f } else { first }
        });
        if first_focus {
            if let View::Choice(_) = self.dm.view {
                self.dm.focus = rows.iter().position(|r| r.check == 1).unwrap_or(first);
            }
        }
        self.dm.rows = rows;
        let ui = self.ui();
        let st = ui.global::<DownloadsMenuState>();
        st.set_title(title.into());
        st.set_status(status_line(d.as_ref(), settings.as_ref()).into());
        st.set_open(true);
        self.dl_menu_push();
    }

    fn dl_menu_push(&mut self) {
        let (ys, total) = layout(&self.dm.rows);
        if let (Some(&y), Some(r)) = (ys.get(self.dm.focus), self.dm.rows.get(self.dm.focus)) {
            // Reveal the section header above the first row of a view as well.
            let top = if self.dm.rows[..self.dm.focus].iter().all(|r| !r.focusable()) { 0.0 } else { y };
            if top < self.dm.scroll {
                self.dm.scroll = top;
            } else if y + r.height() > self.dm.scroll + VIEWPORT {
                self.dm.scroll = y + r.height() - VIEWPORT;
            }
        }
        self.dm.scroll = self.dm.scroll.clamp(0.0, (total - VIEWPORT).max(0.0));
        let v: Vec<DlMenuRow> = self.dm.rows.iter().zip(&ys).map(|(r, &y)| DlMenuRow {
            label: r.label.as_str().into(), value: r.value.as_str().into(), sub: r.sub.as_str().into(), progress: r.progress,
            kind: match r.kind { RowKind::Item => 0, RowKind::Header => 1, RowKind::Note => 2 },
            enabled: r.enabled, check: r.check, dim_bar: r.dim_bar, y, h: r.height(),
        }).collect();
        self.dm.model.set_vec(v);
        let ui = self.ui();
        let st = ui.global::<DownloadsMenuState>();
        st.set_scroll_y(self.dm.scroll);
        st.set_focus(if self.dm.rows.get(self.dm.focus).is_some_and(Row::focusable) { self.dm.focus as i32 } else { -1 });
    }

    fn dl_menu_enter(&mut self, view: View) {
        self.dm.back.push(self.dm.focus);
        self.dm.view = view;
        self.dm.scroll = 0.0;
        self.dl_menu_rebuild(true);
    }

    fn dl_menu_back(&mut self) {
        let parent = match self.dm.view {
            View::Choice(_) => View::Settings,
            View::Actions(_) | View::Settings => View::List,
            View::List | View::Closed => View::Closed,
        };
        if parent == View::Closed {
            self.dl_menu_reset();
            self.build_menu();
            return;
        }
        self.dm.view = parent;
        self.dm.focus = self.dm.back.pop().unwrap_or(0);
        self.dm.scroll = 0.0;
        self.dl_menu_rebuild(false);
    }

    pub fn handle_dl_menu(&mut self, a: Action) {
        match a {
            Action::Up | Action::Down => {
                if let Some(i) = step(&self.dm.rows, self.dm.focus, a) {
                    self.dm.focus = i;
                    self.dl_menu_push();
                }
            }
            Action::B | Action::Left => self.dl_menu_back(),
            Action::A => self.dl_menu_activate(),
            _ => {}
        }
    }

    fn dl_menu_activate(&mut self) {
        let Some(row) = self.dm.rows.get(self.dm.focus).cloned() else { return };
        if !row.enabled {
            return;
        }
        match row.target {
            Target::None => {}
            Target::Item(id) => self.dl_menu_enter(View::Actions(id)),
            Target::OpenSettings => {
                self.dl_menu_enter(View::Settings);
                self.dl_menu_load_settings();
            }
            Target::OpenPage => {
                self.dl_menu_reset();
                self.navigate(Route::Downloads);
            }
            Target::Op(Op::ViewInLibrary(id)) => {
                if let Some(&gi) = self.gidx.get(&id) {
                    self.dl_menu_reset();
                    self.sm.open = false;
                    self.open_game(gi);
                } else {
                    self.toast(i18n::tr_ref("downloads_menu.the_game_is_not_in_the_deckshell"));
                }
            }
            Target::Op(op) => {
                if let Some(js) = op.js() {
                    self.dl_menu_write("download", js);
                }
                if let View::Actions(_) = self.dm.view {
                    self.dl_menu_back();
                }
            }
            Target::Toggle(setting, on) => {
                if let Some(js) = setting_write(setting, &json!(on)) {
                    self.dl_menu_write(setting.key(), js);
                }
            }
            Target::Choose(setting) => self.dl_menu_enter(View::Choice(setting)),
            Target::Custom(_) => {
                CUSTOM.with(|t| t.borrow_mut().clear());
                super::osk::attach(self, CUSTOM_SINK);
            }
            Target::Pick(setting, v) => {
                if let Some(js) = setting_write(setting, &json!(v)) {
                    self.dl_menu_write(setting.key(), js);
                }
                self.dl_menu_back();
            }
        }
    }

    /// Send one write to Steam, then read the result back. The panel changes only from what Steam reports.
    pub(crate) fn dl_menu_write(&mut self, what: &'static str, js: String) {
        if crate::actions::dryrun() {
            eprintln!("[deckshell] DRYRUN downloads menu {what}: {js}");
            #[cfg(test)]
            WRITES.with(|w| w.borrow_mut().push(js.clone()));
            self.toast(i18n::tr_ref("downloads_menu.verification_mode_recording_on_steam_is_not"));
            return;
        }
        let Some(bridge) = crate::actions::bridge() else {
            self.toast(i18n::tr_ref("downloads_menu.steam_unavailable_change_not_sent"));
            return;
        };
        std::thread::spawn(move || {
            let sent = bridge.eval_json(&js);
            if let Err(e) = &sent {
                eprintln!("[deckshell] downloads menu {what} failed: {e}");
            }
            // Steam applies the call asynchronously; one short wait before the readback.
            std::thread::sleep(std::time::Duration::from_millis(400));
            let settings = bridge.download_settings().ok();
            let downloads = bridge.downloads().ok();
            super::defer(move |a| {
                if sent.is_err() {
                    a.toast(i18n::tr_ref("downloads_menu.steam_did_not_accept_the_change"));
                }
                a.dm.settings = Some(settings);
                match downloads {
                    // on_downloads_data refreshes the panel as well.
                    Some(d) => {
                        a.live.downloads = Some(d);
                        a.on_downloads_data();
                    }
                    None => a.dl_menu_refresh(),
                }
            });
        });
    }

    pub(crate) fn dl_menu_load_settings(&mut self) {
        self.dm.generation = self.dm.generation.wrapping_add(1);
        let generation = self.dm.generation;
        let Some(bridge) = crate::actions::bridge() else {
            self.dm.settings = Some(None);
            self.dl_menu_refresh();
            return;
        };
        std::thread::spawn(move || {
            let settings = bridge.download_settings().ok();
            super::defer(move |a| {
                if a.dm.generation == generation {
                    a.dm.settings = Some(settings);
                    a.dl_menu_refresh();
                    if a.router.current() == &Route::Downloads {
                        a.rebuild_dl();
                    }
                }
            });
        });
    }
}

#[cfg(test)]
#[path = "downloads_menu_tests.rs"]
mod tests;
