//! Controller layouts of one game: the list behind «Загрузить раскладку» (official, templates,
//! community), the active layout, select / clear, and the community item details and votes.
//!
//! Sources (all stock Steam, verified on the Deck 2026-11-13, Balatro 2379780):
//! - list: `controllerConfiguratorStore.QueryConfigsForApp` (wraps
//!   `SteamClient.Input.QueryControllerConfigsForApp`), then `GetAllConfigs()`;
//! - active layout: `controllerConfiguratorStore.GetActiveConfigForApp(appid, controllerIndex)`;
//! - select / clear: `SteamClient.Input.SetSelectedConfigForApp` / `ClearSelectedConfigForApp`,
//!   with the argument order used by the stock configurator (sp.js, `SetActiveConfigForApp`,
//!   `ClearConfigSelectionAndTriggerReload`);
//! - details and votes: `PublishedFile.GetDetails` / `GetUserVoteSummary` through the stock
//!   service transport (`window.cm.GetServiceTransport()`). The protobuf modules are located by
//!   their text, not by fixed ids, so a Steam update changes nothing here unless the text changes.
//!
//! Controller index: the Deck's own controller is `nControllerIndex` of its list entry (15 on the
//! test Deck), not 0. Read it from the Steam controller list before calling anything.
//!
//! The list's `bSelected` flag does not follow a selection change (checked on the Deck). Use
//! [`Bridge::layout_active`] to find the selected layout.
//!
//! Writes are sent once through `eval_json_once`; they are never replayed after an error.
use steam_assets::i18n::tr_ref;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::{Bridge, Error, Result};

// ---------------------------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------------------------

/// Which section of the stock «Загрузить раскладку» list an entry belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayoutSource {
    /// Flagged `bOfficial` or `bRecommended` by Steam (the «Рекомендации» tab).
    Official,
    /// `template://…` shipped with Steam (the «Шаблоны» tab).
    Template,
    /// `workshop://<publishedfileid>` (the «Сообщество» tab).
    Community,
    /// The current account’s personal layout (stock chooser membership 8).
    User,
}

/// The stock app-ID exclusion (sp.js module 40214, export `bp`).
pub fn community_allowed(app_id: u32) -> bool {
    !matches!(app_id, 241100 | 413080 | 413090 | 413100 | 443510 | 769)
}

#[derive(Debug)]
pub struct LayoutCatalog {
    pub entries: Vec<LayoutEntry>,
    pub connected: bool,
}

/// One row of the stock chooser.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutEntry {
    /// Key for select / clear: `template://…` or `workshop://<id>`.
    pub url: String,
    pub title: String,
    /// Empty when Steam sends none.
    pub description: String,
    pub app_id: u32,
    /// Steam controller type of this layout (`nControllerType`, 4 = Steam Deck).
    pub controller_type: u32,
    pub source: LayoutSource,
    /// `None` for templates and official layouts (Steam sends "0").
    pub published_file_id: Option<u64>,
    /// Publisher account id, when Steam sends one; `author_name` is its Steam persona.
    pub author_account_id: Option<u32>,
    pub author_name: String,
    pub game_name: String,
    pub title_prefix_width: f32,
    pub playtime_number_width: f32,
    /// Stock chooser membership: official=1, template=2, workshop=4, personal=8, search=16, recommended=32.
    pub sections: u32,
    pub recommended: bool,
    pub downloaded: bool,
    pub downloading: bool,
    pub owned: bool,
    pub search_order: usize,
    pub personal_order: usize,
    pub updated_label: String,
    pub revision_mismatch: u32,
    pub has_playtime: bool,
    /// Wrapped description lines at the stock 492 CSS-pixel text width.
    pub description_lines: usize,
    /// Unix seconds (`timeUpdated`).
    pub time_updated: u64,
    /// Hours played across all users, already formatted as hours by Steam (`playtime`).
    pub playtime_seconds: u64,
    /// `eSelectionType` of the entry: the value the stock passes back to clear it.
    pub selection_type: u32,
    pub uses_gamepad: bool,
    pub uses_mouse: bool,
    pub uses_keyboard: bool,
    pub uses_siapi: bool,
}

/// Active layout of a game on one controller, from `GetActiveConfigForApp`.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveLayout {
    pub url: String,
    pub title: String,
    pub author_name: String,
    pub published_file_id: Option<u64>,
    pub selection_type: u32,
    /// Steam Input is enabled for this game on this controller.
    pub enabled: bool,
}

/// Tab filter of the list (stock tabs; «Поиск» is the `text` field of [`LayoutQuery`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceFilter {
    #[default]
    All,
    Official,
    Template,
    Community,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutSort {
    /// Official, then templates, then community; keeps Steam's order inside each section.
    #[default]
    Source,
    /// Title, case-insensitive, ascending.
    Title,
    /// Playtime across users, descending.
    Playtime,
    /// `timeUpdated`, newest first.
    Updated,
}

/// What the user typed and picked. `text` matches title or description, case-insensitively.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutQuery {
    pub text: String,
    pub source: SourceFilter,
    pub sort: LayoutSort,
}

/// Community item details (`CPublishedFileDetails`, read with `PublishedFile.GetDetails`).
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutDetails {
    pub published_file_id: u64,
    /// EResult of this item; 1 = OK, 8 = not found / not accessible for this request.
    pub result: u32,
    pub title: String,
    pub description: String,
    /// Steam id of the creator as a decimal string (not resolved to a name).
    pub creator_steamid: String,
    pub time_created: u64,
    pub time_updated: u64,
    pub file_size: u64,
    pub consumer_appid: u32,
    pub file_type: u32,
    pub votes: Option<Votes>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Votes {
    pub up: u32,
    pub down: u32,
    pub score: f64,
}

/// The current user's vote on one item (`GetUserVoteSummary`).
#[derive(Debug, Clone, PartialEq)]
pub struct UserVote {
    pub published_file_id: u64,
    /// `Some(true)` = up, `Some(false)` = down, `None` = no vote.
    pub vote: Option<bool>,
    pub reported: bool,
}

// ---------------------------------------------------------------------------------------------
// Parsing (strict: a missing or mistyped field is an error, never a default)
// ---------------------------------------------------------------------------------------------

fn obj<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    v.as_object().ok_or_else(|| Error::Protocol(format!("{what} is not an object: {v}")))
}

fn req_str<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a str> {
    o.get(k).and_then(Value::as_str).ok_or_else(|| Error::Protocol(format!("{k} missing or not a string")))
}

fn opt_str<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a str> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(""),
        Some(v) => v.as_str().ok_or_else(|| Error::Protocol(format!("{k} is not a string"))),
    }
}

fn req_bool(o: &Map<String, Value>, k: &str) -> Result<bool> {
    o.get(k).and_then(Value::as_bool).ok_or_else(|| Error::Protocol(format!("{k} missing or not a bool")))
}

fn opt_bool(o: &Map<String, Value>, k: &str) -> Result<bool> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| Error::Protocol(format!("{k} is not a bool"))),
    }
}

fn req_u64(o: &Map<String, Value>, k: &str) -> Result<u64> {
    o.get(k).and_then(Value::as_u64).ok_or_else(|| Error::Protocol(format!("{k} missing or not an unsigned number")))
}

fn req_u32(o: &Map<String, Value>, k: &str) -> Result<u32> {
    u32::try_from(req_u64(o, k)?).map_err(|_| Error::Protocol(format!("{k} out of u32 range")))
}

/// Steam sends some 64-bit ids and counters as decimal strings ("0", "1738261176").
fn str_u64(o: &Map<String, Value>, k: &str) -> Result<u64> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::String(s)) => s.parse::<u64>().map_err(|_| Error::Protocol(format!("{k} is not a number: {s:?}"))),
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| Error::Protocol(format!("{k} is not unsigned"))),
        Some(v) => Err(Error::Protocol(format!("{k} has unexpected type: {v}"))),
    }
}

fn classify(url: &str, official: bool, recommended: bool) -> LayoutSource {
    if official || recommended {
        LayoutSource::Official
    } else if url.starts_with("workshop://") {
        LayoutSource::Community
    } else {
        LayoutSource::Template
    }
}

