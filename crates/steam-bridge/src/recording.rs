//! Observations of Steam's existing native recording store and parser for native
//! GetClips / ClipSummary responses.
//!
//! Source contract: context/archive/docs/diagnostics/STEAM_RECORDING_CONTRACT.md and context/archive/docs/diagnostics/STEAM_RECORDING_MEDIA.md.
//! Existing-store observations and explicit once-only commands. No initialization,
//! subscriptions, webpack access, mode changes, or physical-chord toggles.
use crate::{Bridge, Error, Result};
use serde_json::Value;

pub const ERESULT_OK: u32 = 1;

const CACHE_SNAPSHOT_JS: &str = r#"(()=>{const s=window.g_GRS;if(!s)return {schema:1,available:false};return {schema:1,available:true,session:s.m_recordingState===null?null:{game_id:s.m_recordingState?.m_gameID},clips_requested:s.m_bClipLoadingTriggered,clips_loading:s.m_bLoadingClips,cached_clip_count:s.m_clips.size,enough_disk_space_cached:s.m_bEnoughDiskSpace,h265_cached:s.m_bEnableH265Recording}})()"#;

/// A full, nonzero uint64 decimal GameID. This is not a user SteamID or a
/// gamescope appid. Syntax/range validation does not prove the game is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameId(String);

