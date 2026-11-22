//! «Отправить на телефон» for screenshots and clips (tasks 311, 313, 315). Source notes:
//! context/reference/steamui/send-to-phone.md (stock `sp.js` evidence, offsets and limits there).
//!
//! The phone check (`MobileDevice.HasMobileDevice`) is a query. A live send exports a clip (stock
//! `fnExportClip`) or picks a screenshot file (stock `u`), then uploads it through the stock upload
//! builder with the phone bucket. [`LIVE_SEND_ENABLED`] is the owner's switch for that path.
use crate::{Bridge, Error, Result};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Owner's word (Telegram, 2026-11-15): real sends to the owner's phone are allowed. While this is false,
/// [`send_expr`] never exports or uploads, whatever the request says.
pub const LIVE_SEND_ENABLED: bool = true;

/// Shared scope fragment: finds the MobileDevice module, exposes its service and upload builder, defines `__phoneCheck`.
const SCOPE_JS: &str = include_str!("../js/phone_scope.js");
/// Send step body. Tokens are filled by [`send_expr`].
const SEND_JS: &str = include_str!("../js/phone_send.js");

/// Stock clip limit for the menu item: a clip longer than this disables «Отправить на телефон».
pub const CLIP_MAX_MS: u64 = 60_000;
/// Stock upload limit for the phone bucket (`Ji = 100` MB).
pub const MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;
/// Minimum Steam mobile app version per media kind (stock `an` and `L`).
pub const MIN_APP_CLIP: &str = "3.9.0";
pub const MIN_APP_SCREENSHOT: &str = "3.9.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Screenshot,
    Clip,
}

impl MediaKind {
    pub fn min_app_version(self) -> &'static str {
        match self {
            MediaKind::Screenshot => MIN_APP_SCREENSHOT,
            MediaKind::Clip => MIN_APP_CLIP,
        }
    }
}

/// Account and region facts the stock menu reads (`ShareSheet_*` flags and the China check).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountFlags {
    /// The app forbids this kind of upload (`#ShareSheet_ScreenUploadNotAllowed` / `#ShareSheet_ClipUploadNotAllowed`).
    pub upload_forbidden: bool,
    /// The account is limited (`#ShareSheet_LimitedScreenshotNotAllowed`).
    pub limited: bool,
    /// The China flag: stock hides the item completely.
    pub china: bool,
}

/// Why the menu item is disabled or hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Hidden entirely (China flag). Not shown, not disabled.
    Hidden,
    /// Disabled because the clip is longer than [`CLIP_MAX_MS`]. `duration_ms` is the clip length.
    ClipTooLong { duration_ms: u64 },
    UploadForbidden,
    LimitedAccount,
}

impl Block {
    /// Localization key and Russian fallback for the disabled tooltip. `None` when the stock key
    /// for this case is not known (the limited-clip case is not in the reference note).
    pub fn message(&self, kind: MediaKind) -> Option<(&'static str, &'static str)> {
        match (self, kind) {
            (Block::ClipTooLong { .. }, _) => Some(("#SendToChat_ClipTooLong",
                "Некоторые опции недоступны для роликов длиннее 60 сек.")),
            (Block::UploadForbidden, MediaKind::Screenshot) => Some(("#ShareSheet_ScreenUploadNotAllowed",
                "Загрузка скриншотов для этого приложения запрещена.")),
            (Block::UploadForbidden, MediaKind::Clip) => Some(("#ShareSheet_ClipUploadNotAllowed",
                "Загрузка роликов для этой игры запрещена.")),
            (Block::LimitedAccount, MediaKind::Screenshot) => Some(("#ShareSheet_LimitedScreenshotNotAllowed",
                "Ограниченная учётная запись не может отправлять скриншоты.")),
            (Block::LimitedAccount, MediaKind::Clip) => None,
            (Block::Hidden, _) => None,
        }
    }
}

/// Eligibility before any phone query, in the stock order of checks.
pub fn menu_block(kind: MediaKind, duration_ms: Option<u64>, flags: AccountFlags) -> Option<Block> {
    if flags.china {
        return Some(Block::Hidden);
    }
    if kind == MediaKind::Clip {
        if let Some(ms) = duration_ms.filter(|ms| *ms > CLIP_MAX_MS) {
            return Some(Block::ClipTooLong { duration_ms: ms });
        }
    }
    if flags.upload_forbidden {
        return Some(Block::UploadForbidden);
    }
    if flags.limited {
        return Some(Block::LimitedAccount);
    }
    None
}

/// Reply of `MobileDevice.HasMobileDevice` as the stock reads it (`found_device`, `up_to_date`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneReply {
    /// The request itself failed.
    QueryError,
    /// Reply without a device: stock has the text `#SendToPhone_NoPhone`.
    NoPhone,
    /// A phone with the Steam app, but the app is older than the minimum for this kind.
    OutdatedApp,
    Ready,
}