/// Parse the value of `window.controllerConfiguratorStore.GetAllConfigs()`.
pub fn parse_layout_list(v: &Value) -> Result<Vec<LayoutEntry>> {
    let arr = v.as_array().ok_or_else(|| Error::Protocol(format!("layout list is not an array: {v}")))?;
    arr.iter().map(parse_layout_entry).collect()
}

fn parse_layout_catalog(value: &Value, app_id: u32) -> Result<LayoutCatalog> {
    let catalog = obj(value, "layout catalog")?;
    let connected = req_bool(catalog, "connected")?;
    let rows = catalog.get("rows").ok_or_else(|| Error::Protocol("layout catalog has no rows".into()))?;
    let mut entries = parse_layout_list(rows)?;
    for entry in &mut entries {
        if !community_allowed(app_id) || (!connected && !entry.downloaded) {
            entry.sections &= !4;
        }
    }
    // Stock filters only the Community tab. Search retains its independent membership.
    entries.retain(|entry| entry.sections != 0);
    Ok(LayoutCatalog { entries, connected })
}

fn parse_layout_entry(v: &Value) -> Result<LayoutEntry> {
    let o = obj(v, "layout entry")?;
    let url = req_str(o, "URL")?.to_owned();
    let official = req_bool(o, "bOfficial")?;
    let recommended = req_bool(o, "bRecommended")?;
    let pfid = str_u64(o, "publishedFileID")?;
    let account = match o.get("accountID") {
        None | Some(Value::Null) => None,
        Some(_) => Some(req_u32(o, "accountID")?).filter(|&a| a != 0),
    };
    let sections = o.get("sections").and_then(Value::as_u64).unwrap_or(0) as u32;
    Ok(LayoutEntry {
        source: if sections & 8 != 0 { LayoutSource::User } else { classify(&url, official, recommended) },
        url,
        title: req_str(o, "Title")?.to_owned(),
        description: opt_str(o, "Description")?.to_owned(),
        app_id: req_u32(o, "appID")?,
        controller_type: req_u32(o, "nControllerType")?,
        published_file_id: (pfid != 0).then_some(pfid),
        author_account_id: account,
        author_name: opt_str(o, "authorName")?.to_owned(),
        game_name: opt_str(o, "gameName")?.to_owned(),
        title_prefix_width: o.get("titlePrefixWidth").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        playtime_number_width: o.get("playtimeNumberWidth").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        sections,
        recommended,
        downloaded: opt_bool(o, "downloaded")?,
        downloading: opt_bool(o, "downloading")?,
        owned: opt_bool(o, "owned")?,
        personal_order: o.get("personalOrder").and_then(Value::as_u64).unwrap_or(0) as usize,
        updated_label: opt_str(o, "updatedLabel")?.to_owned(),
        revision_mismatch: o.get("eMajorRevisionMismatch").and_then(Value::as_u64).unwrap_or(0) as u32,
        has_playtime: o.get("playtime").is_some_and(|playtime| !playtime.is_null()),
        search_order: o.get("searchOrder").and_then(Value::as_u64).unwrap_or(0) as usize,
        description_lines: o.get("descriptionLines").and_then(Value::as_u64).unwrap_or(0) as usize,
        time_updated: str_u64(o, "timeUpdated")?,
        playtime_seconds: str_u64(o, "playtime")?,
        selection_type: match o.get("eSelectionType") {
            None | Some(Value::Null) => 0,
            Some(_) => req_u32(o, "eSelectionType")?,
        },
        uses_gamepad: opt_bool(o, "bUsesGamepad")?,
        uses_mouse: opt_bool(o, "bUsesMouse")?,
        uses_keyboard: opt_bool(o, "bUsesKeyboard")?,
        uses_siapi: opt_bool(o, "bUsesSIAPI")?,
    })
}

/// Parse the value of `GetActiveConfigForApp`. `null` (nothing selected) is `Ok(None)`.
pub fn parse_active_layout(v: &Value) -> Result<Option<ActiveLayout>> {
    if v.is_null() {
        return Ok(None);
    }
    let o = obj(v, "active layout")?;
    let pfid = str_u64(o, "publishedFileID")?;
    Ok(Some(ActiveLayout {
        url: req_str(o, "URL")?.to_owned(),
        title: req_str(o, "Title")?.to_owned(),
        author_name: opt_str(o, "authorName")?.to_owned(),
        published_file_id: (pfid != 0).then_some(pfid),
        selection_type: req_u32(o, "eSelectionType")?,
        enabled: req_bool(o, "bConfigurationEnabled")?,
    }))
}

/// Parse `PublishedFile.GetDetails` (`{eresult, body:{publishedfiledetails:[…]}}`, as produced by
/// [`DETAILS_JS`]). An item with `result != 1` is returned with its result code, not dropped.
pub fn parse_layout_details(v: &Value) -> Result<Vec<LayoutDetails>> {
    let o = obj(v, "details reply")?;
    let eresult = req_u64(o, "eresult")?;
    if eresult != 1 {
        return Err(Error::Protocol(format!("PublishedFile.GetDetails eresult {eresult}")));
    }
    let body = obj(o.get("body").ok_or_else(|| Error::Protocol("details reply has no body".into()))?, "details body")?;
    let items = body
        .get("publishedfiledetails")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Protocol("publishedfiledetails missing".into()))?;
    items.iter().map(parse_one_detail).collect()
}

fn parse_one_detail(v: &Value) -> Result<LayoutDetails> {
    let o = obj(v, "detail")?;
    let votes = match o.get("vote_data") {
        None | Some(Value::Null) => None,
        Some(vd) => {
            let vd = obj(vd, "vote_data")?;
            Some(Votes {
                up: req_u32(vd, "votes_up")?,
                down: req_u32(vd, "votes_down")?,
                score: vd.get("score").and_then(Value::as_f64).ok_or_else(|| Error::Protocol("score missing".into()))?,
            })
        }
    };
    Ok(LayoutDetails {
        published_file_id: str_u64(o, "publishedfileid")?,
        result: req_u32(o, "result")?,
        title: opt_str(o, "title")?.to_owned(),
        description: opt_str(o, "file_description")?.to_owned(),
        creator_steamid: opt_str(o, "creator")?.to_owned(),
        time_created: req_u64(o, "time_created")?,
        time_updated: req_u64(o, "time_updated")?,
        file_size: str_u64(o, "file_size")?,
        consumer_appid: req_u32(o, "consumer_appid")?,
        file_type: req_u32(o, "file_type")?,
        votes,
    })
}

