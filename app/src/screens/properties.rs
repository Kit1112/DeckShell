//! Game Properties (gamepad route /routes/app/<id>/properties/<page>), ported from the sp.js AppProperties component (pages list at
//! sp.js:519280, General 519470, Compatibility 26981, Updates 519995, Installed files 520030, Betas 520340, Controller 520371, DLC 520583,
//! Shortcut 520807, Customization 519240, Privacy 518800, Recording 519020). Left rail = pages that exist for the app (Steam game vs non-Steam
//! shortcut, installed / DLC / betas present), right pane = flowed fields. Values are read from `SteamClient.Apps.RegisterForAppDetails`
//! through the bridge; every write is the same `SteamClient.Apps.*` setter the native page calls (DRYRUN-gated by `setters::write`) and is
//! applied optimistically to the cached details so the page reflects it at once.
//! This module also owns the modal layer shared with the game menu: list popup (drop-down / file picker), text prompt + on-screen keyboard,
//! confirm dialog (`prompt`, `confirm`).
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, VecModel};
use steam_data::GameKind;

use crate::actions::Act;
use crate::app::{fmt_size, App};
use crate::assets::{t_or, tf};
use crate::input::Action;
use crate::setters::{self, js_str};
use crate::{AppWindow, PField, PropsState};

static OPEN: AtomicBool = AtomicBool::new(false);
static SIG: AtomicI64 = AtomicI64::new(0);
static MODE: AtomicI64 = AtomicI64::new(0);

pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}
/// Extra slot of the navigation-sound signature.
pub fn sig() -> i64 {
    SIG.load(Ordering::Relaxed)
}

type HintRow = (&'static str, &'static str, i32, &'static str, Action);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    General,
    Compat,
    Updates,
    LocalFiles,
    Betas,
    Controller,
    Dlc,
    Recording,
    Privacy,
    Custom,
    Shortcut,
}

impl Tab {
    fn title(self) -> String {
        let (tok, fb) = match self {
            Tab::General => ("#AppProperties_GeneralPage", "Общие"),
            Tab::Compat => ("#AppProperties_CompatibilityPage", "Совместимость"),
            Tab::Updates => ("#AppProperties_UpdatesPage", "Обновления"),
            Tab::LocalFiles => ("#AppProperties_LocalFilesPage", "Установленные файлы"),
            Tab::Betas => ("#AppProperties_BetasPage", "Версии и бета-версии"),
            Tab::Controller => ("#AppProperties_ControllerPage", "Контроллер"),
            Tab::Dlc => ("#AppProperties_DLCPage", "Доп. контент"),
            Tab::Recording => ("#AppProperties_GameRecording", "Запись игры"),
            Tab::Privacy => ("#AppProperties_PrivacyPage", "Конфиденциальность"),
            Tab::Custom => ("#AppProperties_Customization", "Кастомизация"),
            Tab::Shortcut => ("#AppProperties_ShortcutPage", "Ярлык"),
        };
        t_or(tok, fb)
    }
    fn from_route(s: &str) -> Option<Tab> {
        Some(match s {
            "general" => Tab::General,
            "compatibility" => Tab::Compat,
            "updates" => Tab::Updates,
            "localfiles" => Tab::LocalFiles,
            "betas" => Tab::Betas,
            "controller" => Tab::Controller,
            "dlc" => Tab::Dlc,
            "gamerecording" => Tab::Recording,
            "privacy" => Tab::Privacy,
            "customization" => Tab::Custom,
            "shortcut" => Tab::Shortcut,
            _ => return None,
        })
    }
}

/// What a field does when activated.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fid {
    None,
    Overlay,
    Lang,
    Res,
    Cloud,
    CloudSuspend,
    LaunchOpts,
    CompatTool,
    ShowAll,
    AutoUpdate,
    BgDl,
    Browse,
    Backup,
    Restore,
    Verify,
    Beta,
    BetaCode,
    SteamInput,
    Rumble,
    DesktopCfg,
    CtrlConfig,
    Dlc(u32),
    DlcStore,
    Hidden,
    Private,
    SortAs,
    SName,
    SExe,
    SDir,
    SOpts,
    SIcon,
    SVr,
    RecGlobal,
    RecEnable,
    ArtChange,
}

#[derive(Clone, Debug)]
struct F {
    kind: i32,
    label: String,
    desc: String,
    value: String,
    btn: String,
    caption: String,
    on: bool,
    dis: bool,
    id: Fid,
}

impl F {
    fn new(kind: i32, id: Fid, label: impl Into<String>) -> F {
        F { kind, label: label.into(), desc: String::new(), value: String::new(), btn: String::new(), caption: String::new(), on: false, dis: false, id }
    }
    fn desc(mut self, d: impl Into<String>) -> F {
        self.desc = d.into();
        self
    }
    fn val(mut self, v: impl Into<String>) -> F {
        self.value = v.into();
        self
    }
    fn btn(mut self, b: impl Into<String>) -> F {
        self.btn = b.into();
        self
    }
    fn cap(mut self, c: impl Into<String>) -> F {
        self.caption = c.into();
        self
    }
    fn on(mut self, o: bool) -> F {
        self.on = o;
        self
    }
    fn dis(mut self, d: bool) -> F {
        self.dis = d;
        self
    }
    fn focusable(&self) -> bool {
        matches!(self.kind, 2..=5 | 7) && !self.dis
    }
}

#[derive(Clone, Debug)]
enum PopupKind {
    /// drop-down: values parallel to `items`
    Opt(Fid, Vec<String>),
    /// file picker (dir = current directory, pick_dir = choose a folder, fid = destination field)
    Files { dir: PathBuf, pick_dir: bool, fid: Fid },
}

struct Popup {
    title: String,
    sub: String,
    items: Vec<String>,
    idx: usize,
    cur: i32,
    kind: PopupKind,
    /// file picker entries (parallel to `items`): (path, is_dir)
    entries: Vec<(PathBuf, bool)>,
}

type TextCb = Box<dyn FnOnce(&mut App, String)>;
type OkCb = Box<dyn FnOnce(&mut App)>;

struct Prompt {
    title: String,
    text: String,
    cb: Option<TextCb>,
}

struct Dlg {
    title: String,
    text: String,
    btns: Vec<String>,
    idx: usize,
    cb: Option<OkCb>,
}

struct St {
    open: bool,
    app_id: u32,
    shortcut: bool,
    installed: bool,
    name: String,
    d: Value,
    loading: bool,
    tabs: Vec<Tab>,
    tab: usize,
    zone: i32,
    fields: Vec<F>,
    laid: Vec<(f32, f32)>,
    content_h: f32,
    fidx: i32,
    fsub: i32,
    popup: Option<Popup>,
    prompt: Option<Prompt>,
    dlg: Option<Dlg>,
    show_all: bool,
    status: Option<String>,
    gen: u64,
}

impl St {
    fn new() -> St {
        St {
            open: false,
            app_id: 0,
            shortcut: false,
            installed: false,
            name: String::new(),
            d: Value::Null,
            loading: false,
            tabs: vec![],
            tab: 0,
            zone: 0,
            fields: vec![],
            laid: vec![],
            content_h: 0.0,
            fidx: -1,
            fsub: 0,
            popup: None,
            prompt: None,
            dlg: None,
            show_all: false,
            status: None,
            gen: 0,
        }
    }
}

thread_local! { static ST: &'static RefCell<St> = Box::leak(Box::new(RefCell::new(St::new()))); }
fn st() -> std::cell::RefMut<'static, St> {
    ST.with(|s| s.borrow_mut())
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    super::defer(f);
}

pub fn wire(ui: &AppWindow) {
    let s = ui.global::<PropsState>();
    s.on_rail_clicked(|i| post(move |a| {
        {
            let mut s = st();
            s.zone = 0;
        }
        select_tab(a, i as usize);
    }));
    s.on_field_clicked(|i, sub| post(move |a| {
        {
            let mut s = st();
            s.zone = 1;
            s.fidx = i;
            s.fsub = sub;
        }
        activate(a);
    }));
    s.on_popup_clicked(|i| post(move |a| {
        {
            let mut s = st();
            if let Some(p) = s.popup.as_mut() {
                p.idx = i as usize;
            }
        }
        popup_activate(a);
    }));
    s.on_popup_dismiss(|| post(|a| {
        st().popup = None;
        sync(a);
    }));
    s.on_dlg_clicked(|i| post(move |a| {
        if let Some(d) = st().dlg.as_mut() {
            d.idx = i as usize;
        }
        dlg_activate(a);
    }));
    s.on_back(|| post(|a| {
        handle(a, Action::B);
    }));
}