/// Maps the stock checks (query error, no phone, outdated, ready) in their stock order.
pub fn phone_reply(query_ok: bool, found_device: bool, up_to_date: bool) -> PhoneReply {
    if !query_ok {
        PhoneReply::QueryError
    } else if !found_device {
        PhoneReply::NoPhone
    } else if !up_to_date {
        PhoneReply::OutdatedApp
    } else {
        PhoneReply::Ready
    }
}

/// User-facing text for a phone reply. Keys are from the reference note.
pub fn phone_message(reply: PhoneReply, kind: MediaKind) -> Option<&'static str> {
    match reply {
        PhoneReply::QueryError => Some("Не удалось отправить запрос на ваше мобильное устройство. Повторите попытку позже."),
        PhoneReply::NoPhone => Some("Мобильное приложение Steam не установлено на устройстве, поддерживающем push-уведомления."),
        PhoneReply::OutdatedApp => Some(match kind {
            MediaKind::Clip => "Обновите мобильное приложение Steam, чтобы отправлять ролики. Нужна версия не ниже 3.9.0.",
            MediaKind::Screenshot => "Обновите мобильное приложение Steam, чтобы отправлять скриншоты. Нужна версия не ниже 3.9.1.",
        }),
        PhoneReply::Ready => None,
    }
}

/// Compares dotted app versions (`3.9.1`). `None` when either side is not numeric, so the caller
/// treats the app as not confirmed rather than guessing.
pub fn version_at_least(app: &str, min: &str) -> Option<bool> {
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.split('.').map(|p| p.parse::<u64>().ok()).collect()
    }
    let (a, m) = (parts(app)?, parts(min)?);
    let n = a.len().max(m.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied().unwrap_or(0), m.get(i).copied().unwrap_or(0));
        if x != y {
            return Some(x > y);
        }
    }
    Some(true)
}

/// One export preset of the stock bitrate ladder (`S`, order as stock lists it: highest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    pub kbps: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

pub const LADDER: [Preset; 6] = [
    Preset { kbps: 30000, width: 3840, height: 2160, fps: 60 },
    Preset { kbps: 24000, width: 3840, height: 2160, fps: 30 },
    Preset { kbps: 12000, width: 1920, height: 1080, fps: 60 },
    Preset { kbps: 8000, width: 1920, height: 1080, fps: 30 },
    Preset { kbps: 5000, width: 1280, height: 720, fps: 30 },
    Preset { kbps: 4000, width: 1280, height: 720, fps: 30 },
];

/// Bitrate the stock assumes when the clip summary has none (`nEstimatedBitrateKbps ?? 12e3`).
pub const DEFAULT_ESTIMATE_KBPS: u32 = 12_000;

/// Export settings the stock picks for a clip (`g` in module 96513, exported as `B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settings {
    /// Stock returns `{}`: the default export fits the budget, so no preset is applied.
    Default,
    /// The first ladder preset whose bitrate fits the budget, highest quality first.
    Preset(Preset),
}

/// Stock `g(seconds, budget, estimate)`: the budget in kbps is `budget_bytes * 8 / 1024 / seconds`. If the estimate
/// fits, the default settings are used; otherwise the first preset at or under the budget. `None` when nothing fits
/// (stock returns null and the result is `eResult TE`).
pub fn pick_settings(duration_ms: u64, estimated_kbps: Option<u32>, budget_bytes: u64) -> Option<Settings> {
    if duration_ms == 0 {
        return None;
    }
    let budget_kbps = budget_bytes as f64 * 8.0 / 1024.0 / (duration_ms as f64 / 1000.0);
    let estimate = estimated_kbps.unwrap_or(DEFAULT_ESTIMATE_KBPS) as f64;
    if estimate <= budget_kbps {
        return Some(Settings::Default);
    }
    LADDER.iter().copied().find(|p| (p.kbps as f64) <= budget_kbps).map(Settings::Preset)
}

/// Budget the phone sheet passes to the export (stock `mi`: `aQ(clip, Ji * 1024 * 1024)`, Ji = 100).
pub const EXPORT_BUDGET_BYTES: u64 = MAX_UPLOAD_BYTES;

/// Size check on the real exported file. Only a file within the limit may go on to upload.
pub fn check_exported_size(bytes: u64) -> Result<()> {
    if bytes == 0 {
        return Err(Error::Protocol("exported file is empty".into()));
    }
    if bytes > MAX_UPLOAD_BYTES {
        return Err(Error::Protocol(format!("exported file is {bytes} bytes, over the 100 MB limit")));
    }
    Ok(())
}

/// States of one send, with the stock labels («Экспорт...», «Загрузка…», «Отправлено»).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SendPhase {
    #[default]
    Idle,
    Checking,
    Exporting,
    Uploading,
    Sent,
    Failed(String),
}

