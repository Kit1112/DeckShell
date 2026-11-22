//! Downloads panel of the left side menu (STEAM → «Загрузки») and its download settings.
//!
//! Source of every rule here: Steam's own gamepad UI (`tools/cdp/build/src/sp.js`, downloads page module and
//! `downloadsStore`), see docs/downloads-menu-2026-11-15.md. Sections, item states and the calls behind each action
//! are ports of that code. A value Steam did not report is shown as unavailable and its action is not offered.
//! Nothing is mirrored optimistically: after a write the panel waits for Steam's next snapshot.
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
            false => Some(act(Op::EnableAll(false), "#Downloads_Pause_Tooltip", "Приостановить")),
            true => Some(act(Op::EnableAll(true), "#Downloads_Resume_Tooltip", "Возобновить")),
        },
        Section::Queued | Section::Scheduled | Section::NotQueued => Some(if st == Status::Error {
            act(Op::DownloadNow(appid), "#Downloads_Retry_Tooltip", "Повторить")
        } else {
            act(Op::DownloadNow(appid), "#Downloads_MoveToTop_Tooltip", "Загрузить сейчас")
        }),
        Section::Completed => Some(act(Op::ViewInLibrary(appid), "#Downloads_GoToGame_Tooltip", "Перейти к игре")),
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
        v.push(ItemAction { check: Some(i.launch_on_completion), ..act(Op::LaunchOnComplete { appid, on: !i.launch_on_completion }, "#Downloads_LaunchWhenComplete", "Запустить после загрузки") });
        if let (Some(rate), Some(suspended)) = (throttle_rate, d.overview.throttling_suspended) {
            if rate > 0 {
                v.push(ItemAction { check: Some(suspended), ..act(Op::SuspendThrottle(!suspended), "#Downloads_OverrideDownloadThrottle", "Не ограничивать скорость этой загрузки") });
            }
        }
    }
    if section == Section::Queued {
        let last = d.items.iter().filter(|x| x.queue_index != -1 && x.deferred_time == 0).map(|x| x.queue_index).max().unwrap_or(0);
        // Index 0 is the active slot: moving there is "download now" above. Only moves inside the waiting queue.
        if i.queue_index >= 2 {
            v.push(act(Op::QueueIndex(appid, i.queue_index as u32 - 1), "", "Выше в очереди"));
        }
        if i.queue_index >= 1 && i.queue_index < last {
            v.push(act(Op::QueueIndex(appid, i.queue_index as u32 + 1), "", "Ниже в очереди"));
        }
    }
    if matches!(section, Section::Queued | Section::Scheduled) {
        v.push(act(Op::Unqueue(appid), "", "Убрать из очереди (не скачивать)"));
    }
    // sp.js bShowRemoveButton: not in "not queued", not while active, not when disabled by the app.
    if section != Section::NotQueued && !matches!(st, Status::Active | Status::ActiveDownloading | Status::DisabledByApp) {
        let reenable = i.queue_index == 0 && d.overview.paused == Some(true);
        v.push(if i.completed {
            act(Op::Remove { appid, reenable }, "#Downloads_RemoveFromList_Tooltip", "Убрать из списка")
        } else {
            act(Op::Remove { appid, reenable }, "#Downloads_RemoveFromQueue_Tooltip", "Удалить из очереди")
        });
    }
    if section != Section::Completed {
        v.push(act(Op::ViewInLibrary(appid), "#Generic_ViewInLibrary", "Показать в библиотеке"));
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
            Setting::Limit => ("#DownloadSettings_BandwidthLimit", "Ограничить скорость скачивания"),
            Setting::RateBits => ("#DownloadSettings_DisplayInBitsPerSecond", "Скорость в битах в секунду"),
            Setting::UpdateBehavior => ("#DownloadSettings_GameUpdateTiming", "Обновление игр"),
            Setting::Restrict => ("#DownloadSettings_ScheduleAutoUpdates", "Автообновление по расписанию"),
            Setting::RestrictStart => ("#DownloadSettings_RestrictUpdatesFrom", "Обновлять с"),
            Setting::RestrictEnd => ("#DownloadSettings_RestrictUpdatesTo", "до"),
            Setting::WhileRunning => ("#DownloadSettings_AllowDownloadDuringGameplay", "Разрешить загрузки во время игры"),
            Setting::ThrottleStreaming => ("#DownloadSettings_ThrottleDuringStreaming", "Ограничить загрузки во время трансляции"),
            Setting::PeerContent => ("#DownloadSettings_PeerContent", "Передача файлов по локальной сети"),
            Setting::Region => ("#DownloadSettings_SelectRegion", "Регион загрузки"),
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

const UNAVAILABLE: &str = "недоступно";

/// Stored unit is kbit/s (sp.js: stored = floor(input * (bits ? 1 : 8))); shown in the unit Steam's own field uses.
pub fn limit_label(rate: i64, bits: Option<bool>) -> String {
    if rate <= 0 {
        return "Без ограничения".into();
    }
    match bits {
        Some(false) if rate % 8 == 0 => format!("{} КБ/с", rate / 8),
        Some(false) => crate::app::comma(format!("{:.1} КБ/с", rate as f64 / 8.0)),
        _ => format!("{rate} кбит/с"),
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
            (BEHAVIOR_KEEP_UPDATED, "Разрешить Steam выбирать время обновления".into()),
            (AUTO_UPDATE_ON_LAUNCH, "Обновлять только при запуске игры".into()),
        ],
        Setting::RestrictStart | Setting::RestrictEnd => s.hours.clone(),
        Setting::PeerContent => vec![
            (0, "Выключено".into()),
            (1, "Только мои устройства".into()),
            (2, "Мои устройства и друзья".into()),
            (3, "Любой пользователь".into()),
        ],
        _ => vec![],
    }
}