// ---------------------------------------------------------------------------------------------------------- bridge reads / writes

/// Everything the pages need, from AppDetails + a few stores (sp.js field names kept).
fn read_js(id: u32) -> String {
    format!(
        "(async()=>{{const id={id};const o=appStore.GetAppOverviewByAppID(id);if(!o)return null;\
const d=await new Promise(res=>{{let h;const t=setTimeout(()=>res(null),2500);h=SteamClient.Apps.RegisterForAppDetails(id,x=>{{clearTimeout(t);setTimeout(()=>h&&h.unregister(),0);res(x)}})}});\
if(!d)return null;const r={{}};\
['strDisplayName','strLaunchOptions','strResolutionOverride','bOverrideInternalResolution','vecLanguages','selectedLanguage','eAutoUpdateValue','eBackgroundDownloads','bCloudEnabledForAccount','bCloudAvailable','strCloudBytesUsed','strCloudBytesAvailable','bCloudEnabledForApp','bCloudSyncOnSuspendAvailable','bCloudSyncOnSuspendEnabled','lDiskUsageBytes','lDlcUsageBytes','nBuildID','rtLastUpdated','strInstallFolder','bIsThirdPartyUpdater','bHasAnyLocalContent','bCanMoveInstallFolder','bOverlayEnabled','bWorkshopVisible','bHasLockedPrivateBetas','strSelectedBeta','eEnableThirdPartyControllerConfiguration','bEnableAllowDesktopConfiguration','eControllerRumbleIntensity','strCompatToolName','strCompatToolDisplayName','strShortcutExe','strShortcutStartDir','strShortcutLaunchOptions','bShortcutIsVR','bHasDifferentCopies','bIsExcludedFromSharing','bIsSubscribedTo','unAppID'].forEach(k=>{{try{{if(d[k]!==undefined)r[k]=JSON.parse(JSON.stringify(d[k]))}}catch(e){{}}}});\
r.vecBetas=(d.vecBetas||[]).map(b=>({{strName:b.strName,strDescription:b.strDescription,rtLastUpdated:b.rtLastUpdated}}));\
r.vecDLC=(d.vecDLC||[]).map(x=>({{unAppID:x.unAppID,strName:x.strName,bEnabled:x.bEnabled,lDiskUsageBytes:x.lDiskUsageBytes,strState:x.strState,rtPurchaseDate:x.rtPurchaseDate}}));\
r.app_type=o.app_type;r.is_shortcut=!!o.BIsShortcut();\
try{{r.hidden=!!collectionStore.BIsHidden(id)}}catch(e){{}}\
try{{r.sort_as=(o.BIsShortcut()?o.sort_as:appStore.GetCustomSortAs(id))||''}}catch(e){{r.sort_as=''}}\
try{{const s=settingsStore.settings;r.overlay_global=s.enable_overlay;r.rec_mode=s.gamerecording_background_mode;r.show_all=s.compat_show_all_tools}}catch(e){{}}\
return r}})()"
    )
}

fn refresh(id: u32) {
    let gen = {
        let mut s = st();
        s.loading = true;
        s.gen += 1;
        s.gen
    };
    setters::read(read_js(id), move |a, v| {
        {
            let mut s = st();
            if s.gen != gen || s.app_id != id {
                return;
            }
            s.loading = false;
            if v.is_object() {
                s.d = v;
            }
            if s.name.is_empty() {
                s.name = s_of(&s.d, "strDisplayName");
            }
            if let Some(b) = s.d["show_all"].as_bool() {
                s.show_all = b;
            }
        }
        rebuild(a, false);
    });
}

/// Setter write + optimistic update of the cached details (`key` = AppDetails property to patch) + a re-read when really connected.
fn write(a: &mut App, tag: &str, js: String, patch: Option<(&str, Value)>) {
    setters::write(tag, js);
    let id = {
        let mut s = st();
        if let Some((k, v)) = patch {
            if !s.d.is_object() {
                s.d = json!({});
            }
            s.d[k] = v;
        }
        s.app_id
    };
    if !crate::actions::dryrun() && crate::actions::bridge().is_some() {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(900));
            post(move |_| {
                if st().open && st().app_id == id {
                    refresh(id);
                }
            });
        });
    }
    rebuild(a, false);
}

fn apps(call: &str, id: u32, args: &str) -> String {
    if args.is_empty() {
        format!("SteamClient.Apps.{call}({id})")
    } else {
        format!("SteamClient.Apps.{call}({id},{args})")
    }
}

// ---------------------------------------------------------------------------------------------------------- open / close

pub fn open(a: &mut App, id: u32, page: Option<&str>) {
    // games hidden from the library list still have properties: fall back to a Steam app, the name comes from AppDetails
    let g = match a.gidx.get(&id) {
        Some(&gi) => a.games()[gi].clone(),
        None => steam_data::Game { app_id: id, kind: GameKind::Steam, name: String::new(), sort_name: String::new(), installed: true, install_dir: None, size_on_disk: 0, playtime_minutes: 0, last_played: None, update_pending: false, collections: vec![], art: Default::default() },
    };
    if a.sm.open {
        a.sm.open = false;
    }
    {
        let mut s = st();
        *s = St::new();
        s.open = true;
        s.app_id = id;
        s.shortcut = g.kind == GameKind::Shortcut;
        s.installed = g.installed;
        s.name = g.name.clone();
        s.zone = if page.is_some() { 1 } else { 0 };
        s.show_all = false;
    }
    rebuild_tabs(page.and_then(Tab::from_route));
    rebuild(a, true);
    refresh(id);
}

pub fn close(a: &mut App) {
    {
        let mut s = st();
        if !s.open && s.popup.is_none() && s.prompt.is_none() && s.dlg.is_none() {
            return;
        }
        s.open = false;
        s.popup = None;
        s.prompt = None;
        s.dlg = None;
        s.gen += 1;
    }
    sync(a);
}

fn rebuild_tabs(want: Option<Tab>) {
    let mut s = st();
    let cur = want.or_else(|| s.tabs.get(s.tab).copied());
    let d = s.d.clone();
    let has = |k: &str| d[k].as_array().map(|v| !v.is_empty()).unwrap_or(false);
    let mut t = vec![];
    if s.shortcut {
        t.extend([Tab::Shortcut, Tab::Compat, Tab::Controller, Tab::Recording, Tab::Custom]);
    } else {
        t.extend([Tab::General, Tab::Compat, Tab::Updates]);
        let local = if d.is_object() { d["bHasAnyLocalContent"].as_bool().unwrap_or(false) } else { s.installed };
        if local {
            t.push(Tab::LocalFiles);
        }
        if has("vecBetas") || d["bHasLockedPrivateBetas"].as_bool().unwrap_or(false) {
            t.push(Tab::Betas);
        }
        t.push(Tab::Controller);
        if has("vecDLC") {
            t.push(Tab::Dlc);
        }
        t.extend([Tab::Recording, Tab::Privacy, Tab::Custom]);
    }
    s.tab = cur.and_then(|c| t.iter().position(|x| *x == c)).unwrap_or(0);
    s.tabs = t;
}

fn select_tab(a: &mut App, i: usize) {
    {
        let mut s = st();
        if i >= s.tabs.len() {
            return;
        }
        s.tab = i;
        s.fidx = -1;
        s.fsub = 0;
    }
    rebuild(a, true);
}

// ---------------------------------------------------------------------------------------------------------- pages

fn s_of(d: &Value, k: &str) -> String {
    d[k].as_str().unwrap_or("").to_string()
}
fn b_of(d: &Value, k: &str) -> bool {
    d[k].as_bool().unwrap_or(false)
}
fn i_of(d: &Value, k: &str) -> i64 {
    d[k].as_i64().unwrap_or(0)
}
fn quote(s: &str) -> String {
    if s.is_empty() { String::new() } else { s.to_string() }
}

fn lang_label(short: &str) -> String {
    let tok = format!("#Language_{short}");
    t_or(&tok, &short.to_string())
}