impl SendPhase {
    pub fn label(&self) -> Option<&'static str> {
        match self {
            SendPhase::Exporting => Some("Экспорт..."),
            SendPhase::Uploading => Some("Загрузка…"),
            SendPhase::Sent => Some("Отправлено"),
            _ => None,
        }
    }

    /// Next phase on a step. Anything that is not the next step in order is refused, so a
    /// finished or failed send cannot restart by accident.
    pub fn advance(&self, step: SendStep) -> Result<SendPhase> {
        use SendPhase::*;
        if matches!(self, Sent | Failed(_)) {
            return Err(Error::Protocol("send already finished".into()));
        }
        match (self, &step) {
            (Idle, SendStep::Check) => Ok(Checking),
            (Checking, SendStep::Export) => Ok(Exporting),
            (Exporting, SendStep::Upload) => Ok(Uploading),
            (Uploading, SendStep::Commit) => Ok(Sent),
            (_, SendStep::Fail(msg)) => Ok(Failed(msg.clone())),
            _ => Err(Error::Protocol(format!("step {step:?} is not allowed from {self:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendStep {
    Check,
    Export,
    Upload,
    Commit,
    Fail(String),
}

/// Plan made before the upload. It holds the facts that the upload step needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPlan {
    pub kind: MediaKind,
    /// Export settings for a clip; `None` for a screenshot (no export).
    pub settings: Option<Settings>,
}

/// Everything that can be checked without a network call or file write. A clip gets a preset;
/// a screenshot does not go through the ladder.
pub fn plan(kind: MediaKind, duration_ms: Option<u64>, flags: AccountFlags, reply: PhoneReply) -> Result<SendPlan> {
    if let Some(block) = menu_block(kind, duration_ms, flags) {
        return Err(Error::Protocol(format!("blocked: {block:?}")));
    }
    match reply {
        PhoneReply::Ready => {},
        other => return Err(Error::Protocol(format!("phone not ready: {other:?}"))),
    }
    let settings = match kind {
        MediaKind::Clip => {
            let ms = duration_ms.ok_or_else(|| Error::Protocol("clip duration unknown".into()))?;
            Some(pick_settings(ms, None, EXPORT_BUDGET_BYTES)
                .ok_or_else(|| Error::Protocol("no export preset fits 100 MB".into()))?)
        },
        MediaKind::Screenshot => None,
    };
    Ok(SendPlan { kind, settings })
}

/// Read-only probe. It scans the webpack module texts for the three stock entry points and
/// returns the module ids and a short text snippet around each marker. It calls no service and
/// writes nothing except the empty chunk registration that the PublishedFile helper also uses.
pub const PROBE_JS: &str = r#"(()=>{const __req=(()=>{let r=null;window.webpackChunksteamui.push([["fable-send-probe-"+Math.random()],{},(x)=>{r=x}]);return r})();
const __m=__req.m;const __markers={mobile_service:"HasMobileDevice",export_clip:"fnExportClip",upload_begin:"tempcloud/beginfileupload/",upload_commit:"tempcloud/commitfileupload/"};
const out={};for(const k of Object.keys(__markers)){const hits=[];for(const id of Object.keys(__m)){const t=__m[id].toString();const i=t.indexOf(__markers[k]);if(i>=0)hits.push({id,len:t.length,snippet:t.slice(Math.max(0,i-120),i+120)});}out[k]=hits.slice(0,4);}
out.cached_exports=Object.keys(__req.c||{}).filter(id=>{const e=__req.c[id].exports;return e&&Object.keys(e).some(n=>/MobileDevice|Upload|Clip/.test(n))}).slice(0,20);
return out;})()"#;

/// Parsed probe reply: which markers were found and in which modules.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProbeReport {
    pub entries: Vec<ProbeEntry>,
    pub cached_export_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeEntry {
    pub marker: String,
    pub module_id: String,
    pub module_len: u64,
    pub snippet: String,
}

pub fn parse_probe(v: &Value) -> Result<ProbeReport> {
    let obj = v.as_object().ok_or_else(|| Error::Protocol("probe reply is not an object".into()))?;
    let mut report = ProbeReport::default();
    for marker in ["mobile_service", "export_clip", "upload_begin", "upload_commit"] {
        let hits = obj.get(marker).and_then(Value::as_array)
            .ok_or_else(|| Error::Protocol(format!("probe missing {marker}")))?;
        for h in hits {
            report.entries.push(ProbeEntry {
                marker: marker.to_string(),
                module_id: h.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                module_len: h.get("len").and_then(Value::as_u64).unwrap_or(0),
                snippet: h.get("snippet").and_then(Value::as_str).unwrap_or_default().to_string(),
            });
        }
    }
    if let Some(ids) = obj.get("cached_exports").and_then(Value::as_array) {
        report.cached_export_ids = ids.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    }
    Ok(report)
}

/// What one send request asks for. `live` is honoured only when [`LIVE_SEND_ENABLED`] is true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub kind: MediaKind,
    pub clip_id: Option<String>,
    pub app: u32,
    pub duration_ms: Option<u64>,
    /// Screenshots only: the file on disk (checked by [`check_shot_path`] before use).
    pub shot_path: Option<String>,
    pub live: bool,
}