/// Parse `PublishedFile.GetUserVoteSummary` as produced by [`USER_VOTES_JS`].
pub fn parse_user_votes(v: &Value) -> Result<Vec<UserVote>> {
    let o = obj(v, "vote reply")?;
    let eresult = req_u64(o, "eresult")?;
    if eresult != 1 {
        return Err(Error::Protocol(format!("PublishedFile.GetUserVoteSummary eresult {eresult}")));
    }
    let body = obj(o.get("body").ok_or_else(|| Error::Protocol("vote reply has no body".into()))?, "vote body")?;
    let arr = body
        .get("summaries")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Protocol("summaries missing".into()))?;
    arr.iter()
        .map(|s| {
            let s = obj(s, "vote summary")?;
            let up = opt_bool(s, "vote_for")?;
            let down = opt_bool(s, "vote_against")?;
            Ok(UserVote {
                published_file_id: str_u64(s, "publishedfileid")?,
                vote: match (up, down) {
                    (true, _) => Some(true),
                    (false, true) => Some(false),
                    (false, false) => None,
                },
                reported: opt_bool(s, "reported")?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Filter and sort (pure; used by the UI and by tests)
// ---------------------------------------------------------------------------------------------

/// Apply the tab filter, the search text and the sort. Returns references in display order.
pub fn select_layouts<'a>(entries: &'a [LayoutEntry], q: &LayoutQuery) -> Vec<&'a LayoutEntry> {
    let needle = q.text.trim().to_lowercase();
    let mut out: Vec<&LayoutEntry> = entries
        .iter()
        .filter(|e| match q.source {
            SourceFilter::All => true,
            SourceFilter::Official => e.source == LayoutSource::Official,
            SourceFilter::Template => e.source == LayoutSource::Template,
            SourceFilter::Community => e.source == LayoutSource::Community,
        })
        .filter(|e| {
            needle.is_empty()
                || e.title.to_lowercase().contains(&needle)
                || e.description.to_lowercase().contains(&needle)
        })
        .collect();
    // `sort_by` is stable, so Steam's order is kept inside equal keys.
    match q.sort {
        LayoutSort::Source => out.sort_by_key(|e| e.source),
        LayoutSort::Title => out.sort_by_key(|e| e.title.to_lowercase()),
        LayoutSort::Playtime => out.sort_by(|a, b| b.playtime_seconds.cmp(&a.playtime_seconds)),
        LayoutSort::Updated => out.sort_by(|a, b| b.time_updated.cmp(&a.time_updated)),
    }
    out
}

/// True when `entry` is the layout currently active for the game.
pub fn is_active(entry: &LayoutEntry, active: Option<&ActiveLayout>) -> bool {
    active.is_some_and(|a| a.url == entry.url)
}

// ---------------------------------------------------------------------------------------------
// JavaScript (only integers and validated strings are interpolated)
// ---------------------------------------------------------------------------------------------

/// Set the stock «show compatible only» flag (`m_bFilterOtherControllerTypes`) and start the stock
/// list query for the app. Returns the previous flag, which the caller restores.
pub const LIST_START_JS: &str = "(async()=>{await document.fonts.load(\"12px Motiva Sans\");const s=window.controllerConfiguratorStore;const prev=!!s.m_bFilterOtherControllerTypes;\
s.SetFilterOtherControllerTypes({FILTER});s.QueryConfigsForApp({APP},{IDX});return prev})()";

/// Restore the flag after the list was read.
pub const LIST_RESTORE_JS: &str = "(()=>{window.controllerConfiguratorStore.SetFilterOtherControllerTypes({FILTER});return true})()";

/// Progress of the list for the app: entry count, and whether every entry is complete (the
/// store can hold placeholder entries without a URL while the reply streams in).
pub const LIST_PROGRESS_JS: &str = "(()=>{const a=Array.from(window.controllerConfiguratorStore.GetConfigsForApp({APP}));\
return {n:a.length,complete:!window.controllerConfiguratorStore.BConfigurationQueryInFlight&&a.every(x=>typeof x.URL==='string'&&typeof x.Title==='string')}})()";

/// Chooser projection using the stock section selectors. The six type ids below are the
/// `si` built-in template exclusions in sp.js (2, 30, 31, 36, 37, 45); the active type is retained.
/// Search keeps GetAllConfigs order and game names, including other previously queried apps.
/// Rows are merged by app/type/URL; identical titles are not an identity.
pub const LIST_JS: &str = r#"(async () => {
    const s = window.controllerConfiguratorStore;
    const type = window.ControllerStore.GetController({IDX})?.eControllerType;
    if (type == null) throw Error('controller unavailable');
    const workshop = window.workshopStore;
    let installed;
    try {
        installed = await SteamClient.Apps.GetDownloadedWorkshopItems(241100);
    } catch {
        installed = workshop.GetDownloadedItems(241100) ?? [];
    }
    const downloaded = new Set(installed.map(item => String(item.publishedfileid)));
    const downloading = new Set((workshop.GetDownloadingItems(241100) ?? []).map(item => String(item.publishedfileid)));
    const connected = workshop.BConnectedToSteam();

    const templates = s.GetTemplateConfigsForApp({APP}, type)
        .filter(c => c.publishedFileID == '0' && (c.nControllerType === type || ![2,30,31,36,37,45].includes(c.nControllerType)))
        .sort((a,b) => Number(b.bRecommended) - Number(a.bRecommended));
    const community = s.GetWorkshopConfigsForApp({APP}, type)
        .filter(c => !c.bOfficial);
    const friends = window.friendStore;
    const account = friends?.currentUserSteamID?.GetAccountID();
    const personal = account ? s.GetUserConfigsForApp({APP}, type, account) : [];
    const groups = [
        [1, s.GetOfficialConfigsForApp({APP}, type)], [2, templates], [4, community],
        [8, personal], [16, s.GetAllConfigs()], [32, templates.filter(c => c.bRecommended)],
    ];
    const measure = font => {
        const ctx = document.createElement('canvas').getContext('2d');
        ctx.font = font + 'px "Motiva Sans", Helvetica, sans-serif';
        return ctx;
    };
    const ctx = measure(12), titleCtx = measure(16), infoCtx = measure(10);
    const lines = text => {
        if (!text) return 0;
        let line = '', count = 1;
        for (const word of text.trim().split(/\s+/)) {
            const next = line ? line + ' ' + word : word;
            if (line && ctx.measureText(next).width > 492) { count++; line = word; }
            else line = next;
        }
        return count;
    };
    const rows = new Map();
    for (const [section, configs] of groups) {
        for (const [order, config] of configs.entries()) {
            if (s.IsDeletedConfig(config.URL)) continue;
            const key = JSON.stringify([config.appID, config.nControllerType, config.URL]);
            let row = rows.get(key);
            if (!row) {
                row = { ...config, sections: 0 };
                const hours = String(config.playtime ?? '0') + ' ';
                const id = String(config.publishedFileID);
                row.playtimeNumberWidth = infoCtx.measureText(hours).width * 1.5 + hours.length * 0.75;
                row.descriptionLines = lines(config.Description);
                row.downloaded = downloaded.has(id);
                row.downloading = !row.downloaded && downloading.has(id);
                row.owned = !!account && config.accountID == account;
                row.authorName = config.accountID ? friends?.GetFriendState(config.accountID)?.display_name ?? '' : '';
                row.gameName = window.appStore?.GetAppOverviewByAppID(config.appID)?.display_name ?? '';
                row.titlePrefixWidth = titleCtx.measureText((row.gameName || (config.appID ? String(config.appID) : {TEMPLATE_LABEL})) + ': ').width * 1.5;
                rows.set(key, row);
            }
            row.sections |= section;
            if (section === 16) row.searchOrder = order;
            if (section === 8) {
                row.personalOrder = order;
                if (Number(config.timeUpdated) > 0) {
                    row.updatedLabel = new Date(Number(config.timeUpdated) * 1000).toLocaleString({DATE_LOCALE}, {
                        year: 'numeric', month: 'short', day: 'numeric', hour: 'numeric', minute: 'numeric',
                    });
                }
            }
        }
    }
    return { connected, rows: Array.from(rows.values()) };
})()"#;

/// Active layout for the app and controller, from the native `GetConfigForAppAndController`
/// (returns the full layout object for a real controller index, `null` when there is none).
pub const ACTIVE_JS: &str = r#"(async () => {
    const config = await SteamClient.Input.GetConfigForAppAndController({APP}, {IDX});
    if (!config || typeof config.URL !== "string") return null;
    let authorName = "";
    if ({WITH_AUTHOR} && config.bConfigurationEnabled) {
        const url = config.URL;
        const friends = window.friendStore;
        const details = window.appDetailsStore;
        let name;
        let subscription;
        try {
            // sp.js Ia/na: official uses the game's developer; autosave uses the
            // current persona; Workshop uses the active config's publisher.
            if (config.bOfficial || url.startsWith("game")) {
                name = () => details?.GetAppDetails({APP})?.strDeveloperName;
                if (!name()) subscription = details?.RegisterForAppData({APP}, () => {});
            } else if (url.startsWith("autosave")) {
                name = () => friends?.GetFriendState(friends.currentUserSteamID)?.display_name;
            } else if (url.startsWith("workshop") && config.accountID) {
                name = () => friends?.GetFriendState(config.accountID)?.display_name;
            }
            if (name) {
                authorName = name() || "";
                for (let attempt = 0; !authorName && attempt < 5; attempt++) {
                    await new Promise(resolve => setTimeout(resolve, 200));
                    authorName = name() || "";
                }
            }
        } catch {
            // The native layout remains usable if Steam has no author metadata.
        } finally {
            subscription?.unregister?.();
        }
    }
    return {URL: config.URL, Title: config.Title, publishedFileID: config.publishedFileID,
        eSelectionType: config.eSelectionType, bConfigurationEnabled: !!config.bConfigurationEnabled, authorName};
})()"#;