fn auto_update_opts() -> Vec<(String, String)> {
    vec![
        ("0".into(), t_or("#AppProperties_AutoUpdateUseDefault", "Использовать общие настройки")),
        ("1".into(), t_or("#AppProperties_AutoUpdateOnLaunch", "Обновлять при запуске")),
        ("2".into(), t_or("#AppProperties_AutoUpdateAlways", "Разрешить Steam выбирать время обновления")),
        ("3".into(), t_or("#AppProperties_AutoUpdateHighPriority", "Скачивать обновления сразу")),
    ]
}
fn bg_opts() -> Vec<(String, String)> {
    vec![
        ("0".into(), t_or("#AppProperties_BackgroundDownloadGlobal_Allow", "Разрешить фоновые загрузки во время игры")),
        ("1".into(), t_or("#AppProperties_BackgroundDownloadAlways", "Всегда разрешать фоновые загрузки")),
        ("2".into(), t_or("#AppProperties_BackgroundDownloadNever", "Всегда приостанавливать фоновые загрузки")),
    ]
}
fn input_opts() -> Vec<(String, String)> {
    vec![
        ("0".into(), t_or("#AppProperties_SteamInputOff", "Отключить систему ввода Steam")),
        ("1".into(), t_or("#AppProperties_SteamInputDefaultSettings", "Использовать настройки по умолчанию")),
        ("2".into(), t_or("#AppProperties_SteamInputOn", "Включить систему ввода Steam")),
    ]
}
fn rumble_opts() -> Vec<(String, String)> {
    let l = |n: &str, fb: &str| t_or(&format!("#Settings_Controller_RumbleIntensity_{n}"), fb);
    vec![
        ("0".into(), t_or("#AppProperties_RumbleIntensity_UseGlobal", "Использовать общие настройки")),
        ("16000".into(), l("ExtraLow", "Очень низкая")),
        ("8000".into(), l("Low", "Низкая")),
        ("1000".into(), l("MediumLow", "Ниже средней")),
        ("320".into(), l("Medium", "Средняя")),
        ("200".into(), l("MediumHigh", "Выше средней")),
        ("100".into(), l("High", "Высокая")),
        ("60".into(), l("ExtraHigh", "Очень высокая")),
    ]
}
fn res_opts(cur: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![("Default".into(), t_or("#Settings_Display_GameResolution_GlobalSetting", "Общие настройки"))];
    for r in ["1280x800", "1280x720", "1920x1080", "1920x1200", "960x600", "800x500", "640x400"] {
        v.push((r.into(), r.into()));
    }
    if cur != "Default" && !cur.is_empty() && !v.iter().any(|x| x.0 == cur) {
        v.push((cur.into(), cur.into()));
    }
    v
}
fn opt_label(opts: &[(String, String)], val: &str) -> String {
    opts.iter().find(|o| o.0 == val).map(|o| o.1.clone()).unwrap_or_else(|| val.to_string())
}

fn betas_opts(d: &Value) -> Vec<(String, String, String)> {
    let mut v = vec![("".to_string(), t_or("#AppProperties_Beta_SelectNoBeta", "Нет"), t_or("#AppProperties_Beta_SelectNoBeta_Desc", "Отказаться от участия во всех бета-программах"))];
    for b in d["vecBetas"].as_array().cloned().unwrap_or_default() {
        let n = s_of(&b, "strName");
        if n == "public" {
            v.push((n, t_or("#AppProperties_Beta_Select_Default", "Стандартная общедоступная версия"), t_or("#AppProperties_Beta_Select_Default_Desc", "")));
        } else {
            v.push((n.clone(), n, s_of(&b, "strDescription")));
        }
    }
    v
}

// --- Steam compat tools (Proton): installed Steam tools + compatibilitytools.d, explicit choice from config.vdf CompatToolMapping

fn steam_root() -> PathBuf {
    let h = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(h).join(".local/share/Steam")
}

