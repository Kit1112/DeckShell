//! /routes/media/grid - screenshots grid. Items: Steam userdata/<acct>/760/remote/<appid>/screenshots/*.jpg (full image with thumbnail fallback),
//! newest first, laid out like the captured page (docs/parity ledger /routes/media/grid): 3 columns, card 335x188 css, pitch 342.5 x 196.
//! `handle_media` must be dispatched by app.rs (`Route::Media => self.handle_media(a)`); pointer/touch work through `wire`.
use steam_assets::i18n;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, Instant, Duration};

use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::screens::settings::{PRow, SC};
use crate::{AppWindow, GPageRow, MediaState};
use steam_bridge::send_to_phone::{self, AccountFlags, MediaKind, SendPhase, SendReply, SendRequest};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[path = "media_thumbs.rs"]
pub(crate) mod media_thumbs;
#[path = "media_clips.rs"]
pub(crate) mod media_clips;
#[path = "media_player.rs"]
pub(crate) mod media_player;
#[path = "media_batch.rs"]
mod media_batch;
use media_batch::{Batch, Outcome, Selection};

const PAGE: &str = include_str!("../../ui/generated/media/page.json");

#[derive(Clone, Debug)]
pub struct Item {
    /// 0 screenshot, 1 clip, 2 recording
    pub kind: u8,
    /// image shown on the card (thumbnail when Steam wrote one)
    pub thumb: Option<PathBuf>,
    /// Screenshots only. Native summaries have no invented local file path.
    pub full: Option<PathBuf>,
    pub native: Option<steam_bridge::recording::ClipSummary>,
    pub app: u32,
    pub mtime: SystemTime,
}

// Account + exact native source identity: never use an appid/hash as clip identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ThumbKey {
    pub account: u32,
    pub clip_id: String,
    pub url: String,
    pub screenshot: Option<(PathBuf, Option<PathBuf>)>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThumbTicket { pub key: ThumbKey, pub epoch: u64 }
const THUMB_CACHE_CAP: usize = 32; // <=32.8 MB RGBA at 640x400, excluding renderer overhead
const THUMB_PENDING_CAP: usize = 24;
const THUMB_RETRY: Duration = Duration::from_secs(5);

struct ThumbCache<T> { entries: VecDeque<(ThumbKey,T)> }
impl<T> Default for ThumbCache<T> { fn default() -> Self { Self { entries: VecDeque::new() } } }
impl<T: Clone> ThumbCache<T> {
    fn get(&mut self, key: &ThumbKey, visible: bool) -> Option<T> {
        let index=self.entries.iter().position(|(k,_)| k==key)?;
        // Materialised off-screen rows must not outrank visible thumbnails:
        // otherwise every rebuild can evict and reload a visible card forever.
        if !visible { return Some(self.entries[index].1.clone()); }
        let entry=self.entries.remove(index)?; let value=entry.1.clone();
        self.entries.push_back(entry); Some(value)
    }
    fn insert(&mut self, key: ThumbKey, value: T) {
        self.entries.retain(|(k,_)| k!=&key);
        if self.entries.len()>=THUMB_CACHE_CAP { self.entries.pop_front(); }
        self.entries.push_back((key,value));
    }
}

#[derive(Default)]
struct St {
    account_epoch: u64,
    thumb_epoch: u64,
    thumb_retry_scheduled: bool,
    thumb_cache: ThumbCache<slint::Image>,
    thumb_pending: HashSet<ThumbKey>,
    thumb_failed: VecDeque<(ThumbKey,Instant)>,
    items: Vec<Item>,
    /// indices into `items` after the type / game filters
    view: Vec<usize>,
    focus: usize,
    preferred_col: Option<usize>,
    vy: f32,
    /// open viewer: index into `view`
    viewer: Option<usize>,
    vfocus: usize,
    /// Item key whose delete is armed: the next delete on that same item really deletes; any other item re-arms.
    confirm_key: Option<String>,
    /// Clip whose Steam delete is in flight (a second delete is refused until it finishes).
    deleting: Option<String>,
    note: String,
    /// 0 all, 1 screenshots, 2 clips, 3 recordings
    type_filter: u8,
    game: Option<u32>,
    /// Names of clip games that the library list lacks, from Steam's store; `clip_name_asked` bounds the lookups.
    clip_names: HashMap<u32, String>,
    clip_name_asked: HashSet<u32>,
    /// «Отправить на телефон»: the state shown in the viewer and whether a check or send is in flight.
    phone: SendPhase,
    phone_busy: bool,
    /// What the phone panel in the viewer shows (`Quiet` hides it).
    phone_tone: PhoneTone,
    /// Items whose phone send finished (or whose result is unknown) in this session: never sent again.
    phone_sent: HashSet<String>,
    selection: Option<Selection>,
    batch: Option<Batch>,
    batch_note: String,
}
thread_local! { static ST: RefCell<St> = RefCell::new(St::default()); }

/// The phone send panel in the viewer: `Busy` while a send runs, then its outcome. `Quiet` hides the panel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum PhoneTone { #[default] Quiet, Busy, Sent, Failed, Info }

impl PhoneTone {
    /// Code of `MediaState.phone-tone` in media.slint.
    fn code(self) -> i32 {
        match self { PhoneTone::Quiet => 0, PhoneTone::Busy => 1, PhoneTone::Sent => 2, PhoneTone::Failed => 3, PhoneTone::Info => 4 }
    }
}

/// Hides a finished phone panel so that another note cannot show under its colours. A send in flight keeps its panel.
fn clear_phone_panel(s: &mut St) {
    if !s.phone_busy { s.phone_tone = PhoneTone::Quiet; }
}

fn phone_phase_label(phase: &SendPhase) -> Option<&'static str> {
    Some(i18n::tr_ref(match phase {
        SendPhase::Exporting => "media_phone.exporting",
        SendPhase::Uploading => "media_phone.uploading",
        SendPhase::Sent => "media_phone.sent",
        _ => return None,
    }))
}

fn phone_block_label(token: &str) -> Option<String> {
    let key = match token {
        "#SendToChat_ClipTooLong" => "media_phone.too_long",
        "#ShareSheet_ScreenUploadNotAllowed" => "media_phone.screenshots_blocked",
        "#ShareSheet_ClipUploadNotAllowed" => "media_phone.clips_blocked",
        "#ShareSheet_LimitedScreenshotNotAllowed" => "media_phone.limited_account",
        _ => return None,
    };
    Some(super::stock_label(token, key))
}

fn phone_reply_label(reply: send_to_phone::PhoneReply, kind: MediaKind) -> Option<String> {
    use send_to_phone::PhoneReply;
    Some(match (reply, kind) {
        (PhoneReply::Ready, _) => return None,
        (PhoneReply::QueryError, _) => i18n::tr("media_phone.query_error"),
        (PhoneReply::NoPhone, _) => super::stock_label("#SendToPhone_NoPhone", "media_phone.no_phone"),
        (PhoneReply::OutdatedApp, MediaKind::Clip) => i18n::tr("media_phone.update_clips"),
        (PhoneReply::OutdatedApp, MediaKind::Screenshot) => i18n::tr("media_phone.update_screenshots"),
    })
}

fn phone_refusal_label(reason: &str, kind: MediaKind) -> String {
    use send_to_phone::PhoneReply;
    let reply = match reason {
        "query" => PhoneReply::QueryError,
        "no_phone" => PhoneReply::NoPhone,
        "outdated" => PhoneReply::OutdatedApp,
        "no_preset" => return i18n::tr("media_phone.clip_size"),
        "no_path" => return i18n::tr("media_phone.missing_path"),
        "too_big" => return i18n::tr("media_phone.size_limit"),
        _ => return i18n::tr_args("media_phone.refused", &[("reason", reason)]),
    };
    phone_reply_label(reply, kind).unwrap_or_default()
}

/// Stage of the send bar in media.slint: 0 check, 1 export, 2 upload, 3 done.
fn phone_stage(phase: &SendPhase) -> i32 {
    match phase { SendPhase::Exporting => 1, SendPhase::Uploading => 2, SendPhase::Sent => 3, _ => 0 }
}
/// Newest `enter_media` scan; an older scan that finishes later is dropped.
static MEDIA_SCAN_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Rebuilds the filtered `view` from `items` and keeps the focus inside it.
fn refilter_state(s: &mut St) {
    let (tf, game) = (s.type_filter, s.game);
    s.view = s
        .items
        .iter()
        .enumerate()
        .filter(|(_, it)| (tf == 0 || it.kind + 1 == tf) && game.map(|g| g == it.app).unwrap_or(true))
        .map(|(i, _)| i)
        .collect();
    let old_focus = s.focus;
    s.focus = s.focus.min(s.view.len().saturating_sub(1));
    if s.focus != old_focus { s.preferred_col = None; }
}

/// Removes the item with `key` from `items` and leaves focus on its grid position, so the next item takes it.
/// The caller refilters afterwards. Closes the viewer and drops any armed confirmation.
fn remove_item_in_state(s: &mut St, key: &str) {
    let Some(i) = s.items.iter().position(|it| item_key(it) == key) else { return };
    if let Some(p) = s.view.iter().position(|&v| v == i) {
        s.focus = p;
        s.preferred_col = None;
    }
    s.items.remove(i);
    s.viewer = None;
    s.vfocus = 0;
    s.confirm_key = None;
    s.note.clear();
}

/// Confirmation rule for delete: a delete arms the item shown; a delete on the item that is already armed executes.
#[derive(Debug, PartialEq, Eq)]
enum DeleteStep { Arm, Execute }

fn delete_step(armed: Option<&str>, key: &str) -> DeleteStep {
    if armed == Some(key) { DeleteStep::Execute } else { DeleteStep::Arm }
}

/// Removes a screenshot file and its separate thumbnail. The full file goes first: if that fails, the thumbnail stays.
fn remove_screenshot_files(full: &std::path::Path, thumb: Option<&std::path::Path>) -> std::io::Result<()> {
    std::fs::remove_file(full)?;
    if let Some(t) = thumb {
        if t != full && t.is_file() {
            let _ = std::fs::remove_file(t);
        }
    }
    Ok(())
}

fn native_clip_id_of(it: &Item) -> Option<String> {
    media_player::native_clip_id(it).map(str::to_owned)
}

/// Viewer action row. A native clip: Play (only when its recording can play) and Delete. A screenshot: Share, Upload, Delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewerAct { Play, Share, Upload, PhoneSend, Delete }

fn viewer_actions(it: &Item, playable: bool) -> Vec<ViewerAct> {
    if it.native.is_some() {
        let mut v = Vec::with_capacity(3);
        if playable { v.push(ViewerAct::Play); }
        v.push(ViewerAct::PhoneSend);
        v.push(ViewerAct::Delete);
        v
    } else if it.kind == 0 {
        vec![ViewerAct::Share, ViewerAct::Upload, ViewerAct::PhoneSend, ViewerAct::Delete]
    } else {
        vec![ViewerAct::Delete]
    }
}

/// File of a screenshot for «Отправить на телефон» (the Steam screenshot path, checked again in the send).
fn phone_shot_path(it: &Item) -> Option<String> {
    if it.native.is_some() || it.kind != 0 { return None; }
    it.full.as_ref().map(|p| p.to_string_lossy().into_owned())
}