/// Screenshot files the send may read: an absolute path to a `.jpg`, `.jpeg` or `.png` inside a Steam
/// `userdata/<acct>/760/remote/<appid>/screenshots/` folder. Anything else is refused before any JS runs.
pub fn check_shot_path(path: &str) -> Result<()> {
    let lower = path.to_ascii_lowercase();
    let ext_ok = [".jpg", ".jpeg", ".png"].iter().any(|e| lower.ends_with(e));
    let inside = path.starts_with('/') && lower.contains("/userdata/") && lower.contains("/760/remote/")
        && lower.contains("/screenshots/") && !path.contains("..") && !path.contains("/thumbnails/");
    if ext_ok && inside {
        Ok(())
    } else {
        Err(Error::Protocol("screenshot path is not a Steam screenshot file".into()))
    }
}

/// Timing of one send. The send script runs in Steam until the upload ends, which can take minutes, so its reply
/// deadline is far longer than the 2 s of ordinary calls. If the reply is lost anyway, the page state is polled
/// for up to `window` (after `start` with no state at all, the send is taken as never started).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendTiming {
    pub call: Duration,
    pub window: Duration,
    pub start: Duration,
    pub step: Duration,
}

pub const SEND_TIMING: SendTiming = SendTiming {
    call: Duration::from_secs(300),
    window: Duration::from_secs(600),
    start: Duration::from_secs(15),
    step: Duration::from_secs(1),
};

static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// A new id for one send. The page stores it with every progress write, so a poll never reads an earlier send's state.
pub fn new_run_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos:x}-{}", RUN_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Progress of one send (`run_id`) from `globalThis.__fablePhone`. `null` while this send has not written yet.
pub fn progress_expr(run_id: &str) -> String {
    format!("(()=>{{const s=globalThis.__fablePhone;return s&&s.id==={}?s:null}})()", js_string(run_id))
}

/// Whether a page state of one send is still running, finished with a reply, or absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Running,
    Done(SendReply),
    Missing,
}

pub fn settled(v: &Value) -> Settled {
    match v.get("step").and_then(Value::as_str) {
        Some("check" | "export" | "upload") => Settled::Running,
        Some(_) => parse_send(v).map(Settled::Done).unwrap_or(Settled::Missing),
        None => Settled::Missing,
    }
}

/// Maps a progress reply to the phase it shows. `check`, `sent` and anything else show nothing.
pub fn parse_progress(v: &Value) -> Option<SendPhase> {
    match v.get("step").and_then(Value::as_str)? {
        "export" => Some(SendPhase::Exporting),
        "upload" => Some(SendPhase::Uploading),
        _ => None,
    }
}

fn js_string(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into())
}

/// Phone check expression: `HasMobileDevice` for the minimum version of the kind. A query only.
pub fn check_expr(kind: MediaKind) -> String {
    format!("(async()=>{{{SCOPE_JS}\nreturn await __phoneCheck({});}})()", js_string(kind.min_app_version()))
}

/// Fills the placeholders of `js/phone_send.js`. Shared with the dry-run tool through [`send_expr`].
pub fn fill_send_tokens(template: &str, req: &SendRequest, live: bool) -> String {
    template
        .replace("__KIND__", match req.kind { MediaKind::Clip => "clip", MediaKind::Screenshot => "shot" })
        .replace("__CLIP_ID__", &req.clip_id.as_deref().map(js_string).unwrap_or_else(|| "null".into()))
        .replace("__APP__", &req.app.to_string())
        .replace("__DURATION_MS__", &req.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "null".into()))
        .replace("__SHOT_PATH__", &req.shot_path.as_deref().map(js_string).unwrap_or_else(|| "null".into()))
        .replace("__LIVE__", if live { "true" } else { "false" })
}

/// Send expression. A dry run (the default, and always while [`LIVE_SEND_ENABLED`] is false) returns the planned steps
/// and makes no export and no upload. The body runs inside a wrapper that turns any exception into a terminal state:
/// `unknown` if the upload had started, `failed` before it. The terminal state is written to `__fablePhone`, next to
/// the result that is returned, so [`Bridge::phone_send`] can settle a lost reply from the page.
pub fn send_expr(req: &SendRequest, run_id: &str) -> String {
    let live = req.live && LIVE_SEND_ENABLED;
    let body = fill_send_tokens(SEND_JS, req, live);
    let run = js_string(run_id);
    format!(
        "(async()=>{{{SCOPE_JS}\nconst __fableRun={run};\
const __fableRes=await(async()=>{{{body}}})().catch(__e=>({{step:(globalThis.__fablePhone||{{}}).step===\"upload\"?\"unknown\":\"failed\",why:\"exception\",message:String(__e&&__e.message||__e)}}));\
globalThis.__fablePhone=Object.assign({{id:__fableRun}},__fableRes);return __fableRes;}})()"
    )
}