fn compat_tools() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![];
    if let Ok(rd) = std::fs::read_dir(steam_root().join("steamapps/common")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !n.starts_with("Proton") || n.contains("EasyAntiCheat") || n.contains("BattlEye") {
                continue;
            }
            let id = if n.contains("Experimental") {
                "proton_experimental".to_string()
            } else if n.contains("Hotfix") {
                "proton_hotfix".to_string()
            } else {
                let num: String = n.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
                if num.is_empty() {
                    continue;
                }
                format!("proton_{num}")
            };
            let label = n.replace(" - ", " ");
            v.push((id, label));
        }
    }
    if let Ok(rd) = std::fs::read_dir(steam_root().join("compatibilitytools.d")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("LegacyRuntime") || !e.path().is_dir() {
                continue;
            }
            v.push((n.clone(), n));
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// (explicit tool for the app, global default) from config.vdf CompatToolMapping.
fn compat_mapping(id: u32) -> (Option<String>, Option<String>) {
    let Ok(txt) = std::fs::read_to_string(steam_root().join("config/config.vdf")) else { return (None, None) };
    let Some(p) = txt.find("\"CompatToolMapping\"") else { return (None, None) };
    let mut depth = 0i32;
    let (mut cur, mut app, mut m_app, mut m_glob) = (String::new(), String::new(), None, None);
    for line in txt[p..].lines().skip(1) {
        let l = line.trim();
        if l == "{" {
            depth += 1;
            if depth == 2 {
                app = cur.clone();
            }
            continue;
        }
        if l == "}" {
            depth -= 1;
            if depth <= 0 {
                break;
            }
            continue;
        }
        let parts: Vec<&str> = l.split('"').filter(|x| !x.trim().is_empty()).collect();
        if parts.len() == 1 {
            cur = parts[0].to_string();
        } else if parts.len() >= 2 && parts[0] == "name" && depth == 2 {
            if app == id.to_string() {
                m_app = Some(parts[1].to_string());
            } else if app == "0" {
                m_glob = Some(parts[1].to_string());
            }
        }
    }
    (m_app, m_glob)
}

fn tool_label(tools: &[(String, String)], id: &str) -> String {
    tools.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_else(|| id.to_string())
}

fn compat_options(s: &St) -> (Vec<(String, String)>, String) {
    let tools = compat_tools();
    let (mine, glob) = compat_mapping(s.app_id);
    let active = s_of(&s.d, "strCompatToolName");
    let def = match &glob {
        Some(g) => tf("#Settings_SteamPlay_DefaultToolEntry", &[&tool_label(&tools, g)]),
        None => t_or("#Settings_SteamPlay_DefaultToolEntry_Fallback", "По умолчанию"),
    };
    let mut opts = vec![(String::new(), def.clone())];
    for (id, l) in &tools {
        let legacy = id == "proton_7" || id == "proton_8" || id == "proton_6";
        if !s.show_all && legacy && Some(id.as_str()) != mine.as_deref() && *id != active {
            continue;
        }
        opts.push((id.clone(), l.clone()));
    }
    let cur = match &mine {
        Some(m) if !m.is_empty() => m.clone(),
        _ => String::new(),
    };
    (opts, cur)
}

fn build(s: &St, tab: Tab) -> Vec<F> {
    let d = &s.d;
    let name = if s.name.is_empty() { s_of(d, "strDisplayName") } else { s.name.clone() };
    let mut f: Vec<F> = vec![];
    let hdr = |f: &mut Vec<F>, tok: &str, fb: &str| f.push(F::new(0, Fid::None, t_or(tok, fb)));
    let para = |f: &mut Vec<F>, text: String| f.push(F::new(1, Fid::None, text));
    match tab {
        Tab::General => {
            let og = d["overlay_global"].as_bool().unwrap_or(true);
            f.push(F::new(2, Fid::Overlay, t_or("#AppProperties_EnableOverlay", "Включить оверлей Steam в игре")).on(b_of(d, "bOverlayEnabled") && og).dis(!og));
            let langs = d["vecLanguages"].as_array().cloned().unwrap_or_default();
            if langs.len() > 1 {
                let sel = s_of(&d["selectedLanguage"], "strShortName");
                f.push(F::new(3, Fid::Lang, t_or("#AppProperties_SelectLanguage", "Язык")).val(lang_label(&sel)));
            }
            if d["strResolutionOverride"].is_string() {
                let cur = s_of(d, "strResolutionOverride");
                let opts = res_opts(&cur);
                f.push(F::new(3, Fid::Res, t_or("#AppProperties_ResolutionOverride", "Разрешение игры")).val(opt_label(&opts, &cur)));
            }
            if b_of(d, "bCloudAvailable") {
                hdr(&mut f, "#AppProperties_SteamCloudSection", "Steam Cloud");
                para(&mut f, t_or("#AppProperties_SteamCloudDescription", "Сохранять данные игры в облаке Steam."));
                let acc = b_of(d, "bCloudEnabledForAccount");
                f.push(F::new(2, Fid::Cloud, tf("#AppProperties_EnableSteamCloud", &[&name])).on(b_of(d, "bCloudEnabledForApp") && acc).dis(!acc));
                if b_of(d, "bCloudSyncOnSuspendAvailable") {
                    f.push(F::new(2, Fid::CloudSuspend, tf("#AppProperties_EnableSteamCloudSyncOnSuspend", &[&name])).on(b_of(d, "bCloudSyncOnSuspendEnabled") && acc).dis(!acc));
                }
                if acc {
                    f.push(F::new(6, Fid::None, tf("#AppProperties_SteamCloudStored", &[&s_of(d, "strCloudBytesUsed")])).val(tf("#AppProperties_SteamCloudAvailable", &[&s_of(d, "strCloudBytesAvailable")])));
                }
            }
            hdr(&mut f, "#AppProperties_LaunchOptionsSection", "Параметры запуска");
            para(&mut f, t_or("#AppProperties_LaunchOptionsDescription", "Дополнительные аргументы командной строки для запуска игры."));
            f.push(F::new(4, Fid::LaunchOpts, "").val(s_of(d, "strLaunchOptions")));
            if b_of(d, "bHasDifferentCopies") || b_of(d, "bIsExcludedFromSharing") {
                let k = if b_of(d, "bIsExcludedFromSharing") { "#AppProperties_SharingNotSupported" } else { "#AppProperties_OwnedButOtherAvailable" };
                para(&mut f, t_or(k, ""));
            }
        }
        Tab::Shortcut => {
            f.push(F::new(4, Fid::SName, "").val(name.clone()));
            f.push(F::new(4, Fid::SExe, "").cap(t_or("#AppProperties_Shortcut_TargetExecutable", "Расположение").to_uppercase()).val(s_of(d, "strShortcutExe")).btn(t_or("#AppProperties_Shortcut_BrowseFiles", "Обзор...")));
            f.push(F::new(4, Fid::SDir, "").cap(t_or("#AppProperties_Shortcut_StartInFolder", "Рабочая папка").to_uppercase()).val(s_of(d, "strShortcutStartDir")).btn(t_or("#AppProperties_Shortcut_BrowseFiles", "Обзор...")));
            f.push(F::new(4, Fid::SOpts, "").cap(t_or("#AppProperties_LaunchOptionsSection", "Параметры запуска").to_uppercase()).val(s_of(d, "strShortcutLaunchOptions")));
            f.push(F::new(5, Fid::SIcon, t_or("#AppProperties_Shortcut_ChooseIconDialogTitle", "Выбрать иконку")).btn(t_or("#AppProperties_Shortcut_BrowseFiles", "Обзор...")));
            let og = d["overlay_global"].as_bool().unwrap_or(true);
            f.push(F::new(2, Fid::Overlay, t_or("#AppProperties_EnableOverlay", "Включить оверлей Steam в игре")).on(b_of(d, "bOverlayEnabled") && og).dis(!og));
            f.push(F::new(2, Fid::SVr, t_or("#AppProperties_Shortcut_InVR", "Добавить в библиотеку виртуальной реальности")).on(b_of(d, "bShortcutIsVR")));
        }
        Tab::Compat => {
            hdr(&mut f, "#Settings_SteamPlay_SteamPlay", "Steam Play");
            let (opts, cur) = compat_options(s);
            let active = s_of(d, "strCompatToolDisplayName");
            let mut desc = String::new();
            if !cur.is_empty() && !active.is_empty() && tool_label(&compat_tools(), &cur) != active {
                desc = active;
            }
            f.push(F::new(3, Fid::CompatTool, t_or("#AppProperties_Compat_SelectTool", "Выбрать инструмент совместимости")).val(opt_label(&opts, &cur)).desc(desc));
            f.push(F::new(2, Fid::ShowAll, t_or("#Settings_SteamPlay_ShowAll", "Показать все инструменты совместимости")).on(s.show_all));
        }
        Tab::Updates => {
            if b_of(d, "bHasAnyLocalContent") {
                let o = auto_update_opts();
                let cur = i_of(d, "eAutoUpdateValue").to_string();
                f.push(F::new(3, Fid::AutoUpdate, t_or("#AppProperties_AutoUpdateSection", "Автоматические обновления")).val(opt_label(&o, &cur)).dis(b_of(d, "bIsThirdPartyUpdater")));
            }
            let b = bg_opts();
            let cur = i_of(d, "eBackgroundDownloads").to_string();
            f.push(F::new(3, Fid::BgDl, t_or("#AppProperties_BackgroundDownloadsSection", "Фоновые загрузки")).desc(tf("#AppProperties_BackgroundDownloadsDescription", &[&name])).val(opt_label(&b, &cur)));
            f.push(F::new(6, Fid::None, tf("#AppProperties_AppID", &[&s.app_id.to_string()]).to_string()));
            f.push(F::new(6, Fid::None, tf("#AppProperties_AutoUpdateBuild", &[&i_of(d, "nBuildID").to_string()])));
            let up = d["rtLastUpdated"].as_u64().unwrap_or(0);
            if up > 0 && !b_of(d, "bIsThirdPartyUpdater") {
                let dt = chrono::DateTime::from_timestamp(up as i64, 0).map(|x| x.with_timezone(&chrono::Local));
                if let Some(x) = dt {
                    f.push(F::new(6, Fid::None, tf("#AppProperties_AutoUpdateDate", &[&x.format("%d.%m.%Y").to_string(), &x.format("%H:%M").to_string()])));
                }
            }
        }
        Tab::LocalFiles => {
            let ty = "game";
            f.push(F::new(6, Fid::None, t_or("#AppProperties_LocalFilesSize", "Размер установленных файлов:")).val(tf("#AppProperties_LocalFilesSizeOnDrive", &[&fmt_size(d["lDiskUsageBytes"].as_u64().unwrap_or(0)), &s_of(d, "strInstallFolder")])));
            f.push(F::new(5, Fid::Browse, s_of(d, "strInstallFolder")).btn(t_or("#AppProperties_LocalFilesBrowse", "Обзор...")));
            f.push(F::new(5, Fid::Backup, t_or(&format!("#AppProperties_LocalFilesBackupLabel_{ty}"), "Создать резервную копию файлов")).btn(t_or(&format!("#AppProperties_LocalFilesBackup_{ty}"), "Создать резервную копию...")));
            f.push(F::new(5, Fid::Restore, t_or("#AppProperties_LocalFilesRestoreLabel", "Восстановить игру из резервной копии")).btn(t_or("#AppProperties_LocalFilesRestore", "Восстановить из копии...")));
            let verify = match &s.status {
                Some(m) => m.clone(),
                None => t_or(&format!("#AppProperties_LocalFilesVerifyLabel_{ty}"), "Проверить целостность файлов"),
            };
            f.push(F::new(5, Fid::Verify, verify).btn(t_or(&format!("#AppProperties_LocalFilesVerify_{ty}"), "Проверить целостность файлов")).dis(b_of(d, "bIsThirdPartyUpdater")));
        }
        Tab::Betas => {
            let o = betas_opts(d);
            let cur = s_of(d, "strSelectedBeta");
            let lab = o.iter().find(|x| x.0 == cur).map(|x| x.1.clone()).unwrap_or(cur);
            f.push(F::new(3, Fid::Beta, t_or("#AppProperties_Beta_Selection", "Выбранная версия")).desc(t_or("#AppProperties_Beta_SelectionDescription", "")).val(lab));
            hdr(&mut f, "#AppProperties_Beta_Private", "Скрытые версии");
            f.push(F::new(4, Fid::BetaCode, "").cap(t_or("#AppProperties_Beta_AccessCode", "Введите код доступа")).val(String::new()).btn(t_or("#AppProperties_Beta_CheckCode", "Проверить код")));
        }
        Tab::Controller => {
            para(&mut f, t_or("#AppProperties_SteamInput", "Система ввода Steam позволяет использовать любые контроллеры в играх Steam и менять их раскладку."));
            f.push(F::new(5, Fid::CtrlConfig, tf("#AppProperties_SteamInputGeneralSetting", &[&t_or("#AppProperties_SteamInputGeneralSetting_Link", "конфигуратор контроллера")])).btn(t_or("#AppProperties_SteamInputGeneralSetting_Link", "Конфигуратор контроллера")));
            let o = input_opts();
            let cur = i_of(d, "eEnableThirdPartyControllerConfiguration").to_string();
            f.push(F::new(3, Fid::SteamInput, tf("#AppProperties_SteamInputOverride", &[&name])).desc(t_or("#AppProperties_SteamInputRequiresRestart", "")).val(opt_label(&o, &cur)));
            let r = rumble_opts();
            let cur = i_of(d, "eControllerRumbleIntensity").to_string();
            f.push(F::new(3, Fid::Rumble, t_or("#AppProperties_RumbleIntensity", "Интенсивность вибрации")).val(opt_label(&r, &cur)));
            if !s_of(d, "strShortcutExe").is_empty() || s.shortcut {
                f.push(F::new(2, Fid::DesktopCfg, t_or("#AppProperties_SteamInputDesktopConfigInLauncher", "Использовать управление для ПК в программе запуска")).on(b_of(d, "bEnableAllowDesktopConfiguration")));
            }
        }
        Tab::Dlc => {
            f.push(F::new(5, Fid::DlcStore, t_or("#AppProperties_Dlc_Name", "Название") + " / " + &t_or("#AppProperties_Dlc_SizeDisk", "Размер")).btn(t_or("#AppProperties_Dlc_ViewInStore", "Ещё в магазине")));
            for x in d["vecDLC"].as_array().cloned().unwrap_or_default() {
                let id = x["unAppID"].as_u64().unwrap_or(0) as u32;
                let st = t_or(&format!("#AppProperties_Dlc_State_{}", s_of(&x, "strState")), "");
                let size = x["lDiskUsageBytes"].as_u64().filter(|v| *v > 0).map(fmt_size).unwrap_or_default();
                let desc = [st, size].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("  -  ");
                f.push(F::new(2, Fid::Dlc(id), s_of(&x, "strName")).desc(desc).on(b_of(&x, "bEnabled")));
            }
        }
        Tab::Recording => {
            let mode = match d["rec_mode"].as_i64().unwrap_or(2) {
                0 => t_or("#AppProperties_GameRecording_GlobalStatus_Background", "Запись в фоновом режиме"),
                1 => t_or("#AppProperties_GameRecording_GlobalStatus_Manual", "Запись по нажатию кнопки").replace("%1$s", ""),
                _ => t_or("#AppProperties_GameRecording_GlobalStatus_Never", "Запись игр отключена"),
            };
            f.push(F::new(5, Fid::RecGlobal, t_or("#AppProperties_GameRecording_GlobalStatus", "Запись игр") + ": " + &mode).btn(t_or("#AppProperties_GameRecording_GlobalStatus_Button", "Управление функцией записи игр")));
            f.push(F::new(3, Fid::RecEnable, t_or("#AppProperties_GameRecording_Enabled_Label", "Запись этой игры")).desc(t_or("#AppProperties_GameRecording_Enabled_Description", "")).val(t_or("#AppProperties_GameRecording_Enabled", "Включена")));
        }
        Tab::Privacy => {
            f.push(F::new(2, Fid::Hidden, t_or("#AppProperties_Privacy_MarkAppHidden", "Скрыть в библиотеке")).desc(t_or("#AppProperties_Privacy_MarkAppHidden_Description", "")).on(b_of(d, "hidden")));
            if !s.shortcut {
                f.push(F::new(2, Fid::Private, t_or("#AppProperties_Privacy_MarkAppPrivate", "Сделать приватной")).desc(t_or("#AppProperties_Privacy_MarkAppPrivate_Description", "")).on(b_of(d, "private")).dis(true));
            }
        }
        Tab::Custom => {
            hdr(&mut f, "#AppProperties_Customization_ArtworkSection", "Изображения");
            f.push(F::new(5, Fid::ArtChange, t_or("#AppProperties_Customization_ArtworkSection", "Изображения")).btn(t_or("#GameAction_ChangeArt", "Сменить картинку...")));
            hdr(&mut f, "#AppProperties_Customization_MiscSection", "Разное");
            f.push(F::new(7, Fid::SortAs, t_or("#AppProperties_Customization_SortOrder", "Отдельное название для сортировки")).desc(t_or("#AppProperties_Customization_SortOrder_Description", "")).val(s_of(d, "sort_as")));
        }
    }
    f
}

// ---------------------------------------------------------------------------------------------------------- flow layout + sync

const CW: f32 = 905.0;

fn lines(text: &str, w: f32, px: f32) -> f32 {
    let per = (w / (px * 0.52)).max(8.0);
    text.split('\n').map(|l| ((l.chars().count() as f32) / per).ceil().max(1.0)).sum()
}

fn row_h(f: &F) -> f32 {
    match f.kind {
        0 => 46.0,
        1 => lines(&f.label, CW, 15.6) * 20.0 + 14.0,
        2 => 56.0 + if f.desc.is_empty() { 0.0 } else { lines(&f.desc, CW - 100.0, 14.4) * 18.0 - 2.0 },
        3 => (62.0f32).max(48.0 + if f.desc.is_empty() { 0.0 } else { lines(&f.desc, CW - 500.0, 14.4) * 18.0 }),
        4 => (if f.caption.is_empty() { 52.0 } else { 86.0 }) + 12.0,
        5 => (62.0f32).max(lines(&f.label, CW - 390.0, 17.0) * 22.0 + 24.0),
        6 => 38.0,
        _ => (76.0f32).max(52.0 + lines(&f.desc, CW - 410.0, 14.4) * 18.0),
    }
}

fn rebuild(a: &mut App, reset_scroll: bool) {
    if !st().open {
        return sync(a);
    }
    rebuild_tabs(None);
    {
        let mut s = st();
        let tab = s.tabs.get(s.tab).copied().unwrap_or(Tab::General);
        let fields = build(&s, tab);
        let mut y = 8.0f32;
        let mut laid = vec![];
        for f in &fields {
            let h = row_h(f);
            laid.push((y, h));
            y += h + if f.kind == 0 { 0.0 } else { 8.0 };
        }
        s.content_h = y + 40.0;
        s.laid = laid;
        s.fields = fields;
        let n = s.fields.len() as i32;
        if s.fidx >= n || (s.fidx >= 0 && !s.fields[s.fidx as usize].focusable()) || s.fidx < 0 {
            s.fidx = s.fields.iter().position(|f| f.focusable()).map(|i| i as i32).unwrap_or(-1);
        }
        if s.fidx < 0 && s.zone == 1 {
            s.zone = 0;
        }
    }
    if reset_scroll {
        a.ui().global::<PropsState>().set_vy(0.0);
    }
    sync(a);
}

fn ensure_visible(a: &App) {
    let s = st();
    if s.fidx < 0 {
        return;
    }
    let ui = a.ui();
    let g = ui.global::<PropsState>();
    let (y, h) = s.laid[s.fidx as usize];
    let vis = (ui.window().size().height as f32 / ui.window().scale_factor()) - 48.0 - 50.2 - 70.0;
    let vy = -g.get_vy();
    let ny = if y < vy + 8.0 {
        (y - 8.0).max(0.0)
    } else if y + h > vy + vis - 8.0 {
        y + h - vis + 8.0
    } else {
        vy
    };
    let max = (s.content_h - vis).max(0.0);
    g.set_vy(-(ny.min(max)));
}


fn sync(a: &mut App) {
    let ui = a.ui();
    let g = ui.global::<PropsState>();
    let s = st();
    let any = s.open || s.popup.is_some() || s.prompt.is_some() || s.dlg.is_some();
    g.set_open(s.open);
    g.set_title(s.name.clone().into());
    g.set_loading(s.loading);
    let rail: Vec<slint::SharedString> = s.tabs.iter().map(|t| t.title().into()).collect();
    g.set_rail(ModelRc::new(VecModel::from(rail)));
    g.set_tab(s.tab as i32);
    g.set_page_title(s.tabs.get(s.tab).map(|t| t.title()).unwrap_or_default().into());
    g.set_zone(s.zone);
    let fields: Vec<PField> = s
        .fields
        .iter()
        .zip(s.laid.iter())
        .map(|(f, (y, h))| PField {
            kind: f.kind,
            label: f.label.clone().into(),
            desc: f.desc.clone().into(),
            value: f.value.clone().into(),
            btn: f.btn.clone().into(),
            on: f.on,
            dis: f.dis,
            y: *y,
            h: *h,
            caption: f.caption.clone().into(),
        })
        .collect();
    g.set_fields(ModelRc::new(VecModel::from(fields)));
    g.set_fidx(s.fidx);
    g.set_fsub(s.fsub);
    g.set_content_h(s.content_h);
    match &s.popup {
        Some(p) => {
            g.set_popup_open(true);
            g.set_popup_title(p.title.clone().into());
            g.set_popup_sub(p.sub.clone().into());
            let it: Vec<slint::SharedString> = p.items.iter().map(|x| x.clone().into()).collect();
            g.set_popup_items(ModelRc::new(VecModel::from(it)));
            g.set_popup_idx(p.idx as i32);
            g.set_popup_cur(p.cur);
            let vis = 560.0f32.min(ui.window().size().height as f32 / ui.window().scale_factor() - 190.0 - 76.0);
            let top = p.idx as f32 * 52.0;
            let vy = -g.get_popup_vy();
            let ny = if top < vy { top } else if top + 52.0 > vy + vis { top + 52.0 - vis } else { vy };
            g.set_popup_vy(-ny.max(0.0));
        }
        None => g.set_popup_open(false),
    }
    match &s.prompt {
        Some(p) => {
            g.set_osk_open(true);
            g.set_osk_title(p.title.clone().into());
            g.set_osk_text(p.text.clone().into());
            // ввод — общая виртуальная клавиатура (screens/osk.rs); открывается один раз на время запроса
            crate::screens::osk::attach(a, PROMPT_SINK);
        }
        None => {
            g.set_osk_open(false);
            crate::screens::osk::detach(a, PROMPT_SINK);
        }
    }
    match &s.dlg {
        Some(d) => {
            g.set_dlg_open(true);
            g.set_dlg_title(d.title.clone().into());
            g.set_dlg_text(d.text.clone().into());
            let b: Vec<slint::SharedString> = d.btns.iter().map(|x| x.clone().into()).collect();
            g.set_dlg_btns(ModelRc::new(VecModel::from(b)));
            g.set_dlg_idx(d.idx as i32);
        }
        None => g.set_dlg_open(false),
    }
    let sg = (s.tab as i64) * 7 + (s.fidx as i64) * 131 + s.zone as i64 * 3 + s.fsub as i64 + s.popup.as_ref().map(|p| 1000 + p.idx as i64).unwrap_or(0) + s.prompt.as_ref().map(|p| 5000 + p.text.len() as i64).unwrap_or(0) + s.dlg.as_ref().map(|d| 9000 + d.idx as i64).unwrap_or(0) + if any { 100_000 } else { 0 };
    let mode = s.popup.is_some() as i64 + 2 * s.prompt.is_some() as i64 + 4 * s.dlg.is_some() as i64 + 8 * s.open as i64;
    let was = OPEN.swap(any, Ordering::Relaxed);
    let mode_changed = MODE.swap(mode, Ordering::Relaxed) != mode;
    SIG.store(sg, Ordering::Relaxed);
    drop(s);
    ensure_visible(a);
    if was != any || mode_changed {
        a.update_hint();
    }
}

// ---------------------------------------------------------------------------------------------------------- activation

fn open_popup(a: &mut App, fid: Fid, title: String, sub: String, opts: Vec<(String, String)>, cur: &str) {
    let idx = opts.iter().position(|o| o.0 == cur).unwrap_or(0);
    {
        let mut s = st();
        s.popup = Some(Popup {
            title,
            sub,
            items: opts.iter().map(|o| o.1.clone()).collect(),
            idx,
            cur: idx as i32,
            kind: PopupKind::Opt(fid, opts.iter().map(|o| o.0.clone()).collect()),
            entries: vec![],
        });
    }
    sync(a);
}

fn list_dir(dir: &PathBuf, pick_dir: bool) -> (Vec<String>, Vec<(PathBuf, bool)>) {
    let mut items = vec![];
    let mut ents = vec![];
    if pick_dir {
        items.push("[ ] ".to_string() + &t_or("#Button_OK", "Выбрать эту папку"));
        ents.push((dir.clone(), true));
        items.pop();
        items.push(format!("OK - {}", dir.display()));
    }
    if let Some(p) = dir.parent() {
        items.push("..".into());
        ents.push((p.to_path_buf(), true));
    }
    let mut rd: Vec<(String, PathBuf, bool)> = std::fs::read_dir(dir)
        .map(|r| r.flatten().filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.') {
                return None;
            }
            let d = e.path().is_dir();
            Some((n, e.path(), d))
        }).collect())
        .unwrap_or_default();
    rd.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    for (n, p, d) in rd {
        if pick_dir && !d {
            continue;
        }
        items.push(if d { format!("{n}/") } else { n });
        ents.push((p, d));
    }
    (items, ents)
}