/// What «Отправить на телефон» sends for this item: a clip (its Steam id, length) or a screenshot with a file.
fn phone_target(it: &Item) -> Option<(MediaKind, Option<u64>, Option<String>)> {
    if let Some(c) = &it.native {
        return Some((MediaKind::Clip, Some(c.duration_ms), Some(c.clip_id.clone())));
    }
    if it.kind == 0 && it.full.is_some() {
        return Some((MediaKind::Screenshot, None, None));
    }
    None
}

/// Item label. A clip longer than 60 s keeps the item but says it is unavailable; the press shows the stock reason.
fn phone_label(it: &Item) -> String {
    let base = t_or("#ExportClip_SendToPhone", i18n::tr_ref("media.send_to_phone"));
    match phone_target(it) {
        Some((kind, duration, _)) if send_to_phone::menu_block(kind, duration, AccountFlags::default()).is_some() => {
            i18n::tr_args("media.unavailable", &[("base", &(base).to_string())])
        }
        _ => base,
    }
}

fn delete_label(armed: bool) -> String {
    if armed {
        t_or("#MediaPage_Delete_Confirm_Short", i18n::tr_ref("media.confirm_removal"))
    } else {
        t_or("#MediaPage_ScreenshotContext_Delete", i18n::tr_ref("media.delete"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaOp {
    Type(u8),
    Game(Option<u32>),
    Open,
    Share,
    Upload,
    Delete,
    NativeStart,
    NativeStop,
    NativeRefresh,
    NativeLoadClips,
    NativeAcknowledge,
    PhoneSend,
    SelectMany,
    BatchSend,
    BatchDelete,
}

/// Media-type filter entries (value, Steam key, fallback). Steam's gamepad branch of the type filter (sp.js FilterModal: the
/// list is [All, Screenshot] when the gamepad UI is active, [All, Clip, Screenshot, Recording] on desktop), so DeckShell,
/// which is the gamepad UI, offers All and Screenshot only.
pub(crate) fn type_filter_options() -> Vec<(u8, (&'static str, &'static str))> {
    vec![(0, ("#MediaManager_MediaType_All", i18n::tr_ref("media.all"))), (1, ("#MediaManager_MediaType_Screenshot", i18n::tr_ref("media.screenshots")))]
}
/// Row shown for the active filter. A value outside the list (clip / recording) shows the first entry, as Steam's
/// `options.find(o => o.data == value) || options[0]` does.
pub(crate) fn type_filter_index(opts: &[(u8, (&'static str, &'static str))], tf: u8) -> usize {
    opts.iter().position(|o| o.0 == tf).unwrap_or(0)
}

pub fn init_strings(ui: &AppWindow) {
    let st = ui.global::<MediaState>();
    st.set_title(t_or("#MainTabsMedia", "").into());
    st.set_empty_text(t_or("#MediaPage_NoContentAvailable", i18n::tr_ref("media.no_media_files_available")).into());
}

pub fn wire(ui: &AppWindow) {
    ui.global::<MediaState>().on_entered(|| super::defer(|a| a.enter_media()));
    ui.global::<MediaState>().on_activated(|i| {
        // Capture the card identity before deferring (a clip refresh can re-sort the grid).
        let focus = (i / 2).max(0) as usize + window_start();
        let key = ST.with(|s| key_at(&s.borrow(), focus));
        super::defer(move |a| {
            if a.router.current() != &crate::router::Route::Media || a.media_batch_busy() { return; }
            let focused = ST.with(|s| {
                let mut s = s.borrow_mut();
                let Some(pos) = key.as_deref().and_then(|key| view_index_of(&s, key)) else { return false };
                s.focus = pos;
                s.preferred_col = None;
                true
            });
            if focused { a.media_activate_card(); }
        });
    });
    ui.global::<MediaState>().on_viewer_action(|i| super::defer(move |a| {
        ST.with(|s| s.borrow_mut().vfocus = i.max(0) as usize);
        a.media_viewer_activate();
    }));
    ui.global::<MediaState>().on_viewer_close(|| super::defer(|a| {
        a.media_back();
    }));
}

fn thumb_key(item: &Item, account: u32) -> Option<ThumbKey> {
    if let Some(full) = &item.full {
        return Some(ThumbKey { account, clip_id: String::new(), url: String::new(), screenshot: Some((full.clone(), item.thumb.clone())) });
    }
    let clip=item.native.as_ref()?;
    if clip.clip_id.len()>128 || clip.thumbnail_url.len()>1024 || clip.thumbnail_url.is_empty() { return None; }
    Some(ThumbKey { account, clip_id: clip.clip_id.clone(), url: clip.thumbnail_url.clone(), screenshot: None })
}
fn preserve_async_scroll(s: &mut St, ui_vy: f32) {
    if ui_vy.is_finite() { s.vy=(-ui_vy/SC).max(0.0); }
}
fn thumbnail_near_view(y: f32, height: f32, scroll: f32, screen_css_height: f32) -> bool {
    // Window height is a conservative overscan; footer/panel reduce the actual
    // clip. This avoids focus-only loading when the user scrolls with touch.
    y+height>=scroll && y<=scroll+screen_css_height
}
fn invalidate_thumbs(s: &mut St) {
    s.thumb_epoch=s.thumb_epoch.wrapping_add(1);
    s.thumb_pending.clear();
}
fn thumbnail_result_is_current(s: &St, ticket: &ThumbTicket, account: Option<u32>) -> bool {
    s.thumb_epoch==ticket.epoch && account==Some(ticket.key.account)
        && s.thumb_pending.contains(&ticket.key)
        && s.view.iter().filter_map(|&i| s.items.get(i)).any(|it| thumb_key(it,ticket.key.account).as_ref()==Some(&ticket.key))
}
fn apply_thumbnail_result(s: &mut St, ticket: &ThumbTicket, account: Option<u32>, image: Option<slint::Image>) -> bool {
    let current=thumbnail_result_is_current(s,ticket,account);
    if s.thumb_epoch==ticket.epoch { s.thumb_pending.remove(&ticket.key); }
    if !current { return false; }
    if let Some(image)=image { s.thumb_cache.insert(ticket.key.clone(),image); }
    else {
        s.thumb_failed.retain(|(key,_)| key!=&ticket.key);
        if s.thumb_failed.len()>=THUMB_CACHE_CAP { s.thumb_failed.pop_front(); }
        s.thumb_failed.push_back((ticket.key.clone(),Instant::now()));
    }
    true
}

fn cols() -> usize {
    let g: Value = serde_json::from_str(PAGE).unwrap_or_default();
    g["geo"]["cols"].as_u64().unwrap_or(3).max(1) as usize
}
/// Only this many cards (3 rows x cols around the focus) are materialised; the rest of the list scrolls in as focus moves.
const WINDOW: usize = 60;

fn window_start() -> usize {
    ST.with(|s| {
        let s = s.borrow();
        window_start_of(s.focus, cols())
    })
}
fn window_start_of(focus: usize, cols: usize) -> usize {
    (focus / cols).saturating_sub(6) * cols
}

fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    let (n, unit) = match s {
        0..=3599 => (s / 60, i18n::tr_ref("media.minute")),
        3600..=86399 => (s / 3600, i18n::tr_ref("media.backwards")),
        86400..=2591999 => (s / 86400, i18n::tr_ref("media.backwards_2")),
        _ => (s / 2592000, i18n::tr_ref("media.month_back")),
    };
    format!("{} {}", n.max(1), unit)
}

// Screenshots of shortcuts use the 64-bit game ID, while the library is
// indexed by the high 32-bit shortcut app ID.
fn screenshot_app_id(dir: &str) -> Option<u32> {
    let id = dir.parse::<u64>().ok()?;
    if let Ok(id) = u32::try_from(id) { return Some(id); }
    if id & 0xffff_ffff == 0x0200_0000 && id >> 63 == 1 { Some((id >> 32) as u32) } else { None }
}

fn scan(root: &std::path::Path, acct: u32) -> Vec<Item> {
    let user = root.join("userdata").join(acct.to_string());
    let base = user.join("760/remote");
    let mut out = vec![];
    if let Ok(apps) = std::fs::read_dir(&base) {
        for a in apps.flatten() {
            let app = screenshot_app_id(&a.file_name().to_string_lossy()).unwrap_or(0);
            let shots = a.path().join("screenshots");
            let Ok(rd) = std::fs::read_dir(&shots) else { continue };
            for f in rd.flatten() {
                let p = f.path();
                if !p.extension().map(|e| e == "jpg" || e == "png").unwrap_or(false) || !p.is_file() {
                    continue;
                }
                let th = shots.join("thumbnails").join(f.file_name());
                if let Ok(m) = f.metadata().and_then(|m| m.modified()) {
                    out.push(Item { kind: 0, thumb: Some(if th.exists() { th } else { p.clone() }), full: Some(p), native: None, app, mtime: m });
                }
            }
        }
    }
    // Clips Steam wrote to disk. The native cache may be empty (it only fills on request), so the folders are listed too.
    // Their app is resolved on the UI thread (see `resolve_native_apps`); a native summary with the same id replaces them.
    for clip in media_clips::disk_clip_summaries(root, acct) {
        let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(clip.date_recorded as u64);
        out.push(Item { kind: 1, thumb: None, full: None, native: Some(clip), app: 0, mtime });
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    out
}

fn kind_label(kind: u8) -> String {
    match kind {
        0 => t_or("#MediaPage_Screenshot", i18n::tr_ref("media.screenshot")),
        1 => i18n::tr_ref("media.clip").into(),
        _ => i18n::tr_ref("media.recording").into(),
    }
}

fn media_name(name: Option<&str>, app: u32) -> String {
    name.map(str::to_owned).unwrap_or_else(|| if app == 0 { String::new() } else { i18n::tr_ref("media.unknown_game").into() })
}

/// The app of a native clip. Steam writes the recording's 64-bit GameID: a library entry whose rungame id matches
/// wins; otherwise a GameID without high bits is a Steam app, so its appid is the low 24 bits (even when the library
/// list lacks that game; its name is then looked up in the store). A shortcut GameID not in the library gives 0.
fn app_for_clip_game(games: &[steam_data::Game], game_id: &str) -> u32 {
    let Ok(id) = game_id.parse::<u64>() else { return 0 };
    if let Some(g) = games.iter().find(|g| g.rungame_id() == id) { return g.app_id; }
    let appid = (id & 0xff_ffff) as u32;
    if id >> 32 != 0 || appid == 0 { return 0; }
    games.iter().find(|g| g.kind == steam_data::GameKind::Steam && g.app_id == appid)
        .or_else(|| games.iter().find(|g| g.app_id == appid))
        .map(|g| g.app_id).unwrap_or(appid)
}

thread_local! { static UNRESOLVED_CLIPS_LOGGED: RefCell<HashSet<String>> = RefCell::new(HashSet::new()); }

/// Stable identity of a grid entry: the native clip ID, or the screenshot file.
fn item_key(it: &Item) -> String {
    match &it.native {
        Some(c) => format!("clip:{}", c.clip_id),
        None => format!("shot:{}", it.full.as_ref().map(|p| p.display().to_string()).unwrap_or_default()),
    }
}

fn key_at(s: &St, view_pos: usize) -> Option<String> {
    s.view.get(view_pos).and_then(|&i| s.items.get(i)).map(item_key)
}

fn view_index_of(s: &St, key: &str) -> Option<usize> {
    s.view.iter().position(|&i| s.items.get(i).map(item_key).as_deref() == Some(key))
}

fn media_caption(name: &str, kind: u8, time: SystemTime) -> String {
    [name.to_string(), kind_label(kind), ago(time)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ·  ")
}

// The header is reachable only from the first grid row (or an empty grid),
// and never from the fullscreen viewer.
pub(crate) fn grid_header_boundary() -> bool {
    ST.with(|s| grid_at_header_boundary(&s.borrow(), cols()))
}
fn grid_at_header_boundary(s: &St, cols: usize) -> bool {
    s.viewer.is_none() && (s.view.is_empty() || s.focus < cols)
}
pub(crate) fn navigation_signature() -> (usize, Option<usize>, usize) {
    ST.with(|s| state_navigation_signature(&s.borrow()))
}
fn state_navigation_signature(s: &St) -> (usize, Option<usize>, usize) {
    (s.focus, s.viewer, s.vfocus)
}

fn navigate_grid(s: &mut St, action: Action, cols: usize, y0: f32, py: f32, bottom: f32) {
    let n = s.view.len();
    if n == 0 { return; }
    let old = s.focus;
    match action {
        Action::Left if old % cols > 0 => { s.focus -= 1; s.preferred_col = None; }
        Action::Right if old % cols + 1 < cols && old + 1 < n => { s.focus += 1; s.preferred_col = None; }
        Action::Up if old >= cols => {
            let col = *s.preferred_col.get_or_insert(old % cols);
            s.focus = (old / cols - 1) * cols + col;
        }
        Action::Down if (old / cols + 1) * cols < n => {
            let col = *s.preferred_col.get_or_insert(old % cols);
            s.focus = ((old / cols + 1) * cols + col).min(n - 1);
        }
        _ => return,
    }
    let top = y0 + (s.focus / cols) as f32 * py;
    if top - s.vy < 60.0 { s.vy = (top - 60.0).max(0.0); }
    else if top + py - s.vy > bottom { s.vy = top + py - bottom; }
}

impl App {
    fn preserve_media_async_scroll(&self) {
        if self.router.current()==&crate::router::Route::Media {
            let y=self.ui().global::<MediaState>().get_vy();
            ST.with(|s| preserve_async_scroll(&mut s.borrow_mut(),y));
        }
    }

    fn native_media_thumbnail(&mut self, item: &Item, request: bool) -> Option<slint::Image> {
        let account=self.base.active_user?;
        let key=thumb_key(item,account)?;
        let (cached,ticket)=ST.with(|s| {
            let mut s=s.borrow_mut();
            if let Some(image)=s.thumb_cache.get(&key, request) { return (Some(image),None); }
            if !request || s.thumb_pending.contains(&key) || s.thumb_pending.len()>=THUMB_PENDING_CAP
                || s.thumb_failed.iter().any(|(k,t)| k==&key && t.elapsed()<THUMB_RETRY) { return (None,None); }
            (None,Some(ThumbTicket { key:key.clone(),epoch:s.thumb_epoch }))
        });
        if let Some(ticket)=ticket {
            if self.router.current()==&crate::router::Route::Media {
                if self.loader.push_native_thumbnail(ticket,self.base.steam_root.clone()) {
                    ST.with(|s| { s.borrow_mut().thumb_pending.insert(key); });
                } else { self.retry_native_thumbnail_admission(); }
            }
        }
        cached
    }

    fn retry_native_thumbnail_admission(&mut self) {
        // Share the worker pool without starving thumbnails after a full art
        // queue. One coalesced UI retry, only while Media remains the route.
        if ST.with(|s| { let mut s=s.borrow_mut(); if s.thumb_retry_scheduled { true } else { s.thumb_retry_scheduled=true; false } }) { return; }
        slint::Timer::single_shot(Duration::from_secs(1),|| super::defer(|a| {
            ST.with(|s| s.borrow_mut().thumb_retry_scheduled=false);
            if a.router.current()==&crate::router::Route::Media { a.preserve_media_async_scroll(); a.push_media(); }
        }));
    }

    pub(crate) fn on_native_thumbnail_ready(&mut self, ticket: ThumbTicket, pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>) {
        // No state reset/re-filter/navigation on completion. Verify identity before
        // creating an image; background completions cannot repaint another route.
        let current=ST.with(|s| thumbnail_result_is_current(&s.borrow(),&ticket,self.base.active_user));
        if !current {
            // Metadata can replace the URL without changing the page epoch.
            // Release the old ticket without caching/repainting stale pixels.
            let released=ST.with(|s| {
                let mut s=s.borrow_mut();
                let had=s.thumb_epoch==ticket.epoch && s.thumb_pending.contains(&ticket.key);
                apply_thumbnail_result(&mut s,&ticket,self.base.active_user,None); had
            });
            if released && self.router.current()==&crate::router::Route::Media { self.retry_native_thumbnail_admission(); }
            return;
        }
        let image=pixels.map(slint::Image::from_rgba8);
        let applied=ST.with(|s| apply_thumbnail_result(&mut s.borrow_mut(),&ticket,self.base.active_user,image));
        if applied && self.router.current()==&crate::router::Route::Media { self.preserve_media_async_scroll(); self.push_media(); }
    }

    /// Discard user-owned media and invalidate in-flight scans/thumbnails on identity change.
    pub(crate) fn media_account_changed(&mut self) {
        MEDIA_SCAN_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.media_play_stop();
        ST.with(|s| {
            let mut s = s.borrow_mut();
            let epoch = s.thumb_epoch.wrapping_add(1);
            *s = St { account_epoch: s.account_epoch.wrapping_add(1), thumb_epoch: epoch, ..St::default() };
        });
        self.loader.set_native_thumbnail_epoch(ST.with(|s| s.borrow().thumb_epoch));
        if self.router.current() == &crate::router::Route::Media {
            self.push_media();
            self.update_hint();
            self.enter_media();
        }
    }

    /// Entering Media scans every app's screenshot and clip folder. That walk runs on a worker so a large
    /// library cannot stall the UI thread; the newest scan's result is applied on the UI thread.
    pub fn enter_media(&mut self) {
        if self.media_batch_busy() { return; }
        self.media_selection_back();
        let root = self.base.steam_root.clone();
        let acct = self.base.active_user.unwrap_or(0);
        let generation = MEDIA_SCAN_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let items = scan(&root, acct);
            crate::screens::defer(move |a| a.media_scan_done(generation, items));
        });
    }

    fn media_scan_done(&mut self, generation: u64, items: Vec<Item>) {
        if generation != MEDIA_SCAN_GEN.load(std::sync::atomic::Ordering::SeqCst) || self.router.current() != &crate::router::Route::Media {
            return;
        }
        if self.media_selecting() || self.media_batch_busy() { return; }
        self.media_play_stop();
        ST.with(|s| {
            let mut s = s.borrow_mut();
            invalidate_thumbs(&mut s);
            s.items = items;
            s.focus = 0;
            s.preferred_col = None;
            s.vy = 0.0;
            s.viewer = None;
            s.confirm_key = None;
            s.batch_note.clear();
            s.note.clear();
        });
        self.loader.set_native_thumbnail_epoch(ST.with(|s| s.borrow().thumb_epoch));
        self.resolve_native_apps();
        self.media_refilter();
        self.push_media();
        self.update_hint();
        self.refresh_native_recording();
    }

    /// Clip entries read from disk carry only their GameID: give each its app (library entry, or a store name lookup).
    fn resolve_native_apps(&mut self) {
        let ids: Vec<(usize, String)> = ST.with(|s| s.borrow().items.iter().enumerate()
            .filter_map(|(i, it)| it.native.as_ref().map(|c| (i, c.game_id.as_str().to_owned()))).collect());
        let mut unnamed = vec![];
        crate::diag::log("media.scan", serde_json::json!({"clips_from_disk": ids.len()}));
        for (i, gid) in ids {
            let app = app_for_clip_game(self.games(), &gid);
            ST.with(|s| { if let Some(it) = s.borrow_mut().items.get_mut(i) { it.app = app; } });
            if app != 0 && !self.gidx.contains_key(&app) { unnamed.push(app); }
        }
        self.request_clip_app_names(unnamed);
    }

    pub fn media_merge_native_clips(&mut self, clips: &[steam_bridge::recording::ClipSummary]) {
        self.preserve_media_async_scroll();
        let additions: Vec<Item> = clips.iter().map(|clip| {
            let app = app_for_clip_game(self.games(), clip.game_id.as_str());
            // Logged once per clip: the merge runs on every observation while Media is open.
            if UNRESOLVED_CLIPS_LOGGED.with(|l| l.borrow_mut().insert(clip.clip_id.clone())) {
                let in_library = self.gidx.contains_key(&app);
                crate::diag::log("media.clip_app", serde_json::json!({"clip": clip.clip_id, "game_id": clip.game_id.as_str(), "app": app, "in_library": in_library, "games": self.games().len()}));
            }
            Item { kind: 1, thumb: None, full: None, native: Some(clip.clone()), app,
                mtime: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(clip.date_recorded as u64) }
        }).collect();
        let unnamed: Vec<u32> = additions.iter().map(|it| it.app).filter(|&a| a != 0 && !self.gidx.contains_key(&a)).collect();
        self.request_clip_app_names(unnamed);
        // The focused entry and the open viewer keep their identity across the re-sort.
        let (focused, viewed) = ST.with(|s| { let s = s.borrow(); (key_at(&s, s.focus), s.viewer.and_then(|v| key_at(&s, v))) });
        ST.with(|s| {
            let mut s=s.borrow_mut();
            // A cache disappearance is not proof of deletion or an empty library.
            for it in additions {
                let id=&it.native.as_ref().unwrap().clip_id;
                if let Some(old)=s.items.iter_mut().find(|old| old.native.as_ref().map(|c| &c.clip_id)==Some(id)) { *old=it; }
                else { s.items.push(it); }
            }
            // Newest first, as screenshots are scanned. A new clip belongs at the top of the grid, not after the screenshots.
            s.items.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        });
        self.media_refilter();
        ST.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(pos) = focused.as_deref().and_then(|k| view_index_of(&s, k)) { s.focus = pos; s.preferred_col = None; }
            if let Some(v) = viewed.as_deref().and_then(|k| view_index_of(&s, k)) { if s.viewer.is_some() { s.viewer = Some(v); } }
        });
        self.push_media();
    }

    fn media_refilter(&mut self) {
        ST.with(|s| refilter_state(&mut s.borrow_mut()));
    }

    fn media_game_name(&self, app: u32) -> String {
        if let Some(&gi) = self.gidx.get(&app) {
            return media_name(Some(self.games()[gi].name.as_str()), app);
        }
        let named = ST.with(|s| s.borrow().clip_names.get(&app).cloned());
        media_name(named.as_deref(), app)
    }

    /// Store names for clip games missing from the library list: one read-only lookup per appid.
    fn request_clip_app_names(&mut self, appids: Vec<u32>) {
        let Some(bridge) = crate::actions::bridge() else { return };
        for appid in appids {
            if !ST.with(|s| s.borrow_mut().clip_name_asked.insert(appid)) { continue; }
            let bridge = bridge.clone();
            let spawned = std::thread::Builder::new().name("clip-app-name".into()).spawn(move || {
                let name = bridge.app_display_name(appid).ok().flatten();
                crate::screens::defer(move |a| a.clip_app_named(appid, name));
            });
            if spawned.is_err() { ST.with(|s| { s.borrow_mut().clip_name_asked.remove(&appid); }); }
        }
    }

    fn clip_app_named(&mut self, appid: u32, name: Option<String>) {
        crate::diag::log("media.clip_name", serde_json::json!({"app": appid, "found": name.is_some()}));
        let Some(name) = name else { return };
        ST.with(|s| { s.borrow_mut().clip_names.insert(appid, name); });
        self.push_media();
    }

    pub fn push_media(&mut self) {
        let g: Value = serde_json::from_str(PAGE).unwrap_or_default();
        let geo = &g["geo"];
        let f = |k: &str| geo[k].as_f64().unwrap_or(0.0) as f32;
        let (outer_pad, inner_pad) = (g["outer"].as_str().unwrap_or("touch,mouse"), g["inner"].as_str().unwrap_or("A,START,FOCUS,touch,mouse,long_press"));
        let cols = f("cols").max(1.0) as usize;
        let (items, view, focus, vy, viewer, vfocus, confirm, note, tf, game) = ST.with(|s| {
            let s = s.borrow();
            (s.items.clone(), s.view.clone(), s.focus, s.vy, s.viewer, s.vfocus, s.confirm_key.clone(), s.note.clone(), s.type_filter, s.game)
        });
        let start = window_start_of(focus, cols);
        let (selection, selection_status) = ST.with(|s| {
            let s = s.borrow();
            (s.selection.clone(), media_batch::selection_status(&s))
        });
        let window=self.ui();
        let screen_css_height=window.window().size().height as f32 / window.window().scale_factor().max(0.1) / SC;
        let mut rows: Vec<GPageRow> = vec![];
        for (n, &ii) in view.iter().enumerate().skip(start).take(WINDOW) {
            let it = &items[ii];
            let (c, r) = ((n % cols) as f32, (n / cols) as f32);
            let (x, y) = (f("x0") + c * f("px"), f("y0") + r * f("py"));
            let name = self.media_game_name(it.app);
            let label = media_caption(&name, it.kind, it.mtime);
            let img = self.native_media_thumbnail(it, thumbnail_near_view(y,f("h"),vy,screen_css_height));
            let base = PRow { role: "button".into(), x, y, w: f("w"), h: f("h"), lab: label.clone(), ..Default::default() };
            let mut o = crate::screens::settings::to_grow(&PRow { k: 0, pad: outer_pad.into(), ..base.clone() }, 0);
            o.label = label.as_str().into();
            rows.push(o);
            let mut nn = crate::screens::settings::to_grow(&PRow { k: 8, pad: inner_pad.into(), w: f("iw"), h: f("ih"), ..base }, 0);
            nn.label = label.as_str().into();
            if let Some(selection) = &selection {
                nn.on = i32::from(selection.contains(&item_key(it)));
                nn.label = format!("{label} · {}", if nn.on == 1 { i18n::tr_ref("media.selected") } else { i18n::tr_ref("media.no_mark") }).into();
            }
            nn.has_img = img.is_some();
            nn.img = img.unwrap_or_default();
            rows.push(nn);
        }
        let rowsn = (view.len() + cols - 1) / cols;
        let ui = self.ui();
        let st = ui.global::<MediaState>();
        st.set_selecting(selection.is_some());
        st.set_cards(ModelRc::new(VecModel::from(rows)));
        st.set_focus(if view.is_empty() { -1 } else { ((focus - start) * 2 + 1) as i32 });
        st.set_content_h(((f("y0") + rowsn as f32 * f("py") + 40.0 - 40.0) * SC).max(588.0 * SC).into());
        st.set_vy((-vy * SC).into());
        st.set_wrap_pad(g["wrap"].as_str().unwrap_or("B,X,Y,FOCUS,keyboard").into());
        // empty state: NoContentAvailable vs AllContentFilteredOut
        let filtered = tf != 0 || game.is_some();
        st.set_empty_text(if filtered && !items.is_empty() { i18n::tr_ref("media.all_contents_are_hidden_by_filters").to_string() } else { i18n::tr_ref("media.no_screenshots_or_famous_clips_steam_cache").to_string() }.into());
        let mut ft = vec![];
        if let Some(g) = game {
            ft.push(self.media_game_name(g));
        }
        if tf != 0 {
            ft.push(["", i18n::tr_ref("media.screenshots"), i18n::tr_ref("media.clips"), i18n::tr_ref("media.records")][tf as usize].to_string());
        }
        if !selection_status.is_empty() { ft.push(selection_status); }
        st.set_filter_text(ft.join("  -  ").into());
        self.push_native_recording();
        // viewer
        st.set_viewer_open(viewer.is_some());
        let (tone, phase) = ST.with(|s| { let s = s.borrow(); (s.phone_tone, s.phone.clone()) });
        let panel = viewer.is_some() && tone != PhoneTone::Quiet;
        st.set_phone_tone(if panel { tone.code() } else { 0 });
        st.set_phone_stage(phone_stage(&phase));
        st.set_phone_text(if panel { note.as_str().into() } else { "".into() });
        if let Some(v) = viewer {
            if let Some(it) = view.get(v).and_then(|&i| items.get(i)) {
                let clip = media_player::native_clip_id(it).map(str::to_owned);
                let status = clip.as_deref().and_then(media_player::status_for);
                // While a clip has playback state (running, paused or ended), its last frame stays on screen: a grid refresh
                // must not put the thumbnail back.
                if status.is_none() {
                    let img = if it.native.is_some() {
                        // Reuse the bounded native thumbnail loader for the still image.
                        self.native_media_thumbnail(it, true)
                    } else {
                        it.full.as_ref().or(it.thumb.as_ref()).and_then(|p| slint::Image::load_from_path(p).ok())
                    };
                    st.set_viewer_has(img.is_some());
                    st.set_viewer_img(img.unwrap_or_default());
                }
                st.set_viewer_title(self.media_game_name(it.app).as_str().into());
                let sub = match status {
                    Some((_, _, pos, duration)) => self.media_clip_sub(it, pos, duration),
                    None => format!("{}  -  {}", kind_label(it.kind), ago(it.mtime)),
                };
                st.set_viewer_sub(sub.as_str().into());
                let armed = confirm.as_deref() == Some(item_key(it).as_str());
                let playable = clip.as_deref().is_some_and(|cid| self.clip_playable(cid));
                let acts: Vec<String> = viewer_actions(it, playable).into_iter().map(|a| match a {
                    ViewerAct::Play => match status {
                        Some((true, _, _, _)) => i18n::tr_ref("media.pause"),
                        Some((false, true, _, _)) => i18n::tr_ref("media.first_of_all"),
                        Some((false, false, pos, _)) if pos > 0 => i18n::tr_ref("media.continue"),
                        _ => i18n::tr_ref("media.play"),
                    }.to_owned(),
                    ViewerAct::Share => t_or("#MediaPage_ScreenshotContext_Share", i18n::tr_ref("media.share")),
                    ViewerAct::Upload => t_or("#MediaPage_ScreenshotContext_Upload", i18n::tr_ref("media.upload")),
                    ViewerAct::PhoneSend => phone_label(it),
                    ViewerAct::Delete => delete_label(armed),
                }).collect();
                st.set_viewer_actions(ModelRc::new(VecModel::from(acts.iter().map(|a| slint::SharedString::from(a.as_str())).collect::<Vec<_>>())));
                st.set_viewer_focus(vfocus.min(acts.len().saturating_sub(1)) as i32);
                // A message from playback or the viewer wins over the clip's summary line.
                let native_note = it.native.as_ref().map(|c| {
                    let how = match clip.as_deref() {
                        Some(cid) if self.clip_playable(cid) && media_player::is_playing(cid) => i18n::tr_ref("media.a_pause_10_s"),
                        Some(cid) if self.clip_playable(cid) => i18n::tr_ref("media.a_reproduction"),
                        _ => i18n::tr_ref("media.no_recordings_of_the_clip_have_been"),
                    };
                    i18n::tr_args("media.steam_ms_bytes", &[("duration_ms", &(c.duration_ms).to_string()), ("file_size", &(c.file_size).to_string()), ("temporary", &(if c.temporary { i18n::tr_ref("media.temporary") } else { i18n::tr_ref("media.permanent") }).to_string()), ("how", &(how).to_string())])
                });
                // While the phone panel shows, its text replaces the note line (no duplicate).
                let line = if panel { String::new() } else if note.is_empty() { native_note.unwrap_or_default() } else { note.clone() };
                st.set_viewer_note(line.as_str().into());
            }
        }
    }

    fn media_open_viewer(&mut self) {
        let ok = ST.with(|s| {
            let mut s = s.borrow_mut();
            if s.view.is_empty() || s.selection.is_some() || s.batch.is_some() {
                return false;
            }
            s.viewer = Some(s.focus);
            s.vfocus = 0;
            s.confirm_key = None;
            s.note.clear();
            clear_phone_panel(&mut s);
            true
        });
        if ok {
            eprintln!("[deckshell] media viewer open");
            self.push_media();
            self.update_hint();
        }
    }

    /// B while the viewer / a confirmation is up: close it (returns true when it consumed the press).
    pub fn media_back(&mut self) -> bool {
        if self.media_selection_back() { return true; }
        let used = ST.with(|s| {
            let mut s = s.borrow_mut();
            if s.confirm_key.is_some() {
                s.confirm_key = None;
                s.note.clear();
                true
            } else if s.viewer.is_some() {
                s.viewer = None;
                clear_phone_panel(&mut s);
                true
            } else {
                false
            }
        });
        // Closing the viewer ends any clip playback.
        if ST.with(|s| s.borrow().viewer.is_none()) { self.media_play_stop(); }
        if used {
            self.push_media();
            self.update_hint();
        }
        used
    }

    pub fn media_viewer_open(&self) -> bool {
        ST.with(|s| s.borrow().viewer.is_some())
    }

    fn media_current(&self) -> Option<Item> {
        ST.with(|s| {
            let s = s.borrow();
            let v = s.viewer.unwrap_or(s.focus);
            s.view.get(v).and_then(|&i| s.items.get(i)).cloned()
        })
    }

    /// The viewer's action row for the shown item (A runs the focused one).
    fn viewer_actions_now(&self) -> Vec<ViewerAct> {
        let Some(it) = self.media_current() else { return vec![] };
        let playable = media_player::native_clip_id(&it).is_some_and(|cid| self.clip_playable(cid));
        viewer_actions(&it, playable)
    }

    fn media_viewer_activate(&mut self) {
        let vf = ST.with(|s| s.borrow().vfocus);
        match self.viewer_actions_now().get(vf).copied() {
            Some(ViewerAct::Play) => self.media_clip_toggle(),
            Some(ViewerAct::Share) => self.media_op(MediaOp::Share),
            Some(ViewerAct::Upload) => self.media_op(MediaOp::Upload),
            Some(ViewerAct::PhoneSend) => self.media_op(MediaOp::PhoneSend),
            Some(ViewerAct::Delete) => self.media_op(MediaOp::Delete),
            None => {}
        }
    }

    pub fn media_op(&mut self, op: MediaOp) {
        match op {
            MediaOp::NativeStart => self.native_recording_action(true),
            MediaOp::NativeStop => self.native_recording_action(false),
            MediaOp::NativeRefresh => self.refresh_native_recording(),
            MediaOp::NativeLoadClips => self.load_native_clips(),
            MediaOp::NativeAcknowledge => self.native_recording_acknowledge_unknown(),
            MediaOp::Type(t) => {
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    invalidate_thumbs(&mut s);
                    s.type_filter = t;
                    s.focus = 0;
                    s.preferred_col = None;
                    s.vy = 0.0;
                });
                self.loader.set_native_thumbnail_epoch(ST.with(|s| s.borrow().thumb_epoch));
                self.media_refilter();
                self.push_media();
            }
            MediaOp::Game(g) => {
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    invalidate_thumbs(&mut s);
                    s.game = g;
                    s.focus = 0;
                    s.preferred_col = None;
                    s.vy = 0.0;
                });
                self.loader.set_native_thumbnail_epoch(ST.with(|s| s.borrow().thumb_epoch));
                self.media_refilter();
                self.push_media();
            }
            MediaOp::Open => self.media_open_viewer(),
            MediaOp::Share | MediaOp::Upload => {
                let Some(it) = self.media_current() else { return };
                if it.native.is_some() { return; }
                let Some(full) = &it.full else { return };
                let fname = full.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                // SteamClient.Screenshots (docs/api/steamclient.md): resolve Steam's handle for this file, then upload; share = upload + activity-feed dialog
                let js = format!(
                    "(async()=>{{const n={};const all=await SteamClient.Screenshots.GetAllLocalScreenshots({});const e=(all||[]).find(x=>JSON.stringify(x).includes(n));if(!e)return 'not found';return await SteamClient.Screenshots.UploadLocalScreenshot({},e.hHandle||e.hScreenshot||e.handle)}})()",
                    crate::setters::js_str(&fname),
                    it.app,
                    it.app
                );
                crate::setters::write(if op == MediaOp::Share { "share-screenshot" } else { "upload-screenshot" }, js);
                ST.with(|s| s.borrow_mut().note = t_or("#MediaPage_Upload_Notification_Success", i18n::tr_ref("media.sent_to_steam")));
                self.push_media();
            }
            MediaOp::Delete => self.media_delete_press(),
            MediaOp::PhoneSend => self.media_phone_send(),
            MediaOp::SelectMany => self.media_select_many(),
            MediaOp::BatchSend => self.media_batch_send(),
            MediaOp::BatchDelete => self.media_batch_confirm_delete(),
        }
    }

    /// Delete on the shown item (viewer button, Y menu). The first press arms the confirmation for that item; a
    /// delete on the same armed item really deletes. Nothing runs while another clip's Steam delete is in flight.
    pub(crate) fn media_delete_press(&mut self) {
        let Some(it) = self.media_current() else { return };
        let key = item_key(&it);
        let (armed, busy) = ST.with(|s| { let s = s.borrow(); (s.confirm_key.clone(), s.deleting.is_some() || s.phone_busy || s.batch.is_some()) });
        if busy { return; }
        match delete_step(armed.as_deref(), &key) {
            DeleteStep::Arm => {
                ST.with(|s| {
                    let mut s = s.borrow_mut();
                    s.confirm_key = Some(key);
                    clear_phone_panel(&mut s);
                    s.note = t_or("#MediaPage_Delete_Confirm_Prompt", i18n::tr_ref("media.delete_a_b"));
                });
                self.push_media();
            }
            DeleteStep::Execute => {
                ST.with(|s| s.borrow_mut().confirm_key = None);
                self.media_delete_item(it);
            }
        }
    }

    /// Runs a confirmed delete: a screenshot's files now; a clip through Steam's own delete.
    fn media_delete_item(&mut self, it: Item) {
        if let Some(cid) = native_clip_id_of(&it) {
            return self.media_clip_delete_start(cid);
        }
        if crate::actions::dryrun() {
            eprintln!("[deckshell] DRYRUN delete media {:?}", it.full);
            return self.media_delete_failed(i18n::tr_ref("media.dryrun_file_not_deleted"));
        }
        let Some(full) = it.full.as_deref() else { return self.media_delete_failed(i18n::tr_ref("media.file_not_found_nothing_deleted")) };
        match remove_screenshot_files(full, it.thumb.as_deref()) {
            Ok(()) => {
                eprintln!("[deckshell] media delete {:?}: ok", it.full);
                self.media_item_removed(&item_key(&it));
                self.media_delete_succeeded();
            }
            Err(e) => {
                eprintln!("[deckshell] media delete {:?}: {e}", it.full);
                self.media_delete_failed(&i18n::tr_args("media.could_not_delete", &[("error", &(e).to_string())]));
            }
        }
    }

    fn clips_root(&self) -> Option<PathBuf> {
        let acct = self.base.active_user?;
        Some(self.base.steam_root.join("userdata").join(acct.to_string()).join("gamerecordings").join("clips"))
    }

    /// Deletes a clip the way Steam deletes its own: Steam's store call (`GameRecordingStore.DeleteClip`, which also drops
    /// the clip from Steam's cache), then the clip folder is checked on disk. DeckShell removes nothing itself.
    fn media_clip_delete_start(&mut self, clip_id: String) {
        if ST.with(|s| s.borrow().deleting.is_some()) { return; }
        let Some(clips) = self.clips_root() else { return self.media_delete_failed(i18n::tr_ref("media.steam_profile_not_found_clip_not_deleted")) };
        let Some(dir) = media_clips::clip_dir_for_delete(&clips, &clip_id) else {
            return self.media_delete_failed(i18n::tr_ref("media.the_path_of_the_clip_has_not"));
        };
        if crate::actions::dryrun() {
            eprintln!("[deckshell] DRYRUN delete clip {clip_id}");
            return self.media_delete_failed(i18n::tr_ref("media.dryrun_no_video_deleted"));
        }
        let Some(bridge) = crate::actions::bridge() else { return self.media_delete_failed(i18n::tr_ref("media.steam_is_unavailable_the_clip_is_not")) };
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.deleting = Some(clip_id.clone());
            clear_phone_panel(&mut s);
            s.note = i18n::tr_ref("media.removal_of_the_clip_via_steam").to_owned();
        });
        self.push_media();
        let id = clip_id.clone();
        let account_epoch = ST.with(|s| s.borrow().account_epoch);
        let spawned = std::thread::Builder::new().name("clip-delete".into()).spawn(move || {
            let reply = bridge.delete_native_clip(&id).map_err(|e| e.to_string());
            // Steam removes the folder itself. A reply that may still be in progress gets a few seconds to show it.
            let polls = if matches!(reply, Ok(steam_bridge::recording::DeleteReply::Reported) | Err(_)) { 20 } else { 1 };
            let mut gone = !dir.exists();
            for _ in 1..polls {
                if gone { break; }
                std::thread::sleep(Duration::from_millis(150));
                gone = !dir.exists();
            }
            crate::screens::defer(move |a| {
                if ST.with(|s| s.borrow().account_epoch == account_epoch) {
                    a.media_clip_deleted(id, reply, gone);
                }
            });
        });
        if spawned.is_err() {
            ST.with(|s| s.borrow_mut().deleting = None);
            self.media_delete_failed(i18n::tr_ref("media.failed_to_begin_deleting_the_clip_is"));
        }
    }

    /// Result of the Steam delete, judged by the clip folder: gone means deleted, whatever Steam's reply said.
    fn media_clip_deleted(&mut self, clip_id: String, reply: Result<steam_bridge::recording::DeleteReply, String>, gone: bool) {
        use steam_bridge::recording::DeleteReply;
        ST.with(|s| {
            let mut s = s.borrow_mut();
            if s.deleting.as_deref() == Some(clip_id.as_str()) { s.deleting = None; }
        });
        crate::diag::log("media.clip_delete", serde_json::json!({"gone": gone, "reply": format!("{reply:?}")}));
        if gone {
            self.media_item_removed(&format!("clip:{clip_id}"));
            self.media_delete_succeeded();
            return;
        }
        let msg = match reply {
            Ok(DeleteReply::NotCached) => i18n::tr_ref("media.the_clip_isn_t_downloaded_from_the").to_owned(),
            Ok(DeleteReply::Refused(code)) => i18n::tr_args("media.steam_did_not_delete_the_clip_code", &[("code", &(code).to_string())]),
            Ok(DeleteReply::Unavailable) => i18n::tr_ref("media.steam_has_not_accepted_the_deletion_the").to_owned(),
            Ok(DeleteReply::Reported) => i18n::tr_ref("media.steam_has_reported_the_deletion_but_the").to_owned(),
            Err(e) => i18n::tr_args("media.could_not_reach_steam_the_clip_folder", &[("error", &(e).to_string())]),
        };
        self.media_delete_failed(&msg);
    }

    /// «Отправить на телефон». Step 1 checks the phone (a query, off the UI thread). Step 2 plans the send, then runs the
    /// live send (export for a clip, file pick for a screenshot, then upload) on its own thread. A poller shows «Экспорт...»
    /// and «Загрузка…». One send at a time, and an item that was sent (or whose result is unknown) is never sent again here.
    fn media_phone_send(&mut self) {
        if ST.with(|s| { let s = s.borrow(); s.phone_busy || s.deleting.is_some() || s.batch.is_some() }) { return; }
        let Some(it) = self.media_current() else { return };
        self.media_phone_send_item(it);
    }

    fn media_phone_send_item(&mut self, it: Item) {
        let Some((kind, duration_ms, clip_id)) = phone_target(&it) else {
            self.media_phone_done(i18n::tr_ref("media.the_type_of_media_is_not_supported").into(), PhoneTone::Info);
            return;
        };
        if let Some(block) = send_to_phone::menu_block(kind, duration_ms, AccountFlags::default()) {
            let msg = block.message(kind).and_then(|(token, _)| phone_block_label(token))
                .unwrap_or_else(|| i18n::tr_ref("media.sending_to_the_phone_for_this_video").to_owned());
            if self.media_batch_busy() {
                self.media_phone_done(msg, PhoneTone::Info);
            } else {
                self.media_phone_line(PhoneTone::Failed, msg);
            }
            return;
        }
        let key = item_key(&it);
        if ST.with(|s| s.borrow().phone_sent.contains(&key)) {
            return self.media_phone_done(i18n::tr_ref("media.the_item_has_already_been_sent_or").into(), PhoneTone::Info);
        }
        let shot_path = phone_shot_path(&it);
        if kind == MediaKind::Screenshot {
            match shot_path.as_deref().map(send_to_phone::check_shot_path) {
                Some(Ok(())) => {}
                _ => return self.media_phone_done(i18n::tr_ref("media.screenshot_file_not_found_nothing_sent").to_owned(), PhoneTone::Failed),
            }
        }
        let Some(bridge) = crate::actions::bridge() else {
            return self.media_phone_done(i18n::tr_ref("media.steam_is_unavailable_nothing_has_been_sent").to_owned(), PhoneTone::Failed);
        };
        let req = SendRequest { kind, clip_id, app: it.app, duration_ms, shot_path, live: !crate::actions::dryrun() };
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.phone_busy = true;
            s.phone = SendPhase::Checking;
        });
        // Same frame as the press: the panel shows before the check query returns.
        self.media_phone_line(PhoneTone::Busy, i18n::tr_ref("media.checking_your_mobile_device").to_owned());
        // One id per send: the progress poll and the send's own state read only this send's writes.
        let run_id = send_to_phone::new_run_id();
        let account_epoch = ST.with(|s| s.borrow().account_epoch);
        let spawned = std::thread::Builder::new().name("phone-check".into()).spawn(move || {
            let reply = bridge.phone_check(kind).map_err(|e| e.to_string());
            crate::screens::defer(move |a| {
                if ST.with(|s| s.borrow().account_epoch == account_epoch) {
                    a.media_phone_checked(req, key, run_id, bridge, reply);
                }
            });
        });
        if spawned.is_err() {
            ST.with(|s| s.borrow_mut().phone_busy = false);
            self.media_phone_done(i18n::tr_ref("media.failed_to_start_the_check_nothing_sent").to_owned(), PhoneTone::Failed);
        }
    }

    fn media_phone_checked(&mut self, req: SendRequest, key: String, run_id: String, bridge: steam_bridge::Bridge, reply: Result<steam_bridge::send_to_phone::PhoneReply, String>) {
        let phone = match reply {
            Ok(r) => r,
            Err(e) => return self.media_phone_done(i18n::tr_args("media.could_not_reach_steam_nothing_was_sent", &[("error", &(e).to_string())]), PhoneTone::Failed),
        };
        if let Some(msg) = phone_reply_label(phone, req.kind) {
            return self.media_phone_done(msg, PhoneTone::Failed);
        }
        if let Err(e) = send_to_phone::plan(req.kind, req.duration_ms, AccountFlags::default(), phone) {
            return self.media_phone_done(i18n::tr_args("media.could_not_start_sending", &[("error", &(e).to_string())]), PhoneTone::Failed);
        }
        self.media_phone_line(PhoneTone::Busy, i18n::tr_ref("media.phone_found_preparation").to_owned());
        let done = Arc::new(AtomicBool::new(false));
        let poll_done = done.clone();
        let poll_run = run_id.clone();
        let account_epoch = ST.with(|s| s.borrow().account_epoch);
        // Progress poller: its own connection, so it never waits behind the send's evaluation.
        let _ = std::thread::Builder::new().name("phone-progress".into()).spawn(move || {
            let poller = steam_bridge::Bridge::lazy();
            let expr = steam_bridge::send_to_phone::progress_expr(&poll_run);
            while !poll_done.load(AtomicOrdering::Relaxed) {
                if let Ok(v) = poller.eval_json(&expr) {
                    if let Some(phase) = steam_bridge::send_to_phone::parse_progress(&v) {
                        let done = poll_done.clone();
                        crate::screens::defer(move |a| {
                            if !done.load(AtomicOrdering::Relaxed) && ST.with(|s| s.borrow().account_epoch == account_epoch) {
                                a.media_phone_progress(phase);
                            }
                        });
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        let stop_if_not_started = done.clone();
        let spawned = std::thread::Builder::new().name("phone-send".into()).spawn(move || {
            let reply = bridge.phone_send(&req, &run_id).map_err(|e| e.to_string());
            done.store(true, AtomicOrdering::Relaxed);
            crate::screens::defer(move |a| {
                if ST.with(|s| s.borrow().account_epoch == account_epoch) {
                    a.media_phone_result(req, key, reply);
                }
            });
        });
        if spawned.is_err() {
            stop_if_not_started.store(true, AtomicOrdering::Relaxed);
            self.media_phone_done(i18n::tr_ref("media.failed_to_start_shipping_nothing_sent").to_owned(), PhoneTone::Failed);
        }
    }

    /// Shows «Экспорт...» / «Загрузка…» while the send runs. Only changes are shown.
    fn media_phone_progress(&mut self, phase: SendPhase) {
        let changed = ST.with(|s| {
            let mut s = s.borrow_mut();
            if !s.phone_busy || s.phone == phase { return false; }
            s.phone = phase.clone();
            true
        });
        if changed {
            if let Some(label) = phone_phase_label(&phase) { self.media_phone_line(PhoneTone::Busy, label.to_owned()); }
        }
    }

    fn media_phone_result(&mut self, req: SendRequest, key: String, reply: Result<SendReply, String>) {
        let unknown = matches!(&reply, Err(_) | Ok(SendReply::Unknown(_)));
        let (msg, tone) = match reply {
            // Only a send whose result could not be read from Steam gets here (the bridge settles a lost reply first).
            // The upload may have reached the phone: do not try again in this session.
            Err(e) => {
                ST.with(|s| { let mut s = s.borrow_mut(); s.phone_sent.insert(key); s.phone = SendPhase::Failed(e.clone()); });
                (i18n::tr_args("media.connection_to_steam_was_lost_while_sending", &[("error", &(e).to_string())]), PhoneTone::Failed)
            }
            Ok(SendReply::Unknown(m)) => {
                ST.with(|s| { let mut s = s.borrow_mut(); s.phone_sent.insert(key); s.phone = SendPhase::Failed(m.clone()); });
                (i18n::tr_args("media.sending_was_interrupted_check_the_steam_mobile", &[("message", &(m).to_string())]), PhoneTone::Failed)
            }
            Ok(SendReply::Refused(why)) => (phone_refusal_label(&why, req.kind), PhoneTone::Failed),
            Ok(SendReply::Dry { .. }) => (i18n::tr_ref("media.the_phone_is_found_and_ready_the").to_owned(), PhoneTone::Info),
            Ok(SendReply::Failed(m)) => {
                ST.with(|s| s.borrow_mut().phone = SendPhase::Failed(m.clone()));
                (i18n::tr_args("media.sending_failed_nothing_was_sent", &[("message", &(m).to_string())]), PhoneTone::Failed)
            }
            Ok(SendReply::Sent) => {
                ST.with(|s| { let mut s = s.borrow_mut(); s.phone_sent.insert(key); s.phone = SendPhase::Sent; });
                if !self.media_batch_busy() {
                    self.toast(&t_or("#SendToPhone_Finished", i18n::tr_ref("media.item_sent")));
                }
                let text = match req.kind {
                    MediaKind::Screenshot => i18n::tr_ref("media.the_screenshot_will_come_as_a_notification").to_owned(),
                    MediaKind::Clip => i18n::tr_ref("media.sent_the_video_will_come_as_a").to_owned(),
                };
                (text, PhoneTone::Sent)
            }
        };
        if unknown && self.media_batch_busy() {
            ST.with(|s| s.borrow_mut().phone_busy = false);
            self.media_batch_outcome(Outcome::Unknown(msg));
            return;
        }
        self.media_phone_done(msg, tone);
    }

    /// Ends a phone send with its outcome in the panel (not busy any more).
    fn media_phone_done(&mut self, msg: String, tone: PhoneTone) {
        ST.with(|s| s.borrow_mut().phone_busy = false);
        if self.media_batch_busy() {
            let outcome = match tone {
                PhoneTone::Sent => Outcome::Done,
                PhoneTone::Info => Outcome::Skipped(msg),
                _ => Outcome::Failed(msg),
            };
            self.media_batch_outcome(outcome);
            return;
        }
        self.media_phone_line(tone, msg);
    }

    /// Shows a phone send state in the viewer's panel and the note text behind it.
    fn media_phone_line(&mut self, tone: PhoneTone, msg: String) {
        ST.with(|s| { let mut s = s.borrow_mut(); s.phone_tone = tone; s.note = msg; });
        self.push_media();
    }

    fn media_delete_succeeded(&mut self) {
        if self.media_batch_busy() {
            self.media_batch_outcome(Outcome::Done);
        } else {
            self.toast(&t_or("#MediaPage_Delete_Notification_Success", i18n::tr_ref("media.deleted")));
        }
    }

    /// Shows a delete failure (or refusal) and leaves the item in the list.
    fn media_delete_failed(&mut self, msg: &str) {
        if self.media_batch_busy() {
            self.media_batch_outcome(Outcome::Failed(msg.to_owned()));
            return;
        }
        ST.with(|s| {
            let mut s = s.borrow_mut();
            s.confirm_key = None;
            s.note = msg.to_owned();
        });
        self.push_media();
        self.toast(msg);
    }

    /// Removes a deleted item from the list. Focus moves to the item that followed it (the same grid position).
    fn media_item_removed(&mut self, key: &str) {
        ST.with(|s| {
            let mut s = s.borrow_mut();
            let focused = if s.batch.is_some() { key_at(&s, s.focus) } else { None };
            remove_item_in_state(&mut s, key);
            refilter_state(&mut s);
            if let Some(pos) = focused.as_deref().and_then(|key| view_index_of(&s, key)) {
                s.focus = pos;
            }
        });
        self.push_media();
        self.update_hint();
    }

    /// X: game picker, START: media-type filter; Y retains the shell item options.
    pub fn media_menu(&mut self, which: char) {
        if self.media_batch_busy() { return; }
        if self.media_selecting() {
            if which == 's' { self.media_batch_menu(); }
            return;
        }
        use super::library::{MenuAct, MenuKind, MItem};
        let it = |label: String, act: MenuAct| MItem { label, kind: 0, sep: false, arrow: false, act };
        let (items, title, cur) = match which {
            'x' => {
                let mut apps: Vec<u32> = ST.with(|s| s.borrow().items.iter().map(|i| i.app).collect());
                apps.sort();
                apps.dedup();
                apps.sort_by_key(|a| self.media_game_name(*a).to_lowercase());
                let game = ST.with(|s| s.borrow().game);
                let mut v = vec![it(t_or("#MediaPage_Filter_None", i18n::tr_ref("media.all_games")), MenuAct::Media(MediaOp::Game(None)))];
                v.extend(apps.iter().map(|&a| it(self.media_game_name(a), MenuAct::Media(MediaOp::Game(Some(a))))));
                let cur = game.and_then(|g| apps.iter().position(|&a| a == g)).map(|p| p + 1).unwrap_or(0);
                (v, t_or("#MediaPage_Filter", i18n::tr_ref("media.select_game")), cur)
            }
            'y' => {
                let tf = ST.with(|s| s.borrow().type_filter);
                let opts = type_filter_options();
                let v = opts.iter().map(|&(i, l)| it(t_or(l.0, l.1), MenuAct::Media(MediaOp::Type(i)))).collect();
                (v, t_or("#MediaPage_Filter", i18n::tr_ref("media.filter")), type_filter_index(&opts, tf))
            }
            _ => {
                let kind = self.media_current().map(|i| i.kind).unwrap_or(0);
                let mut v = vec![it(t_or("#Button_Open", i18n::tr_ref("media.open")), MenuAct::Media(MediaOp::Open))];
                if !self.media_viewer_open() && self.media_current().is_some() {
                    v.push(it(t_or("#MediaManager_EnterSelectMode", i18n::tr_ref("media.multi_select")), MenuAct::Media(MediaOp::SelectMany)));
                }
                if kind == 0 {
                    v.push(it(t_or("#MediaPage_ScreenshotContext_Share", i18n::tr_ref("media.share")), MenuAct::Media(MediaOp::Share)));
                    v.push(it(t_or("#MediaPage_ScreenshotContext_Upload", i18n::tr_ref("media.upload")), MenuAct::Media(MediaOp::Upload)));
                }
                if let Some(cur) = self.media_current() {
                    if phone_target(&cur).is_some() {
                        v.push(it(phone_label(&cur), MenuAct::Media(MediaOp::PhoneSend)));
                    }
                }
                // Delete works for screenshots and clips alike. Once armed for this item, the entry is the confirmation and
                // the menu opens on it, so the prompt is visible where the press was made.
                let armed = self.armed_for_current();
                let delete_at = v.len();
                if self.media_current().is_some() {
                    v.push(it(delete_label(armed), MenuAct::Media(MediaOp::Delete)));
                }
                let (start, stop) = self.native_recording_controls();
                if start { v.push(it(i18n::tr_ref("media.steam_start_recording").into(), MenuAct::Media(MediaOp::NativeStart))); }
                if stop { v.push(it(i18n::tr_ref("media.steam_stop_recording").into(), MenuAct::Media(MediaOp::NativeStop))); }
                v.push(it(i18n::tr_ref("media.read_the_state_of_steam").into(), MenuAct::Media(MediaOp::NativeRefresh)));
                v.push(it(i18n::tr_ref("media.request_the_cache_of_steam_clips_once").into(), MenuAct::Media(MediaOp::NativeLoadClips)));
                if self.native_recording_can_acknowledge() {
                    v.push(it(i18n::tr_ref("media.accept_an_unknown_outcome_without_repeating_commands").into(), MenuAct::Media(MediaOp::NativeAcknowledge)));
                }
                v.push(it(t_or("#Button_Cancel", i18n::tr_ref("media.cancel")), MenuAct::Cancel));
                if armed {
                    (v, t_or("#MediaPage_Delete_Confirm_Prompt", i18n::tr_ref("media.delete_a_b")), delete_at)
                } else {
                    (v, String::new(), 0)
                }
            }
        };
        self.open_custom_menu(MenuKind::Media, title, items, cur);
    }

    fn armed_for_current(&self) -> bool {
        let Some(it) = self.media_current() else { return false };
        ST.with(|s| s.borrow().confirm_key.as_deref() == Some(item_key(&it).as_str()))
    }

    /// The item menu's Delete entry. Called after the menu closed: a first press arms and reopens the menu on the
    /// confirmation; the next press on the armed item deletes.
    pub(crate) fn media_menu_delete(&mut self) {
        self.media_delete_press();
        if self.armed_for_current() {
            self.media_menu('s');
        }
    }

    pub fn handle_media(&mut self, a: Action) {
        if self.media_viewer_open() {
            // Left/Right seek a playable native clip; otherwise they move between the viewer's actions as before.
            if matches!(a, Action::Left | Action::Right) && self.media_clip_key(a) { return; }
            // L1/R1 and Up/Down change the shown item: any playback of the previous clip ends first.
            if matches!(a, Action::LB | Action::RB | Action::Up | Action::Down) { self.media_play_stop(); }
            // Y opens the item menu here too (the footer's "Действия"), with the same Delete as in the grid.
            if a == Action::Y { return self.media_menu('s'); }
            let last = self.viewer_actions_now().len().saturating_sub(1);
            match a {
                Action::Left => ST.with(|s| {
                    let mut s = s.borrow_mut();
                    s.vfocus = s.vfocus.saturating_sub(1);
                }),
                Action::Right => ST.with(|s| {
                    let mut s = s.borrow_mut();
                    s.vfocus = (s.vfocus + 1).min(last);
                }),
                // L1/R1: previous / next item inside the viewer
                Action::LB | Action::Up => ST.with(|s| {
                    let mut s = s.borrow_mut();
                    if let Some(v) = s.viewer {
                        if v > 0 {
                            s.viewer = Some(v - 1);
                            s.focus = v - 1;
                            s.preferred_col = None;
                            s.confirm_key = None;
                            s.note.clear();
                        }
                    }
                }),
                Action::RB | Action::Down => ST.with(|s| {
                    let mut s = s.borrow_mut();
                    if let Some(v) = s.viewer {
                        if v + 1 < s.view.len() {
                            s.viewer = Some(v + 1);
                            s.focus = v + 1;
                            s.preferred_col = None;
                            s.confirm_key = None;
                            s.note.clear();
                        }
                    }
                }),
                Action::A => return self.media_viewer_activate(),
                _ => return,
            }
            self.push_media();
            return;
        }
        match a {
            Action::X => return self.media_menu('x'),
            Action::Y => return self.media_menu('s'),
            Action::Menu => return self.media_menu('y'),
            _ => {}
        }
        let g: Value = serde_json::from_str(PAGE).unwrap_or_default();
        let cols = g["geo"]["cols"].as_u64().unwrap_or(3) as usize;
        let (py, y0) = (g["geo"]["py"].as_f64().unwrap_or(196.0) as f32, g["geo"]["y0"].as_f64().unwrap_or(56.0) as f32);
        // Same logical area as MediaScreen (grid viewport ends at the footer hint row),
        // then convert back to the existing CSS-space scroll model.
        let ui = self.ui();
        let bottom = ((ui.window().size().height as f32 / ui.window().scale_factor()) - 50.2) / SC;
        // Touch scrolling can change the viewport independently of grid focus.
        self.preserve_media_async_scroll();
        let open = ST.with(|s| {
            let mut s = s.borrow_mut();
            let n = s.view.len();
            if n == 0 {
                return false;
            }
            if a == Action::A { return true; }
            navigate_grid(&mut s, a, cols, y0, py, bottom);
            false
        });
        if open {
            return self.media_activate_card();
        }
        self.push_media();
        if self.media_selecting() { self.update_hint(); }
    }
}

#[cfg(test)]
mod native_thumbnail_tests {
    #[test]
    fn screenshot_directory_resolves_shortcut_game_id() {
        let shortcut = 0x8123_4567u32;
        let game_id = ((shortcut as u64) << 32) | 0x0200_0000;
        assert_eq!(super::screenshot_app_id(&game_id.to_string()), Some(shortcut));
        assert_eq!(super::screenshot_app_id("730"), Some(730));
        assert_eq!(super::screenshot_app_id("4294967296"), None);
    }
    use super::*;
    fn item() -> Item {
        let value: Value=serde_json::from_str(include_str!("../../../crates/steam-bridge/tests/fixtures/native-clip-sanitized.json")).unwrap();
        Item {kind:1,thumb:None,full:None,native:Some(steam_bridge::recording::parse_clip_summary(&value).unwrap()),app:730,mtime:SystemTime::UNIX_EPOCH}
    }
    fn state() -> (St,ThumbTicket) {
        let item=item(); let key=thumb_key(&item,42).unwrap();
        let ticket=ThumbTicket {key:key.clone(),epoch:7};
        let mut s=St {items:vec![item],view:vec![0],focus:0,vy:137.5,viewer:Some(0),vfocus:2,
            type_filter:2,game:Some(730),thumb_epoch:7,note:"preserve".into(),..St::default()};
        s.thumb_pending.insert(key); (s,ticket)
    }
    #[test]
    fn media_preview_quality_regression() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("deckshell-media-quality-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&dir).unwrap();
        let full = dir.join("full.png"); let thumb = dir.join("thumb.png");
        image::RgbImage::new(1000, 600).save(&full).unwrap();
        image::RgbImage::new(120, 72).save(&thumb).unwrap();
        let read = || media_thumbs::read_screenshot(&full, Some(&thumb));
        let selected = read().unwrap(); assert_eq!((selected.width(), selected.height()), (640, 384));
        image::RgbImage::new(80, 48).save(&full).unwrap();
        let selected_thumb = read().unwrap(); assert_eq!((selected_thumb.width(), selected_thumb.height()), (120, 72));
        std::fs::write(&full, b"invalid").unwrap();
        let fallback = read().unwrap(); assert_eq!((fallback.width(), fallback.height()), (120, 72));
        std::fs::remove_file(&thumb).unwrap(); assert!(read().is_none());
        std::fs::remove_file(&full).unwrap(); assert!(read().is_none());
        let caption = media_caption("Portal", 0, SystemTime::UNIX_EPOCH);
        assert!(caption.contains("Portal")); assert!(!caption.to_lowercase().contains("appid"));
        assert!(!media_caption(&media_name(None, 730), 0, SystemTime::UNIX_EPOCH).to_lowercase().contains("appid"));
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn touch_scroll_survives_async_thumbnail_and_visible_requests_follow_scroll_not_focus() {
        let (mut s,t)=state();
        preserve_async_scroll(&mut s,-500.0);
        let scroll=s.vy;
        assert!(apply_thumbnail_result(&mut s,&t,Some(42),Some(slint::Image::default())));
        assert_eq!(s.vy,scroll); assert_eq!(s.focus,0); assert_eq!(s.game,Some(730));
        assert!(!thumbnail_near_view(0.0,188.0,700.0,667.0));
        assert!(thumbnail_near_view(800.0,188.0,700.0,667.0));
        preserve_async_scroll(&mut s,f32::NAN); assert_eq!(s.vy,scroll);
    }
    #[test]
    fn matching_late_thumbnail_changes_only_cache_and_preserves_navigation() {
        let (mut s,t)=state();
        assert!(apply_thumbnail_result(&mut s,&t,Some(42),Some(slint::Image::default())));
        assert_eq!((s.focus,s.vy,s.viewer,s.vfocus,s.type_filter,s.game),(0,137.5,Some(0),2,2,Some(730)));
        assert_eq!(s.note,"preserve"); assert_eq!(s.view,vec![0]);
        assert!(s.thumb_cache.get(&t.key,true).is_some()); assert!(!s.thumb_pending.contains(&t.key));
        assert!(!apply_thumbnail_result(&mut s,&t,Some(42),Some(slint::Image::default()))); // duplicate
    }
    #[test]
    fn stale_generation_account_filter_and_source_cannot_apply() {
        let (mut s,t)=state();
        assert!(!thumbnail_result_is_current(&s,&t,Some(43)));
        s.view.clear(); assert!(!thumbnail_result_is_current(&s,&t,Some(42)));
        s.view=vec![0]; s.items[0].native.as_mut().unwrap().thumbnail_url="https://untrusted.invalid/x".into();
        assert!(!thumbnail_result_is_current(&s,&t,Some(42)));
        assert!(!apply_thumbnail_result(&mut s,&t,Some(42),None));
        assert!(!s.thumb_pending.contains(&t.key)); assert!(s.thumb_failed.is_empty());
        let (mut s,t)=state(); invalidate_thumbs(&mut s);
        assert!(!apply_thumbnail_result(&mut s,&t,Some(42),Some(slint::Image::default())));
        assert!(s.thumb_cache.entries.is_empty()); assert!(s.thumb_pending.is_empty());
    }
    #[test]
    fn memory_lru_is_bounded_deduplicated_and_keeps_recently_used_identity() {
        let (_,t)=state(); let mut cache=ThumbCache::<usize>::default();
        let key=|n| ThumbKey {clip_id:format!("fixture-{n}"),..t.key.clone()};
        for n in 0..THUMB_CACHE_CAP { cache.insert(key(n),n); }
        assert_eq!(cache.get(&key(0),true),Some(0)); cache.insert(key(THUMB_CACHE_CAP),99);
        assert_eq!(cache.entries.len(),THUMB_CACHE_CAP); assert_eq!(cache.get(&key(1),true),None);
        cache.insert(key(0),12); assert_eq!(cache.entries.len(),THUMB_CACHE_CAP); assert_eq!(cache.get(&key(0),true),Some(12));
        assert_eq!(cache.get(&ThumbKey {account:43,..key(0)},true),None);
    }
    #[test]
    fn offscreen_rebuilds_do_not_evict_visible_thumbnails() {
        let (_,t)=state(); let mut cache=ThumbCache::<usize>::default();
        let key=|n| ThumbKey {clip_id:format!("fixture-{n}"),..t.key.clone()};
        for n in 0..THUMB_CACHE_CAP { cache.insert(key(n),n); }
        assert_eq!(cache.get(&key(0),true),Some(0));
        for n in 1..THUMB_CACHE_CAP { assert!(cache.get(&key(n),false).is_some()); }
        cache.insert(key(THUMB_CACHE_CAP),99);
        assert_eq!(cache.get(&key(0),true),Some(0));
        assert_eq!(cache.get(&key(1),false),None);
    }
    #[test]
    fn failure_backoff_is_bounded_and_oversized_source_identity_is_not_queued() {
        let (mut s,t)=state(); assert!(apply_thumbnail_result(&mut s,&t,Some(42),None));
        assert!(s.thumb_cache.entries.is_empty()); assert_eq!(s.thumb_failed.len(),1);
        assert!(s.thumb_failed[0].1.elapsed()<THUMB_RETRY);
        let mut it=item(); it.native.as_mut().unwrap().thumbnail_url="x".repeat(1025);
        assert!(thumb_key(&it,42).is_none());
    }
}

#[cfg(test)]
mod media_grid_navigation_regression {
    use super::*;

    #[test]
    fn rows_boundaries_partial_row_and_scroll() {
        let mut s = St::default();
        s.view = (0..14).collect();
        let step = |s: &mut St, a| navigate_grid(s, a, 3, 56.0, 196.0, 500.0);
        step(&mut s, Action::Right); step(&mut s, Action::Right);
        for expected in [5, 8, 11, 13] {
            let before = s.focus;
            step(&mut s, Action::Down);
            assert_eq!(s.focus, expected);
            assert_ne!(s.focus, before);
            assert!(!grid_at_header_boundary(&s, 3));
        }
        let bottom_scroll = s.vy;
        assert!(bottom_scroll > 0.0);
        step(&mut s, Action::Down);
        assert_eq!((s.focus, s.vy), (13, bottom_scroll));
        for expected in [11, 8, 5, 2] {
            let before = s.focus;
            let signature = state_navigation_signature(&s);
            step(&mut s, Action::Up);
            assert_ne!(state_navigation_signature(&s), signature);
            assert_eq!(s.focus, expected);
            assert_ne!(s.focus, before);
        }
        assert_eq!(s.vy, 0.0);
        assert!(grid_at_header_boundary(&s, 3));
        step(&mut s, Action::Up);
        assert_eq!(s.focus, 2);
        step(&mut s, Action::Right);
        assert_eq!(s.focus, 2);
        step(&mut s, Action::Left);
        assert_eq!(s.focus, 1);
        s.viewer = Some(1);
        assert!(!grid_at_header_boundary(&s, 3));
        s.viewer = None; s.view.clear();
        assert!(grid_at_header_boundary(&s, 3));
    }
}

#[cfg(test)]
mod type_filter_tests {
    use super::*;

    #[test]
    fn gamepad_type_filter_offers_all_and_screenshot_only() {
        let opts = type_filter_options();
        assert_eq!(opts.iter().map(|o| o.0).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(opts[0].1 .0, "#MediaManager_MediaType_All");
        assert_eq!(opts[1].1 .0, "#MediaManager_MediaType_Screenshot");
    }

    #[test]
    fn type_filter_row_follows_value_and_falls_back_to_all() {
        let opts = type_filter_options();
        assert_eq!(type_filter_index(&opts, 0), 0);
        assert_eq!(type_filter_index(&opts, 1), 1);
        // clip (2) and recording (3) are not offered on the gamepad: the first entry is shown
        assert_eq!(type_filter_index(&opts, 2), 0);
        assert_eq!(type_filter_index(&opts, 3), 0);
    }
}

#[cfg(test)]
mod native_clip_app_tests {
    use super::app_for_clip_game;
    use steam_data::{Art, Game, GameKind};

    fn game(app_id: u32, kind: GameKind) -> Game {
        Game { app_id, kind, name: String::new(), sort_name: String::new(), installed: true, install_dir: None,
            size_on_disk: 0, playtime_minutes: 0, last_played: None, update_pending: false, collections: vec![], art: Art::default() }
    }

    #[test]
    fn steam_game_clip_resolves_by_its_appid() {
        let games = [game(730, GameKind::Steam), game(2379780, GameKind::Steam)];
        assert_eq!(app_for_clip_game(&games, "2379780"), 2379780);
        assert_eq!(app_for_clip_game(&games, "730"), 730);
    }

    #[test]
    fn steam_clip_finds_a_game_that_the_library_lists_as_shortcut() {
        // Steam records the store GameID (no high bits) even when DeckShell's kind for the entry is Shortcut.
        let games = [game(2379780, GameKind::Shortcut)];
        assert_eq!(app_for_clip_game(&games, "2379780"), 2379780);
    }

    #[test]
    fn shortcut_clip_resolves_by_its_rungame_id_only() {
        let shortcut = game(0x1234_5678, GameKind::Shortcut);
        let rung = shortcut.rungame_id();
        let games = [shortcut];
        assert_eq!(app_for_clip_game(&games, &rung.to_string()), 0x1234_5678);
        // A GameID without high bits is read as a Steam appid: the store lookup then finds no such app.
        assert_eq!(app_for_clip_game(&games, "305419896"), 305419896 & 0xff_ffff);
    }

    #[test]
    fn unlisted_steam_clip_keeps_its_appid_for_a_store_name_lookup() {
        // Balatro (2379780) was missing from the library list while its clip was written.
        let games = [game(730, GameKind::Steam)];
        assert_eq!(app_for_clip_game(&games, "2379780"), 2379780);
    }

    #[test]
    fn malformed_or_modified_ids_do_not_resolve_to_a_steam_app() {
        let games = [game(730, GameKind::Steam)];
        assert_eq!(app_for_clip_game(&games, ""), 0);
        assert_eq!(app_for_clip_game(&games, "730x"), 0);
        assert_eq!(app_for_clip_game(&games, "0"), 0);
        // High modifier bits make a non-Steam GameID: not appid 730.
        assert_eq!(app_for_clip_game(&games, &((1u64 << 40) | 730).to_string()), 0);
    }
}

#[cfg(test)]
mod delete_tests {
    use super::*;

    pub(super) fn shot(name: &str) -> Item {
        Item { kind: 0, thumb: None, full: Some(PathBuf::from(format!("/shots/{name}.jpg"))), native: None, app: 730, mtime: SystemTime::UNIX_EPOCH }
    }

    pub(super) fn clip(id: &str) -> Item {
        let mut value: Value = serde_json::from_str(include_str!("../../../crates/steam-bridge/tests/fixtures/native-clip-sanitized.json")).unwrap();
        value["clip_id"] = Value::String(id.to_owned());
        Item { kind: 1, thumb: None, full: None, native: Some(steam_bridge::recording::parse_clip_summary(&value).unwrap()), app: 730, mtime: SystemTime::UNIX_EPOCH }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("deckshell-media-delete-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_delete_arms_the_item_and_only_the_same_armed_item_executes() {
        assert_eq!(delete_step(None, "a"), DeleteStep::Arm);
        assert_eq!(delete_step(Some("a"), "a"), DeleteStep::Execute);
        // Moving to another item and pressing Delete arms that item again; it never deletes on the old arm.
        assert_eq!(delete_step(Some("a"), "b"), DeleteStep::Arm);
    }

    #[test]
    fn screenshot_removal_takes_the_file_and_its_thumbnail_and_keeps_the_thumbnail_when_the_file_is_missing() {
        let dir = temp_dir("files");
        let (full, thumb) = (dir.join("shot.jpg"), dir.join("thumb.jpg"));
        std::fs::write(&full, b"full").unwrap();
        std::fs::write(&thumb, b"thumb").unwrap();
        remove_screenshot_files(&full, Some(&thumb)).unwrap();
        assert!(!full.exists() && !thumb.exists());

        std::fs::write(&thumb, b"thumb").unwrap();
        assert!(remove_screenshot_files(&full, Some(&thumb)).is_err(), "a missing file is a reported failure");
        assert!(thumb.exists(), "a failed delete leaves the thumbnail alone");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn viewer_row_offers_play_only_when_playable_and_delete_for_every_item() {
        let c = clip("clip_2379780_20261112_040949");
        assert_eq!(viewer_actions(&c, true), vec![ViewerAct::Play, ViewerAct::PhoneSend, ViewerAct::Delete]);
        assert_eq!(viewer_actions(&c, false), vec![ViewerAct::PhoneSend, ViewerAct::Delete]);
        assert_eq!(viewer_actions(&shot("a"), false),
            vec![ViewerAct::Share, ViewerAct::Upload, ViewerAct::PhoneSend, ViewerAct::Delete]);
    }

    #[test]
    fn phone_target_is_a_clip_or_a_screenshot_file_only() {
        let c = clip("clip_2379780_20261112_040949");
        assert_eq!(phone_target(&c).map(|t| t.0), Some(MediaKind::Clip));
        let mut s = shot("a");
        s.full = None;
        assert_eq!(phone_target(&s).map(|t| t.0), None, "a screenshot without a file has no target");
        s.full = Some(PathBuf::from("/tmp/a.png"));
        assert_eq!(phone_target(&s).map(|t| t.0), Some(MediaKind::Screenshot));
        let mut rec = shot("r");
        rec.kind = 2;
        rec.full = Some(PathBuf::from("/tmp/r.mp4"));
        assert_eq!(phone_target(&rec), None, "recordings are not sent to the phone");
    }

    #[test]
    fn phone_label_marks_a_clip_over_60_seconds_unavailable() {
        let mut c = clip("clip_2379780_20261112_040949");
        if let Some(n) = c.native.as_mut() { n.duration_ms = 75_000; }
        assert!(phone_label(&c).ends_with(i18n::tr_ref("media.unavailable_2")));
        if let Some(n) = c.native.as_mut() { n.duration_ms = 60_000; }
        assert!(!phone_label(&c).contains(i18n::tr_ref("media.unavailable_3")));
    }

    #[test]
    fn removal_leaves_focus_on_the_next_item_and_closes_the_viewer() {
        let mut s = St {
            items: vec![shot("a"), shot("b"), shot("c")],
            view: vec![0, 1, 2],
            focus: 1,
            viewer: Some(1),
            vfocus: 2,
            confirm_key: Some(item_key(&shot("b"))),
            note: i18n::tr_ref("media.delete_a_b").into(),
            ..St::default()
        };
        remove_item_in_state(&mut s, &item_key(&shot("b")));
        refilter_state(&mut s);
        assert_eq!(s.items.len(), 2);
        assert_eq!((s.focus, s.viewer, s.vfocus), (1, None, 0));
        assert!(s.confirm_key.is_none() && s.note.is_empty());
        assert_eq!(item_key(&s.items[s.view[s.focus]]), item_key(&shot("c")), "the item after the deleted one is focused");
    }

    #[test]
    fn removing_the_last_item_moves_focus_to_the_previous_one() {
        let mut s = St { items: vec![shot("a"), shot("b")], view: vec![0, 1], focus: 1, ..St::default() };
        remove_item_in_state(&mut s, &item_key(&shot("b")));
        refilter_state(&mut s);
        assert_eq!(s.focus, 0);
        assert_eq!(item_key(&s.items[s.view[s.focus]]), item_key(&shot("a")));
    }

    #[test]
    fn a_deleted_clip_is_removed_by_its_clip_id() {
        let mut s = St { items: vec![shot("a"), clip("clip_2379780_20261112_040949")], view: vec![0, 1], focus: 1, ..St::default() };
        remove_item_in_state(&mut s, &item_key(&clip("clip_2379780_20261112_040949")));
        refilter_state(&mut s);
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.focus, 0);
        remove_item_in_state(&mut s, "no-such-clip");
        assert_eq!(s.items.len(), 1, "an unknown key removes nothing");
    }
}