fn choice_label(setting: Setting, s: &DownloadSettings) -> String {
    let Some(v) = setting.number(s) else { return UNAVAILABLE.into() };
    if setting == Setting::Limit {
        return limit_label(v, s.rate_bits);
    }
    let named = if setting == Setting::Region { s.regions.clone() } else { choices(setting, s) };
    named.into_iter().find(|(x, _)| *x == v).map(|(_, l)| l).unwrap_or_else(|| format!("неизвестно ({v})"))
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
    let (v, units) = if bits == Some(true) { (bytes_per_sec as f64 * 8.0, ["кбит/с", "Мбит/с"]) } else { (bytes_per_sec as f64, ["КБ/с", "МБ/с"]) };
    crate::app::comma(if v >= 1e6 { format!("{:.1} {}", v / 1e6, units[1]) } else { format!("{:.0} {}", v / 1e3, units[0]) })
}

pub(crate) fn state_text(state: &str, t: &Txt) -> String {
    let fallback = match state {
        "Downloading" => "Загрузка",
        "Starting" => "Запуск",
        "Validating" | "Verifying" => "Проверка",
        "Preallocating" => "Выделение места",
        "Updating" | "Patching" => "Обновление",
        "Unpacking" => "Распаковка",
        "Installing" => "Установка",
        "Finalizing" => "Завершение",
        "Stopping" => "Остановка",
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
        s if s <= 60 => format!("осталось {s} с"),
        s => format!("осталось {} мин", s / 60),
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
        Status::ActivePaused => (t.tr)("#Downloads_State_Paused", "Приостановлено"),
        Status::Queued => (t.tr)("#Downloads_State_Queued", "В очереди"),
        Status::QueuedNext => (t.tr)("#Downloads_State_UpNext", "Далее"),
        Status::Scheduled => (t.tr)("#Downloads_State_Scheduled", "Запланировано"),
        Status::RemovedFromQueue => (t.tr)("#Downloads_State_Paused", "Приостановлено"),
        Status::Error => "Ошибка".into(),
        Status::DisabledByUser | Status::DisabledByApp => "Отключено".into(),
        Status::Completed => (t.tr)("#Downloads_State_Complete", "Завершено"),
        Status::Unknown => "Неизвестно".into(),
    };
    let mut sub: Vec<String> = Vec::new();
    match st {
        Status::Error => {
            let code = i.update_result.unwrap_or_default();
            sub.push((t.tr)(&format!("#Steam_AppUpdateError_{code}"), &format!("Ошибка обновления (код {code})")));
            sub.extend(i.update_error.clone());
        }
        Status::Scheduled => sub.push(format!("на {}", (t.time)(i.deferred_time))),
        Status::DisabledByUser => sub.push((t.tr)("#Downloads_AutoUpdate_DoNotUpdate", "Обновляется только при запуске")),
        Status::DisabledByApp => sub.push((t.tr)("#Downloads_State_UpdatesDiabledByApp", "Обновления отключены приложением")),
        Status::Completed => sub.push((t.time)(i.completed_time)),
        Status::Unknown => sub.push("Steam не сообщил состояние".into()),
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
        sub.push("запуск после загрузки".into());
    }
    row.dim_bar = !matches!(st, Status::ActiveDownloading | Status::Active);
    row.sub = sub.join(" · ");
    row
}

fn section_title(s: Section, n: usize, t: &Txt) -> String {
    let (tok, fb) = match s {
        Section::Active => ("#Downloads_Section_Active", "Сейчас"),
        Section::Queued => ("#Downloads_Section_Current", "Далее"),
        Section::Scheduled => ("#Downloads_Section_Scheduled", "Запланировано"),
        Section::NotQueued => ("#Downloads_Section_NotQueued", "Не в очереди"),
        Section::Completed => ("#Downloads_Section_Completed", "Завершено"),
    };
    format!("{} ({n})", (t.tr)(tok, fb).to_uppercase())
}

/// One-line state of the whole download system. `None` data = the bridge delivered nothing.
pub fn status_line(d: Option<&Downloads>, s: Option<&DownloadSettings>) -> String {
    let Some(d) = d else { return "Нет данных от Steam".into() };
    let ov = &d.overview;
    let bits = s.and_then(|s| s.rate_bits);
    let mut parts: Vec<String> = Vec::new();
    match ov.paused {
        Some(true) => parts.push("Загрузки приостановлены".into()),
        None => parts.push("Пауза: неизвестно".into()),
        Some(false) if ov.appid != 0 && ov.state != "None" => {
            parts.push(if ov.is_upload { "Отправка".into() } else { state_text(&ov.state, &Txt { tr: &|_, f| f.to_string(), time: &|_| String::new(), size: &|_| String::new() }) });
            if ov.net_bytes_per_sec > 0 {
                parts.push(speed(ov.net_bytes_per_sec, bits));
            }
        }
        Some(false) => parts.push("Нет активной загрузки".into()),
    }
    if let Some(rate) = s.and_then(|s| s.throttle_rate).filter(|&r| r > 0) {
        parts.push(format!("лимит {}{}", limit_label(rate, bits), if ov.throttling_suspended == Some(true) { " (снят для текущей)" } else { "" }));
    }
    if let Some(host) = ov.lan_peer_hostname.as_deref().filter(|h| !h.is_empty()) {
        parts.push(format!("локальная сеть: {host}"));
    }
    parts.join(" · ")
}