/// Select a layout. Arguments as the stock `SetActiveConfigForApp` passes them: `false` is the
/// 4th argument on every stock call site; the 5th is the selection type.
pub const SELECT_JS: &str =
    "(async()=>{await SteamClient.Input.SetSelectedConfigForApp({APP},{IDX},{URL},false,{SEL});return true})()";

/// Clear the selection. Same order as the stock `ClearConfigSelectionAndTriggerReload`: the native
/// clear, then the store's own cache for the app.
pub const CLEAR_JS: &str = "(async()=>{await SteamClient.Input.ClearSelectedConfigForApp({APP},{IDX},{SEL});\
window.controllerConfiguratorStore.ClearSelectedConfigCache({APP});return true})()";

/// Locate the PublishedFile service module and the request builder by their text, create a
/// `GetDetails` request for `{IDS}` (JSON array of decimal strings), and return
/// `{eresult, body}`. The stock registers webpack chunks through `webpackChunksteamui`.
const WEBPACK_JS: &str = r#"const __req=(()=>{let r=null;window.webpackChunksteamui.push([["fable-cfg-"+Math.random()],{},(x)=>{r=x}]);return r})();
const __mods=__req.m;
"#;

const PF_HELPER_JS: &str = r#"const __svc=Object.keys(__mods).find(id=>__mods[id].toString().includes("PublishedFile.QueryFiles#1"));
const __bld=Object.keys(__mods).find(id=>/I8:\(\)=>\w+,lI:\(\)=>\w+,w:\(\)=>\w+/.test(__mods[id].toString()));
if(!__svc||!__bld)throw new Error("PublishedFile modules not found");
const __pf=__req(__svc), __b=__req(__bld);
function __cls(name){return Object.keys(__pf).map(k=>__pf[k]).find(v=>typeof v==="function"&&v.prototype&&typeof v.prototype.getClassName==="function"&&v.prototype.getClassName()===name)}
"#;

/// `PublishedFile.GetDetails` for `{IDS}`. Replies `{eresult, body}`.
pub const DETAILS_JS: &str = concat!(
    "(async()=>{",
    r#"const __ids={IDS};"#,
    "\n",
    r#"const __w=__b.w.Init(__cls("CPublishedFile_GetDetails_Request"));const __body=__w.Body();"#,
    "\n",
    r#"__body.set_publishedfileids(__ids);__body.set_includevotes(true);__body.set_includetags(true);__body.set_includekvtags(true);"#,
    r#"__body.set_short_description(false);__body.set_strip_description_bbcode(true);__body.set_language(0);"#,
    "\n",
    r#"const __res=await __pf.a5.GetDetails(window.cm.GetServiceTransport(),__w);"#,
    r#"return {eresult:__res.GetEResult(),body:__res.Body().toObject()};})()"#,
);

/// `PublishedFile.GetUserVoteSummary` for `{IDS}`. Replies `{eresult, body}`.
pub const USER_VOTES_JS: &str = concat!(
    "(async()=>{",
    r#"const __ids={IDS};"#,
    "\n",
    r#"const __w=__b.w.Init(__cls("CPublishedFile_GetUserVoteSummary_Request"));__w.Body().set_publishedfileids(__ids);"#,
    "\n",
    r#"const __res=await __pf.a5.GetUserVoteSummary(window.cm.GetServiceTransport(),__w);"#,
    r#"return {eresult:__res.GetEResult(),body:__res.Body().toObject()};})()"#,
);

/// Build the details script for the given ids (the helper and the call in one expression).
pub fn details_expr(ids: &[u64]) -> Result<String> {
    let ids_json = id_array(ids)?;
    Ok(format!("(async()=>{{{WEBPACK_JS}{PF_HELPER_JS}return {DETAILS_JS_BODY};}})()", DETAILS_JS_BODY = call_body(DETAILS_JS, &ids_json)))
}

/// Build the user-vote script for the given ids.
pub fn user_votes_expr(ids: &[u64]) -> Result<String> {
    let ids_json = id_array(ids)?;
    Ok(format!("(async()=>{{{WEBPACK_JS}{PF_HELPER_JS}return {USER_BODY};}})()", USER_BODY = call_body(USER_VOTES_JS, &ids_json)))
}

fn call_body(template: &str, ids_json: &str) -> String {
    // The templates carry their own `(async()=>{…})()` wrapper; keep only the inner statements.
    let inner = template.strip_prefix("(async()=>{").unwrap_or(template);
    let inner = inner.strip_suffix("})()").unwrap_or(inner);
    let inner = inner.replace("{IDS}", ids_json);
    // Returned as an async IIFE value for the outer `return`.
    format!("(async()=>{{{inner}}})()")
}

fn id_array(ids: &[u64]) -> Result<String> {
    if ids.is_empty() || ids.len() > 50 {
        return Err(Error::Protocol(format!("expected 1..=50 ids, got {}", ids.len())));
    }
    let parts: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
    Ok(format!("[{}]", parts.join(",")))
}

/// Layout URL check before it is put into JavaScript: `template://<name>.vdf` with the usual
/// Steam name characters, or `workshop://<decimal id>`. Anything else is refused.
pub fn check_layout_url(url: &str) -> Result<()> {
    let ok = if let Some(name) = url.strip_prefix("template://") {
        !name.is_empty()
            && name.len() <= 200
            && name.ends_with(".vdf")
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-+".contains(&b))
    } else if let Some(id) = url.strip_prefix("workshop://") {
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
    } else {
        false
    };
    if ok {
        Ok(())
    } else {
        Err(Error::Protocol(format!("layout url refused: {url:?}")))
    }
}

fn fill(template: &str, app: u32, idx: u32, extra: &[(&str, String)]) -> String {
    let mut s = template.replace("{APP}", &app.to_string()).replace("{IDX}", &idx.to_string());
    for (k, v) in extra {
        s = s.replace(k, v);
    }
    s
}

/// The expression for a select call. Validates the URL first.
pub fn select_expr(app: u32, idx: u32, url: &str, selection_type: u32) -> Result<String> {
    if url.starts_with("autosave://") { check_personal_url(url)?; } else { check_layout_url(url)?; }
    let url_js = serde_json::to_string(url).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(fill(SELECT_JS, app, idx, &[("{URL}", url_js), ("{SEL}", selection_type.to_string())]))
}

/// The expression for a clear call.
pub fn clear_expr(app: u32, idx: u32, selection_type: u32) -> String {
    fill(CLEAR_JS, app, idx, &[("{SEL}", selection_type.to_string())])
}

/// Personal (own) layout URL: only the `autosave://` copies seen on the Deck (task 210). Anything
/// else (templates, workshop items, unknown schemes) is refused, so a delete can never reach a
/// shared layout.
pub fn check_personal_url(url: &str) -> Result<()> {
    let ok = url.strip_prefix("autosave://").is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 300 && !rest.contains("..") && rest.ends_with(".vdf")
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-+/".contains(&b))
    });
    if ok { Ok(()) } else { Err(Error::Protocol(format!("personal layout url refused: {url:?}"))) }
}

/// Stock delete (sp.js, `va`): `DeletePersonalControllerConfiguration(url)`, the store forgets the
/// entry, and the selection is cleared when the deleted layout was the selected one.
pub fn delete_personal_expr(app: u32, idx: u32, url: &str, selected_type: Option<u32>) -> Result<String> {
    check_personal_url(url)?;
    let url_js = serde_json::to_string(url).map_err(|e| Error::Protocol(e.to_string()))?;
    let clear = selected_type.map(|t| format!("await SteamClient.Input.ClearSelectedConfigForApp({app},{idx},{t});")).unwrap_or_default();
    Ok(format!(
        "(async()=>{{const u={url_js};await SteamClient.Input.DeletePersonalControllerConfiguration(u);window.controllerConfiguratorStore.AddDeletedConfig(u);{clear}return true}})()"
    ))
}