fn open_files(a: &mut App, fid: Fid, dir: PathBuf, pick_dir: bool, title: String) {
    let (items, entries) = list_dir(&dir, pick_dir);
    {
        let mut s = st();
        s.popup = Some(Popup { title, sub: dir.display().to_string(), items, idx: if pick_dir { 0 } else { 0 }, cur: -1, kind: PopupKind::Files { dir, pick_dir, fid }, entries });
    }
    sync(a);
}

fn start_dir(cur: &str) -> PathBuf {
    let c = cur.trim().trim_matches('"');
    let p = PathBuf::from(c);
    if p.is_dir() {
        p
    } else if let Some(par) = p.parent().filter(|x| x.is_dir()) {
        par.to_path_buf()
    } else {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
    }
}

fn popup_activate(a: &mut App) {
    let (kind, idx, entries) = {
        let s = st();
        let Some(p) = &s.popup else { return };
        (p.kind.clone(), p.idx, p.entries.clone())
    };
    match kind {
        PopupKind::Opt(fid, vals) => {
            st().popup = None;
            if let Some(v) = vals.get(idx).cloned() {
                apply_option(a, fid, &v);
            }
            sync(a);
        }
        PopupKind::Files { dir: _, pick_dir, fid } => {
            let Some((path, is_dir)) = entries.get(idx).cloned() else { return };
            let is_here = pick_dir && idx == 0;
            if is_here || !is_dir {
                st().popup = None;
                apply_path(a, fid, path);
                sync(a);
            } else {
                let title = st().popup.as_ref().map(|p| p.title.clone()).unwrap_or_default();
                open_files(a, fid, path, pick_dir, title);
            }
        }
    }
}