/// Reads a settled result of one send from its progress state (see [`Settled`]).
fn settle_lost(bridge: &Bridge, run_id: &str, timing: SendTiming, lost: Error) -> Result<SendReply> {
    let expr = progress_expr(run_id);
    let started = Instant::now();
    loop {
        if let Ok(v) = bridge.eval_json(&expr) {
            match settled(&v) {
                Settled::Done(reply) => return Ok(reply),
                Settled::Running => {}
                Settled::Missing if started.elapsed() >= timing.start => break,
                Settled::Missing => {}
            }
        }
        if started.elapsed() >= timing.window {
            break;
        }
        std::thread::sleep(timing.step);
    }
    // No readable result: the outcome is genuinely unknown. The caller reports it and never resends.
    Err(lost)
}

/// Reads the phone check reply: `(query_ok, found_device, up_to_date)`.
pub fn parse_check(v: &Value) -> Result<(bool, bool, bool)> {
    let o = v.as_object().ok_or_else(|| Error::Protocol("phone check reply is not an object".into()))?;
    let query_ok = o.get("query_ok").and_then(Value::as_bool)
        .ok_or_else(|| Error::Protocol("phone check reply has no query_ok".into()))?;
    let found = o.get("found_device").and_then(Value::as_bool).unwrap_or(false);
    let up = o.get("up_to_date").and_then(Value::as_bool).unwrap_or(false);
    Ok((query_ok, found, up))
}

/// Result of a send step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendReply {
    /// Stopped before any export: the reason is a stable short code (`no_phone`, `outdated`, `no_preset`, ...).
    Refused(String),
    /// Dry run: checks passed and the steps are planned. Nothing was exported or uploaded.
    Dry { kind: String, detail: String },
    Sent,
    Failed(String),
    /// The send started an upload and then ended without a readable result. The file may or may not be on the phone.
    Unknown(String),
}

pub fn parse_send(v: &Value) -> Result<SendReply> {
    let o = v.as_object().ok_or_else(|| Error::Protocol("send reply is not an object".into()))?;
    let step = o.get("step").and_then(Value::as_str).ok_or_else(|| Error::Protocol("send reply has no step".into()))?;
    let text = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(match step {
        "refused" => SendReply::Refused(text("why")),
        "dry" => SendReply::Dry {
            kind: text("kind"),
            detail: o.get("settings").and_then(Value::as_str).map(str::to_string)
                .unwrap_or_else(|| text("unverified")),
        },
        "sent" => SendReply::Sent,
        "failed" => SendReply::Failed(format!("{} {}", text("why"), text("message")).trim().to_string()),
        "unknown" => SendReply::Unknown(text("why")),
        other => return Err(Error::Protocol(format!("unknown send step {other}"))),
    })
}

/// Text for a refused send (the short codes from [`parse_send`]). Phone texts come from the stock keys in [`phone_message`].
pub fn refusal_message(why: &str, kind: MediaKind) -> String {
    match why {
        "query" => phone_message(PhoneReply::QueryError, kind).unwrap_or_default().to_owned(),
        "no_phone" => phone_message(PhoneReply::NoPhone, kind).unwrap_or_default().to_owned(),
        "outdated" => phone_message(PhoneReply::OutdatedApp, kind).unwrap_or_default().to_owned(),
        "no_preset" => "Ролик не получается уместить в 100 МБ для отправки; ничего не отправлено.".to_owned(),
        "no_path" => "Файл скриншота не найден; ничего не отправлено.".to_owned(),
        "too_big" => "Файл больше 100 МБ или пустой; ничего не отправлено.".to_owned(),
        other => format!("Отправка остановлена ({other}); ничего не отправлено."),
    }
}

impl Bridge {
    /// Phone check (read only, a query). Returns the stock reply class for the kind.
    pub fn phone_check(&self, kind: MediaKind) -> Result<PhoneReply> {
        let v = self.eval_json(&check_expr(kind))?;
        let (q, f, u) = parse_check(&v)?;
        Ok(phone_reply(q, f, u))
    }

    /// Send step. Evaluated once, never replayed: a send that may have run must not run twice. The reply is
    /// awaited for the whole send. If it is lost, the page state of `run_id` decides: a finished send gives its
    /// real result, and only a send with no readable result is reported as unknown.
    pub fn phone_send(&self, req: &SendRequest, run_id: &str) -> Result<SendReply> {
        self.phone_send_timed(req, run_id, SEND_TIMING)
    }