/// Own copy of a template (stock "unique configuration"): select the template, open it for editing,
/// then `ConvertConfigToSelectionType(app, 1)` (value 1 observed on the Deck, task 210 §10.1).
pub fn create_from_template_expr(app: u32, idx: u32, template_url: &str) -> Result<String> {
    if !template_url.starts_with("template://") { return Err(Error::Protocol(format!("not a template: {template_url:?}"))); }
    check_layout_url(template_url)?;
    let url_js = serde_json::to_string(template_url).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(fill(SELECT_JS, app, idx, &[("{URL}", url_js), ("{SEL}", "0".into())]))
}

fn create_current_expr(app: u32, idx: u32, active_url: &str) -> Result<String> {
    let url = serde_json::to_string(active_url).map_err(|err| Error::Protocol(err.to_string()))?;
    Ok(format!("(async()=>{{const s=window.controllerConfiguratorStore;await s.m_updatingEditingConfigurationPromise;s.LoadEditingConfiguration({app},{idx});await s.m_updatingEditingConfigurationPromise;if(s.m_appId!=={app}||s.m_unControllerIndex!=={idx}||s.EditingConfiguration?.url!=={url})throw new Error('Editing layout changed');s.ConvertConfigToSelectionType({app},1);return true}})()"))
}

/// Stock export type «Personal Local» (`Y.qz` in sp.js; the same value `ConvertConfigToSelectionType` takes for a personal copy).
pub const EXPORT_PERSONAL_LOCAL: u32 = 1;
pub const EXPORT_TITLE_MAX: usize = 64;
pub const EXPORT_DESCRIPTION_MAX: usize = 512;

/// Trimmed title and description of an export, or why the stock dialog would refuse them (empty or too long).
pub fn check_export_text(title: &str, description: &str) -> Result<(String, String)> {
    let (title, description) = (title.trim(), description.trim());
    if title.is_empty() { return Err(Error::Protocol(tr_ref("controller.layout_name_is_empty").into())); }
    if title.chars().count() > EXPORT_TITLE_MAX || description.chars().count() > EXPORT_DESCRIPTION_MAX {
        return Err(Error::Protocol(tr_ref("controller.layout_name_or_description_is_too_long").into()));
    }
    if title.chars().chain(description.chars()).any(|c| c.is_control()) { return Err(Error::Protocol(tr_ref("controller.layout_name_contains_invalid_characters").into())); }
    Ok((title.to_owned(), description.to_owned()))
}

/// Stock «Сохранить как…» (sp.js `ExportEditingConfiguration`): load the editing copy of the active layout, then one
/// `ExportCurrentControllerConfiguration(idx, app, type, title, description, "")` through the store wrapper. Only the local type is built here.
pub fn export_local_expr(app: u32, idx: u32, active_url: &str, title: &str, description: &str) -> Result<String> {
    let (title, description) = check_export_text(title, description)?;
    let js = |v: &str| serde_json::to_string(v).map_err(|err| Error::Protocol(err.to_string()));
    let (url, title, description) = (js(active_url)?, js(&title)?, js(&description)?);
    Ok(format!("(async()=>{{const s=window.controllerConfiguratorStore;await s.m_updatingEditingConfigurationPromise;s.LoadEditingConfiguration({app},{idx});await s.m_updatingEditingConfigurationPromise;if(s.m_appId!=={app}||s.m_unControllerIndex!=={idx}||s.EditingConfiguration?.url!=={url})throw new Error('Editing layout changed');s.ExportEditingConfiguration({app},{EXPORT_PERSONAL_LOCAL},{title},{description},\"\");return true}})()"))
}

fn demo_layouts(locale: &steam_assets::i18n::Locale) -> Result<Vec<LayoutEntry>> {
    let rows: Value = serde_json::from_str(include_str!("../tests/fixtures/controller_layouts/list_balatro_deck.json"))
        .map_err(|err| Error::Protocol(err.to_string()))?;
    let mut entries = parse_layout_list(&rows)?;
    for entry in &mut entries {
        for text in [&mut entry.title, &mut entry.description] {
            if text.starts_with("controller.demo.") {
                *text = locale.tr(text);
            }
        }
    }
    Ok(entries)
}

/// Demo catalog projected from the sanitized fixture. Its rows are read-only.
pub fn offline_catalog() -> Result<Vec<LayoutEntry>> {
    let mut entries = demo_layouts(steam_assets::i18n::current())?;
    for entry in &mut entries {
        entry.author_account_id = None;
        entry.author_name.clear();
        entry.sections = 16 | match (entry.url.starts_with("template://"), entry.source) {
            (true, LayoutSource::Official) => 1 | 2,
            (true, _) => 2,
            (_, LayoutSource::Official) => 1,
            (_, LayoutSource::Community) => 4,
            _ => 0,
        };
        if !entry.description.is_empty() { entry.description_lines = entry.description.chars().count().div_ceil(72); }
    }
    if let Some(template) = entries.iter().find(|entry| entry.sections & 2 != 0) {
        let mut personal = template.clone();
        personal.url = "autosave://fixture/example.vdf".into();
        personal.title = tr_ref("controller.gamepad_sample_personal_copy").into();
        personal.description.clear();
        personal.sections = 8 | 16;
        personal.source = LayoutSource::User;
        personal.owned = true;
        personal.recommended = false;
        personal.selection_type = 1;
        entries.push(personal);
    }
    Ok(entries)
}

fn list_poll_finished(stable: u32, elapsed: Duration) -> Result<bool> {
    if stable >= 5 {
        Ok(true)
    } else if elapsed > Duration::from_secs(12) {
        Err(Error::Timeout)
    } else {
        Ok(false)
    }
}

// ---------------------------------------------------------------------------------------------
// Bridge methods
// ---------------------------------------------------------------------------------------------

impl Bridge {
    /// Chooser rows for an app/controller, including the global Search inventory. Use `sections`
    /// for tab membership; Search rows can belong to another app or to app 0 (templates).
    /// `show_all` = «Показать все» (the stock flag is `!show_all`). The stock flag is restored
    /// afterwards, also on error. Waits until the list is complete and its count is stable
    /// (at most 12 s). Read-only: nothing is selected or saved.
    pub fn layout_list(&self, app_id: u32, controller_index: u32, show_all: bool) -> Result<Vec<LayoutEntry>> {
        Ok(self.layout_catalog(app_id, controller_index, show_all)?.entries)
    }

    pub fn layout_catalog(&self, app_id: u32, controller_index: u32, show_all: bool) -> Result<LayoutCatalog> {
        // All callers share the Steam store's compatibility flag and in-flight query.
        static LIST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LIST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let start = fill(LIST_START_JS, app_id, controller_index, &[("{FILTER}", (!show_all).to_string())]);
        let prev = self.eval_json(&start)?.as_bool().unwrap_or(true);
        let result = self.wait_and_read_list(app_id, controller_index);
        let restore = fill(LIST_RESTORE_JS, 0, 0, &[("{FILTER}", prev.to_string())]);
        let restored = self.eval_json(&restore);
        let list = result?;
        restored?;
        Ok(list)
    }