fn apply_option(a: &mut App, fid: Fid, v: &str) {
    let id = st().app_id;
    match fid {
        Fid::Lang => write(a, "app-language", apps("SetAppCurrentLanguage", id, &js_str(v)), Some(("selectedLanguage", json!({"strShortName": v})))),
        Fid::Res => write(a, "app-resolution", apps("SetAppResolutionOverride", id, &js_str(v)), Some(("strResolutionOverride", json!(v)))),
        Fid::AutoUpdate => {
            let n: i64 = v.parse().unwrap_or(0);
            write(a, "auto-update", apps("SetAppAutoUpdateBehavior", id, &n.to_string()), Some(("eAutoUpdateValue", json!(n))))
        }
        Fid::BgDl => {
            let n: i64 = v.parse().unwrap_or(0);
            write(a, "bg-downloads", apps("SetAppBackgroundDownloadsBehavior", id, &n.to_string()), Some(("eBackgroundDownloads", json!(n))))
        }
        Fid::SteamInput => {
            let n: i64 = v.parse().unwrap_or(1);
            write(a, "steam-input", apps("SetThirdPartyControllerConfiguration", id, &n.to_string()), Some(("eEnableThirdPartyControllerConfiguration", json!(n))))
        }
        Fid::Rumble => {
            let n: i64 = v.parse().unwrap_or(0);
            write(a, "rumble", apps("SetControllerRumbleIntensity", id, &n.to_string()), Some(("eControllerRumbleIntensity", json!(n))))
        }
        Fid::Beta => write(a, "beta", apps("JoinAppContentBeta", id, &js_str(v)), Some(("strSelectedBeta", json!(v)))),
        Fid::CompatTool => {
            let tools = compat_tools();
            let label = if v.is_empty() { String::new() } else { tool_label(&tools, v) };
            write(a, "compat-tool", apps("SpecifyCompatTool", id, &js_str(v)), Some(("strCompatToolDisplayName", json!(label))));
        }
        _ => {}
    }
}