impl GameId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        let text = value.as_str().ok_or_else(|| malformed("game_id"))?;
        // Reject JS numbers: a shortcut GameID can exceed Number.MAX_SAFE_INTEGER.
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit())
            || text.starts_with('0') || text.parse::<u64>().ok().filter(|id| *id != 0).is_none()
        {
            return Err(malformed("game_id"));
        }
        Ok(Self(text.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionObservation {
    /// Store cache is null. It starts null, so this does NOT establish Idle,
    /// a successful Stop, or absence of a session whose start event was missed.
    NoActiveSessionObserved,
    /// Native store observed a start. The cache does not retain recording_type,
    /// session_id or timestamp: do not infer Manual mode or freshness from this.
    ActiveGame(GameId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipCacheObservation {
    pub load_requested: bool,
    pub loading: bool,
    /// Count of cached summaries; this is not a fresh native GetClips result.
    /// Loading can become false on failure too, so zero does not prove no clips.
    pub summary_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedState {
    pub session: SessionObservation,
    pub clips: ClipCacheObservation,
    /// Defaults false before native disk check: not an authoritative permission.
    pub enough_disk_space_cached: bool,
    pub h265_cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Snapshot {
    /// g_GRS does not exist in this context. Recorder mode/state is unknown.
    StoreUnavailable,
    /// A cache observation only. Native transport readiness and UI preconditions
    /// are not established, and the context can be replaced after this read.
    Cached(CachedState),
}

/// Represents the parsed outcome of a normalized GetClips envelope.
/// Explicitly distinguishes between a valid empty clip list and non-success EResult.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GetClipsOutcome {
    Success(Vec<ClipSummary>),
    NonSuccess { eresult: u32 },
}

/// Parsed native Steam Game Recording ClipSummary (from Protobuf CGameRecording_ClipSummary).
/// Confirmed source in pretty/sp.js:209871–209940.
///
/// All uint64 fields (game_id, duration_ms, file_size, start_offset_ms, published_file_id)
/// are parsed from decimal strings preserving full 64-bit precision without IEEE-754 loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipSummary {
    pub clip_id: String,
    pub game_id: GameId,
    pub duration_ms: u64,
    pub date_recorded: u32,
    pub start_timeline_id: String,
    pub start_offset_ms: u64,
    pub published_file_id: u64,
    pub file_size: u64,
    pub name: String,
    pub date_clipped: u32,
    pub temporary: bool,
    pub original_device: String,
    pub original_gaming_device_type: u32,
    pub date_downloaded: u32,
    pub thumbnail_url: String,
    pub thumbnail_width: u32,
    pub thumbnail_height: u32,
}

/// In-memory catalog of clip summaries.
/// Implements deterministic deduplication and update policy strictly by `clip_id`,
/// matching the ID-deduplication policy of CGameRecordingStore::InternalAddClipSummary
/// (pretty/sp.js:85006–85019). Does not attempt to mirror game-grouped indexes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipCatalog {
    clips: Vec<ClipSummary>,
}

impl ClipCatalog {
    pub fn new() -> Self {
        Self { clips: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn as_slice(&self) -> &[ClipSummary] {
        &self.clips
    }

    /// Merges or updates a clip summary by its unique `clip_id`.
    /// Never matches or deduplicates by name or filesystem path.
    pub fn merge_summary(&mut self, summary: ClipSummary) {
        if let Some(existing) = self.clips.iter_mut().find(|c| c.clip_id == summary.clip_id) {
            *existing = summary;
        } else {
            self.clips.push(summary);
        }
    }

    /// Merges an iterator of summaries deterministically.
    pub fn merge_all(&mut self, summaries: impl IntoIterator<Item = ClipSummary>) {
        for s in summaries {
            self.merge_summary(s);
        }
    }

    /// Returns an iterator over permanent clips (temporary == false).
    pub fn permanent_clips(&self) -> impl Iterator<Item = &ClipSummary> {
        self.clips.iter().filter(|c| !c.temporary)
    }

    /// Returns an iterator over temporary clips (temporary == true).
    pub fn temporary_clips(&self) -> impl Iterator<Item = &ClipSummary> {
        self.clips.iter().filter(|c| c.temporary)
    }
}

impl Bridge {
    /// Read only Steam's already-created g_GRS. Does not initialize Game Recording
    /// or execute any native request. CDP/JS errors propagate, never become Idle.
    pub fn recording_cache_snapshot(&self) -> Result<Snapshot> {
        parse_snapshot(&self.eval_json(CACHE_SNAPSHOT_JS)?)
    }
}

fn malformed(field: &str) -> Error {
    // Do not include private game/session/clip identifiers or raw payloads.
    Error::Protocol(format!("recording: invalid {field}"))
}

fn boolean(value: &Value, key: &str) -> Result<bool> {
    value.get(key).and_then(Value::as_bool).ok_or_else(|| malformed(key))
}

fn parse_snapshot(value: &Value) -> Result<Snapshot> {
    if value.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(malformed("schema"));
    }
    if !boolean(value, "available")? {
        return Ok(Snapshot::StoreUnavailable);
    }
    let session = match value.get("session") {
        Some(Value::Null) => SessionObservation::NoActiveSessionObserved,
        Some(Value::Object(session)) => SessionObservation::ActiveGame(GameId::from_value(
            session.get("game_id").ok_or_else(|| malformed("game_id"))?,
        )?),
        _ => return Err(malformed("session")),
    };
    Ok(Snapshot::Cached(CachedState {
        session,
        clips: ClipCacheObservation {
            load_requested: boolean(value, "clips_requested")?,
            loading: boolean(value, "clips_loading")?,
            summary_count: value.get("cached_clip_count").and_then(Value::as_u64)
                .ok_or_else(|| malformed("cached_clip_count"))?,
        },
        enough_disk_space_cached: boolean(value, "enough_disk_space_cached")?,
        h265_cached: boolean(value, "h265_cached")?,
    }))
}

/// Parses a uint64 decimal string without precision loss. Rejects JS numbers, floats, and overflow.
fn parse_uint64_decimal(value: &Value, field: &str) -> Result<u64> {
    let text = value.as_str().ok_or_else(|| malformed(field))?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed(field));
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(malformed(field));
    }
    text.parse::<u64>().map_err(|_| malformed(field))
}

fn parse_uint64_decimal_opt(value: Option<&Value>, field: &str) -> Result<u64> {
    match value {
        None => Ok(0),
        Some(v) => parse_uint64_decimal(v, field),
    }
}

/// Parses a uint32 field from a JSON number. Rejects strings, null, and numbers exceeding u32::MAX.
fn parse_uint32_num(value: &Value, field: &str) -> Result<u32> {
    match value.as_u64() {
        Some(n) if n <= u32::MAX as u64 => Ok(n as u32),
        _ => Err(malformed(field)),
    }
}

fn parse_uint32_num_opt(value: Option<&Value>, field: &str) -> Result<u32> {
    match value {
        None => Ok(0),
        Some(v) => parse_uint32_num(v, field),
    }
}

/// Parses an optional string field from JSON.
/// If omitted from protobuf toObject(), defaults to empty string.
/// If present with an incorrect type (number, boolean, null, object), returns Protocol error.
fn parse_string_opt(value: Option<&Value>, field: &str) -> Result<String> {
    match value {
        None => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(malformed(field)),
    }
}

/// Parses an explicit boolean field. Rejects missing, null, string, or number values.
fn parse_bool_required(value: Option<&Value>, field: &str) -> Result<bool> {
    match value {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(malformed(field)),
    }
}

/// Parses a single ClipSummary from Protobuf toObject() JSON.
/// Confirmed fields are strictly type-validated.
/// Unknown extra fields in `value` are ignored without dropping confirmed fields.
pub fn parse_clip_summary(value: &Value) -> Result<ClipSummary> {
    let obj = value.as_object().ok_or_else(|| malformed("clip_summary"))?;

    let clip_id = obj
        .get("clip_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| malformed("clip_id"))?
        .to_owned();

    let game_id = GameId::from_value(
        obj.get("game_id").ok_or_else(|| malformed("game_id"))?,
    )?;

    let duration_ms = parse_uint64_decimal(
        obj.get("duration_ms").ok_or_else(|| malformed("duration_ms"))?,
        "duration_ms",
    )?;

    let date_recorded = parse_uint32_num(
        obj.get("date_recorded").ok_or_else(|| malformed("date_recorded"))?,
        "date_recorded",
    )?;

    let start_timeline_id = parse_string_opt(obj.get("start_timeline_id"), "start_timeline_id")?;
    let start_offset_ms = parse_uint64_decimal_opt(obj.get("start_offset_ms"), "start_offset_ms")?;
    let published_file_id = parse_uint64_decimal_opt(obj.get("published_file_id"), "published_file_id")?;

    let file_size = parse_uint64_decimal(
        obj.get("file_size").ok_or_else(|| malformed("file_size"))?,
        "file_size",
    )?;

    let name = parse_string_opt(obj.get("name"), "name")?;
    let date_clipped = parse_uint32_num_opt(obj.get("date_clipped"), "date_clipped")?;
    let temporary = parse_bool_required(obj.get("temporary"), "temporary")?;
    let original_device = parse_string_opt(obj.get("original_device"), "original_device")?;
    let original_gaming_device_type = parse_uint32_num_opt(obj.get("original_gaming_device_type"), "original_gaming_device_type")?;
    let date_downloaded = parse_uint32_num_opt(obj.get("date_downloaded"), "date_downloaded")?;
    let thumbnail_url = parse_string_opt(obj.get("thumbnail_url"), "thumbnail_url")?;
    let thumbnail_width = parse_uint32_num_opt(obj.get("thumbnail_width"), "thumbnail_width")?;
    let thumbnail_height = parse_uint32_num_opt(obj.get("thumbnail_height"), "thumbnail_height")?;

    Ok(ClipSummary {
        clip_id,
        game_id,
        duration_ms,
        date_recorded,
        start_timeline_id,
        start_offset_ms,
        published_file_id,
        file_size,
        name,
        date_clipped,
        temporary,
        original_device,
        original_gaming_device_type,
        date_downloaded,
        thumbnail_url,
        thumbnail_width,
        thumbnail_height,
    })
}

/// Parses the explicit normalization contract envelope:
/// `{ eresult: response.GetEResult(), body: response.Body().toObject() }`.
///
/// Distinguishes non-success EResult from a valid empty clip list.
/// Rejects unconfirmed aliases, string numbers for eresult, and malformed body structures.
pub fn parse_get_clips_response(value: &Value) -> Result<GetClipsOutcome> {
    let eresult_val = value.get("eresult").ok_or_else(|| malformed("eresult"))?;
    let eresult = match eresult_val.as_u64() {
        Some(n) if n <= u32::MAX as u64 => n as u32,
        _ => return Err(malformed("eresult")),
    };

    if eresult != ERESULT_OK {
        return Ok(GetClipsOutcome::NonSuccess { eresult });
    }

    let body = value.get("body").ok_or_else(|| malformed("body"))?;
    let body_obj = body.as_object().ok_or_else(|| malformed("body"))?;
    let clip_array = body_obj
        .get("clip")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("body.clip"))?;

    let mut clips = Vec::with_capacity(clip_array.len());
    for item in clip_array {
        clips.push(parse_clip_summary(item)?);
    }

    Ok(GetClipsOutcome::Success(clips))
}

/// Extended observation of the existing store. Cached summaries are not an
/// authoritative enumeration; an empty cache never means GetClips succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeView {
    pub cache: Snapshot,
    pub mode: Option<u32>,
    pub running_game: Option<GameId>,
    pub can_start: bool,
    pub can_stop: bool,
    pub clips: Vec<ClipSummary>,
}

const VIEW_JS: &str = r#"(()=>{
 const s=window.g_GRS;if(!s)return {schema:2,available:false};
 const a=window.SteamUIStore?.ActiveWindowInstance?.MainRunningApp;
 const g=a&&typeof a.GetGameID==='function'?a.GetGameID():null;
 const mode=window.settingsStore?.clientSettings?.gamerecording_background_mode;
 // MobX summaries may expose only non-enumerable accessors. CDP serializes
 // them as {} unless each confirmed protobuf field is read into a plain object.
 const clipKeys=['clip_id','game_id','duration_ms','date_recorded','start_timeline_id','start_offset_ms','published_file_id','file_size','name','date_clipped','temporary','original_device','original_gaming_device_type','date_downloaded','thumbnail_url','thumbnail_width','thumbnail_height'];
 const clips=Array.from(s.m_clips.values(),c=>Object.fromEntries(clipKeys.map(k=>[k,c[k]])));
 return {schema:2,available:true,session:s.m_recordingState===null?null:{game_id:s.m_recordingState?.m_gameID},
 clips_requested:s.m_bClipLoadingTriggered,clips_loading:s.m_bLoadingClips,cached_clip_count:s.m_clips.size,
 enough_disk_space_cached:s.m_bEnoughDiskSpace,h265_cached:s.m_bEnableH265Recording,
 mode:mode===undefined?null:mode,running_game:g,
 can_start:typeof s.StartRecording==='function'&&window.g_FriendsUIApp?.BroadcastStore?.status?.bIsBroadcasting===false&&Array.isArray(window.RemotePlayStore_SteamUI?.devices)&&!window.RemotePlayStore_SteamUI.devices.some(d=>d.status==='Streaming'),can_stop:typeof s.StopRecording==='function',clips};
})()"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeCommand { Start(GameId), Stop(GameId) }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandReply {
    /// Native Start returned EResult OK. Still wait for an observed session.
    StartAccepted,
    Rejected(u32),
    /// Stop returns void and hides EResult: never treat this as a saved clip.
    StopDispatched,
    /// Capability/identity/precondition changed before dispatch. No mutation.
    Unavailable,
}