    fn wait_and_read_list(&self, app_id: u32, controller_index: u32) -> Result<LayoutCatalog> {
        let progress = fill(LIST_PROGRESS_JS, app_id, 0, &[]);
        let started = Instant::now();
        let mut last: Option<u64> = None;
        let mut stable = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let p = self.eval_json(&progress)?;
            let n = p.get("n").and_then(Value::as_u64).unwrap_or(0);
            let complete = p.get("complete").and_then(Value::as_bool).unwrap_or(false);
            if complete && last == Some(n) {
                stable += 1;
            } else {
                stable = 0;
                last = Some(n);
            }
            if list_poll_finished(stable, started.elapsed())? {
                break;
            }
        }
        let template_label = serde_json::Value::String(steam_assets::i18n::tr("controller.template"));
        let date_locale = if steam_assets::i18n::current().code() == "ru" {
            "navigator.languages"
        } else {
            "'en'"
        };
        let expr = fill(LIST_JS, app_id, controller_index, &[])
            .replace("{TEMPLATE_LABEL}", &template_label.to_string())
            .replace("{DATE_LOCALE}", date_locale);
        let read = || parse_layout_catalog(&self.eval_json(&expr)?, app_id);
        let mut catalog = read()?;
        for _ in 0..5 {
            if !catalog.entries.iter().any(|e| e.author_account_id.is_some() && e.author_name.is_empty()) { break; }
            std::thread::sleep(Duration::from_millis(200));
            catalog = read()?;
        }
        if catalog.entries.iter().any(|e| e.app_id != app_id && e.app_id != 0 && e.sections & !16 != 0) {
            return Err(Error::Protocol("layout list holds another app's entries".into()));
        }
        Ok(catalog)
    }

    /// The active layout for an app on a controller (read-only).
    pub fn layout_active(&self, app_id: u32, controller_index: u32) -> Result<Option<ActiveLayout>> {
        self.read_active_layout(app_id, controller_index, false)
    }

    /// The active card's persona or developer, resolved from the same native response.
    pub fn layout_active_with_author(&self, app_id: u32, controller_index: u32) -> Result<Option<ActiveLayout>> {
        self.read_active_layout(app_id, controller_index, true)
    }

    fn read_active_layout(&self, app_id: u32, controller_index: u32, author: bool) -> Result<Option<ActiveLayout>> {
        let expr = fill(ACTIVE_JS, app_id, controller_index, &[("{WITH_AUTHOR}", author.to_string())]);
        parse_active_layout(&self.eval_json(&expr)?)
    }

    /// Select a layout for an app on a controller. One call, no retry; read the result back with
    /// [`Bridge::layout_active`].
    pub fn layout_select(&self, app_id: u32, controller_index: u32, url: &str, selection_type: u32) -> Result<()> {
        self.eval_json_once(&select_expr(app_id, controller_index, url, selection_type)?)?;
        self.wait_for_active(app_id, controller_index, |active| active.url == url).map(|_| ())
    }

    fn wait_for_active(&self, app: u32, idx: u32, matches: impl Fn(&ActiveLayout) -> bool) -> Result<ActiveLayout> {
        for _ in 0..20 {
            if let Some(active) = self.layout_active(app, idx)?.filter(&matches) { return Ok(active); }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(Error::Protocol("Steam did not confirm the new selection".into()))
    }

    /// Clear the selection for an app on a controller. One call, no retry.
    pub fn layout_clear(&self, app_id: u32, controller_index: u32, selection_type: u32) -> Result<()> {
        self.eval_json_once(&clear_expr(app_id, controller_index, selection_type))?;
        Ok(())
    }

    /// Delete one of the user's own layouts (stock call, one attempt). Refuses non-personal URLs.
    pub fn layout_delete_personal(&self, app_id: u32, controller_index: u32, url: &str, _selected_type: Option<u32>) -> Result<()> {
        check_personal_url(url)?;
        let selected_type = self.layout_active(app_id, controller_index)?.filter(|active| active.url == url).map(|active| active.selection_type);
        self.eval_json_once(&delete_personal_expr(app_id, controller_index, url, selected_type)?)?;
        Ok(())
    }

    /// Make an own copy of a template (one attempt); read the result back with [`Bridge::layout_active`].
    pub fn layout_create_from_template(&self, app_id: u32, controller_index: u32, template_url: &str) -> Result<()> {
        if self.layout_active(app_id, controller_index)?.is_some_and(|active| active.url.starts_with("autosave://")) {
            return Err(Error::Protocol("Refusing to replace the active personal copy".into()));
        }
        self.eval_json_once(&create_from_template_expr(app_id, controller_index, template_url)?)?;
        self.wait_for_active(app_id, controller_index, |active| active.url == template_url)?;
        self.layout_create_from_current(app_id, controller_index, Some(template_url))
    }

    pub fn layout_create_from_current(&self, app: u32, idx: u32, expected_url: Option<&str>) -> Result<()> {
        let active = self.layout_active(app, idx)?.ok_or_else(|| Error::Protocol("No active layout".into()))?;
        if expected_url.is_some_and(|url| url != active.url) || active.url.starts_with("autosave://") {
            return Err(Error::Protocol("The current layout changed or is already personal".into()));
        }
        self.eval_json_once(&create_current_expr(app, idx, &active.url)?)?;
        self.wait_for_active(app, idx, |active| active.selection_type == 1 && check_personal_url(&active.url).is_ok()).map(|_| ())
    }

    /// Export the active layout as a new personal local one under `title` (stock «Сохранить как…», one attempt, no cloud).
    /// Success means the re-read list holds a personal entry with that title.
    pub fn layout_export_local(&self, app: u32, idx: u32, title: &str, description: &str) -> Result<()> {
        let (title, description) = check_export_text(title, description)?;
        let active = self.layout_active(app, idx)?.ok_or_else(|| Error::Protocol("No active layout".into()))?;
        self.eval_json_once(&export_local_expr(app, idx, &active.url, &title, &description)?)?;
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(500));
            if self.layout_list(app, idx, false)?.iter().any(|e| e.sections & 8 != 0 && e.title == title) { return Ok(()); }
        }
        Err(Error::Protocol("Steam did not confirm the exported layout".into()))
    }

    /// Download one controller layout via Steam's Workshop store without selecting it.
    pub fn layout_download(&self, published_id: u64) -> Result<()> {
        if published_id == 0 { return Err(Error::Protocol("Missing Workshop layout id".into())); }
        let downloaded = format!("(async()=> (await SteamClient.Apps.GetDownloadedWorkshopItems(241100)).some(item=>String(item.publishedfileid)==='{published_id}'))()");
        if self.eval_json(&downloaded)?.as_bool() == Some(true) { return Ok(()); }
        self.eval_json_once(&format!("(()=>{{if(!window.workshopStore.BConnectedToSteam())throw Error('Steam is offline');window.workshopStore.Download(241100,'{published_id}');return true}})()"))?;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(250));
            if self.eval_json(&downloaded)?.as_bool() == Some(true) { return Ok(()); }
        }
        Err(Error::Protocol(tr_ref("controller.steam_did_not_confirm_the_layout_download").into()))
    }

    /// Details of community items (read-only). At most 50 ids per call.
    pub fn layout_details(&self, ids: &[u64]) -> Result<Vec<LayoutDetails>> {
        parse_layout_details(&self.eval_json(&details_expr(ids)?)?)
    }

    /// Submit the same Workshop vote as the stock chooser through its vote module.
    pub fn layout_vote(&self, published_id: u64, up: bool) -> Result<()> {
        if published_id == 0 {
            return Err(Error::Protocol("Missing Workshop layout id".into()));
        }
        // GU is the underlying request used by VoteOnPublishedFile in sp.js. Readback
        // uses GetUserVoteSummary and GetDetails, avoiding the store's optimistic counters.
        let direction = if up { 1 } else { 2 };
        let expr = format!(r#"(async () => {{
            if (!window.workshopStore.BConnectedToSteam()) throw Error('Steam is offline');
            {WEBPACK_JS}
            const id = Object.keys(__mods).find(id => {{
                const source = __mods[id].toString();
                return source.includes('Invalid direction') && source.includes('sharedfiles/') && source.includes('voteup');
            }});
            if (!id) throw Error('Workshop voting module is unavailable');
            return await __req(id).GU('{published_id}', {direction});
        }})()"#);
        let result = self.eval_json_once(&expr)?;
        if result.as_u64() != Some(1) && result.as_str() != Some("1") {
            return Err(Error::Protocol(format!("Steam rejected the Workshop vote: {result}")));
        }
        Ok(())
    }

    /// The current user's votes on items (read-only).
    pub fn layout_user_votes(&self, ids: &[u64]) -> Result<Vec<UserVote>> {
        parse_user_votes(&self.eval_json(&user_votes_expr(ids)?)?)
    }
}