fn apply_path(a: &mut App, fid: Fid, p: PathBuf) {
    let id = st().app_id;
    let s = p.to_string_lossy().to_string();
    match fid {
        Fid::SExe => {
            let q = format!("\"{s}\"");
            write(a, "shortcut-exe", apps("SetShortcutExe", id, &js_str(&q)), Some(("strShortcutExe", json!(q))));
        }
        Fid::SDir => {
            let q = format!("\"{s}\"");
            write(a, "shortcut-dir", apps("SetShortcutStartDir", id, &js_str(&q)), Some(("strShortcutStartDir", json!(q))));
        }
        Fid::SIcon => write(a, "shortcut-icon", apps("SetShortcutIcon", id, &js_str(&s)), None),
        Fid::Restore => write(a, "restore-backup", format!("SteamClient.Installs.OpenInstallBackup({})", js_str(&s)), None),
        _ => {}
    }
}

fn activate(a: &mut App) {
    let (f, sub, id, shortcut) = {
        let s = st();
        if s.fidx < 0 || s.loading {
            return;
        }
        let Some(f) = s.fields.get(s.fidx as usize).cloned() else { return };
        (f, s.fsub, s.app_id, s.shortcut)
    };
    let d = st().d.clone();
    let toggle = |a: &mut App, tag: &str, call: &str, key: &str, cur: bool| write(a, tag, apps(call, id, ""), Some((key, json!(!cur))));
    match f.id {
        Fid::Overlay => toggle(a, "overlay", "ToggleEnableSteamOverlayForApp", "bOverlayEnabled", f.on),
        Fid::Cloud => toggle(a, "cloud", "ToggleAppSteamCloudEnabled", "bCloudEnabledForApp", f.on),
        Fid::CloudSuspend => toggle(a, "cloud-suspend", "ToggleAppSteamCloudSyncOnSuspendEnabled", "bCloudSyncOnSuspendEnabled", f.on),
        Fid::DesktopCfg => toggle(a, "desktop-config", "ToggleAllowDesktopConfiguration", "bEnableAllowDesktopConfiguration", f.on),
        Fid::SVr => write(a, "shortcut-vr", apps("SetShortcutIsVR", id, &(!f.on).to_string()), Some(("bShortcutIsVR", json!(!f.on)))),
        Fid::ShowAll => {
            {
                let mut s = st();
                s.show_all = !s.show_all;
            }
            rebuild(a, false);
        }
        Fid::Hidden => write(a, "hidden", format!("collectionStore.SetAppsAsHidden([{id}],{})", !f.on), Some(("hidden", json!(!f.on)))),
        Fid::Dlc(dlc) => {
            let mut v = d["vecDLC"].clone();
            if let Some(arr) = v.as_array_mut() {
                for x in arr.iter_mut() {
                    if x["unAppID"].as_u64() == Some(dlc as u64) {
                        x["bEnabled"] = json!(!f.on);
                    }
                }
            }
            write(a, "dlc", format!("SteamClient.Apps.SetDLCEnabled({id},{dlc},{})", !f.on), Some(("vecDLC", v)));
        }
        Fid::Lang => {
            let langs = d["vecLanguages"].as_array().cloned().unwrap_or_default();
            let opts: Vec<(String, String)> = langs.iter().map(|l| (s_of(l, "strShortName"), lang_label(&s_of(l, "strShortName")))).collect();
            let cur = s_of(&d["selectedLanguage"], "strShortName");
            open_popup(a, Fid::Lang, f.label.clone(), String::new(), opts, &cur);
        }
        Fid::Res => {
            let cur = s_of(&d, "strResolutionOverride");
            open_popup(a, Fid::Res, f.label.clone(), String::new(), res_opts(&cur), &cur);
        }
        Fid::AutoUpdate => open_popup(a, Fid::AutoUpdate, f.label.clone(), String::new(), auto_update_opts(), &i_of(&d, "eAutoUpdateValue").to_string()),
        Fid::BgDl => open_popup(a, Fid::BgDl, f.label.clone(), f.desc.clone(), bg_opts(), &i_of(&d, "eBackgroundDownloads").to_string()),
        Fid::SteamInput => open_popup(a, Fid::SteamInput, f.label.clone(), String::new(), input_opts(), &i_of(&d, "eEnableThirdPartyControllerConfiguration").to_string()),
        Fid::Rumble => open_popup(a, Fid::Rumble, f.label.clone(), String::new(), rumble_opts(), &i_of(&d, "eControllerRumbleIntensity").to_string()),
        Fid::Beta => {
            let o: Vec<(String, String)> = betas_opts(&d).into_iter().map(|x| (x.0, x.1)).collect();
            open_popup(a, Fid::Beta, f.label.clone(), String::new(), o, &s_of(&d, "strSelectedBeta"));
        }
        Fid::CompatTool => {
            let (opts, cur) = compat_options(&st());
            open_popup(a, Fid::CompatTool, f.label.clone(), String::new(), opts, &cur);
        }
        Fid::LaunchOpts | Fid::SName | Fid::SOpts | Fid::SortAs | Fid::SExe | Fid::SDir | Fid::BetaCode => {
            if sub == 1 {
                match f.id {
                    Fid::SExe | Fid::SDir => {
                        let cur = f.value.clone();
                        let dirp = f.id == Fid::SDir;
                        open_files(a, f.id, start_dir(&cur), dirp, if dirp { t_or("#AppProperties_Shortcut_ChooseStartDirectoryDialogTitle", "Выбрать директорию запуска") } else { t_or("#AppProperties_Shortcut_TargetExecutable", "Расположение") });
                    }
                    Fid::BetaCode => {
                        let code = st().fields.get(st().fidx as usize).map(|x| x.value.clone()).unwrap_or_default();
                        if code.is_empty() {
                            prompt_field(a, &f, &d);
                        } else {
                            let js = format!("SteamClient.Apps.JoinAppContentBetaByPassword({id},{}).then(r=>r.strName,()=>false)", js_str(&code));
                            if crate::actions::dryrun() {
                                eprintln!("[deckshell] DRYRUN setter beta-code: {js}");
                            }
                            setters::read(if crate::actions::dryrun() { "false".into() } else { js }, move |a, v| {
                                let m = match v.as_str() {
                                    Some(n) => format!("{} {n}", t_or("#AppProperties_Beta_AccessCodeSuccess", "Бета-версия разблокирована!")),
                                    None => t_or("#AppProperties_Beta_AccessCodeInvalid", "Неверный код доступа к бете"),
                                };
                                a.toast(&m);
                                refresh(id);
                            });
                        }
                    }
                    _ => {}
                }
            } else {
                prompt_field(a, &f, &d);
            }
        }
        Fid::Browse => {
            let dir = s_of(&d, "strInstallFolder");
            write(a, "browse-files", format!("SteamClient.System.OpenLocalDirectoryInSystemExplorer({})", js_str(&dir)), None);
            a.toast(&dir);
        }
        Fid::Backup => {
            write(a, "backup", format!("SteamClient.URL.ExecuteSteamURL('steam://backup/{id}')"), None);
        }
        Fid::Restore => open_files(a, Fid::Restore, start_dir(""), true, t_or("#AppProperties_LocalFilesRestore", "Восстановить из резервной копии")),
        Fid::Verify => {
            let name = st().name.clone();
            confirm(a, t_or("#AppProperties_LocalFilesVerify_game", "Проверить целостность файлов игры"), name, t_or("#Button_OK", "OK"), Box::new(move |a| {
                write(a, "verify", format!("SteamClient.Apps.VerifyApp({id})"), None);
                st().status = Some(t_or("#AppProperties_LocalFilesVerify_VerifyingFiles", "Проверка целостности файлов..."));
                rebuild(a, false);
            }));
        }
        Fid::SIcon => {
            let dir = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
            open_files(a, Fid::SIcon, dir, false, t_or("#AppProperties_Shortcut_ChooseIconDialogTitle", "Выбрать иконку"));
        }
        Fid::CtrlConfig => {
            a.act(Act::Controller(id));
        }
        Fid::DlcStore => a.act(Act::OpenStore(id)),
        Fid::RecGlobal => {
            close(a);
            a.act(Act::Open(steam_control::SteamPage::Settings));
        }
        Fid::ArtChange => {
            close(a);
            crate::sgdb::open(a, id);
        }
        Fid::RecEnable | Fid::Private | Fid::None => {}
    }
    let _ = shortcut;
}