    pub(crate) fn phone_send_timed(&self, req: &SendRequest, run_id: &str, timing: SendTiming) -> Result<SendReply> {
        match self.eval_own(&send_expr(req, run_id), timing.call) {
            Ok(v) => Ok(parse_send(&v).unwrap_or_else(|e| SendReply::Unknown(e.to_string()))),
            Err(lost) => settle_lost(self, run_id, timing, lost),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flags() -> AccountFlags { AccountFlags::default() }

    #[test]
    fn clip_of_60_seconds_is_allowed_and_61_is_blocked() {
        assert_eq!(menu_block(MediaKind::Clip, Some(60_000), flags()), None);
        assert_eq!(menu_block(MediaKind::Clip, Some(60_001), flags()),
            Some(Block::ClipTooLong { duration_ms: 60_001 }));
    }

    #[test]
    fn clip_too_long_message_uses_stock_key() {
        let b = Block::ClipTooLong { duration_ms: 75_000 };
        assert_eq!(b.message(MediaKind::Clip).map(|m| m.0), Some("#SendToChat_ClipTooLong"));
    }

    #[test]
    fn china_hides_and_forbidden_or_limited_disable() {
        let china = AccountFlags { china: true, ..flags() };
        assert_eq!(menu_block(MediaKind::Screenshot, None, china), Some(Block::Hidden));
        let forbidden = AccountFlags { upload_forbidden: true, ..flags() };
        assert_eq!(menu_block(MediaKind::Screenshot, None, forbidden), Some(Block::UploadForbidden));
        let limited = AccountFlags { limited: true, ..flags() };
        assert_eq!(menu_block(MediaKind::Screenshot, None, limited), Some(Block::LimitedAccount));
        // The limited-clip key is not in the reference, so no message is invented.
        assert_eq!(Block::LimitedAccount.message(MediaKind::Clip), None);
    }

    #[test]
    fn phone_checks_follow_stock_order() {
        assert_eq!(phone_reply(false, true, true), PhoneReply::QueryError);
        assert_eq!(phone_reply(true, false, true), PhoneReply::NoPhone);
        assert_eq!(phone_reply(true, true, false), PhoneReply::OutdatedApp);
        assert_eq!(phone_reply(true, true, true), PhoneReply::Ready);
        assert!(phone_message(PhoneReply::Ready, MediaKind::Clip).is_none());
        assert!(phone_message(PhoneReply::OutdatedApp, MediaKind::Clip).unwrap().contains("3.9.0"));
        assert!(phone_message(PhoneReply::OutdatedApp, MediaKind::Screenshot).unwrap().contains("3.9.1"));
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert_eq!(version_at_least("3.9.1", "3.9.0"), Some(true));
        assert_eq!(version_at_least("3.10.0", "3.9.9"), Some(true));
        assert_eq!(version_at_least("3.9", "3.9.1"), Some(false));
        assert_eq!(version_at_least("3.9.0", "3.9.0"), Some(true));
        assert_eq!(version_at_least("beta", "3.9.0"), None);
    }

    #[test]
    fn stock_budget_is_100_mb_in_kbps_per_second() {
        // 60 s at 100 MB is 13653 kbps; the default estimate (12000) fits, so no preset is applied.
        assert_eq!(pick_settings(60_000, None, EXPORT_BUDGET_BYTES), Some(Settings::Default));
        // A clip recorded at 30 Mbps does not fit: the first preset under 13653 kbps is 12000.
        assert_eq!(pick_settings(60_000, Some(30_000), EXPORT_BUDGET_BYTES), Some(Settings::Preset(LADDER[2])));
        // 5 s is far under the budget at the default estimate: default settings.
        assert_eq!(pick_settings(5_000, None, EXPORT_BUDGET_BYTES), Some(Settings::Default));
    }

    #[test]
    fn stock_budget_with_high_estimate_picks_highest_fitting_preset() {
        // 10 s: budget 81920 kbps; a 100 Mbps estimate does not fit, and 30000 kbps is the first preset under the budget.
        assert_eq!(pick_settings(10_000, Some(100_000), EXPORT_BUDGET_BYTES), Some(Settings::Preset(LADDER[0])));
        // The same clip at a 50 Mbps estimate fits the budget, so the default settings are used.
        assert_eq!(pick_settings(10_000, Some(50_000), EXPORT_BUDGET_BYTES), Some(Settings::Default));
    }

    #[test]
    fn no_settings_for_zero_length_media() {
        assert_eq!(pick_settings(0, None, EXPORT_BUDGET_BYTES), None);
    }

    #[test]
    fn exported_size_is_checked_before_upload() {
        assert!(check_exported_size(MAX_UPLOAD_BYTES).is_ok());
        assert!(check_exported_size(MAX_UPLOAD_BYTES + 1).is_err());
        assert!(check_exported_size(0).is_err());
    }

    #[test]
    fn plan_refuses_until_phone_is_ready_and_clip_has_settings() {
        let ok = plan(MediaKind::Clip, Some(10_000), flags(), PhoneReply::Ready).unwrap();
        assert_eq!(ok.settings, Some(Settings::Default));
        assert!(plan(MediaKind::Clip, Some(10_000), flags(), PhoneReply::NoPhone).is_err());
        assert!(plan(MediaKind::Clip, Some(61_000), flags(), PhoneReply::Ready).is_err());
        assert!(plan(MediaKind::Clip, None, flags(), PhoneReply::Ready).is_err());
        let shot = plan(MediaKind::Screenshot, None, flags(), PhoneReply::Ready).unwrap();
        assert_eq!(shot.settings, None);
    }

    #[test]
    fn state_machine_has_stock_labels_and_no_restart() {
        let mut p = SendPhase::Idle;
        p = p.advance(SendStep::Check).unwrap();
        p = p.advance(SendStep::Export).unwrap();
        assert_eq!(p.label(), Some("Экспорт..."));
        p = p.advance(SendStep::Upload).unwrap();
        assert_eq!(p.label(), Some("Загрузка…"));
        p = p.advance(SendStep::Commit).unwrap();
        assert_eq!(p.label(), Some("Отправлено"));
        assert!(p.advance(SendStep::Check).is_err());
        assert!(SendPhase::Idle.advance(SendStep::Upload).is_err());
    }

    #[test]
    fn failure_is_terminal() {
        let f = SendPhase::Idle.advance(SendStep::Check).unwrap()
            .advance(SendStep::Fail("сеть".into())).unwrap();
        assert!(matches!(f, SendPhase::Failed(_)));
        assert!(f.advance(SendStep::Check).is_err());
    }

    #[test]
    fn probe_reply_parses_all_markers() {
        let v = json!({
            "mobile_service": [{"id": "83433", "len": 1000, "snippet": "…HasMobileDevice…"}],
            "export_clip": [],
            "upload_begin": [{"id": "96513", "len": 2000, "snippet": "tempcloud/beginfileupload/"}],
            "upload_commit": [],
            "cached_exports": ["83433"]
        });
        let r = parse_probe(&v).unwrap();
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.entries[0].marker, "mobile_service");
        assert_eq!(r.entries[0].module_id, "83433");
        assert_eq!(r.cached_export_ids, vec!["83433".to_string()]);
    }

    #[test]
    fn probe_reply_rejects_missing_marker() {
        assert!(parse_probe(&json!({"mobile_service": []})).is_err());
        assert!(parse_probe(&json!("nope")).is_err());
    }

    #[test]
    fn check_reply_maps_to_phone_reply() {
        assert_eq!(parse_check(&json!({"query_ok": true, "found_device": true, "up_to_date": true})).unwrap(), (true, true, true));
        assert_eq!(parse_check(&json!({"query_ok": false, "eresult": 2})).unwrap(), (false, false, false));
        assert!(parse_check(&json!({"found_device": true})).is_err());
    }

    #[test]
    fn check_expression_is_filled_and_read_only() {
        let e = check_expr(MediaKind::Clip);
        assert!(e.contains("MobileDevice.HasMobileDevice#1"));
        assert!(e.contains("\"3.9.0\""));
        assert!(!e.contains("__MIN__"));
        assert!(!e.contains("SendMsg(\"MobileDevice.RegisterMobileDevice"));
        assert!(!e.contains("beginfileupload"));
    }

    #[test]
    fn dry_request_never_exports_even_with_gate_open() {
        const { assert!(LIVE_SEND_ENABLED) };
        let req = SendRequest { kind: MediaKind::Clip, clip_id: Some("123".into()), app: 2379780, duration_ms: Some(12_000), shot_path: None, live: false };
        let e = send_expr(&req, "run-test");
        assert!(e.contains("__live=false"), "a dry request must not go live");
        assert!(e.contains("\"123\""));
        assert!(e.contains("__app=2379780"));
        for token in ["__KIND__", "__CLIP_ID__", "__APP__", "__DURATION_MS__", "__SHOT_PATH__", "__LIVE__"] {
            assert!(!e.contains(token), "unfilled token {token}");
        }
    }

    #[test]
    fn live_request_opens_the_gate_only_when_flag_is_on() {
        let req = SendRequest { kind: MediaKind::Clip, clip_id: Some("123".into()), app: 2379780, duration_ms: Some(12_000), shot_path: None, live: true };
        assert!(send_expr(&req, "run-test").contains("__live=true"));
    }

    #[test]
    fn screenshot_send_expression_carries_checked_path_and_no_clip_id() {
        let path = "/home/deck/.local/share/Steam/userdata/1000000001/760/remote/1000/screenshots/20261111200046_1.jpg";
        let req = SendRequest { kind: MediaKind::Screenshot, clip_id: None, app: 1000, duration_ms: None, shot_path: Some(path.into()), live: true };
        let e = send_expr(&req, "run-test");
        assert!(e.contains("const __kind=\"shot\""));
        assert!(e.contains("__clipId=null"));
        assert!(e.contains("__durMs=null"));
        assert!(e.contains("__live=true"));
        assert!(e.contains(&format!("__shotPath=\"{path}\"")));
        assert!(!e.contains("__SHOT_PATH__"));
    }

    #[test]
    fn shot_path_must_be_a_steam_screenshot_file() {
        let ok = "/home/deck/.local/share/Steam/userdata/1000000001/760/remote/1000/screenshots/20261111200046_1.jpg";
        assert!(check_shot_path(ok).is_ok());
        assert!(check_shot_path("/home/deck/.local/share/Steam/userdata/1000000001/760/remote/1000/screenshots/thumbnails/a.jpg").is_err());
        assert!(check_shot_path("/home/deck/Pictures/a.jpg").is_err());
        assert!(check_shot_path("/home/deck/.local/share/Steam/userdata/1/760/remote/2/screenshots/../../x.jpg").is_err());
        assert!(check_shot_path("relative/userdata/1/760/remote/2/screenshots/a.jpg").is_err());
        assert!(check_shot_path(&ok.replace(".jpg", ".txt")).is_err());
    }

    #[test]
    fn progress_maps_only_export_and_upload() {
        assert_eq!(parse_progress(&json!({"step": "export"})), Some(SendPhase::Exporting));
        assert_eq!(parse_progress(&json!({"step": "upload"})), Some(SendPhase::Uploading));
        assert_eq!(parse_progress(&json!({"step": "sent"})), None);
        assert_eq!(parse_progress(&json!(null)), None);
    }

    #[test]
    fn settled_state_tells_running_done_and_missing_apart() {
        for step in ["check", "export", "upload"] {
            assert_eq!(settled(&json!({"id": "r", "step": step})), Settled::Running, "{step}");
        }
        assert_eq!(settled(&json!({"id": "r", "step": "sent"})), Settled::Done(SendReply::Sent));
        assert_eq!(settled(&json!({"id": "r", "step": "unknown", "why": "boom"})), Settled::Done(SendReply::Unknown("boom".into())));
        assert_eq!(settled(&json!({"id": "r", "step": "failed", "why": "upload", "message": "net"})),
            Settled::Done(SendReply::Failed("upload net".into())));
        assert_eq!(settled(&json!({"id": "r", "step": "refused", "why": "no_phone"})), Settled::Done(SendReply::Refused("no_phone".into())));
        assert_eq!(settled(&json!(null)), Settled::Missing);
        assert_eq!(settled(&json!({"id": "r"})), Settled::Missing);
    }

    #[test]
    fn run_ids_are_unique_and_scope_the_progress_read() {
        let (a, b) = (new_run_id(), new_run_id());
        assert_ne!(a, b);
        let e = progress_expr(&a);
        assert!(e.contains(&format!("\"{a}\"")));
        assert!(!e.contains(&format!("\"{b}\"")));
    }

    #[test]
    fn live_send_expression_writes_terminal_state_with_run_id() {
        let req = SendRequest { kind: MediaKind::Screenshot, clip_id: None, app: 1000,
            duration_ms: None, shot_path: Some("/x/userdata/1/760/remote/2/screenshots/a.jpg".into()), live: true };
        let e = send_expr(&req, "run-xyz");
        assert!(e.contains("const __fableRun=\"run-xyz\""));
        assert!(e.contains("globalThis.__fablePhone=Object.assign({id:__fableRun},__fableRes)"));
        assert!(e.contains("\"unknown\":\"failed\""), "exceptions must end as unknown or failed");
    }

    #[test]
    fn refusal_texts_use_stock_phone_messages() {
        assert!(refusal_message("no_phone", MediaKind::Clip).contains("не установлено"));
        assert!(refusal_message("outdated", MediaKind::Clip).contains("3.9.0"));
        assert!(refusal_message("outdated", MediaKind::Screenshot).contains("3.9.1"));
        assert!(refusal_message("unknown_code", MediaKind::Clip).contains("unknown_code"));
        assert!(refusal_message("too_big", MediaKind::Screenshot).contains("100 МБ"));
    }

    #[test]
    fn send_reply_parses_each_step() {
        assert_eq!(parse_send(&json!({"step": "refused", "why": "no_phone"})).unwrap(), SendReply::Refused("no_phone".into()));
        assert_eq!(parse_send(&json!({"step": "sent"})).unwrap(), SendReply::Sent);
        assert!(matches!(parse_send(&json!({"step": "dry", "kind": "clip", "settings": "default"})).unwrap(),
            SendReply::Dry { .. }));
        assert_eq!(parse_send(&json!({"step": "failed", "why": "upload", "message": "net"})).unwrap(),
            SendReply::Failed("upload net".into()));
        assert!(parse_send(&json!({"step": "other"})).is_err());
    }
}