// ---------------------------------------------------------------------------------------------
// Tests (fixtures are real replies from the Deck, trimmed; see tests/fixtures/controller_layouts/)
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn personal_url_and_expressions_are_strict() {
        assert!(check_personal_url("autosave://x/2379780/FVZZ4470018B.vdf").is_ok());
        for bad in ["template://a.vdf", "workshop://1", "autosave://../x.vdf", "autosave://", "autosave://a b.vdf", "file:///etc/x.vdf"] {
            assert!(check_personal_url(bad).is_err(), "{bad}");
            assert!(delete_personal_expr(1, 15, bad, None).is_err());
        }
        let e = delete_personal_expr(2379780, 15, "autosave://a/b.vdf", Some(1)).unwrap();
        assert!(e.contains("DeletePersonalControllerConfiguration") && e.contains("AddDeletedConfig") && e.contains("ClearSelectedConfigForApp(2379780,15,1)"));
        assert!(!delete_personal_expr(2379780, 15, "autosave://a/b.vdf", None).unwrap().contains("ClearSelected"));
        assert!(create_from_template_expr(1, 15, "workshop://5").is_err());
        let c = create_from_template_expr(2379780, 15, "template://controller_neptune_gamepad_joystick.vdf").unwrap();
        assert!(c.contains("SetSelectedConfigForApp(2379780,15,"));
        let current = create_current_expr(2379780, 15, "workshop://5").unwrap();
        assert!(current.contains("LoadEditingConfiguration(2379780,15)") && current.contains("ConvertConfigToSelectionType(2379780,1)"));
        assert!(select_expr(1, 15, "autosave://a/b.vdf", 1).unwrap().contains("false,1"));
        let export = export_local_expr(2379780, 15, "template://a.vdf", " My \"pad\" ", "").unwrap();
        assert!(export.contains("ExportEditingConfiguration(2379780,1,\"My \\\"pad\\\"\",\"\",\"\")"), "{export}");
        assert!(export.contains("LoadEditingConfiguration(2379780,15)"));
        assert!(export_local_expr(1, 15, "x", "  ", "").is_err());
        assert!(export_local_expr(1, 15, "x", &"a".repeat(65), "").is_err());
        assert!(export_local_expr(1, 15, "x", "a\nb", "").is_err());
        assert!(export_local_expr(1, 15, "x", "a", &"d".repeat(513)).is_err());
        assert!(select_expr(1, 15, "autosave://../x.vdf", 1).is_err());
    }

    #[test]
    fn offline_catalog_has_each_requested_tab_without_account_ids() {
        let rows = offline_catalog().unwrap();
        let en = demo_layouts(steam_assets::i18n::by_code("en").unwrap()).unwrap();
        assert_eq!(en[0].title, "Gamepad");
        assert!(en[0].description.starts_with("This template"));
        for section in [2, 4, 8] { assert!(rows.iter().any(|row| row.sections & section != 0)); }
        assert!(rows.iter().all(|row| row.author_account_id.is_none() && row.author_name.is_empty()));
    }

    use super::*;
    use serde_json::json;

    const LIST: &str = include_str!("../tests/fixtures/controller_layouts/list_balatro_deck.json");
    const DETAILS: &str = include_str!("../tests/fixtures/controller_layouts/details_balatro_deck.json");
    const VOTES: &str = include_str!("../tests/fixtures/controller_layouts/votes_balatro_deck.json");
    const ACTIVE_OFFICIAL: &str = include_str!("../tests/fixtures/controller_layouts/active_official_deck.json");
    const ACTIVE_COMMUNITY: &str = include_str!("../tests/fixtures/controller_layouts/active_community_deck.json");

    fn list() -> Vec<LayoutEntry> {
        parse_layout_list(&serde_json::from_str(LIST).unwrap()).unwrap()
    }

    #[test]
    fn chooser_projection_keeps_persona_units_and_membership() {
        let mut rows: Value = serde_json::from_str(LIST).unwrap();
        let row = &mut rows[0];
        row["authorName"] = json!("Layout author");
        row["gameName"] = json!("Papers, Please");
        row["sections"] = json!(20); // workshop and global Search
        row["searchOrder"] = json!(7);
        row["descriptionLines"] = json!(2);
        row["downloaded"] = json!(true);
        row["playtime"] = json!("60703"); // Steam's value is already hours
        let parsed = parse_layout_list(&rows).unwrap();
        let row = &parsed[0];
        assert_eq!((&*row.author_name, &*row.game_name), ("Layout author", "Papers, Please"));
        assert_eq!((row.sections, row.search_order, row.description_lines), (20, 7, 2));
        assert!(row.downloaded);
        assert_eq!(row.playtime_seconds, 60703);
        // Backward-compatible raw fixtures have no projected memberships or invented name.
        let raw = list();
        assert_eq!(raw[0].sections, 0);
        assert!(raw[0].author_name.is_empty());
    }

    #[test]
    fn chooser_offline_keeps_downloaded_workshop_and_personal_memberships() {
        let rows: Value = serde_json::from_str(LIST).unwrap();
        let mut personal = rows[0].clone();
        personal["sections"] = json!(8 | 16);
        personal["personalOrder"] = json!(4);
        personal["owned"] = json!(true);
        let mut installed = rows[3].clone();
        installed["sections"] = json!(4 | 16);
        installed["downloaded"] = json!(true);
        let mut remote = rows[4].clone();
        remote["sections"] = json!(4 | 16);
        let mut catalog = json!({"connected": false, "rows": [personal, installed, remote]});

        let local = parse_layout_catalog(&catalog, 2379780).unwrap();
        assert!(!local.connected);
        assert_eq!(local.entries.iter().map(|e| e.sections).collect::<Vec<_>>(), [24, 20, 16]);
        assert_eq!(local.entries[0].source, LayoutSource::User);
        assert_eq!(local.entries[0].personal_order, 4);
        assert!(local.entries[0].owned);

        catalog["connected"] = json!(true);
        let online = parse_layout_catalog(&catalog, 2379780).unwrap();
        assert!(online.connected);
        assert_eq!(online.entries[2].sections, 20);
        for app in [241100, 413080, 413090, 413100, 443510, 769] {
            let system = parse_layout_catalog(&catalog, app).unwrap();
            assert!(!community_allowed(app));
            assert!(system.entries.iter().all(|e| e.sections & 4 == 0));
        }
    }

    #[test]
    fn list_timeout_is_not_a_successful_partial_result() {
        assert!(!list_poll_finished(0, Duration::from_secs(12)).unwrap());
        assert!(matches!(list_poll_finished(0, Duration::from_millis(12_001)), Err(Error::Timeout)));
        assert!(list_poll_finished(5, Duration::from_millis(12_001)).unwrap());
    }

    #[test]
    fn parses_real_list_fields() {
        let l = list();
        assert_eq!(l.len(), 6);
        let official = l.iter().find(|e| e.url == "template://controller_neptune_gamepad_joystick.vdf").unwrap();
        assert_eq!(official.source, LayoutSource::Official);
        // The list sends "0" for the official template; its id (4) appears only in the active reply.
        assert_eq!(official.published_file_id, None);
        assert_eq!(official.controller_type, 4);
        // Steam reports bUsesGamepad=false in the list for this entry (the active reply has bUsesGamepad=true).
        assert!(!official.uses_gamepad);
        let community = l.iter().find(|e| e.url == "workshop://3177015367").unwrap();
        assert_eq!(community.source, LayoutSource::Community);
        assert_eq!(community.published_file_id, Some(3177015367));
        assert_eq!(community.title, "Steam Deck - Sort Hand / Deselect");
        assert_eq!(community.time_updated, 1710041184);
        assert_eq!(community.playtime_seconds, 5871);
        let template = l.iter().find(|e| e.url == "template://controller_neptune_wasd.vdf").unwrap();
        assert_eq!(template.source, LayoutSource::Template);
        assert_eq!(template.published_file_id, None);
    }

    #[test]
    fn real_list_has_the_expected_sections() {
        let l = list();
        let count = |s| l.iter().filter(|e| e.source == s).count();
        assert_eq!(count(LayoutSource::Official), 1);
        assert_eq!(count(LayoutSource::Template), 2);
        assert_eq!(count(LayoutSource::Community), 3);
    }

    #[test]
    fn search_matches_title_and_description_case_insensitively() {
        let ru = steam_assets::i18n::by_code("ru").unwrap();
        let l = demo_layouts(ru).unwrap();
        let q = LayoutQuery { text: "  SORT ".into(), ..Default::default() };
        let hits = select_layouts(&l, &q);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "workshop://3177015367");
        // Description match (the official layout's description says «геймпад»).
        let q = LayoutQuery { text: ru.tr("controller.gamepad"), ..Default::default() };
        assert!(select_layouts(&l, &q).iter().any(|e| e.source == LayoutSource::Official));
        assert_eq!(select_layouts(&l, &LayoutQuery { text: "no such layout".into(), ..Default::default() }).len(), 0);
    }

    #[test]
    fn tab_filter_and_default_order() {
        let l = list();
        let all = select_layouts(&l, &LayoutQuery::default());
        let kinds: Vec<LayoutSource> = all.iter().map(|e| e.source).collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        assert_eq!(kinds, sorted, "default order: official, templates, community");
        let c = select_layouts(&l, &LayoutQuery { source: SourceFilter::Community, ..Default::default() });
        assert!(c.iter().all(|e| e.source == LayoutSource::Community));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn sort_by_title_playtime_and_updated() {
        let l = list();
        let by_title = select_layouts(&l, &LayoutQuery { sort: LayoutSort::Title, ..Default::default() });
        let titles: Vec<String> = by_title.iter().map(|e| e.title.to_lowercase()).collect();
        let mut t2 = titles.clone();
        t2.sort();
        assert_eq!(titles, t2);
        let by_play = select_layouts(&l, &LayoutQuery { sort: LayoutSort::Playtime, ..Default::default() });
        assert!(by_play.windows(2).all(|w| w[0].playtime_seconds >= w[1].playtime_seconds));
        let by_upd = select_layouts(&l, &LayoutQuery { sort: LayoutSort::Updated, ..Default::default() });
        assert!(by_upd.windows(2).all(|w| w[0].time_updated >= w[1].time_updated));
    }

    #[test]
    fn active_layout_marks_the_matching_entry_only() {
        let l = list();
        let active = parse_active_layout(&serde_json::from_str(ACTIVE_COMMUNITY).unwrap()).unwrap().unwrap();
        assert_eq!(active.url, "workshop://3177015367");
        assert_eq!(active.published_file_id, Some(3177015367));
        assert_eq!(active.selection_type, 0);
        assert!(active.enabled);
        let marked: Vec<&str> = l.iter().filter(|e| is_active(e, Some(&active))).map(|e| e.url.as_str()).collect();
        assert_eq!(marked, vec!["workshop://3177015367"]);
        // The list's own bSelected flag stays false after a select on the Deck, so it is not used.
    }

    #[test]
    fn active_official_reply_and_null() {
        let a = parse_active_layout(&serde_json::from_str(ACTIVE_OFFICIAL).unwrap()).unwrap().unwrap();
        assert_eq!(a.url, "template://controller_neptune_gamepad_joystick.vdf");
        assert_eq!(a.published_file_id, Some(4));
        assert!(a.author_name.is_empty());
        let mut named: Value = serde_json::from_str(ACTIVE_OFFICIAL).unwrap();
        named["authorName"] = json!("Layout developer");
        assert_eq!(parse_active_layout(&named).unwrap().unwrap().author_name, "Layout developer");
        assert!(parse_active_layout(&Value::Null).unwrap().is_none());
        assert!(matches!(parse_active_layout(&json!({"URL": 5})), Err(Error::Protocol(_))));
    }

    #[test]
    fn list_rejects_missing_or_mistyped_fields() {
        let bad = json!([{"URL":"template://a.vdf","Title":"x","appID":2379780,"nControllerType":4,"bOfficial":false}]);
        assert!(matches!(parse_layout_list(&bad), Err(Error::Protocol(_))), "bRecommended missing");
        let bad = json!([{"URL":"template://a.vdf","Title":"x","appID":"2379780","nControllerType":4,"bOfficial":false,"bRecommended":false}]);
        assert!(matches!(parse_layout_list(&bad), Err(Error::Protocol(_))), "appID must be a number");
        assert!(matches!(parse_layout_list(&json!({})), Err(Error::Protocol(_))));
    }

    #[test]
    fn details_parse_real_reply_and_keep_votes() {
        let v: Value = serde_json::from_str(DETAILS).unwrap();
        let d = parse_layout_details(&v).unwrap();
        assert_eq!(d.len(), 2);
        let first = d.iter().find(|x| x.published_file_id == 3417625332).unwrap();
        assert_eq!(first.result, 1);
        assert_eq!(first.title, "Sample Steam Controller Balatro Bindings");
        assert_eq!(first.consumer_appid, 241100);
        assert_eq!(first.file_type, 12);
        assert_eq!(first.file_size, 15661);
        let votes = first.votes.unwrap();
        assert_eq!((votes.up, votes.down), (13, 4));
        assert!((votes.score - 0.5384615659713745).abs() < 1e-12);
        assert!(first.description.starts_with("Based on the official Balatro layout"));
    }

    #[test]
    fn details_error_reply_is_an_error() {
        let v = json!({"eresult": 8, "body": {"publishedfiledetails": []}});
        assert!(matches!(parse_layout_details(&v), Err(Error::Protocol(_))));
    }

    #[test]
    fn user_votes_parse_real_reply() {
        let v: Value = serde_json::from_str(VOTES).unwrap();
        let votes = parse_user_votes(&v).unwrap();
        assert_eq!(votes.len(), 2);
        assert!(votes.iter().all(|x| x.vote.is_none() && !x.reported));
        let up = json!({"eresult":1,"body":{"summaries":[{"publishedfileid":"5","vote_for":true,"vote_against":false,"reported":false}]}});
        assert_eq!(parse_user_votes(&up).unwrap()[0].vote, Some(true));
    }

    #[test]
    fn layout_url_check_refuses_script_and_paths() {
        assert!(check_layout_url("template://controller_neptune_gamepad_joystick.vdf").is_ok());
        assert!(check_layout_url("template://controller_neptune_gamepad+mouse.vdf").is_ok());
        assert!(check_layout_url("workshop://3177015367").is_ok());
        for bad in [
            "workshop://3177015367\"); alert(1); //",
            "workshop://abc",
            "workshop://",
            "template://../../x.vdf",
            "template://name_without_ext",
            "http://example.com/x.vdf",
            "template://a.vdf'",
        ] {
            assert!(check_layout_url(bad).is_err(), "must refuse {bad:?}");
        }
    }

    #[test]
    fn select_and_clear_expressions_carry_the_stock_arguments() {
        let e = select_expr(2379780, 15, "workshop://3177015367", 0).unwrap();
        assert!(e.contains("SetSelectedConfigForApp(2379780,15,\"workshop://3177015367\",false,0)"), "{e}");
        let c = clear_expr(2379780, 15, 0);
        assert!(c.contains("ClearSelectedConfigForApp(2379780,15,0)"), "{c}");
        assert!(c.contains("ClearSelectedConfigCache(2379780)"));
        assert!(select_expr(2379780, 15, "workshop://1\"", 0).is_err());
    }

    #[test]
    fn details_expression_embeds_only_validated_ids() {
        let e = details_expr(&[3417625332, 3177015367]).unwrap();
        assert!(e.contains(r#"["3417625332","3177015367"]"#));
        assert!(e.contains("CPublishedFile_GetDetails_Request"));
        assert!(details_expr(&[]).is_err());
        let too_many: Vec<u64> = (1..=51).collect();
        assert!(details_expr(&too_many).is_err());
    }
}