fn prompt_field(a: &mut App, f: &F, d: &Value) {
    let id = st().app_id;
    let fid = f.id;
    let init = match fid {
        Fid::SName => st().name.clone(),
        Fid::BetaCode => String::new(),
        Fid::SExe | Fid::SDir => f.value.clone(),
        _ => f.value.clone(),
    };
    let title = if f.caption.is_empty() { if f.label.is_empty() { t_or("#AppProperties_LaunchOptionsSection", "Параметры запуска") } else { f.label.clone() } } else { f.caption.clone() };
    let _ = d;
    prompt(a, title, init, Box::new(move |a, text| match fid {
        Fid::LaunchOpts => write(a, "launch-options", apps("SetAppLaunchOptions", id, &js_str(&text)), Some(("strLaunchOptions", json!(text)))),
        Fid::SOpts => write(a, "shortcut-options", apps("SetShortcutLaunchOptions", id, &js_str(&text)), Some(("strShortcutLaunchOptions", json!(text)))),
        Fid::SName => {
            write(a, "shortcut-name", apps("SetShortcutName", id, &js_str(&text)), Some(("strDisplayName", json!(text.clone()))));
            st().name = text;
            sync(a);
        }
        Fid::SExe => write(a, "shortcut-exe", apps("SetShortcutExe", id, &js_str(&text)), Some(("strShortcutExe", json!(text)))),
        Fid::SDir => write(a, "shortcut-dir", apps("SetShortcutStartDir", id, &js_str(&text)), Some(("strShortcutStartDir", json!(text)))),
        Fid::SortAs => write(a, "sort-as", format!("appStore.SetCustomSortAs({id},{})", js_str(&text)), Some(("sort_as", json!(text)))),
        Fid::BetaCode => {
            if let Some(f) = st().fields.iter_mut().find(|f| f.id == Fid::BetaCode) {
                f.value = text;
            }
            sync(a);
        }
        _ => {}
    }));
    let _ = quote;
}

// ---------------------------------------------------------------------------------------------------------- prompt + osk

/// Text prompt with the on-screen keyboard; `cb` runs with the entered text on OK.
pub fn prompt(a: &mut App, title: String, initial: String, cb: TextCb) {
    {
        let mut s = st();
        s.prompt = Some(Prompt { title, text: initial, cb: Some(cb) });
    }
    sync(a);
}

/// Confirm dialog (OK / Cancel).
pub fn confirm(a: &mut App, title: String, text: String, ok: String, cb: OkCb) {
    {
        let mut s = st();
        s.dlg = Some(Dlg { title, text, btns: vec![t_or("#GameAction_Cancel", "Отмена"), ok], idx: 0, cb: Some(cb) });
    }
    sync(a);
}

fn dlg_activate(a: &mut App) {
    let (idx, cb) = {
        let mut s = st();
        let Some(d) = s.dlg.take() else { return };
        (d.idx, d.cb)
    };
    sync(a);
    if idx == 1 {
        if let Some(cb) = cb {
            cb(a);
        }
    }
}

/// Приёмник общей клавиатуры для запроса текста: символы → поле запроса, Поиск/Enter → OK, закрытие → отмена.
pub const PROMPT_SINK: crate::screens::osk::ExtSink = crate::screens::osk::ExtSink { id: 1, mode: crate::screens::osk::InputMode::Normal, typed: prompt_typed, enter: prompt_ok, cancel: prompt_cancel };

fn prompt_typed(a: &mut App, c: Option<char>) {
    if let Some(p) = st().prompt.as_mut() {
        match c {
            Some(c) => p.text.push(c),
            None => {
                p.text.pop();
            }
        }
    }
    sync(a);
}

fn prompt_ok(a: &mut App) {
    let (text, cb) = {
        let mut s = st();
        let Some(mut p) = s.prompt.take() else { return };
        (p.text.clone(), p.cb.take())
    };
    sync(a);
    if let Some(cb) = cb {
        cb(a, text);
    }
}

fn prompt_cancel(a: &mut App) {
    st().prompt = None;
    sync(a);
}

/// Hardware keyboard characters while the prompt is open.
pub fn wants_text() -> bool {
    st().prompt.is_some()
}

// ---------------------------------------------------------------------------------------------------------- input

pub fn hints() -> Vec<HintRow> {
    let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    let s = st();
    if s.dlg.is_some() || s.popup.is_some() {
        return vec![("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A), b];
    }
    if s.prompt.is_some() {
        return vec![("a", "Ввод", 1, "Enter", Action::A), ("x", "Стереть", 1, "F3", Action::X), ("y", "Пробел", 1, "F4", Action::Y), ("menu", "Готово", 2, "F9", Action::Menu), b];
    }
    vec![("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A), b]
}

fn move_field(dir: i32) {
    let mut s = st();
    let n = s.fields.len() as i32;
    let mut i = s.fidx;
    loop {
        i += dir;
        if i < 0 || i >= n {
            return;
        }
        if s.fields[i as usize].focusable() {
            s.fidx = i;
            s.fsub = 0;
            return;
        }
    }
}

pub fn handle(a: &mut App, act: Action) {
    // dialog first
    if st().dlg.is_some() {
        match act {
            Action::Left | Action::Up => {
                if let Some(d) = st().dlg.as_mut() {
                    d.idx = d.idx.saturating_sub(1);
                }
            }
            Action::Right | Action::Down => {
                if let Some(d) = st().dlg.as_mut() {
                    d.idx = (d.idx + 1).min(d.btns.len() - 1);
                }
            }
            Action::A => return dlg_activate(a),
            Action::B | Action::Menu => st().dlg = None,
            _ => return,
        }
        return sync(a);
    }
    if st().prompt.is_some() {
        // клавиатура перехватывает ввод раньше (input.rs); сюда доходят только кнопки, которые ей не принадлежат
        match act {
            Action::Menu => return prompt_ok(a),
            Action::B => return prompt_cancel(a),
            _ => return,
        }
    }
    if st().popup.is_some() {
        {
            let mut s = st();
            let p = s.popup.as_mut().unwrap();
            let n = p.items.len();
            match act {
                Action::Up => p.idx = p.idx.saturating_sub(1),
                Action::Down => p.idx = (p.idx + 1).min(n.saturating_sub(1)),
                Action::A => {
                    drop(s);
                    return popup_activate(a);
                }
                Action::B | Action::Menu => {
                    s.popup = None;
                }
                _ => return,
            }
        }
        return sync(a);
    }
    let zone = st().zone;
    match act {
        Action::LB | Action::RB => {
            let (n, cur) = {
                let s = st();
                (s.tabs.len() as i32, s.tab as i32)
            };
            let d = if act == Action::LB { -1 } else { 1 };
            return select_tab(a, (cur + d).rem_euclid(n.max(1)) as usize);
        }
        Action::B => {
            if zone == 1 {
                st().zone = 0;
                return sync(a);
            }
            return close(a);
        }
        _ => {}
    }
    if zone == 0 {
        let (n, cur) = {
            let s = st();
            (s.tabs.len(), s.tab)
        };
        match act {
            Action::Up if cur > 0 => select_tab(a, cur - 1),
            Action::Down if cur + 1 < n => select_tab(a, cur + 1),
            Action::Right | Action::A => {
                let mut s = st();
                if s.fields.iter().any(|f| f.focusable()) {
                    s.zone = 1;
                    if s.fidx < 0 {
                        s.fidx = s.fields.iter().position(|f| f.focusable()).map(|i| i as i32).unwrap_or(-1);
                    }
                }
                drop(s);
                sync(a);
            }
            _ => {}
        }
        return;
    }
    match act {
        Action::Up => move_field(-1),
        Action::Down => move_field(1),
        Action::Left => {
            let mut s = st();
            if s.fsub == 1 {
                s.fsub = 0;
            } else {
                s.zone = 0;
            }
        }
        Action::Right => {
            let mut s = st();
            let has_btn = s.fidx >= 0 && s.fields.get(s.fidx as usize).map(|f| f.kind == 4 && !f.btn.is_empty()).unwrap_or(false);
            if has_btn {
                s.fsub = 1;
            }
        }
        Action::A => return activate(a),
        _ => return,
    }
    sync(a);
}