impl NativeView {
    /// Conservatively offer manual controls only in Steam's already-selected
    /// manual mode. Null session does not prove Idle; native Start is explicit,
    /// never a toggle, and the native service remains the final arbiter.
    pub fn start_game(&self) -> Option<GameId> {
        match &self.cache {
            Snapshot::Cached(c) if self.mode == Some(2) && self.can_start
                && c.enough_disk_space_cached
                && c.session == SessionObservation::NoActiveSessionObserved => self.running_game.clone(),
            _ => None,
        }
    }
    pub fn stop_game(&self) -> Option<GameId> {
        match &self.cache {
            Snapshot::Cached(c) if self.mode == Some(2) && self.can_stop => match &c.session {
                SessionObservation::ActiveGame(g) => Some(g.clone()),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Retained across transport errors. A lost reply cannot justify replaying a
/// mutation. Cache additions are reported only as cache additions, never as proof
/// that Stop saved a recording (a background load can discover old clips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCommand {
    pub command: NativeCommand,
    before_clips: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation { Waiting, SessionObserved, SessionEndObserved { additional_cached_clips: usize } }
impl PendingCommand {
    pub fn new(command: NativeCommand, view: &NativeView) -> Self {
        Self { command, before_clips: view.clips.iter().map(|c| c.clip_id.clone()).collect() }
    }
    /// True while an explicit Start is unconfirmed (no session observed yet).
    pub fn is_start(&self) -> bool {
        matches!(self.command, NativeCommand::Start(_))
    }
    pub fn observe(&self, view: &NativeView) -> Confirmation {
        let Snapshot::Cached(c) = &view.cache else { return Confirmation::Waiting };
        match &self.command {
            NativeCommand::Start(g) if c.session == SessionObservation::ActiveGame(g.clone()) => Confirmation::SessionObserved,
            NativeCommand::Stop(g) if c.session == SessionObservation::NoActiveSessionObserved => Confirmation::SessionEndObserved {
                additional_cached_clips: view.clips.iter().filter(|c| &c.game_id == g && !c.temporary && !self.before_clips.contains(&c.clip_id)).count(),
            },
            _ => Confirmation::Waiting,
        }
    }
}

fn command_js(command: &NativeCommand) -> String {
    let (game, start) = match command { NativeCommand::Start(g) => (g, true), NativeCommand::Stop(g) => (g, false) };
    // GameId is canonical uint64 text; JSON encoding still used at the boundary.
    let id = serde_json::to_string(game.as_str()).expect("string serialization");
    let operation = if start {
        r#"const a=window.SteamUIStore?.ActiveWindowInstance?.MainRunningApp;
        if(!a||typeof a.GetGameID!=='function'||a.GetGameID()!==id||s.m_recordingState!==null||s.m_bEnoughDiskSpace!==true||typeof s.StartRecording!=='function')return {outcome:'unavailable'};
        const devices=window.RemotePlayStore_SteamUI?.devices;
        if(!Array.isArray(devices)||devices.some(d=>d.status==='Streaming'))return {outcome:'unavailable'};
        if(window.g_FriendsUIApp?.BroadcastStore?.status?.bIsBroadcasting!==false)return {outcome:'unavailable'};
        const r=await s.StartRecording(id);
        return {outcome:'start',eresult:r.GetEResult()};"#
    } else {
        r#"if(s.m_recordingState?.m_gameID!==id||typeof s.StopRecording!=='function')return {outcome:'unavailable'};
        await s.StopRecording(id);return {outcome:'stop'};"#
    };
    format!("(async()=>{{const s=window.g_GRS;const id={id};if(!s||window.settingsStore?.clientSettings?.gamerecording_background_mode!==2)return {{outcome:'unavailable'}};{operation}}})()")
}

/// Result of Steam's own clip deletion (`GameRecordingStore.DeleteClip`, the call Steam's Media UI makes). The caller
/// decides success from the clip folder on disk: `Reported` only says Steam's service answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteReply {
    /// The service answered EResult OK.
    Reported,
    /// The service answered with a different EResult.
    Refused(u32),
    /// Steam's cache does not hold the clip even after a cache request. Nothing was sent.
    NotCached,
    /// The store or its cache changed before the call. Nothing was sent.
    Unavailable,
}

/// Steam's cache must hold the clip before its delete runs: Steam's DeleteClip reads the cached entry after the
/// service call, so an uncached clip would fail there after the service may already have removed the files.
const CLIP_CACHE_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

fn clip_cached_js(clip_id: &str) -> String {
    let id = serde_json::to_string(clip_id).expect("string serialization");
    format!("(()=>{{const s=window.g_GRS;const id={id};return !!(s&&s.m_clips&&typeof s.m_clips.has==='function'&&s.m_clips.has(id));}})()")
}

fn clip_delete_js(clip_id: &str) -> String {
    let id = serde_json::to_string(clip_id).expect("string serialization");
    format!("(async()=>{{const s=window.g_GRS;const id={id};if(!s||typeof s.DeleteClip!=='function'||!s.m_clips||typeof s.m_clips.has!=='function'||!s.m_clips.has(id))return {{outcome:'unavailable'}};const e=await s.DeleteClip(id);return {{outcome:'deleted',eresult:e}};}})()")
}

fn parse_delete_reply(v: &Value) -> Result<DeleteReply> {
    match v.get("outcome").and_then(Value::as_str) {
        Some("unavailable") => Ok(DeleteReply::Unavailable),
        Some("deleted") => {
            let code = parse_uint32_num(v.get("eresult").ok_or_else(|| malformed("eresult"))?, "eresult")?;
            Ok(if code == ERESULT_OK { DeleteReply::Reported } else { DeleteReply::Refused(code) })
        },
        _ => Err(malformed("delete reply")),
    }
}

fn parse_command_reply(v: &Value) -> Result<CommandReply> {
    match v.get("outcome").and_then(Value::as_str) {
        Some("unavailable") => Ok(CommandReply::Unavailable),
        Some("stop") => Ok(CommandReply::StopDispatched),
        Some("start") => {
            let code = parse_uint32_num(v.get("eresult").ok_or_else(|| malformed("eresult"))?, "eresult")?;
            Ok(if code == ERESULT_OK { CommandReply::StartAccepted } else { CommandReply::Rejected(code) })
        },
        _ => Err(malformed("command reply")),
    }
}

fn parse_view(v: &Value) -> Result<NativeView> {
    if v.get("schema").and_then(Value::as_u64) != Some(2) { return Err(malformed("view schema")); }
    if !boolean(v, "available")? {
        return Ok(NativeView { cache: Snapshot::StoreUnavailable, mode: None, running_game: None,
            can_start: false, can_stop: false, clips: vec![] });
    }
    let mut base = v.clone(); base["schema"] = Value::from(1);
    let cache = parse_snapshot(&base)?;
    let mode = match v.get("mode") {
        Some(Value::Null) => None,
        Some(n) => Some(parse_uint32_num(n, "mode")?),
        None => return Err(malformed("mode")),
    };
    let running_game = match v.get("running_game") {
        Some(Value::Null) => None,
        Some(g) => Some(GameId::from_value(g)?),
        None => return Err(malformed("running_game")),
    };
    let clips = v.get("clips").and_then(Value::as_array).ok_or_else(|| malformed("clips"))?
        .iter().map(parse_clip_summary).collect::<Result<Vec<_>>>()?;
    let Snapshot::Cached(c) = &cache else { unreachable!() };
    if c.clips.summary_count != clips.len() as u64 { return Err(malformed("clip count")); }
    Ok(NativeView { cache, mode, running_game, clips,
        can_start: boolean(v, "can_start")?, can_stop: boolean(v, "can_stop")? })
}

impl Bridge {
    /// Read-only: the store's display name for an appid. Used for a clip's game when Steam's `allApps` list, which
    /// DeckShell's library comes from, does not contain that game. `None` when the store does not know the app.
    pub fn app_display_name(&self, appid: u32) -> Result<Option<String>> {
        let expr = format!("(()=>{{const o=appStore.GetAppOverviewByAppID({appid});return o&&typeof o.display_name==='string'&&o.display_name.trim()?o.display_name:null}})()");
        match self.eval_json(&expr)? {
            Value::String(name) => Ok(Some(name)),
            _ => Ok(None),
        }
    }

    /// Pure observation; no Ensure/Init, getters that lazy-load, or subscriptions.
    pub fn native_recording_view(&self) -> Result<NativeView> {
        parse_view(&self.eval_json(VIEW_JS)?)
    }
    /// Explicit UI command ONLY. Never use for physical Steam Input chords.
    /// No replay even when the socket dies after Steam received the operation.
    pub fn native_recording_command(&self, command: &NativeCommand) -> Result<CommandReply> {
        parse_command_reply(&self.eval_json_once(&command_js(command))?)
    }
    /// Steam's existing one-shot cache loader. It swallows GetClips errors, so
    /// this says only 'requested', never 'enumeration succeeded'. No new store.
    pub fn request_native_clip_cache(&self) -> Result<()> {
        self.eval_json_once("(()=>{const s=window.g_GRS;if(s&&s.m_bClipLoadingTriggered===false&&typeof s.LazyLoadClips==='function'){s.LazyLoadClips();}return null})()")?;
        Ok(())
    }

    fn native_clip_cached(&self, clip_id: &str) -> Result<bool> {
        match self.eval_json(&clip_cached_js(clip_id))? {
            Value::Bool(cached) => Ok(cached),
            _ => Err(malformed("clip cache")),
        }
    }

    /// Deletes one clip through Steam's own store call. Explicit UI action only: one delete call, never replayed.
    /// If the clip is not cached, asks Steam for its one-shot cache and waits briefly (short polls, under the
    /// socket read timeout) before deciding. Callers verify the clip folder afterwards.
    pub fn delete_native_clip(&self, clip_id: &str) -> Result<DeleteReply> {
        if !self.native_clip_cached(clip_id)? {
            self.request_native_clip_cache()?;
            let deadline = std::time::Instant::now() + CLIP_CACHE_WAIT;
            while !self.native_clip_cached(clip_id)? {
                if std::time::Instant::now() >= deadline {
                    return Ok(DeleteReply::NotCached);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
        parse_delete_reply(&self.eval_json_once(&clip_delete_js(clip_id))?)
    }
}

#[cfg(test)]
mod delete_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn delete_reply_maps_eresult_and_refusals() {
        assert_eq!(parse_delete_reply(&json!({"outcome":"deleted","eresult":1})).unwrap(), DeleteReply::Reported);
        assert_eq!(parse_delete_reply(&json!({"outcome":"deleted","eresult":2})).unwrap(), DeleteReply::Refused(2));
        assert_eq!(parse_delete_reply(&json!({"outcome":"unavailable"})).unwrap(), DeleteReply::Unavailable);
        assert!(parse_delete_reply(&json!({"outcome":"deleted"})).is_err(), "a delete without EResult is not a success");
        assert!(parse_delete_reply(&json!({"outcome":"deleted","eresult":"1"})).is_err(), "EResult must be a number");
        assert!(parse_delete_reply(&json!({"outcome":"maybe"})).is_err());
        assert!(parse_delete_reply(&json!(null)).is_err());
    }

    #[test]
    fn delete_checks_the_cache_before_calling_steam_delete() {
        let js = clip_delete_js("clip_2379780_20261112_040949");
        let guard = js.find("!s.m_clips.has(id)").expect("cache guard");
        let call = js.find("await s.DeleteClip(id)").expect("steam delete call");
        assert!(guard < call, "the guard must come before Steam's DeleteClip");
        assert!(js.contains("\"clip_2379780_20261112_040949\""), "the id is embedded as a JSON string");
    }

    #[test]
    fn clip_id_is_embedded_as_an_escaped_json_string() {
        let js = clip_cached_js("a\"b');alert(1)//");
        assert!(js.contains(r#"const id="a\"b');alert(1)//";"#), "quotes must not break out of the literal: {js}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cache() -> Value {
        json!({"schema":1,"available":true,"session":null,"clips_requested":false,
            "clips_loading":true,"cached_clip_count":0,"enough_disk_space_cached":false,
            "h265_cached":false})
    }

    fn sample_clip_json(clip_id: &str, temporary: bool) -> Value {
        json!({
            "clip_id": clip_id,
            "game_id": "730",
            "duration_ms": "15000",
            "date_recorded": 1700000000u64,
            "start_timeline_id": "timeline_abc",
            "start_offset_ms": "0",
            "published_file_id": "0",
            "file_size": "25000000",
            "name": "Ace Round",
            "date_clipped": 1700000010u64,
            "temporary": temporary,
            "original_device": "deck",
            "original_gaming_device_type": 1,
            "date_downloaded": 0,
            "thumbnail_url": "https://steamloopback.host/media/thumb.jpg",
            "thumbnail_width": 320,
            "thumbnail_height": 180
        })
    }

    #[test]
    fn actual_deck_stop_summary_with_sanitized_identifiers_and_omitted_optional_fields() {
        // Source: root test5, 2026-11-09. Only shape, duration, size, dimensions,
        // temporary and start offset retained. All identifiers/URL/dates replaced.
        let value: Value = serde_json::from_str(include_str!("../tests/fixtures/native-clip-sanitized.json")).unwrap();
        let clip=parse_clip_summary(&value).unwrap();
        assert_eq!(clip.duration_ms,21816); assert_eq!(clip.file_size,15843111);
        assert!(!clip.temporary); assert_eq!(clip.game_id.as_str(),"13224077663011864576");
        assert_eq!(clip.name,""); assert_eq!(clip.published_file_id,0);
        assert_eq!(clip.original_device,""); assert_eq!(clip.original_gaming_device_type,0);
        assert_eq!(clip.date_downloaded,0);
        let mut view=view_value(); view["cached_clip_count"]=json!(1); view["clips"]=json!([value]);
        assert_eq!(parse_view(&view).unwrap().clips,vec![clip]);
    }

    fn view_value() -> Value {
        let mut v=cache(); v["schema"]=json!(2); v["mode"]=json!(2);
        v["running_game"]=json!("730"); v["can_start"]=json!(true);
        v["can_stop"]=json!(true); v["clips"]=json!([]);
        v["enough_disk_space_cached"]=json!(true); v
    }

    #[test]
    fn manual_controls_require_known_mode_identity_capabilities_and_disk_cache() {
        let view=parse_view(&view_value()).unwrap();
        assert!(view.start_game().is_some()); assert!(view.stop_game().is_none());
        for (key,bad) in [("mode",json!(null)),("mode",json!(1)),("mode",json!(0)),
            ("running_game",json!(null)),("can_start",json!(false)),("enough_disk_space_cached",json!(false)),
            ("session",json!({"game_id":"730"}))] {
            let mut v=view_value(); v[key]=bad;
            assert!(parse_view(&v).unwrap().start_game().is_none(), "{key}");
        }
        let mut v=view_value(); v["session"]=json!({"game_id":"730"});
        assert!(parse_view(&v).unwrap().stop_game().is_some());
    }

    #[test]
    fn native_view_rejects_malformed_and_count_mismatch_without_empty_success() {
        for (key,bad) in [("clips",json!(null)),("mode",json!("2")),("running_game",json!(730)),
            ("can_start",json!(null)),("cached_clip_count",json!(1))] {
            let mut v=view_value(); v[key]=bad; assert!(parse_view(&v).is_err(), "{key}");
        }
        assert_eq!(parse_view(&json!({"schema":2,"available":false})).unwrap().cache,Snapshot::StoreUnavailable);
        assert!(parse_view(&view_value()).unwrap().clips.is_empty()); // cache only
    }

    #[test]
    fn native_reply_never_converts_void_or_errors_to_saved_success() {
        assert_eq!(parse_command_reply(&json!({"outcome":"start","eresult":1})).unwrap(),CommandReply::StartAccepted);
        assert_eq!(parse_command_reply(&json!({"outcome":"start","eresult":15})).unwrap(),CommandReply::Rejected(15));
        assert_eq!(parse_command_reply(&json!({"outcome":"stop"})).unwrap(),CommandReply::StopDispatched);
        for v in [Value::Null,json!({"outcome":"start"}),json!({"outcome":"start","eresult":"1"}),
            json!({"outcome":"start","eresult":4294967297u64}),json!({"outcome":"saved"})] {
            assert!(parse_command_reply(&v).is_err());
        }
    }

    #[test]
    fn start_confirmation_needs_matching_observed_session_not_response_or_null() {
        let view=parse_view(&view_value()).unwrap();
        let pending=PendingCommand::new(NativeCommand::Start(view.running_game.clone().unwrap()),&view);
        assert_eq!(pending.observe(&view),Confirmation::Waiting);
        let mut v=view_value(); v["session"]=json!({"game_id":"999"});
        assert_eq!(pending.observe(&parse_view(&v).unwrap()),Confirmation::Waiting);
        v["session"]=json!({"game_id":"730"});
        assert_eq!(pending.observe(&parse_view(&v).unwrap()),Confirmation::SessionObserved);
        assert_eq!(pending.observe(&parse_view(&json!({"schema":2,"available":false})).unwrap()),Confirmation::Waiting);
    }

    #[test]
    fn stop_observation_distinguishes_session_end_from_cache_additions_without_claiming_save() {
        let mut value=view_value(); value["session"]=json!({"game_id":"730"});
        value["clips"]=json!([sample_clip_json("old",false)]); value["cached_clip_count"]=json!(1);
        let view=parse_view(&value).unwrap();
        let pending=PendingCommand::new(NativeCommand::Stop(view.stop_game().unwrap()),&view);
        value["session"]=Value::Null;
        assert_eq!(pending.observe(&parse_view(&value).unwrap()),Confirmation::SessionEndObserved { additional_cached_clips: 0 }); // old clip
        value["clips"]=json!([sample_clip_json("new",true)]);
        assert_eq!(pending.observe(&parse_view(&value).unwrap()),Confirmation::SessionEndObserved { additional_cached_clips: 0 }); // temporary
        value["clips"][0]["temporary"]=json!(false); value["clips"][0]["game_id"]=json!("999");
        assert_eq!(pending.observe(&parse_view(&value).unwrap()),Confirmation::SessionEndObserved { additional_cached_clips: 0 }); // other game
        value["clips"][0]["game_id"]=json!("730"); value["session"]=json!({"game_id":"730"});
        assert_eq!(pending.observe(&parse_view(&value).unwrap()),Confirmation::Waiting); // session still active
        value["session"]=Value::Null;
        assert_eq!(pending.observe(&parse_view(&value).unwrap()),Confirmation::SessionEndObserved { additional_cached_clips: 1 });
    }

    #[test]
    fn absent_store_is_distinct_from_present_null_session() {
        assert_eq!(parse_snapshot(&json!({"schema":1,"available":false})).unwrap(), Snapshot::StoreUnavailable);
        let Snapshot::Cached(state) = parse_snapshot(&cache()).unwrap() else { panic!("expected cache") };
        assert_eq!(state.session, SessionObservation::NoActiveSessionObserved);
        assert!(!state.clips.load_requested);
        assert!(state.clips.loading);
    }

    #[test]
    fn preserves_uint64_game_id_beyond_javascript_precision() {
        let mut value = cache();
        value["session"] = json!({"game_id":u64::MAX.to_string()});
        let Snapshot::Cached(state) = parse_snapshot(&value).unwrap() else { panic!("expected cache") };
        let SessionObservation::ActiveGame(game) = state.session else { panic!("expected observed session") };
        assert_eq!(game.as_str(), "18446744073709551615");
    }

    #[test]
    fn rejects_lossy_numbers_invalid_ids_and_out_of_range_values() {
        for game_id in [json!(42), json!(null), json!("0"), json!(""), json!("-1"),
            json!("01"), json!("1e3"), json!("18446744073709551616")]
        {
            let mut value = cache();
            value["session"] = json!({"game_id":game_id});
            assert!(matches!(parse_snapshot(&value), Err(Error::Protocol(_))));
        }
    }

    #[test]
    fn invalid_or_missing_fields_never_become_plausible_defaults() {
        for key in ["schema", "available", "session", "clips_requested", "clips_loading",
            "cached_clip_count", "enough_disk_space_cached", "h265_cached"]
        {
            let mut value = cache();
            value.as_object_mut().unwrap().remove(key);
            assert!(parse_snapshot(&value).is_err(), "missing {key}");
        }
        for (key, bad) in [("available", json!("false")), ("session", json!(false)),
            ("clips_loading", json!(0)), ("cached_clip_count", json!(-1)),
            ("cached_clip_count", json!(0.5)), ("schema", json!(2))]
        {
            let mut value = cache();
            value[key] = bad;
            assert!(parse_snapshot(&value).is_err(), "invalid {key}");
        }
    }

    #[test]
    fn finished_empty_cache_is_an_observation_not_a_successful_enumeration() {
        let mut value = cache();
        value["clips_requested"] = json!(true);
        value["clips_loading"] = json!(false);
        let Snapshot::Cached(state) = parse_snapshot(&value).unwrap() else { panic!("expected cache") };
        assert_eq!(state.clips, ClipCacheObservation { load_requested: true, loading: false, summary_count: 0 });
        assert_eq!(state.session, SessionObservation::NoActiveSessionObserved);
    }

    #[test]
    fn parser_errors_do_not_echo_private_payload() {
        let mut value = cache();
        value["session"] = json!({"game_id":"private-invalid-fixture"});
        let error = parse_snapshot(&value).unwrap_err().to_string();
        assert!(!error.contains("private-invalid-fixture"));
    }

    #[test]
    fn test_oversized_and_invalid_eresult_must_fail_protocol_error() {
        // 4294967297 = 2^32 + 1. Must NOT truncate to 1 and must NOT become Success(empty).
        let oversized = json!({
            "eresult": 4294967297u64,
            "body": { "clip": [] }
        });
        assert!(parse_get_clips_response(&oversized).is_err(), "oversized eresult must fail");

        // String representation for eresult is unconfirmed and rejected
        let string_eresult = json!({
            "eresult": "1",
            "body": { "clip": [] }
        });
        assert!(parse_get_clips_response(&string_eresult).is_err(), "string eresult must fail");

        // Negative and fractional eresult rejected
        assert!(parse_get_clips_response(&json!({"eresult": -1, "body": {"clip": []}})).is_err());
        assert!(parse_get_clips_response(&json!({"eresult": 1.5, "body": {"clip": []}})).is_err());

        // Unconfirmed alias "result" rejected
        assert!(parse_get_clips_response(&json!({"result": 1, "body": {"clip": []}})).is_err());
    }

    #[test]
    fn test_get_clips_success_empty() {
        let payload = json!({
            "eresult": 1,
            "body": {
                "clip": []
            }
        });
        let outcome = parse_get_clips_response(&payload).unwrap();
        assert_eq!(outcome, GetClipsOutcome::Success(vec![]));

        // Direct top-level clip without body must be rejected (unconfirmed format)
        let unconfirmed_direct = json!({
            "eresult": 1,
            "clip": []
        });
        assert!(parse_get_clips_response(&unconfirmed_direct).is_err());
    }

    #[test]
    fn test_get_clips_failure_with_empty_body_distinguished_from_empty_media() {
        let payload_err = json!({
            "eresult": 2,
            "body": {}
        });
        let outcome = parse_get_clips_response(&payload_err).unwrap();
        assert_eq!(outcome, GetClipsOutcome::NonSuccess { eresult: 2 });
        // Must NEVER be an empty success list!
        assert_ne!(outcome, GetClipsOutcome::Success(vec![]));

        let payload_denied = json!({
            "eresult": 15,
            "body": null
        });
        assert_eq!(parse_get_clips_response(&payload_denied).unwrap(), GetClipsOutcome::NonSuccess { eresult: 15 });
    }

    #[test]
    fn test_get_clips_missing_or_invalid_clip_list_or_summary() {
        // Missing eresult
        assert!(parse_get_clips_response(&json!({"body": {"clip": []}})).is_err());

        // Body.clip is not an array
        assert!(parse_get_clips_response(&json!({"eresult": 1, "body": {"clip": "not_an_array"}})).is_err());

        // Missing clip_id
        let mut bad_clip = sample_clip_json("test_clip", false);
        bad_clip.as_object_mut().unwrap().remove("clip_id");
        assert!(parse_get_clips_response(&json!({"eresult": 1, "body": {"clip": [bad_clip]}})).is_err());

        // Empty clip_id
        let mut empty_id = sample_clip_json("test_clip", false);
        empty_id["clip_id"] = json!("");
        assert!(parse_get_clips_response(&json!({"eresult": 1, "body": {"clip": [empty_id]}})).is_err());

        // Missing game_id
        let mut missing_game = sample_clip_json("test_clip", false);
        missing_game.as_object_mut().unwrap().remove("game_id");
        assert!(parse_get_clips_response(&json!({"eresult": 1, "body": {"clip": [missing_game]}})).is_err());
    }

    #[test]
    fn test_temporary_field_strict_type_and_no_false_permanent_defaults() {
        // Missing temporary field must NOT silently become permanent false default
        let mut missing_temp = sample_clip_json("c_notemp", false);
        missing_temp.as_object_mut().unwrap().remove("temporary");
        assert!(parse_clip_summary(&missing_temp).is_err(), "missing temporary must fail");

        // String "true" must NOT silently become permanent false default
        let mut string_true = sample_clip_json("c_str", false);
        string_true["temporary"] = json!("true");
        assert!(parse_clip_summary(&string_true).is_err(), "string temporary must fail");

        // String "false" must NOT be accepted
        let mut string_false = sample_clip_json("c_str2", false);
        string_false["temporary"] = json!("false");
        assert!(parse_clip_summary(&string_false).is_err(), "string temporary must fail");

        // Number 1 or null must NOT be accepted
        let mut num_temp = sample_clip_json("c_num", false);
        num_temp["temporary"] = json!(1);
        assert!(parse_clip_summary(&num_temp).is_err(), "number temporary must fail");

        let mut null_temp = sample_clip_json("c_null", false);
        null_temp["temporary"] = json!(null);
        assert!(parse_clip_summary(&null_temp).is_err(), "null temporary must fail");
    }

    #[test]
    fn test_strict_types_for_string_and_numeric_fields() {
        // Explicit wrong types must NOT be converted to defaults
        let mut bad_name = sample_clip_json("c1", false);
        bad_name["name"] = json!(12345); // number instead of string
        assert!(parse_clip_summary(&bad_name).is_err(), "numeric name must fail");

        let mut null_name = sample_clip_json("c2", false);
        null_name["name"] = json!(null);
        assert!(parse_clip_summary(&null_name).is_err(), "null name must fail");

        let mut bad_timeline = sample_clip_json("c3", false);
        bad_timeline["start_timeline_id"] = json!(true);
        assert!(parse_clip_summary(&bad_timeline).is_err(), "boolean start_timeline_id must fail");

        let mut bad_width = sample_clip_json("c4", false);
        bad_width["thumbnail_width"] = json!("320"); // string instead of number
        assert!(parse_clip_summary(&bad_width).is_err(), "string thumbnail_width must fail");

        let mut oversized_width = sample_clip_json("c5", false);
        oversized_width["thumbnail_width"] = json!(4294967296u64); // > u32::MAX
        assert!(parse_clip_summary(&oversized_width).is_err(), "oversized thumbnail_width must fail");

        let mut null_date = sample_clip_json("c6", false);
        null_date["date_clipped"] = json!(null);
        assert!(parse_clip_summary(&null_date).is_err(), "null date_clipped must fail");
    }

    #[test]
    fn test_get_clips_uint64_beyond_safe_integer_and_overflow() {
        let mut clip = sample_clip_json("c1", false);
        // > 2^53 integer (9007199254740993 > MAX_SAFE_INTEGER 9007199254740991)
        clip["duration_ms"] = json!("9007199254740993");
        // Maximum uint64
        clip["file_size"] = json!("18446744073709551615");

        let parsed = parse_clip_summary(&clip).unwrap();
        assert_eq!(parsed.duration_ms, 9007199254740993u64);
        assert_eq!(parsed.file_size, 18446744073709551615u64);

        // Overflow > u64::MAX
        let mut overflow_clip = clip.clone();
        overflow_clip["duration_ms"] = json!("18446744073709551616");
        assert!(parse_clip_summary(&overflow_clip).is_err());

        // Reject lossy JS number representation for uint64
        let mut number_clip = clip.clone();
        number_clip["duration_ms"] = json!(9007199254740993u64);
        assert!(parse_clip_summary(&number_clip).is_err());
    }

    #[test]
    fn test_temporary_vs_permanent_preservation() {
        let temp_clip = parse_clip_summary(&sample_clip_json("c_temp", true)).unwrap();
        let perm_clip = parse_clip_summary(&sample_clip_json("c_perm", false)).unwrap();

        assert!(temp_clip.temporary);
        assert!(!perm_clip.temporary);

        let mut catalog = ClipCatalog::new();
        catalog.merge_summary(temp_clip);
        catalog.merge_summary(perm_clip);

        assert_eq!(catalog.len(), 2);
        let perm_ids: Vec<&str> = catalog.permanent_clips().map(|c| c.clip_id.as_str()).collect();
        let temp_ids: Vec<&str> = catalog.temporary_clips().map(|c| c.clip_id.as_str()).collect();

        assert_eq!(perm_ids, vec!["c_perm"]);
        assert_eq!(temp_ids, vec!["c_temp"]);
    }

    #[test]
    fn test_repeat_clip_id_deterministic_merge() {
        let mut catalog = ClipCatalog::new();

        let mut initial = sample_clip_json("clip_dup_1", true);
        initial["name"] = json!("Pending Clip");
        catalog.merge_summary(parse_clip_summary(&initial).unwrap());
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.as_slice()[0].name, "Pending Clip");
        assert!(catalog.as_slice()[0].temporary);

        // Updated clip arrived for same clip_id (e.g. from SaveClip or stop notification)
        let mut updated = sample_clip_json("clip_dup_1", false);
        updated["name"] = json!("Saved Permanent Clip");
        catalog.merge_summary(parse_clip_summary(&updated).unwrap());

        // Must deterministically update in place, NOT create a second duplicate row
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.as_slice()[0].name, "Saved Permanent Clip");
        assert!(!catalog.as_slice()[0].temporary);
    }

    #[test]
    fn test_unknown_extra_fields_without_losing_confirmed() {
        let mut clip_with_extras = sample_clip_json("c_future", false);
        clip_with_extras["future_field_protocol_99"] = json!("extra_value");
        clip_with_extras["nested_unknown"] = json!({"feature": true, "code": 123});

        let parsed = parse_clip_summary(&clip_with_extras).unwrap();
        assert_eq!(parsed.clip_id, "c_future");
        assert_eq!(parsed.game_id.as_str(), "730");
        assert_eq!(parsed.duration_ms, 15000);
        assert_eq!(parsed.file_size, 25000000);
    }
}