/// Main view: every stock section that has items, then the global rows.
pub fn list_rows(d: Option<&Downloads>, t: &Txt) -> Vec<Row> {
    let mut rows = Vec::new();
    match d {
        None => rows.push(Row::note("Steam не передал список загрузок: состояние неизвестно, действия недоступны.")),
        Some(d) => {
            let secs = sections(d);
            if secs.iter().all(|(_, v)| v.is_empty()) {
                rows.push(Row::note((t.tr)("#Downloads_NoTransfers", "Нет загрузок")));
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
    rows.push(Row::header("УПРАВЛЕНИЕ"));
    let mut all = match d.and_then(|d| d.overview.paused) {
        Some(true) => Row::item("Возобновить все загрузки", Target::Op(Op::EnableAll(true))),
        Some(false) => Row::item("Приостановить все загрузки", Target::Op(Op::EnableAll(false))),
        None => Row { enabled: false, value: UNAVAILABLE.into(), ..Row::item("Приостановить все загрузки", Target::None) },
    };
    all.check = -1;
    rows.push(all);
    rows.push(Row::item((t.tr)("#DeckShell_DownloadSettings", "Настройки загрузок"), Target::OpenSettings));
    rows.push(Row::item("Открыть страницу загрузок", Target::OpenPage));
    rows
}

/// Action view of one item. Empty item list (the download is gone) leaves only a note.
pub fn action_rows(d: Option<&Downloads>, appid: u32, s: Option<&DownloadSettings>, t: &Txt) -> Vec<Row> {
    let Some(d) = d else { return vec![Row::note("Steam не передал список загрузок.")] };
    let Some((section, i)) = find(d, appid) else { return vec![Row::note("Этой загрузки больше нет в списке Steam.")] };
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
        rows.push(Row { enabled: false, value: UNAVAILABLE.into(), ..Row::item("Приостановить / возобновить", Target::None) });
    }
    if i.queue_index == 0 && (s.and_then(|s| s.throttle_rate).is_none() || d.overview.throttling_suspended.is_none()) {
        rows.push(Row { enabled: false, value: UNAVAILABLE.into(), ..Row::item((t.tr)("#Downloads_OverrideDownloadThrottle", "Не ограничивать скорость этой загрузки"), Target::None) });
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
        rows.push(Row::note("Steam не передал настройки загрузок: значения неизвестны, изменение недоступно."));
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
                row.value = UNAVAILABLE.into();
            }
            Some(s) => match setting.toggle(s) {
                Some(Some(on)) => {
                    row.check = i32::from(on);
                    row.target = Target::Toggle(setting, !on);
                }
                Some(None) => {
                    row.enabled = false;
                    row.value = UNAVAILABLE.into();
                }
                None => {
                    row.value = choice_label(setting, s);
                    if setting == Setting::Region {
                        // Steam asks for a client restart after a region change; not offered from this panel.
                        row.enabled = false;
                        row.sub = "Меняется на странице настроек Steam (нужен перезапуск клиента)".into();
                    } else if setting.number(s).is_none() || choices(setting, s).is_empty() {
                        row.enabled = false;
                        row.value = UNAVAILABLE.into();
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
    let Some(s) = s else { return vec![Row::note("Steam не передал настройки загрузок.")] };
    let cur = setting.number(s);
    let mut rows: Vec<Row> = choices(setting, s).into_iter().map(|(v, label)| {
        let mut r = Row::item(label, Target::Pick(setting, v));
        r.check = i32::from(cur == Some(v));
        r
    }).collect();
    if setting == Setting::Limit {
        // Steam has a free text field here; the ladder above is DeckShell's own, this row is the field
        let typed = CUSTOM.with(|c| c.borrow().clone());
        let unit = if s.rate_bits == Some(true) { "кбит/с" } else { "КБ/с" };
        rows.push(Row::item(if typed.is_empty() { "Своё значение…".to_string() } else { format!("Своё значение: {typed} {unit}") }, Target::Custom(setting)));
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
    chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&chrono::Local).format("%d.%m %H:%M").to_string()).unwrap_or_else(|| "время неизвестно".into())
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
            View::List => (crate::assets::t_or("#MainTabsDownload", "Загрузки"), list_rows(d.as_ref(), &t)),
            View::Actions(id) => (d.as_ref().and_then(|d| d.items.iter().find(|i| i.appid == id)).map(item_name).unwrap_or_else(|| format!("App {id}")), action_rows(d.as_ref(), id, settings.as_ref(), &t)),
            View::Settings => ("Настройки загрузок".to_string(), match &self.dm.settings {
                None => vec![Row::note("Чтение настроек Steam…")],
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
                    self.toast("Игры нет в библиотеке DeckShell");
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
            self.toast("Проверочный режим: запись в Steam не отправлена");
            return;
        }
        let Some(bridge) = crate::actions::bridge() else {
            self.toast("Steam недоступен: изменение не отправлено");
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
                    a.toast("Steam не принял изменение");
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
mod tests {
    use super::*;
    use steam_bridge::{parse_download_settings, parse_downloads};

    const FIXTURE: &str = include_str!("downloads_menu_fixture.json");

    fn fixture() -> (Downloads, DownloadSettings) {
        let v: Value = serde_json::from_str(FIXTURE).unwrap();
        (parse_downloads(&v["downloads"]).unwrap(), parse_download_settings(&v["settings"]).unwrap())
    }

    fn txt() -> Txt<'static> {
        Txt { tr: &|_, fallback| fallback.to_string(), time: &|ts| format!("T{ts}"), size: &|b| format!("{b}B") }
    }

    fn ids(v: &[&DownloadItem]) -> Vec<u32> {
        v.iter().map(|i| i.appid).collect()
    }

    #[test]
    fn sections_follow_the_stock_store_getters() {
        let (d, _) = fixture();
        let s = sections(&d);
        assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![Section::Active, Section::Queued, Section::Scheduled, Section::NotQueued, Section::Completed]);
        assert_eq!(ids(&s[0].1), vec![100], "head of the queue is the active section");
        assert_eq!(ids(&s[1].1), vec![200, 300, 350], "rest of the queue in queue order");
        assert_eq!(ids(&s[2].1), vec![400], "scheduled: no queue index, deferred time set");
        assert_eq!(ids(&s[3].1), vec![600, 500, 700], "not queued, by name");
        assert_eq!(ids(&s[4].1), vec![900, 800], "completed, newest first");
    }

    #[test]
    fn item_states_are_the_stock_enum() {
        let (d, _) = fixture();
        let st = |id: u32| { let (_, i) = find(&d, id).unwrap(); status(i, item_overview(i, &d.overview)) };
        assert_eq!(st(100), Status::ActiveDownloading);
        assert_eq!(st(200), Status::QueuedNext);
        assert_eq!(st(300), Status::Queued);
        assert_eq!(st(350), Status::Error, "an update error wins over the queue position");
        assert_eq!(st(400), Status::Scheduled);
        assert_eq!(st(500), Status::RemovedFromQueue);
        assert_eq!(st(600), Status::DisabledByUser);
        assert_eq!(st(700), Status::DisabledByApp);
        assert_eq!(st(800), Status::Completed);
        let mut paused = d.clone();
        paused.overview.paused = Some(true);
        paused.overview.state = "None".into();
        paused.items[0].active = false;
        let (_, head) = find(&paused, 100).unwrap();
        assert_eq!(status(head, item_overview(head, &paused.overview)), Status::ActivePaused);
        // The overview never applies to an item that is neither active nor at the head.
        let (_, second) = find(&paused, 200).unwrap();
        assert_eq!(item_overview(second, &paused.overview), None);
    }

    #[test]
    fn ok_action_is_the_stock_button_of_each_section() {
        let js = |a: Option<ItemAction>| a.and_then(|a| a.op.js());
        assert_eq!(js(primary(Section::Active, Status::ActiveDownloading, Some(false), 100)).as_deref(), Some(r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#));
        assert_eq!(js(primary(Section::Active, Status::ActivePaused, Some(true), 100)).as_deref(), Some(r#"(SteamClient.Downloads.EnableAllDownloads(true,"0"),true)"#));
        assert_eq!(primary(Section::Active, Status::Active, None, 100), None, "unknown pause state offers no pause/resume");
        for s in [Section::Queued, Section::Scheduled, Section::NotQueued] {
            assert_eq!(js(primary(s, Status::Queued, Some(false), 7)).as_deref(), Some(r#"(SteamClient.Downloads.ResumeAppUpdate(7,"0"),true)"#));
        }
        assert_eq!(primary(Section::Queued, Status::Error, Some(false), 7).unwrap().token, "#Downloads_Retry_Tooltip");
        assert_eq!(primary(Section::NotQueued, Status::DisabledByApp, Some(false), 7), None);
        assert_eq!(primary(Section::Completed, Status::Completed, Some(false), 7).unwrap().op, Op::ViewInLibrary(7));
    }

    #[test]
    fn item_actions_match_the_stock_menu_and_drag_rules() {
        let (d, s) = fixture();
        let ops = |d: &Downloads, id: u32, rate: Option<i64>| item_actions(d, id, rate).into_iter().map(|a| a.op).collect::<Vec<_>>();
        // Active: pause, launch-when-complete (currently off), throttle override (limit set), view. No remove while active.
        assert_eq!(ops(&d, 100, s.throttle_rate), vec![Op::EnableAll(false), Op::LaunchOnComplete { appid: 100, on: true }, Op::SuspendThrottle(true), Op::ViewInLibrary(100)]);
        assert!(!ops(&d, 100, Some(0)).contains(&Op::SuspendThrottle(true)), "no override without a limit");
        assert!(!ops(&d, 100, None).iter().any(|o| matches!(o, Op::SuspendThrottle(_))), "unknown limit: not offered");
        // Second in the queue: cannot move up into the active slot by SetQueueIndex; can move down.
        assert_eq!(ops(&d, 200, None), vec![Op::DownloadNow(200), Op::QueueIndex(200, 2), Op::Unqueue(200), Op::Remove { appid: 200, reenable: false }, Op::ViewInLibrary(200)]);
        assert_eq!(ops(&d, 300, None), vec![Op::DownloadNow(300), Op::QueueIndex(300, 1), Op::QueueIndex(300, 3), Op::Unqueue(300), Op::Remove { appid: 300, reenable: false }, Op::ViewInLibrary(300)]);
        // Last in the queue: only up.
        assert!(ops(&d, 350, None).contains(&Op::QueueIndex(350, 2)) && !ops(&d, 350, None).contains(&Op::QueueIndex(350, 4)));
        assert_eq!(ops(&d, 400, None), vec![Op::DownloadNow(400), Op::Unqueue(400), Op::Remove { appid: 400, reenable: false }, Op::ViewInLibrary(400)]);
        // Not queued: download now only; stock shows no remove button there.
        assert_eq!(ops(&d, 500, None), vec![Op::DownloadNow(500), Op::ViewInLibrary(500)]);
        assert_eq!(ops(&d, 700, None), vec![Op::ViewInLibrary(700)], "disabled by the app: no button");
        assert_eq!(ops(&d, 800, None), vec![Op::ViewInLibrary(800), Op::Remove { appid: 800, reenable: false }]);
        assert_eq!(item_actions(&d, 800, None)[1].token, "#Downloads_RemoveFromList_Tooltip");
        // Paused head of the queue: remove also re-enables downloads, launch-when-complete is cleared with appid 0.
        let mut p = d.clone();
        p.overview.paused = Some(true);
        p.items[0].active = false;
        p.items[0].launch_on_completion = true;
        let a = item_actions(&p, 100, None);
        assert_eq!(a[0].op, Op::EnableAll(true));
        assert_eq!(a[1].op.js().as_deref(), Some("(SteamClient.Downloads.SetLaunchOnUpdateComplete(0),true)"));
        assert_eq!(a[1].check, Some(true));
        assert!(a.iter().any(|x| x.op == Op::Remove { appid: 100, reenable: true }));
        assert!(item_actions(&d, 12345, None).is_empty(), "an item Steam no longer lists has no actions");
    }

    #[test]
    fn list_shows_sections_states_and_progress() {
        let (d, _) = fixture();
        let rows = list_rows(Some(&d), &txt());
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec![
            "СЕЙЧАС (1)", "Alpha", "ДАЛЕЕ (3)", "Beta", "Gamma", "Broken", "ЗАПЛАНИРОВАНО (1)", "Delta",
            "НЕ В ОЧЕРЕДИ (3)", "Epsilon", "Paused game", "Zeta", "ЗАВЕРШЕНО (2)", "Newest", "Done",
            "УПРАВЛЕНИЕ", "Приостановить все загрузки", "Настройки загрузок", "Открыть страницу загрузок",
        ]);
        let row = |name: &str| rows.iter().find(|r| r.label == name).unwrap();
        assert_eq!(row("Alpha").value, "Загрузка");
        assert_eq!(row("Alpha").sub, "4000B / 10000B · 41% · 2,0 МБ/с · осталось 5 мин");
        assert_eq!(row("Alpha").progress, 0.41);
        assert!(!row("Alpha").dim_bar);
        assert_eq!((row("Beta").value.as_str(), row("Beta").sub.as_str(), row("Beta").progress), ("Далее", "6000B", -1.0));
        assert_eq!((row("Gamma").sub.as_str(), row("Gamma").progress), ("500B / 2000B · 25%", 0.25));
        assert_eq!((row("Broken").value.as_str(), row("Broken").sub.as_str()), ("Ошибка", "Ошибка обновления (код 6) · disk write failure · 100B"));
        assert_eq!((row("Delta").value.as_str(), row("Delta").sub.as_str()), ("Запланировано", "на T1790000000 · 300B"));
        assert_eq!(row("Paused game").value, "Приостановлено");
        assert_eq!(row("Epsilon").sub, "Обновляется только при запуске");
        assert_eq!(row("Zeta").sub, "Обновления отключены приложением");
        assert_eq!((row("Done").value.as_str(), row("Done").sub.as_str()), ("Завершено", "T1780000000"));
        assert_eq!(row("Alpha").target, Target::Item(100));
        assert_eq!(row("Приостановить все загрузки").target, Target::Op(Op::EnableAll(false)));
        // Headers are never focusable; the cursor walks items only and stops at both ends.
        assert_eq!(step(&rows, 1, Action::Up), None);
        assert_eq!(step(&rows, 1, Action::Down), Some(3));
        assert_eq!(step(&rows, 5, Action::Down), Some(7));
        assert_eq!(step(&rows, rows.len() - 1, Action::Down), None);
        assert_eq!(step(&rows, 16, Action::Up), Some(14));
    }

    #[test]
    fn unknown_data_is_shown_as_unavailable_not_guessed() {
        let rows = list_rows(None, &txt());
        assert_eq!(rows[0].kind, RowKind::Note);
        assert!(!rows.iter().any(|r| matches!(r.target, Target::Item(_) | Target::Op(_))), "no download actions without Steam data");
        let all = rows.iter().find(|r| r.label == "Приостановить все загрузки").unwrap();
        assert_eq!((all.enabled, all.value.as_str()), (false, UNAVAILABLE));
        assert_eq!(status_line(None, None), "Нет данных от Steam");
        // A snapshot without the pause flag: no pause/resume anywhere, the rest still listed.
        let (mut d, s) = fixture();
        d.overview.paused = None;
        d.overview.throttling_suspended = None;
        let rows = list_rows(Some(&d), &txt());
        assert!(!rows.iter().any(|r| matches!(r.target, Target::Op(Op::EnableAll(_)))));
        let actions = action_rows(Some(&d), 100, Some(&s), &txt());
        assert!(!actions.iter().any(|r| matches!(r.target, Target::Op(Op::EnableAll(_) | Op::SuspendThrottle(_)))));
        assert_eq!(actions.iter().filter(|r| !r.enabled && r.value == UNAVAILABLE).count(), 2);
        assert!(status_line(Some(&d), Some(&s)).starts_with("Пауза: неизвестно"));
        // Empty Steam list is a real answer, not an error.
        let empty = Downloads { overview: DownloadOverview { state: "None".into(), paused: Some(false), ..Default::default() }, history: vec![], items: vec![] };
        assert_eq!(list_rows(Some(&empty), &txt())[0].label, "Нет загрузок");
        assert_eq!(status_line(Some(&empty), None), "Нет активной загрузки");
        assert_eq!(action_rows(Some(&empty), 100, None, &txt()).len(), 1, "a vanished item leaves only the note");
        // An item without a state Steam recognises stays "unknown".
        let odd = DownloadItem { appid: 1, queue_index: -1, ..Default::default() };
        assert_eq!(status(&odd, None), Status::Unknown);
    }

    #[test]
    fn settings_rows_follow_steam_values() {
        let (_, s) = fixture();
        let rows = settings_rows(Some(&s), &txt());
        let got: Vec<(&str, &str, i32, bool)> = rows.iter().map(|r| (r.label.as_str(), r.value.as_str(), r.check, r.enabled)).collect();
        assert_eq!(got, vec![
            ("Ограничить скорость скачивания", "1000 КБ/с", -1, true),
            ("Скорость в битах в секунду", "", 0, true),
            ("Обновление игр", "Разрешить Steam выбирать время обновления", -1, true),
            ("Автообновление по расписанию", "", 1, true),
            ("Обновлять с", "01:00", -1, true),
            ("до", "05:00", -1, true),
            ("Разрешить загрузки во время игры", "", 0, true),
            ("Ограничить загрузки во время трансляции", "", 1, true),
            ("Передача файлов по локальной сети", "Только мои устройства", -1, true),
            ("Регион загрузки", "Poland - Katowice", -1, false),
        ]);
        assert_eq!(rows[1].target, Target::Toggle(Setting::RateBits, true));
        assert_eq!(rows[4].target, Target::Choose(Setting::RestrictStart));
        // "Update on launch" hides the schedule, as in Steam.
        let on_launch = DownloadSettings { update_behavior: Some(1), ..s.clone() };
        assert!(!settings_rows(Some(&on_launch), &txt()).iter().any(|r| r.label.contains("расписанию") || r.label == "до"));
        let no_schedule = DownloadSettings { restrict_auto_updates: Some(false), ..s.clone() };
        assert_eq!(settings_rows(Some(&no_schedule), &txt()).len(), 8);
        // Nothing read: every row unavailable, none writable.
        let none = settings_rows(None, &txt());
        assert_eq!(none[0].kind, RowKind::Note);
        assert!(none[1..].iter().all(|r| !r.enabled && r.value == UNAVAILABLE && r.target == Target::None));
        // Partly reported: only the missing values are unavailable.
        let partial = DownloadSettings { rate_bits: None, peer_content: None, hours: vec![], ..s.clone() };
        let rows = settings_rows(Some(&partial), &txt());
        let row = |name: &str| rows.iter().find(|r| r.label == name).unwrap();
        assert!(!row("Скорость в битах в секунду").enabled);
        assert!(!row("Передача файлов по локальной сети").enabled);
        assert!(!row("Обновлять с").enabled, "no hour list from Steam: nothing to choose from");
        assert_eq!(row("Ограничить скорость скачивания").value, "8000 кбит/с", "unit setting unknown: the stored unit is shown");
        let unknown_enum = DownloadSettings { update_behavior: Some(9), ..s.clone() };
        assert_eq!(settings_rows(Some(&unknown_enum), &txt())[2].value, "неизвестно (9)");
    }

    #[test]
    fn setting_choices_and_writes() {
        let (_, s) = fixture();
        let limit = choices(Setting::Limit, &s);
        assert_eq!(limit[0], (0, "Без ограничения".to_string()));
        assert!(limit.contains(&(8000, "1000 КБ/с".to_string())), "current value is in the list");
        assert!(limit.windows(2).all(|w| w[0].0 < w[1].0));
        let bits = DownloadSettings { rate_bits: Some(true), throttle_rate: Some(3333), ..s.clone() };
        assert!(choices(Setting::Limit, &bits).contains(&(3333, "3333 кбит/с".to_string())));
        assert_eq!(limit_label(8004, Some(false)), "1000,5 КБ/с");
        assert_eq!(choices(Setting::RestrictStart, &s).len(), 3);
        assert_eq!(choices(Setting::PeerContent, &s).iter().map(|c| c.0).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert_eq!(choices(Setting::UpdateBehavior, &s).iter().map(|c| c.0).collect::<Vec<_>>(), vec![3, 1]);
        let picks = choice_rows(Setting::PeerContent, Some(&s));
        assert_eq!(picks.iter().map(|r| r.check).collect::<Vec<_>>(), vec![0, 1, 0, 0]);
        assert_eq!(picks[2].target, Target::Pick(Setting::PeerContent, 2));
        // download_throttle_rate is field 8012 (varint): tag e0 f4 03, value 8000 = c0 3e.
        assert_eq!(setting_write(Setting::Limit, &json!(8000)).as_deref(), Some(r#"SteamClient.Settings.SetSetting("4PQDwD4=")"#));
        for setting in [Setting::RateBits, Setting::Restrict, Setting::WhileRunning, Setting::ThrottleStreaming] {
            assert!(setting_write(setting, &json!(true)).is_some(), "{setting:?}");
        }
        for setting in [Setting::UpdateBehavior, Setting::RestrictStart, Setting::RestrictEnd, Setting::PeerContent] {
            assert!(setting_write(setting, &json!(1)).is_some(), "{setting:?}");
        }
        assert_eq!(setting_write(Setting::Region, &json!(1)), None, "region is read-only here");
    }

    #[test]
    fn status_line_reports_speed_limit_and_pause() {
        let (d, s) = fixture();
        assert_eq!(status_line(Some(&d), Some(&s)), "Загрузка · 2,0 МБ/с · лимит 1000 КБ/с");
        let bits = DownloadSettings { rate_bits: Some(true), ..s.clone() };
        assert_eq!(status_line(Some(&d), Some(&bits)), "Загрузка · 16,0 Мбит/с · лимит 8000 кбит/с");
        let mut p = d.clone();
        p.overview.paused = Some(true);
        p.overview.throttling_suspended = Some(true);
        assert_eq!(status_line(Some(&p), Some(&s)), "Загрузки приостановлены · лимит 1000 КБ/с (снят для текущей)");
    }

    /// Side menu glue: A on «Загрузки» opens the panel next to the rail, B walks back to the rail, closing the menu
    /// drops the panel. No bridge in tests: settings come back as "not delivered" and no write is sent.
    #[test]
    fn side_menu_opens_and_closes_the_panel() {
        let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        )));
        let ui = AppWindow::new().unwrap();
        let ds = std::sync::Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
        crate::screens::downloads_menu::wire_models(&a, &ui);
        a.live.downloads = Some(fixture().0);
        let route = a.router.current().clone();
        a.toggle_side();
        assert!(a.sm.open);
        let idx = a.sm.acts.iter().position(|x| *x == crate::screens::steam_menu::MenuAct::Downloads).unwrap();
        a.menu_activate(idx);
        assert_eq!(a.dm.view, View::List);
        assert!(a.sm.open, "the rail stays open next to the panel");
        assert_eq!(a.router.current(), &route, "no page navigation");
        let st = ui.global::<DownloadsMenuState>();
        assert!(st.get_open());
        assert_eq!(a.dm.rows[a.dm.focus].label, "Alpha");
        assert_eq!(a.dm.settings, Some(None), "no bridge: settings not delivered");
        // Down walks items only (the next header is skipped).
        a.handle(Action::Down);
        assert_eq!(a.dm.rows[a.dm.focus].label, "Beta");
        // A on an item opens its actions; B returns to the same row.
        a.handle(Action::A);
        assert_eq!(a.dm.view, View::Actions(200));
        a.handle(Action::B);
        assert_eq!((a.dm.view, a.dm.rows[a.dm.focus].label.as_str()), (View::List, "Beta"));
        // Settings without Steam data: every row unavailable, nothing focusable to write.
        a.dm.focus = a.dm.rows.iter().position(|r| r.target == Target::OpenSettings).unwrap();
        a.handle(Action::A);
        assert_eq!(a.dm.view, View::Settings);
        assert!(a.dm.rows.iter().all(|r| !r.enabled));
        a.handle(Action::A);
        assert_eq!(a.dm.view, View::Settings, "a disabled row does nothing");
        a.handle(Action::B);
        a.handle(Action::B);
        assert_eq!(a.dm.view, View::Closed);
        assert!(a.sm.open && !st.get_open(), "B from the list returns to the rail");
        // Closing the side menu (STEAM; called directly to skip the 300 ms guide debounce) drops the panel; reopening the menu does not bring it back.
        a.menu_activate(idx);
        assert!(a.dm.is_open());
        a.toggle_side();
        assert!(!a.sm.open && !a.dm.is_open() && !st.get_open());
        a.toggle_side();
        assert!(a.sm.open && !a.dm.is_open());
        // A write without a bridge (or in dry run) changes nothing locally: the panel waits for Steam.
        a.menu_activate(idx);
        let before = a.live.downloads.clone();
        a.dm.focus = a.dm.rows.iter().position(|r| r.target == Target::Op(Op::EnableAll(false))).unwrap();
        a.handle(Action::A);
        assert_eq!(a.live.downloads, before);
        assert_eq!(a.dm.view, View::List);
    }

    /// The panel drawn by the software renderer from the fixture: list, item actions, settings. PC render only.
    /// With DECKSHELL_GALLERY_DIR set the frames are written there.
    #[test]
    fn panel_renders_from_fixture() {
        use crate::{MenuItem, SteamMenuState};
        let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        )));
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        let menu = ui.global::<SteamMenuState>();
        let names = ["Главная", "Библиотека", "Магазин", "Друзья и чат", "Медиа", "Загрузки", "Настройки", "Выключение"];
        menu.set_items(ModelRc::new(VecModel::from(names.iter().enumerate().map(|(i, s)| MenuItem { label: (*s).into(), current: i == 5, ..Default::default() }).collect::<Vec<_>>())));
        menu.set_row_y(ModelRc::new(VecModel::from((0..names.len()).map(|i| 52.5 + 72.0 * i as f32).collect::<Vec<_>>())));
        menu.set_content_h(681.0);
        menu.set_focus(-1);
        menu.set_game_focus(-1);
        menu.set_open(true);
        let st = ui.global::<DownloadsMenuState>();
        let (d, s) = fixture();
        let out = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = &out { std::fs::create_dir_all(dir).unwrap(); }
        let shot = |name: &str, open: bool, title: &str, rows: &[Row], focus: i32| -> Vec<u8> {
            let (ys, _) = layout(rows);
            st.set_rows(ModelRc::new(VecModel::from(rows.iter().zip(&ys).map(|(r, &y)| DlMenuRow {
                label: r.label.as_str().into(), value: r.value.as_str().into(), sub: r.sub.as_str().into(), progress: r.progress,
                kind: match r.kind { RowKind::Item => 0, RowKind::Header => 1, RowKind::Note => 2 },
                enabled: r.enabled, check: r.check, dim_bar: r.dim_bar, y, h: r.height(),
            }).collect::<Vec<_>>())));
            st.set_title(title.into());
            st.set_status(status_line(Some(&d), Some(&s)).into());
            st.set_focus(focus);
            st.set_open(open);
            let _ = ui.window().take_snapshot().expect("first snapshot");
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
            slint::platform::update_timers_and_animations();
            let snap = ui.window().take_snapshot().expect("snapshot");
            assert_eq!((snap.width(), snap.height()), (1280, 800), "{name}");
            if let Some(dir) = &out {
                image::save_buffer(dir.join(format!("{name}.png")), snap.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
            snap.as_bytes().to_vec()
        };
        let list = list_rows(Some(&d), &txt());
        let closed = shot("downloads-menu-closed", false, "", &list, -1);
        let open = shot("downloads-menu-list", true, "Загрузки", &list, 1);
        let moved = shot("downloads-menu-list-focus-next", true, "Загрузки", &list, 3);
        let actions = shot("downloads-menu-actions", true, "Alpha", &action_rows(Some(&d), 100, Some(&s), &txt()), 1);
        let settings = shot("downloads-menu-settings", true, "Настройки загрузок", &settings_rows(Some(&s), &txt()), 0);
        let unknown = shot("downloads-menu-unavailable", true, "Настройки загрузок", &settings_rows(None, &txt()), 1);
        let px = |b: &[u8], x: usize, y: usize| { let i = (y * 1280 + x) * 4; (b[i], b[i + 1], b[i + 2]) };
        let differs = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
        // The panel sits right of the 360 px rail and covers the dimmed page with the side-menu surface colour.
        assert_ne!(px(&closed, 600, 400), px(&open, 600, 760 - 30), "panel surface is drawn");
        assert_eq!(px(&open, 900, 730), (0x0e, 0x14, 0x1b), "side-menu background (Tokens.menu-bg)");
        assert_eq!(px(&open, 1000, 400), px(&closed, 1000, 400), "page right of the panel is untouched");
        assert_eq!(px(&open, 100, 700), px(&closed, 100, 700), "the rail is untouched");
        assert!(differs(&open, &moved) > 0, "focus moves between rows");
        assert!(differs(&open, &actions) > 0 && differs(&actions, &settings) > 0 && differs(&settings, &unknown) > 0, "each view draws its own rows");
    }

    #[test]
    fn custom_speed_limit_row_opens_numeric_keyboard_and_returns_to_choice() {
        let (ui, mut a) = crate::screens::osk::field_tests::app();
        crate::screens::downloads_menu::wire_models(&a, &ui);
        let (d, s) = fixture();
        a.live.downloads = Some(d);
        a.dm.settings = Some(Some(s));
        a.dm.view = View::Choice(Setting::Limit);
        a.dl_menu_rebuild(true);
        let last = a.dm.rows.len() - 1;
        assert_eq!(a.dm.rows[last].target, Target::Custom(Setting::Limit));
        a.dm.focus = last;
        a.handle_dl_menu(Action::A);
        assert!(a.osk_is_open(), "A на «Своё значение…» открывает клавиатуру");
        assert_eq!(crate::screens::osk::current_mode(&a), Some(crate::screens::osk::InputMode::Numeric));
        for c in "12x5".chars() { a.handle(Action::Char(c)); }
        assert_eq!(a.dm.rows[last].label, "Своё значение: 125 КБ/с");
        crate::screens::osk::field_tests::shot(&ui, "downloads-custom-limit");
        a.handle(Action::B);
        assert!(!a.osk_is_open());
        assert_eq!(a.dm.view, View::Choice(Setting::Limit), "закрытие возвращает фокус списку");
        assert_eq!(a.dm.rows[a.dm.focus].target, Target::Custom(Setting::Limit));
        assert_eq!(a.dm.rows[last].label, "Своё значение…", "отмена очищает ввод");
        assert_eq!(custom_rate("125", Some(false)), Some(1000));
        assert_eq!(custom_rate("1.5", Some(false)), Some(12));
        assert_eq!(custom_rate("3000", Some(true)), Some(3000));
        assert_eq!(custom_rate("0", Some(true)), None);
    }
}
