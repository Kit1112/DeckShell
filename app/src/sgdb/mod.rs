//! Native SteamGridDB cover changer (replaces the Decky plugin "decky-steamgriddb"; see README.md).
//! Full-window overlay (ui/screens/sgdb.slint) opened from the game context menu entry "Сменить картинку..."; everything runs on the
//! UI thread against the thread-local `St`, workers (network, decoding, Steam calls) post results back through `post`.
//! Locking rule: `st()` is borrowed once per entry point and passed down as `&mut St`; the lock-free mirrors (OPEN/MODE/SIG) are
//! what `app.rs` reads (hint_spec, sound signature) while an entry point is running.
use steam_assets::i18n::{tr, tr_args, tr_ref};
pub mod api;
pub mod apply;
pub mod cache;
pub mod hero_crop;
pub mod logo_layout;

use steam_assets::i18n;
use std::cell::{RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};
use steam_data::GameKind;

use crate::app::App;
use crate::input::Action;
use crate::{AppWindow, HeroFrame, SgdbItem, SgdbRow, SgdbState};
use api::{AType, ApiErr, Asset, Client, Filters, Settings};

// ------------------------------------------------------------------ lock-free mirrors for app.rs
static OPEN: AtomicBool = AtomicBool::new(false);
static MODE: AtomicU8 = AtomicU8::new(0);
static SIG: AtomicI64 = AtomicI64::new(0);
static BATCH_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}
/// Hardware-keyboard characters are routed to the on-screen keyboard text field.
pub fn wants_text() -> bool {
    is_open() && MODE.load(Ordering::Relaxed) == Mode::Osk as u8
}
/// Extra slot of App::sig(): any change of position / mode / tab plays Steam's navigation sound.
pub fn sig() -> i64 {
    SIG.load(Ordering::Relaxed)
}

type HintRow = (&'static str, &'static str, i32, &'static str, Action);
pub fn hints() -> Vec<HintRow> {
    let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    if BATCH_RUNNING.load(Ordering::Relaxed) {
        return vec![("b", tr_ref("sgdb.cancel_batch"), 1, "Esc", Action::B)];
    }
    match MODE.load(Ordering::Relaxed) {
        1 => vec![("a", crate::screens::stock_hint("#ActionButtonLabelSelect", "sgdb.select"), 1, "Enter", Action::A), b],
        2 => vec![("a", tr_ref("sgdb.enter"), 1, "Enter", Action::A), ("x", tr_ref("sgdb.erase"), 1, "F3", Action::X), ("y", tr_ref("sgdb.space"), 1, "F4", Action::Y), ("menu", crate::screens::stock_hint("#ActionButtonLabelDone", "sgdb.done"), 2, "F9", Action::Menu), b],
        3 => vec![("a", tr_ref("sgdb.anchor"), 1, "Enter", Action::A), ("x", tr_ref("sgdb.reset"), 1, "F3", Action::X), ("y", tr_ref("sgdb.remove_custom"), 1, "F4", Action::Y), ("menu", crate::screens::stock_hint("#Button_Save", "sgdb.save"), 2, "F9", Action::Menu), ("b", crate::screens::stock_hint("#ActionButtonLabelCancel", "sgdb.cancel"), 1, "Esc", Action::B)],
        4 => vec![("a", tr_ref("sgdb.press"), 1, "Enter", Action::A), ("x", tr_ref("sgdb.reset"), 1, "F3", Action::X), ("lb", tr_ref("sgdb.decrease"), 1, "[", Action::LB), ("rb", tr_ref("sgdb.increase"), 1, "]", Action::RB), ("b", crate::screens::stock_hint("#ActionButtonLabelCancel", "sgdb.cancel"), 1, "Esc", Action::B)],
        _ => vec![("a", tr_ref("sgdb.apply"), 1, "Enter", Action::A), ("x", tr_ref("sgdb.filters"), 1, "F3", Action::X), ("y", tr_ref("sgdb.search"), 1, "F4", Action::Y), ("menu", tr_ref("footer.actions"), 2, "F9", Action::Menu), b],
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Mode {
    Grid = 0,
    List = 1,
    Osk = 2,
    Edit = 3,
    HeroEdit = 4,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListKind {
    Filters,
    Actions,
    Games,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OskFor {
    Search,
    Key,
}

#[derive(Clone, PartialEq, Debug)]
enum FRow {
    Header,
    Stat,
    Anim,
    Untagged,
    Adult,
    Humor,
    Epilepsy,
    Style(String),
    Dim(String),
    Mime(String),
    Reset,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Undo,
    Reset,
    Blank,
    LogoEdit,
    LogoReset,
    LogoUndo,
    HeroEdit,
    HeroReset,
    HeroUndo,
    Filters,
    Search,
    Pick,
    Batch,
    Key,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Slot {
    Cur,
    Hero,
    Logo,
}

struct Item {
    asset: Asset,
    img: Option<Image>,
    bytes: usize,
    requested: bool,
    busy: bool,
    /// Animated preview (SteamGridDB webm thumb) as an on-disk frame clip: frames are decoded one at a time while the tile is
    /// visible. `fi` = frame on screen (picked from the shared clock).
    clip: Option<Arc<media::Clip>>,
    fi: usize,
}

struct St {
    settings: Settings,
    client: Client,
    appid: u32,
    name: String,
    shortcut: bool,
    mode: Mode,
    tab: usize,
    zone: u8,
    items: Vec<Item>,
    focus: usize,
    page: u32,
    end: bool,
    loading: bool,
    gen: u64,
    local_gen: u64,
    offline: bool,
    status: String,
    game: Option<(u64, String)>,
    filters: HashMap<AType, Filters>,
    // list panel
    list: ListKind,
    list_idx: usize,
    frows: Vec<FRow>,
    ops: Vec<Op>,
    games: Vec<(u64, String)>,
    fbefore: Option<Filters>,
    // keyboard
    osk_for: OskFor,
    osk_text: String,
    // logo editor
    lc: logo_layout::LogoLayout,
    // hero crop editor: the crop being edited (saved in hero_crop.rs on A) and the focused editor button
    hc: hero_crop::HeroCrop,
    hfocus: usize,
    // preview images
    loc: [Option<Image>; 3],
    // animated tiles: one timer + one clock drive every visible clip
    tick_timer: Timer,
    epoch: Instant,
    model: Option<Rc<VecModel<SgdbItem>>>,
    applying: bool,
    batch_cancel: Option<Arc<AtomicBool>>,
}

impl St {
    fn new() -> St {
        let settings = Settings::default();
        St {
            client: Client { base: api::DEFAULT_BASE.into(), key: None },
            settings,
            appid: 0,
            name: String::new(),
            shortcut: false,
            mode: Mode::Grid,
            tab: 0,
            zone: 1,
            items: vec![],
            focus: 0,
            page: 0,
            end: false,
            loading: false,
            gen: 0,
            local_gen: 0,
            offline: false,
            status: String::new(),
            game: None,
            filters: HashMap::new(),
            list: ListKind::Actions,
            list_idx: 0,
            frows: vec![],
            ops: vec![],
            games: vec![],
            fbefore: None,
            osk_for: OskFor::Search,
            osk_text: String::new(),
            lc: logo_layout::LogoLayout::default(),
            hc: hero_crop::HeroCrop::default(),
            hfocus: 0,
            loc: [None, None, None],
            tick_timer: Timer::default(),
            epoch: Instant::now(),
            model: None,
            applying: false,
            batch_cancel: None,
        }
    }
    fn t(&self) -> AType {
        AType::ALL[self.tab]
    }
}

thread_local! { static ST: &'static RefCell<St> = Box::leak(Box::new(RefCell::new(St::new()))); }
fn st() -> RefMut<'static, St> {
    ST.with(|s| s.borrow_mut())
}

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    crate::screens::defer(f);
}

// ------------------------------------------------------------------ geometry (mirrors ui/screens/sgdb.slint)
struct Geo {
    cw: f32,
    ch: f32,
    gap: f32,
    gh: f32,
}
fn geo(a: &App, t: AType) -> Geo {
    let ui = a.ui();
    let w = ui.window().size().width as f32 / ui.window().scale_factor();
    let h = ui.window().size().height as f32 / ui.window().scale_factor();
    let gap = 12.0;
    let gw = w - 480.0;
    let cols = t.cols() as f32;
    let cw = (gw - gap * (cols - 1.0)) / cols;
    Geo { cw, ch: cw * t.ratio(), gap, gh: h - 168.0 - 70.0 }
}

// ------------------------------------------------------------------ worker pool (thumbnails, previews, animations)
enum Job {
    Thumb { gen: u64, idx: usize, id: u64, url: String, full: String, animated: bool, w: u32, h: u32 },
    Local { gen: u64, slot: Slot, path: PathBuf, w: u32, h: u32 },
}

struct Pool {
    q: Mutex<VecDeque<Job>>,
    cv: Condvar,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);

fn pool() -> &'static Pool {
    static P: OnceLock<&'static Pool> = OnceLock::new();
    P.get_or_init(|| {
        let p: &'static Pool = Box::leak(Box::new(Pool { q: Mutex::new(VecDeque::new()), cv: Condvar::new() }));
        for n in 0..3 {
            let _ = std::thread::Builder::new().name(format!("sgdb-worker-{n}")).spawn(move || loop {
                let job = {
                    let mut g = p.q.lock().unwrap();
                    loop {
                        if let Some(j) = g.pop_back() {
                            break j; // LIFO: what the user looks at now first
                        }
                        g = p.cv.wait(g).unwrap();
                    }
                };
                run_job(job);
            });
        }
        p
    })
}

fn push_job(j: Job) {
    let p = pool();
    let mut g = p.q.lock().unwrap();
    if g.len() > 200 {
        g.pop_front();
    }
    g.push_back(j);
    p.cv.notify_one();
}

fn client() -> Client {
    CLIENT.lock().unwrap().clone().unwrap_or(Client { base: api::DEFAULT_BASE.into(), key: None })
}

fn decode_still(path: &Path, w: u32, h: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "webm" | "mp4" | "mkv" | "mov" | "avi") {
        // animated candidates normally come through `open_preview_clip`; this is the still of a video file (one decoded frame)
        let c = media::open_clip(path, w, h, 1000).ok()?;
        return clip_frame(&c, 0);
    }
    if let Ok(r) = image::ImageReader::open(path) {
        if let Ok(r) = r.with_guessed_format() {
            if let Ok(img) = r.decode() {
                let img = if img.width() > w || img.height() > h { img.thumbnail(w, h) } else { img };
                let rgba = img.to_rgba8();
                return Some(SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()));
            }
        }
    }
    // animated WebP / APNG / GIF: the media decoder (first frame)
    let c = media::open_clip(path, w, h, 1000).ok()?;
    clip_frame(&c, 0)
}

fn clip_frame(c: &media::Clip, i: usize) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let mut b = SharedPixelBuffer::<Rgba8Pixel>::new(c.width, c.height);
    c.frame_into(i, b.make_mut_bytes()).ok()?;
    Some(b)
}

/// Animated preview clip of an animated asset: its .webm thumb (needs ffmpeg), else - without ffmpeg - the animated WebP/APNG/GIF
/// original when it is small. Every frame is kept (source fps), scaled to the tile.
fn open_preview_clip(thumb: &str, full: &str, w: u32, h: u32) -> Option<Arc<media::Clip>> {
    let ext = Asset::ext_of(thumb);
    let src = if media::find_ffmpeg().is_some() || !matches!(ext.as_str(), "webm" | "mp4") {
        thumb_file(thumb)
    } else if matches!(Asset::ext_of(full).as_str(), "webp" | "apng" | "gif" | "png") {
        let p = cache::full_path(full);
        if !p.is_file() {
            let _ = client().download(full, &p);
        }
        p.metadata().ok().filter(|m| m.len() < 8 << 20).map(|_| p)
    } else {
        None
    }?;
    media::open_clip(&src, w, h, 0).ok().filter(|c| c.len() > 1).map(Arc::new)
}

fn thumb_file(url: &str) -> Option<PathBuf> {
    let p = cache::thumb_path(url);
    if !p.is_file() {
        client().download(url, &p).ok()?;
    } else {
        cache::touch(&p);
    }
    Some(p)
}

fn run_job(j: Job) {
    match j {
        Job::Thumb { gen, idx, id, url, full, animated, w, h } => {
            // one job gives both the still (frame 0) and, for animated assets, the clip that plays in the tile
            let clip = if animated { open_preview_clip(&url, &full, w, h) } else { None };
            let buf = match &clip {
                Some(c) => clip_frame(c, 0),
                None => thumb_file(&url).and_then(|p| decode_still(&p, w, h)),
            };
            post(move |a| on_thumb(a, gen, idx, id, buf, clip));
        }
        Job::Local { gen, slot, path, w, h } => {
            let buf = decode_still(&path, w, h);
            post(move |a| on_local(a, gen, slot, buf));
        }
    }
}

// ------------------------------------------------------------------ open / close
pub fn wire(ui: &AppWindow) {
    let s = ui.global::<SgdbState>();
    s.set_label_banner_position(i18n::tr("sgdb.banner_position").into());
    s.set_label_cancel(crate::screens::stock_label("#Button_Cancel", "sgdb.cancel").into());
    s.set_label_epilepsy(i18n::tr("sgdb.epilepsy").into());
    s.set_label_humour(i18n::tr("sgdb.humor").into());
    s.set_label_it_applies(i18n::tr("sgdb.it_applies").into());
    s.set_label_logo_position(i18n::tr("sgdb.logo_position").into());
    s.set_label_no_picture(i18n::tr("sgdb.no_picture").into());
    s.set_label_reset(i18n::tr("sgdb.reset").into());
    s.set_label_save(crate::screens::stock_label("#Button_Save", "sgdb.save").into());
    s.set_label_the_crucifix_is_to_point_at_the(i18n::tr("sgdb.the_crucifix_is_to_point_at_the").into());
    s.set_label_uploading(i18n::tr("sgdb.uploading").into());
    s.on_tab_clicked(|i| post(move |a| touch_tab(a, i as usize)));
    s.on_tile_clicked(|i| post(move |a| touch_tile(a, i as usize)));
    s.on_scrolled(|_| post(|a| {
        if is_open() {
            let mut s = st();
            request_window(&mut s, a);
        }
    }));
    s.on_list_clicked(|i| post(move |a| {
        if is_open() {
            let mut s = st();
            s.list_idx = i as usize;
            list_activate(&mut s, a);
            finish(&mut s, a);
        }
    }));
    s.on_dismiss(|| post(|a| {
        if is_open() {
            handle(a, Action::B);
        }
    }));
    s.on_close(|| post(|a| close(a)));
    s.on_hero_clicked(|i| post(move |a| touch_hero(a, i)));
    let model = Rc::new(VecModel::<SgdbItem>::default());
    s.set_items(ModelRc::from(model.clone()));
    st().model = Some(model);
    debug_autoopen();
}

/// Test hook: `DECKSHELL_SGDB_AUTOOPEN=<appid>[,<tab 0-4>[,animonly]]` opens the picker as soon as the game list is loaded
/// (live verification without driving the gamepad). Not used in normal runs.
fn debug_autoopen() {
    let Some(spec) = std::env::var("DECKSHELL_SGDB_AUTOOPEN").ok() else { return };
    let mut it = spec.split(',');
    let appid: u32 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let tab: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let anim_only = it.next() == Some("animonly");
    std::thread::spawn(move || {
        for _ in 0..240 {
            std::thread::sleep(Duration::from_millis(500));
            let done = Arc::new(AtomicBool::new(false));
            let d2 = done.clone();
            post(move |a| {
                if a.gidx.contains_key(&appid) && !is_open() {
                    open(a, appid);
                    let mut s = st();
                    let t = AType::ALL[tab % 5];
                    let mut f = s.settings.filters(t);
                    if anim_only {
                        f.stat = false;
                        f.animated = true;
                    }
                    s.filters.insert(t, f);
                    switch_tab(&mut s, a, tab % 5);
                    finish(&mut s, a);
                    d2.store(true, Ordering::Relaxed);
                }
            });
            std::thread::sleep(Duration::from_millis(300));
            if done.load(Ordering::Relaxed) {
                return;
            }
        }
    });
}

/// Entry from the game context menus ("Сменить картинку...").
pub fn open(a: &mut App, appid: u32) {
    let Some(&gi) = a.gidx.get(&appid) else { return };
    let g = a.games()[gi].clone();
    if a.sm.open {
        a.sm.open = false;
    }
    let mut s = st();
    let settings = Settings::load();
    let client = Client::from(&settings);
    *CLIENT.lock().unwrap() = Some(client.clone());
    s.settings = settings;
    s.client = client;
    s.appid = appid;
    s.name = g.name.clone();
    s.shortcut = g.kind == GameKind::Shortcut;
    s.mode = Mode::Grid;
    s.zone = 1;
    s.tab = 0;
    s.filters.clear();
    s.game = s.settings.game_for(appid);
    s.loc = [None, None, None];
    s.fbefore = None;
    OPEN.store(true, Ordering::Relaxed);
    std::thread::spawn(|| {
        cache::prune(cache::budget(), &[]);
    });
    let ui = a.ui();
    let st_ = ui.global::<SgdbState>();
    st_.set_title(tr_args("sgdb.artwork_title", &[("game", &g.name)]).into());
    let tabs: Vec<slint::SharedString> = AType::ALL.iter().map(|t| t.label().into()).collect();
    st_.set_tabs(ModelRc::new(VecModel::from(tabs)));
    st_.set_pname(g.name.as_str().into());
    st_.set_lab_now(tr_ref("sgdb.current").into());
    st_.set_lab_new(tr_ref("sgdb.new").into());
    st_.set_open(true);
    let gen_ = s.gen;
    request_locals(&mut s, a, gen_);
    switch_tab(&mut s, a, 0);
    finish(&mut s, a);
    a.update_hint();
    eprintln!("[deckshell] sgdb: open appid {appid} \"{}\" (key: {}, base {})", s.name, if s.client.key.is_some() { "set" } else { "none" }, s.client.base);
}

pub fn close(a: &mut App) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if let Some(c) = &s.batch_cancel {
        c.store(true, Ordering::Relaxed);
    }
    s.gen += 1;
    s.items.clear();
    s.loc = [None, None, None];
    s.tick_timer.stop();
    if let Some(m) = &s.model {
        m.set_vec(vec![]);
    }
    OPEN.store(false, Ordering::Relaxed);
    MODE.store(0, Ordering::Relaxed);
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_open(false);
    g.set_list_open(false);
    g.set_osk_open(false);
    g.set_edit_open(false);
    g.set_hero_edit_open(false);
    g.set_cur_has(false);
    g.set_cand_has(false);
    g.set_hero_has(false);
    g.set_logo_has(false);
    g.set_vy(0.0);
    drop(s);
    a.update_hint();
    unsafe { libc::malloc_trim(0) };
}

// ------------------------------------------------------------------ search / paging
fn switch_tab(s: &mut St, a: &mut App, tab: usize) {
    s.tab = tab % AType::ALL.len();
    let t = s.t();
    if !s.filters.contains_key(&t) {
        let f = s.settings.filters(t);
        s.filters.insert(t, f);
    }
    s.focus = 0;
    s.tick_timer.stop();
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_cols(t.cols() as i32);
    g.set_ratio(t.ratio());
    g.set_contain(t.contain());
    g.set_pkind(s.tab as i32);
    g.set_vy(0.0);
    g.set_cand_has(false);
    let gen_ = s.gen + 1;
    request_locals(s, a, gen_);
    start_search(s, a);
}

fn game_line(s: &St) -> String {
    match &s.game {
        Some((_, n)) => format!("SteamGridDB: {n}"),
        None if s.shortcut => tr_ref("sgdb.shortcut_search").into(),
        None => tr_args("sgdb.appid_search", &[("appid", &s.appid.to_string())]),
    }
}

fn start_search(s: &mut St, a: &mut App) {
    s.gen += 1;
    s.page = 0;
    s.end = false;
    s.loading = true;
    s.items.clear();
    s.status = if s.client.key.is_none() { String::new() } else { "".into() };
    if let Some(m) = &s.model {
        m.set_vec(vec![]);
    }
    let gen = s.gen;
    let t = s.t();
    let f = s.filters.get(&t).cloned().unwrap_or_else(|| Filters::default_for(t));
    let (client, appid, name, shortcut, game) = (s.client.clone(), s.appid, s.name.clone(), s.shortcut, s.game.clone());
    std::thread::spawn(move || {
        // non-Steam shortcuts have no appid on SteamGridDB: match by name (first autocomplete hit, like the plugin)
        let mut game = game;
        if game.is_none() && shortcut {
            match client.search_games(&name) {
                Ok((rows, _)) => game = rows.into_iter().next(),
                Err(e) => return post(move |a| on_assets(a, gen, 0, Err(e), None)),
            }
        }
        let r = client.assets(t, appid, game.as_ref().map(|g| g.0), &f, 0);
        post(move |a| on_assets(a, gen, 0, r, game));
    });
    sync_all(s, a);
}

fn load_more(s: &mut St) {
    if s.loading || s.end {
        return;
    }
    s.loading = true;
    let gen = s.gen;
    let t = s.t();
    let page = s.page + 1;
    let f = s.filters.get(&t).cloned().unwrap_or_else(|| Filters::default_for(t));
    let (client, appid, game) = (s.client.clone(), s.appid, s.game.clone());
    std::thread::spawn(move || {
        let r = client.assets(t, appid, game.as_ref().map(|g| g.0), &f, page);
        post(move |a| on_assets(a, gen, page, r, None));
    });
}

fn on_assets(a: &mut App, gen: u64, page: u32, r: Result<(Vec<Asset>, bool), ApiErr>, game: Option<(u64, String)>) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if s.gen != gen {
        return;
    }
    s.loading = false;
    if let Some(g) = game {
        s.game = Some(g);
    }
    match r {
        Ok((assets, off)) => {
            s.offline = off;
            s.page = page;
            if assets.is_empty() {
                s.end = true;
            }
            let base = s.items.len();
            let t = s.t();
            let split = s.filters.get(&t).map(|f| f.animated_only()).unwrap_or(false); // dimensions not sent: split capsule / wide here
            let fits: Vec<Asset> = assets.into_iter().filter(|a| !split || t.fits_tab(a)).collect();
            let mut known: HashSet<u64> = s.items.iter().map(|i| i.asset.id).collect();
            for asset in fresh_assets(&mut known, fits) {
                let it = Item { asset, img: None, bytes: 0, requested: false, busy: false, clip: None, fi: usize::MAX };
                if let Some(m) = &s.model {
                    m.push(item_row(&it));
                }
                s.items.push(it);
            }
            if s.items.is_empty() && s.end {
                s.status = tr_ref("sgdb.empty_filters").into();
            } else {
                s.status.clear();
            }
            if s.items.len() == base && !s.end {
                load_more(&mut s); // the whole page was filtered out client side: ask for the next one
            }
        }
        Err(ApiErr::NotFound) if s.game.is_none() => {
            // plugin: unknown game -> game selection dialog
            s.end = true;
            s.status = tr_ref("sgdb.choose_manually").into();
            let name = s.name.clone();
            open_osk(&mut s, a, OskFor::Search, &name);
        }
        Err(ApiErr::NoKey) => {
            s.end = true;
            s.status = tr_ref("sgdb.api_key_instructions").into();
        }
        Err(e) => {
            s.end = true;
            s.status = tr_args("sgdb.error", &[("e", &e.to_string())]);
            a.toast(&format!("SteamGridDB: {e}"));
        }
    }
    sync_all(&mut s, a);
    request_window(&mut s, a);
    finish(&mut s, a);
}

fn item_row(it: &Item) -> SgdbItem {
    SgdbItem {
        img: it.img.clone().unwrap_or_default(),
        has_img: it.img.is_some(),
        flags: it.asset.flags(),
        tag: if it.asset.animated() { it.asset.format_tag().into() } else { "".into() },
        dims: format!("{}×{}", it.asset.width, it.asset.height).into(),
        author: it.asset.author.as_str().into(),
        busy: it.busy,
    }
}

fn set_row(s: &St, i: usize) {
    if let (Some(m), Some(it)) = (&s.model, s.items.get(i)) {
        m.set_row_data(i, item_row(it));
    }
}

// ------------------------------------------------------------------ thumbnails (visible window, byte-budgeted)
const MEM_BUDGET: usize = 10 << 20;

fn window_rows(s: &St, a: &App) -> (usize, usize, usize) {
    let t = s.t();
    let g = geo(a, t);
    let ui = a.ui();
    let vy = -ui.global::<SgdbState>().get_vy().min(0.0);
    let rowh = g.ch + g.gap;
    let first = (vy / rowh).floor().max(0.0) as usize;
    let last = ((vy + g.gh) / rowh).floor().max(0.0) as usize;
    (first, last, t.cols())
}

fn request_window(s: &mut St, a: &mut App) {
    if s.items.is_empty() {
        return;
    }
    let (first, last, cols) = window_rows(s, a);
    let t = s.t();
    let g = geo(a, t);
    let scale = a.ui().window().scale_factor();
    let (tw, th) = (((g.cw * scale).ceil() as u32).clamp(64, 480), ((g.ch * scale).ceil() as u32).clamp(48, 640));
    let lo = first.saturating_sub(1) * cols;
    let hi = ((last + 3) * cols).min(s.items.len());
    for i in (lo..hi).rev() {
        let gen = s.gen;
        let it = &mut s.items[i];
        if it.img.is_none() && !it.requested {
            it.requested = true;
            push_job(Job::Thumb { gen, idx: i, id: it.asset.id, url: it.asset.thumb.clone(), full: it.asset.url.clone(), animated: it.asset.animated(), w: tw, h: th });
        }
    }
    // clips keep a file handle: only the window (visible rows +- prefetch) holds them
    for i in 0..s.items.len() {
        if s.items[i].clip.is_some() && (i < lo || i >= hi) {
            s.items[i].clip = None;
            if s.items[i].img.is_some() {
                set_row(s, i);
            }
        }
    }
    // drop decoded images far outside the window when over budget
    let mut total: usize = s.items.iter().map(|i| i.bytes).sum();
    if total > MEM_BUDGET {
        let mut far: Vec<usize> = (0..s.items.len()).filter(|i| s.items[*i].img.is_some() && (*i < lo || *i >= hi)).collect();
        far.sort_by_key(|i| std::cmp::Reverse((*i as i64 - s.focus as i64).abs()));
        for i in far {
            if total <= MEM_BUDGET {
                break;
            }
            total -= s.items[i].bytes;
            s.items[i].img = None;
            s.items[i].clip = None;
            s.items[i].bytes = 0;
            s.items[i].requested = false;
            set_row(s, i);
        }
    }
    // infinite paging: the last rows are close
    let rows_total = s.items.len().div_ceil(cols);
    if last + 3 >= rows_total {
        load_more(s);
    }
    kick(s, a);
}

fn on_thumb(a: &mut App, gen: u64, idx: usize, id: u64, buf: Option<SharedPixelBuffer<Rgba8Pixel>>, clip: Option<Arc<media::Clip>>) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if s.gen != gen || s.items.get(idx).map(|i| i.asset.id) != Some(id) {
        return;
    }
    if let Some(b) = buf {
        s.items[idx].bytes = (b.width() * b.height() * 4) as usize;
        s.items[idx].img = Some(Image::from_rgba8(b));
    }
    if clip.is_some() {
        let it = &mut s.items[idx];
        it.clip = clip;
        it.fi = usize::MAX; // forces the first tick to show the frame of the shared clock
    }
    set_row(&s, idx);
    if idx == s.focus {
        sync_preview(&mut s, a);
    }
    kick(&mut s, a);
}

// ------------------------------------------------------------------ preview ("now" | candidate in context)
fn request_locals(s: &mut St, a: &mut App, _gen: u64) {
    s.local_gen = s.local_gen.wrapping_add(1);
    let gen = s.local_gen;
    let Some(&gi) = a.gidx.get(&s.appid) else { return };
    let art = a.games()[gi].art.clone();
    let t = s.t();
    let cur = match t {
        AType::Capsule => art.capsule,
        AType::Wide => art.header.clone(),
        AType::Hero => art.hero.clone(),
        AType::Logo => art.logo.clone(),
        AType::Icon => art.icon,
    };
    for (slot, p, w, h) in [(Slot::Cur, cur, 400, 600), (Slot::Hero, art.hero.or(art.header), 760, 260), (Slot::Logo, art.logo, 480, 240)] {
        match p {
            Some(path) => push_job(Job::Local { gen, slot, path, w, h }),
            None => {
                s.loc[slot as usize] = None;
            }
        }
    }
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_cur_has(s.loc[0].is_some());
    g.set_hero_has(s.loc[1].is_some());
    g.set_logo_has(s.loc[2].is_some());
}

fn on_local(a: &mut App, gen: u64, slot: Slot, buf: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if gen != s.local_gen {
        return;
    }
    s.loc[slot as usize] = buf.map(Image::from_rgba8);
    sync_preview(&mut s, a);
}

fn sync_preview(s: &mut St, a: &mut App) {
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_cur(s.loc[0].clone().unwrap_or_default());
    g.set_cur_has(s.loc[0].is_some());
    g.set_hero(s.loc[1].clone().unwrap_or_default());
    g.set_hero_has(s.loc[1].is_some());
    g.set_logo(s.loc[2].clone().unwrap_or_default());
    g.set_logo_has(s.loc[2].is_some());
    let lc = if s.mode == Mode::Edit { s.lc } else { logo_layout::get(s.appid) };
    g.set_logo_layout(HeroFrame { zoom: lc.zoom, pan_x: lc.pan_x, pan_y: lc.pan_y });
    g.set_saved_crop(hero_frame(hero_crop::get(s.appid)));
    g.set_edit_crop(hero_frame(s.hc));
    match s.items.get(s.focus) {
        Some(it) => {
            let img = it.img.clone();
            g.set_cand_has(img.is_some());
            if let Some(i) = img {
                g.set_cand(i);
            }
            let x = &it.asset;
            let mut m = format!("{}×{} · {}", x.width, x.height, x.format_tag());
            if !x.style.is_empty() {
                m += &format!(" · {}", x.style);
            }
            if !x.author.is_empty() {
                m += &format!(" · {}", x.author);
            }
            if !x.notes.is_empty() {
                m += &format!("\n{}", x.notes);
            }
            g.set_cand_meta(m.into());
        }
        None => {
            g.set_cand_has(false);
            g.set_cand_meta("".into());
        }
    }
}

// ------------------------------------------------------------------ animated tiles (every visible one plays)
fn focus_changed(s: &mut St, a: &mut App) {
    sync_preview(s, a);
}

/// (Re)start the animation clock if some visible tile has a clip. Called whenever tiles appear (thumbnail ready, scroll, close of a panel).
fn kick(s: &mut St, a: &mut App) {
    if !s.tick_timer.running() && s.mode == Mode::Grid {
        tick(s, a);
    }
}

/// Show the right frame on every visible animated tile, then sleep until the earliest next frame change. All clips run on ONE clock
/// (`St::epoch`): the frame of a tile is a function of time, so tiles of the same fps change together and the whole window is
/// redrawn once per frame time, not once per tile (femtovg redraws everything: this is the CPU budget). Frames due within
/// `LOOKAHEAD` are taken now to merge near-simultaneous changes. Frames come from the on-disk clips one at a time (pread + lz4 straight
/// into a pixel buffer), memory stays flat however many tiles play. The timer stops when nothing visible animates (idle screen costs
/// nothing) and while a panel covers the grid.
fn tick(s: &mut St, a: &mut App) {
    const LOOKAHEAD: u64 = 4;
    s.tick_timer.stop();
    if s.mode != Mode::Grid || !is_open() {
        return;
    }
    let t0 = Instant::now();
    let (first, last, cols) = window_rows(s, a);
    let lo = first * cols;
    let hi = ((last + 1) * cols).min(s.items.len());
    let now = s.epoch.elapsed().as_millis() as u64;
    let mut wait: Option<u64> = None;
    let mut updates: Vec<(usize, Image)> = vec![];
    for i in lo..hi {
        let it = &mut s.items[i];
        let Some(c) = it.clip.clone() else { continue };
        let (fi, rem) = c.at(now + LOOKAHEAD);
        if fi != it.fi || it.img.is_none() {
            if let Some(b) = clip_frame(&c, fi) {
                let img = Image::from_rgba8(b);
                it.fi = fi;
                it.img = Some(img.clone());
                updates.push((i, img));
            }
        }
        wait = Some(wait.map_or(rem, |w| w.min(rem)));
    }
    if let Some(m) = &s.model {
        for (i, img) in &updates {
            if let Some(mut row) = m.row_data(*i) {
                row.img = img.clone();
                row.has_img = true;
                m.set_row_data(*i, row);
            }
        }
    }
    if let Some((_, img)) = updates.iter().find(|u| u.0 == s.focus) {
        let ui = a.ui();
        let g = ui.global::<SgdbState>();
        g.set_cand(img.clone());
        g.set_cand_has(true);
    }
    if std::env::var_os("DECKSHELL_ARTLOG").is_some() && !updates.is_empty() {
        // [sgdb-tick] every 5 s: ticks that changed frames, tiles updated, µs spent in tick() (UI thread, excludes the renderer)
        static STAT: Mutex<(u64, u64, u64, Option<Instant>)> = Mutex::new((0, 0, 0, None));
        let mut g = STAT.lock().unwrap();
        g.0 += 1;
        g.1 += updates.len() as u64;
        g.2 += t0.elapsed().as_micros() as u64;
        let t = *g.3.get_or_insert(t0);
        if t0.duration_since(t).as_secs() >= 5 {
            eprintln!("[sgdb-tick] {} redraw-ticks, {} tile updates, {} us total in tick over {:.1}s", g.0, g.1, g.2, t0.duration_since(t).as_secs_f32());
            *g = (0, 0, 0, Some(t0));
        }
    }
    if let Some(w) = wait {
        // `rem` was measured at now + LOOKAHEAD: wake when that frame boundary is LOOKAHEAD early, never busy-loop
        s.tick_timer.start(TimerMode::SingleShot, Duration::from_millis(w.max(2)), || {
            crate::with_app(|a| {
                if is_open() {
                    let mut s = st();
                    tick(&mut s, a);
                }
            });
        });
    }
}

// ------------------------------------------------------------------ view sync
fn sync_all(s: &mut St, a: &mut App) {
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_tab(s.tab as i32);
    g.set_zone(s.zone as i32);
    g.set_focus(if s.items.is_empty() { -1 } else { s.focus as i32 });
    g.set_busy(s.loading);
    g.set_status(if s.loading && s.items.is_empty() { tr_ref("sgdb.loading").into() } else { s.status.as_str().into() });
    g.set_subtitle(game_line(s).into());
    g.set_chip(if s.offline { tr_ref("sgdb.offline_cache").into() } else { "".into() });
    sync_preview(s, a);
}

fn ensure_visible(s: &St, a: &App) {
    let t = s.t();
    let g = geo(a, t);
    let ui = a.ui();
    let st_ = ui.global::<SgdbState>();
    let row = (s.focus / t.cols()) as f32;
    let (top, bot) = (row * (g.ch + g.gap), row * (g.ch + g.gap) + g.ch);
    let vy = -st_.get_vy();
    if top < vy {
        st_.set_vy(-top);
    } else if bot > vy + g.gh {
        st_.set_vy(-(bot - g.gh));
    }
}

/// End of every entry point: mirrors, hints, nav signature.
fn finish(s: &mut St, a: &mut App) {
    MODE.store(s.mode as u8, Ordering::Relaxed);
    let sig = (s.mode as i64) * 1_000_000_000 + (s.tab as i64) * 100_000_000 + (s.zone as i64) * 10_000_000 + s.focus as i64 * 100 + s.list_idx as i64 + s.osk_text.len() as i64 * 7 + OPEN.load(Ordering::Relaxed) as i64;
    SIG.store(sig, Ordering::Relaxed);
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_list_open(s.mode == Mode::List);
    g.set_osk_open(s.mode == Mode::Osk);
    // ввод — общая виртуальная клавиатура (screens/osk.rs), привязанная на время режима Osk
    if s.mode == Mode::Osk { crate::screens::osk::attach(a, SGDB_SINK); } else { crate::screens::osk::detach(a, SGDB_SINK); }
    g.set_edit_open(s.mode == Mode::Edit);
    g.set_hero_edit_open(s.mode == Mode::HeroEdit);
    a.update_hint();
    kick(s, a);
}

// ------------------------------------------------------------------ input
pub fn handle(a: &mut App, act: Action) {
    if !is_open() {
        return;
    }
    let mut s = st();
    let s = &mut *s;
    if BATCH_RUNNING.load(Ordering::Relaxed) {
        if act == Action::B {
            if let Some(c) = &s.batch_cancel {
                c.store(true, Ordering::Relaxed);
            }
            a.toast(tr_ref("sgdb.batch_cancelling"));
        }
        return;
    }
    match s.mode {
        Mode::Grid => handle_grid(s, a, act),
        Mode::List => handle_list(s, a, act),
        Mode::Osk => handle_osk(s, a, act),
        Mode::Edit => handle_edit(s, a, act),
        Mode::HeroEdit => handle_hero_edit(s, a, act),
    }
    if is_open() {
        finish(s, a);
    }
}

fn handle_grid(s: &mut St, a: &mut App, act: Action) {
    let cols = s.t().cols();
    match act {
        Action::B => return close_from(s, a),
        Action::LB => return switch_tab(s, a, (s.tab + AType::ALL.len() - 1) % AType::ALL.len()),
        Action::RB => return switch_tab(s, a, s.tab + 1),
        Action::X => return open_filters(s, a),
        Action::Y => {
            let n = s.game.as_ref().map(|g| g.1.clone()).unwrap_or_else(|| s.name.clone());
            return open_osk(s, a, OskFor::Search, &n);
        }
        Action::Menu => return open_actions(s, a),
        _ => {}
    }
    if s.zone == 0 {
        match act {
            Action::Left => switch_tab(s, a, (s.tab + AType::ALL.len() - 1) % AType::ALL.len()),
            Action::Right => switch_tab(s, a, s.tab + 1),
            Action::Down | Action::A if !s.items.is_empty() => {
                s.zone = 1;
                sync_all(s, a);
            }
            _ => {}
        }
        return;
    }
    let n = s.items.len();
    let old = s.focus;
    match act {
        Action::Left if s.focus % cols > 0 => s.focus -= 1,
        Action::Right if s.focus % cols + 1 < cols && s.focus + 1 < n => s.focus += 1,
        Action::Up => {
            if s.focus >= cols {
                s.focus -= cols;
            } else {
                s.zone = 0;
            }
        }
        Action::Down if s.focus + cols < n => s.focus += cols,
        Action::Down if s.focus / cols + 1 < n.div_ceil(cols) => s.focus = n - 1,
        Action::A => return apply_focused(s, a),
        _ => {}
    }
    if s.focus != old || act == Action::Up {
        ensure_visible(s, a);
        sync_all(s, a);
        if s.focus != old {
            focus_changed(s, a);
        }
        request_window(s, a);
    }
}

fn close_from(s: &mut St, a: &mut App) {
    let _ = s;
    // St is borrowed by the caller: close() re-borrows, so defer it
    post(|a| close(a));
    let _ = a;
}

fn touch_tab(a: &mut App, i: usize) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if s.mode != Mode::Grid || BATCH_RUNNING.load(Ordering::Relaxed) {
        return;
    }
    s.zone = 0;
    if i != s.tab {
        switch_tab(&mut s, a, i);
    }
    finish(&mut s, a);
}

/// Touch: first tap selects (preview), second tap on the same tile applies.
fn touch_tile(a: &mut App, i: usize) {
    if !is_open() {
        return;
    }
    let mut s = st();
    if s.mode != Mode::Grid || BATCH_RUNNING.load(Ordering::Relaxed) || i >= s.items.len() {
        return;
    }
    s.zone = 1;
    if s.focus == i && s.items[i].img.is_some() {
        apply_focused(&mut s, a);
    } else {
        s.focus = i;
        ensure_visible(&s, a);
        sync_all(&mut s, a);
        focus_changed(&mut s, a);
    }
    finish(&mut s, a);
}

// ------------------------------------------------------------------ lists: filters / actions / games
fn open_list(s: &mut St, a: &mut App, kind: ListKind, title: &str, rows: Vec<SgdbRow>, wide: bool) {
    s.mode = Mode::List;
    s.list = kind;
    s.list_idx = rows.iter().position(|r| r.kind != 2).unwrap_or(0);
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_list_title(title.into());
    g.set_list_rows(ModelRc::new(VecModel::from(rows)));
    g.set_list_wide(wide);
    g.set_list_vy(0.0);
    sync_list(s, a);
}

fn sync_list(s: &St, a: &App) {
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_list_idx(s.list_idx as i32);
    // keep the focused row inside the 44 px row viewport
    let n = g.get_list_rows().row_count();
    let view_h = ((ui.window().size().height as f32 / ui.window().scale_factor()) - 150.0).min(64.0 + n as f32 * 44.0) - 56.0;
    let (top, bot) = (s.list_idx as f32 * 44.0, s.list_idx as f32 * 44.0 + 44.0);
    let vy = -g.get_list_vy();
    if top < vy {
        g.set_list_vy(-top);
    } else if bot > vy + view_h {
        g.set_list_vy(-(bot - view_h));
    }
}

fn open_filters(s: &mut St, a: &mut App) {
    let t = s.t();
    let f = s.filters.get(&t).cloned().unwrap_or_else(|| Filters::default_for(t));
    s.fbefore = Some(f.clone());
    let rows = filter_rows(t, &f);
    s.frows = rows.iter().map(|r| r.1.clone()).collect();
    open_list(s, a, ListKind::Filters, &tr_args("sgdb.filter_title", &[("asset", t.label())]), rows.into_iter().map(|r| r.0).collect(), false);
}

fn filter_rows(t: AType, f: &Filters) -> Vec<(SgdbRow, FRow)> {
    let hdr = |l: &str| (SgdbRow { label: l.into(), kind: 2, on: false, sub: "".into() }, FRow::Header);
    let tog = |l: &str, on: bool, k: FRow| (SgdbRow { label: l.into(), kind: 1, on, sub: "".into() }, k);
    let mut r = vec![hdr(tr_ref("sgdb.type")), tog(tr_ref("sgdb.static"), f.stat, FRow::Stat), tog(tr_ref("sgdb.animated"), f.animated, FRow::Anim)];
    r.push(hdr(tr_ref("sgdb.tags")));
    r.push(tog(tr_ref("sgdb.untagged"), f.untagged, FRow::Untagged));
    r.push(tog(tr_ref("sgdb.adult"), f.adult, FRow::Adult));
    r.push(tog(tr_ref("sgdb.humor"), f.humor, FRow::Humor));
    r.push(tog(tr_ref("sgdb.epilepsy"), f.epilepsy, FRow::Epilepsy));
    r.push(hdr(tr_ref("sgdb.styles")));
    for (v, l) in t.style_opts() {
        r.push(tog(l, f.styles.iter().any(|x| x == v), FRow::Style(v.into())));
    }
    if !t.dim_opts().is_empty() {
        r.push(hdr(tr_ref("sgdb.sizes")));
        for d in t.dim_opts() {
            r.push(tog(&d.replace('x', "×"), f.dimensions.contains(&d), FRow::Dim(d)));
        }
    }
    r.push(hdr(tr_ref("sgdb.file_types")));
    for (v, l) in t.mime_opts() {
        r.push(tog(l, f.mimes.iter().any(|x| x == v), FRow::Mime(v.into())));
    }
    r.push((SgdbRow { label: tr_ref("sgdb.reset_filters").into(), kind: 0, on: false, sub: "".into() }, FRow::Reset));
    r
}

fn open_actions(s: &mut St, a: &mut App) {
    let t = s.t();
    let mut rows: Vec<(String, String, Op)> = vec![];
    let undo = apply::undo_state(s.appid, t);
    if undo != apply::Undo::Nothing {
        rows.push((tr_ref("sgdb.undo_last").into(), t.label().into(), Op::Undo));
    }
    rows.push((tr_ref("sgdb.reset_default").into(), t.label().into(), Op::Reset));
    if t != AType::Icon {
        rows.push((tr_ref("sgdb.blank").into(), "".into(), Op::Blank));
    }
    if matches!(t, AType::Logo | AType::Hero) {
        rows.push((tr_ref("sgdb.logo_edit").into(), "".into(), Op::LogoEdit));
        if logo_layout::can_undo(s.appid) { rows.push((tr_ref("sgdb.logo_undo").into(), "".into(), Op::LogoUndo)); }
        rows.push((tr_ref("sgdb.logo_reset").into(), "".into(), Op::LogoReset));
    }
    if t == AType::Hero {
        rows.push((tr_ref("sgdb.hero_edit").into(), "".into(), Op::HeroEdit));
        if hero_crop::can_undo(s.appid) {
            rows.push((tr_ref("sgdb.hero_undo").into(), "".into(), Op::HeroUndo));
        }
        rows.push((tr_ref("sgdb.hero_reset").into(), "".into(), Op::HeroReset));
    }
    rows.push((tr_ref("sgdb.filters_option").into(), "X".into(), Op::Filters));
    rows.push((tr_ref("sgdb.search_by_name").into(), "Y".into(), Op::Search));
    rows.push((tr_ref("sgdb.pick_game").into(), "".into(), Op::Pick));
    let missing = missing_for(a, t).len();
    rows.push((tr_args("sgdb.batch_apply", &[("missing", &missing.to_string())]), t.label().into(), Op::Batch));
    rows.push((tr_ref("sgdb.api_key_option").into(), if s.client.key.is_some() { tr_ref("sgdb.key_set") } else { tr_ref("sgdb.key_unset") }.into(), Op::Key));
    rows.push((tr_ref("sgdb.close").into(), "".into(), Op::Close));
    s.ops = rows.iter().map(|r| r.2).collect();
    let srows = rows.into_iter().map(|r| SgdbRow { label: r.0.into(), kind: 0, on: false, sub: r.1.into() }).collect();
    open_list(s, a, ListKind::Actions, tr_ref("footer.actions"), srows, false);
}

fn handle_list(s: &mut St, a: &mut App, act: Action) {
    let n = s.list_rows_len(a);
    let kinds: Vec<i32> = a.ui().global::<SgdbState>().get_list_rows().iter().map(|r| r.kind).collect();
    let mut step = |s: &mut St, d: i32| {
        let mut i = s.list_idx as i32 + d;
        while i >= 0 && (i as usize) < n && kinds[i as usize] == 2 {
            i += d;
        }
        if i >= 0 && (i as usize) < n {
            s.list_idx = i as usize;
        }
    };
    match act {
        Action::Up => step(s, -1),
        Action::Down => step(s, 1),
        Action::A => return list_activate(s, a),
        Action::B | Action::X | Action::Menu => return close_list(s, a),
        _ => return,
    }
    sync_list(s, a);
}

impl St {
    fn list_rows_len(&self, a: &App) -> usize {
        a.ui().global::<SgdbState>().get_list_rows().row_count()
    }
}

fn close_list(s: &mut St, a: &mut App) {
    let was = s.list;
    s.mode = Mode::Grid;
    if was == ListKind::Filters {
        let t = s.t();
        let now = s.filters.get(&t).cloned();
        if let (Some(before), Some(now)) = (s.fbefore.take(), now) {
            if before != now {
                s.settings.set_filters(t, &now);
                start_search(s, a);
            }
        }
    }
    sync_all(s, a);
}

fn list_activate(s: &mut St, a: &mut App) {
    match s.list {
        ListKind::Filters => {
            let t = s.t();
            let Some(row) = s.frows.get(s.list_idx).cloned() else { return };
            let f = s.filters.entry(t).or_insert_with(|| Filters::default_for(t));
            let toggle = |v: &mut Vec<String>, x: &str| {
                if let Some(p) = v.iter().position(|y| y == x) {
                    v.remove(p);
                } else {
                    v.push(x.into());
                }
            };
            match row {
                FRow::Stat => f.stat = !f.stat,
                FRow::Anim => f.animated = !f.animated,
                FRow::Untagged => f.untagged = !f.untagged,
                FRow::Adult => f.adult = !f.adult,
                FRow::Humor => f.humor = !f.humor,
                FRow::Epilepsy => f.epilepsy = !f.epilepsy,
                FRow::Style(v) => toggle(&mut f.styles, &v),
                FRow::Dim(v) => toggle(&mut f.dimensions, &v),
                FRow::Mime(v) => toggle(&mut f.mimes, &v),
                FRow::Reset => *f = Filters::default_for(t),
                FRow::Header => {}
            }
            let f = f.clone();
            let rows = filter_rows(t, &f);
            a.ui().global::<SgdbState>().set_list_rows(ModelRc::new(VecModel::from(rows.into_iter().map(|r| r.0).collect::<Vec<_>>())));
        }
        ListKind::Games => {
            let Some((id, name)) = s.games.get(s.list_idx).cloned() else { return };
            s.settings.set_game_for(s.appid, Some((id, name.clone())));
            s.game = Some((id, name));
            s.mode = Mode::Grid;
            start_search(s, a);
        }
        ListKind::Actions => {
            let Some(op) = s.ops.get(s.list_idx).copied() else { return };
            s.mode = Mode::Grid;
            run_op(s, a, op);
        }
    }
}

fn run_op(s: &mut St, a: &mut App, op: Op) {
    let t = s.t();
    match op {
        Op::Close => post(|a| close(a)),
        Op::Filters => open_filters(s, a),
        Op::Search => {
            let n = s.game.as_ref().map(|g| g.1.clone()).unwrap_or_else(|| s.name.clone());
            open_osk(s, a, OskFor::Search, &n)
        }
        Op::Pick => {
            let n = s.name.clone();
            spawn_game_search(s, n);
        }
        Op::Key => open_osk(s, a, OskFor::Key, ""),
        Op::LogoEdit => enter_edit(s, a),
        Op::HeroEdit => enter_hero_edit(s, a),
        Op::HeroReset => match hero_crop::reset(s.appid) {
            Ok(true) => {
                a.update_bg();
                a.toast(tr_ref("sgdb.hero_reset_done"));
            }
            Ok(false) => a.toast(tr_ref("sgdb.hero_already_default")),
            Err(e) => a.toast(&tr_args("sgdb.error", &[("e", &e.to_string())])),
        },
        Op::HeroUndo => match hero_crop::undo(s.appid) {
            Ok(true) => {
                a.update_bg();
                a.toast(tr_ref("sgdb.hero_undo_done"));
            }
            Ok(false) => a.toast(tr_ref("sgdb.nothing_to_undo")),
            Err(e) => a.toast(&tr_args("sgdb.error", &[("e", &e.to_string())])),
        },
        Op::LogoReset | Op::LogoUndo => {
            let r = if matches!(op, Op::LogoReset) { logo_layout::reset(s.appid) } else { logo_layout::undo(s.appid) };
            match r {
                Ok(true) => { a.refresh_game_page(); a.toast(tr_ref("sgdb.logo_saved")); }
                Ok(false) => a.toast(tr_ref("sgdb.logo_unchanged")),
                Err(e) => a.toast(&tr_args("sgdb.error", &[("e", &e.to_string())])),
            }
            sync_preview(s, a);
        }
        Op::Reset => {
            let (id, b, root, acct) = (s.appid, crate::actions::bridge(), a.base.steam_root.clone(), a.lib.active_user);
            std::thread::spawn(move || {
                let grid = grid_for(b.as_ref(), &root, acct);
                let prev = grid.as_ref().and_then(|g| apply::current_custom(g, id, t));
                if !apply::dry() && t != AType::Icon && prev.is_some() {
                    apply::save_undo(id, t, prev.as_deref());
                }
                let r = if t == AType::Icon { Err(tr("sgdb.icon_reset_hint")) } else { apply::clear_art(b.as_ref(), id, t) };
                post(move |a| {
                    a.toast(&match &r {
                        Ok(()) => tr_args("sgdb.artwork_reset", &[("asset", t.label())]),
                        Err(e) => tr_args("sgdb.error", &[("e", &e.to_string())]),
                    });
                    if r.is_ok() {
                        refresh_art(a, vec![id], expect_art(t, false));
                    }
                });
            });
        }
        Op::Blank => {
            let tmp = cache::root().join("blank.png");
            let _ = std::fs::create_dir_all(cache::root());
            let _ = std::fs::write(&tmp, BLANK_PNG);
            spawn_apply_file(s, a, tmp);
        }
        Op::Undo => {
            match apply::undo_state(s.appid, t) {
                apply::Undo::Restore(p) => spawn_apply_file(s, a, p),
                apply::Undo::Clear => run_op(s, a, Op::Reset),
                apply::Undo::Nothing => a.toast(tr_ref("sgdb.nothing_to_undo")),
            };
        }
        Op::Batch => start_batch(s, a),
    }
}

/// 1x1 transparent PNG (plugin: "Use Invisible Asset").
const BLANK_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xB5, 0x1C, 0x0C, 0x02, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44,
    0x41, 0x54, 0x78, 0xDA, 0x63, 0x64, 0x60, 0x00, 0x00, 0x00, 0x06, 0x00, 0x02, 0x30, 0x81, 0xD0, 0x2F, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn spawn_game_search(s: &mut St, term: String) {
    let client = s.client.clone();
    let gen = s.gen;
    std::thread::spawn(move || {
        let r = client.search_games(&term);
        post(move |a| {
            if !is_open() {
                return;
            }
            let mut s = st();
            if s.gen != gen {
                return;
            }
            match r {
                Ok((rows, _)) if !rows.is_empty() => {
                    s.games = rows.clone();
                    let srows = rows.into_iter().map(|g| SgdbRow { label: g.1.into(), kind: 0, on: false, sub: format!("#{}", g.0).into() }).collect();
                    open_list(&mut s, a, ListKind::Games, tr_ref("sgdb.choose_game"), srows, true);
                }
                Ok(_) => a.toast(tr_ref("sgdb.no_results")),
                Err(e) => a.toast(&format!("SteamGridDB: {e}")),
            }
            finish(&mut s, a);
        });
    });
}

// ------------------------------------------------------------------ on-screen keyboard
/// Приёмник общей клавиатуры: символы → строка запроса/ключа, Поиск/Enter → отправка, закрытие → назад к сетке.
const SGDB_SINK: crate::screens::osk::ExtSink = crate::screens::osk::ExtSink { id: 2, mode: crate::screens::osk::InputMode::Search, typed: osk_typed, enter: osk_enter, cancel: osk_cancel };

fn osk_typed(a: &mut App, c: Option<char>) {
    if !is_open() { return; }
    let mut s = st();
    match c {
        Some(c) if !c.is_control() => s.osk_text.push(c),
        None => { s.osk_text.pop(); }
        _ => {}
    }
    sync_osk(&s, a);
    finish(&mut s, a);
}

fn osk_enter(a: &mut App) {
    if !is_open() { return; }
    let mut s = st();
    osk_submit(&mut s, a);
    finish(&mut s, a);
}

fn osk_cancel(a: &mut App) {
    if !is_open() { return; }
    let mut s = st();
    s.mode = Mode::Grid;
    finish(&mut s, a);
}

fn open_osk(s: &mut St, a: &mut App, f: OskFor, text: &str) {
    s.mode = Mode::Osk;
    s.osk_for = f;
    s.osk_text = text.to_string();
    sync_osk(s, a);
}

fn sync_osk(s: &St, a: &App) {
    let ui = a.ui();
    let g = ui.global::<SgdbState>();
    g.set_osk_prompt(match s.osk_for {
        OskFor::Search => tr_ref("sgdb.search_prompt").into(),
        OskFor::Key => tr_ref("sgdb.api_key_prompt").into(),
    });
    g.set_osk_text(s.osk_text.as_str().into());
}

fn osk_submit(s: &mut St, a: &mut App) {
    let text = s.osk_text.trim().to_string();
    s.mode = Mode::Grid;
    if text.is_empty() {
        return;
    }
    match s.osk_for {
        OskFor::Key => {
            s.settings.set_api_key(&text);
            s.client = Client::from(&s.settings);
            *CLIENT.lock().unwrap() = Some(s.client.clone());
            a.toast(tr_ref("sgdb.api_key_saved"));
            start_search(s, a);
        }
        OskFor::Search => spawn_game_search(s, text),
    }
}

fn handle_osk(s: &mut St, a: &mut App, act: Action) {
    // клавиатура перехватывает ввод раньше; сюда доходят только Menu (готово) и B (назад)
    match act {
        Action::B => s.mode = Mode::Grid,
        Action::Menu => return osk_submit(s, a),
        _ => {}
    }
}

// ------------------------------------------------------------------ logo position editor
fn enter_edit(s: &mut St, a: &mut App) {
    s.lc = logo_layout::get(s.appid);
    s.hfocus = 0;
    s.mode = Mode::Edit;
    hero_refresh(s, a);
}

fn handle_edit(s: &mut St, a: &mut App, act: Action) {
    handle_hero_edit(s, a, act);
}

// ------------------------------------------------------------------ hero crop editor (DeckShell only: Steam has no hero crop call)
// Buttons in on-screen order: index = focus index = `HBtn idx` in sgdb.slint. Rows: pan (← ↑ ↓ →), zoom (− +), actions.
const HERO_ROWS: [(usize, usize); 3] = [(0, 4), (4, 2), (6, 3)]; // (first index, count)
const HERO_BTNS: usize = 9;
const HERO_RESET: usize = 6;
const HERO_CANCEL: usize = 7;
const HERO_SAVE: usize = 8;

fn hero_row(f: usize) -> usize {
    HERO_ROWS.iter().position(|&(first, n)| (first..first + n).contains(&f)).unwrap_or(0)
}

/// Focus after one D-pad press: left/right stay in the row (no wrap), up/down go to the neighbouring row at the same column.
pub fn hero_focus_move(f: usize, dir: Action) -> usize {
    let r = hero_row(f);
    let (first, n) = HERO_ROWS[r];
    let col = f - first;
    match dir {
        Action::Left => first + col.saturating_sub(1),
        Action::Right => first + (col + 1).min(n - 1),
        Action::Up if r > 0 => {
            let (pf, pn) = HERO_ROWS[r - 1];
            pf + col.min(pn - 1)
        }
        Action::Down if r + 1 < HERO_ROWS.len() => {
            let (nf, nn) = HERO_ROWS[r + 1];
            nf + col.min(nn - 1)
        }
        _ => f,
    }
}

fn hero_frame(c: hero_crop::HeroCrop) -> HeroFrame {
    HeroFrame { zoom: c.zoom, pan_x: c.pan_x, pan_y: c.pan_y }
}

/// The picture follows the arrow: ← moves the picture left, so the frame shows more of the image's right side (pan_x grows).
fn hero_pan(s: &mut St, dir: Action, step: f32) {
    if s.mode == Mode::Edit {
        s.lc = match dir {
            Action::Left => s.lc.nudged(-step, 0.0), Action::Right => s.lc.nudged(step, 0.0),
            Action::Up => s.lc.nudged(0.0, -step), Action::Down => s.lc.nudged(0.0, step), _ => s.lc,
        };
        return;
    }
    s.hc = match dir {
        Action::Left => s.hc.panned(step, 0.0),
        Action::Right => s.hc.panned(-step, 0.0),
        Action::Up => s.hc.panned(0.0, step),
        Action::Down => s.hc.panned(0.0, -step),
        _ => s.hc,
    };
}

fn enter_hero_edit(s: &mut St, a: &mut App) {
    s.hc = hero_crop::get(s.appid);
    s.hfocus = 0;
    s.mode = Mode::HeroEdit;
    hero_refresh(s, a);
}

/// Numeric readout: zoom and pan in percent of the full range (-100% = left/top edge on the frame edge).
fn hero_edit_info(s: &St, a: &App) {
    let c = if s.mode == Mode::Edit { hero_crop::HeroCrop { zoom: s.lc.zoom, pan_x: s.lc.pan_x, pan_y: s.lc.pan_y } } else { s.hc };
    // Logo: Y is shown on the stock scale (stock position = +100%); the banner shows the stored pan.
    let (px, py) = if s.mode == Mode::Edit { s.lc.readout() } else { ((c.pan_x * 100.0).round() as i32, (c.pan_y * 100.0).round() as i32) };
    let ui = a.ui();
    ui.global::<SgdbState>().set_hero_edit_info(tr_args("sgdb.hero_position", &[("zoom", &format!("{:.1}", c.zoom)), ("x", &format!("{:+}", px)), ("y", &format!("{:+}", py))]).into());
}

/// Everything the editor shows after a change: readout, focus ring, preview frame.
fn hero_refresh(s: &mut St, a: &mut App) {
    hero_edit_info(s, a);
    let ui = a.ui();
    ui.global::<SgdbState>().set_hero_focus(s.hfocus as i32);
    sync_preview(s, a);
}

/// Leave the editor; `save` stores the crop (one undo level), otherwise the edit is dropped.
fn hero_leave(s: &mut St, a: &mut App, save: bool) {
    let logo = s.mode == Mode::Edit;
    s.mode = Mode::Grid;
    if logo {
        if save {
            match logo_layout::set(s.appid, s.lc) {
                Ok(true) => { a.refresh_game_page(); a.toast(tr_ref("sgdb.logo_saved")); },
                Ok(false) => a.toast(tr_ref("sgdb.logo_unchanged")),
                Err(e) => a.toast(&tr_args("sgdb.error", &[("e", &e.to_string())])),
            }
        }
        return sync_preview(s, a);
    }
    if save {
        let id = s.appid;
        match hero_crop::set(id, s.hc) {
            Ok(true) => a.toast(tr_ref("sgdb.hero_saved")),
            Ok(false) => a.toast(tr_ref("sgdb.hero_unchanged")),
            Err(e) => a.toast(&tr_args("sgdb.error", &[("e", &e.to_string())])),
        }
        a.update_bg();
    }
    sync_preview(s, a);
}

/// A on an editor button (touch sends the same index). Pan and zoom buttons change the edit; the rest leave or reset it.
fn hero_press(s: &mut St, a: &mut App, btn: usize) {
    use hero_crop::{PAN_STEP, ZOOM_STEP};
    match btn {
        0 => hero_pan(s, Action::Left, PAN_STEP),
        1 => hero_pan(s, Action::Up, PAN_STEP),
        2 => hero_pan(s, Action::Down, PAN_STEP),
        3 => hero_pan(s, Action::Right, PAN_STEP),
        4 if s.mode == Mode::Edit => s.lc = s.lc.zoomed(-ZOOM_STEP),
        5 if s.mode == Mode::Edit => s.lc = s.lc.zoomed(ZOOM_STEP),
        HERO_RESET if s.mode == Mode::Edit => s.lc = logo_layout::LogoLayout::default(),
        4 => s.hc = s.hc.zoomed(-ZOOM_STEP),
        5 => s.hc = s.hc.zoomed(ZOOM_STEP),
        HERO_RESET => s.hc = hero_crop::HeroCrop::default(),
        HERO_CANCEL => return hero_leave(s, a, false),
        HERO_SAVE => return hero_leave(s, a, true),
        _ => return,
    }
    hero_refresh(s, a);
}

fn handle_hero_edit(s: &mut St, a: &mut App, act: Action) {
    use hero_crop::PAN_STEP_STICK;
    let analog = crate::input::analog_dir();
    match act {
        // left stick: direct pan, the focus stays put
        Action::Left | Action::Right | Action::Up | Action::Down if analog => hero_pan(s, act, PAN_STEP_STICK),
        // D-pad: move the focus over the buttons
        Action::Left | Action::Right | Action::Up | Action::Down => s.hfocus = hero_focus_move(s.hfocus, act),
        Action::A => {
            let f = s.hfocus;
            return hero_press(s, a, f);
        }
        Action::LB if s.mode == Mode::Edit => s.lc = s.lc.zoomed(-logo_layout::ZOOM_STEP),
        Action::RB if s.mode == Mode::Edit => s.lc = s.lc.zoomed(logo_layout::ZOOM_STEP),
        Action::X if s.mode == Mode::Edit => s.lc = logo_layout::LogoLayout::default(),
        Action::LB => s.hc = s.hc.zoomed(-hero_crop::ZOOM_STEP),
        Action::RB => s.hc = s.hc.zoomed(hero_crop::ZOOM_STEP),
        Action::X => s.hc = hero_crop::HeroCrop::default(),
        Action::B => return hero_leave(s, a, false),
        _ => return,
    }
    hero_refresh(s, a);
}

/// Touch buttons of the hero editor (indices as in sgdb.slint `hero-clicked`): focus the button and press it.
fn touch_hero(a: &mut App, i: i32) {
    if !is_open() || !matches!(st().mode, Mode::HeroEdit | Mode::Edit) {
        return;
    }
    let mut s = st();
    let s = &mut *s;
    match usize::try_from(i) {
        Ok(btn) if btn < HERO_BTNS => {
            s.hfocus = btn;
            hero_press(s, a, btn);
        }
        _ => return,
    }
    if is_open() {
        finish(s, a);
    }
}

// ------------------------------------------------------------------ apply / undo / refresh
fn grid_for(b: Option<&steam_bridge::Bridge>, root: &Path, fallback: Option<u32>) -> Option<PathBuf> {
    let acct = b.and_then(|b| b.active_account().ok()).or(fallback)?;
    Some(apply::grid_dir(root, acct))
}

#[cfg(test)]
fn stat_of(p: &Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What A applies: the focused tile, read once at press time. Download, file and Steam call use `id`/`url`
/// from here, never the index again.
#[derive(Debug, PartialEq)]
struct ApplyPick {
    idx: usize,
    id: u64,
    url: String,
}

fn apply_pick(items: &[Item], focus: usize) -> Option<ApplyPick> {
    let it = items.get(focus)?;
    Some(ApplyPick { idx: focus, id: it.asset.id, url: it.asset.url.clone() })
}

/// Keeps the assets not seen before (`known` = ids already in the list, updated with the new ones). A result page can repeat
/// an asset that the static and animated answers both return, so duplicates inside one page are dropped too.
fn fresh_assets(known: &mut HashSet<u64>, assets: Vec<Asset>) -> Vec<Asset> {
    assets.into_iter().filter(|a| known.insert(a.id)).collect()
}

fn apply_focused(s: &mut St, a: &mut App) {
    if s.applying {
        // Say why the press did nothing instead of only playing the select sound.
        a.toast(tr_ref("sgdb.previous_loading"));
        return;
    }
    let Some(ApplyPick { idx, id: asset_id, url }) = apply_pick(&s.items, s.focus) else { return };
    s.items[idx].busy = true;
    set_row(s, idx);
    let client = s.client.clone();
    let id = s.appid;
    let gen = s.gen;
    let t = s.t();
    // download the full image off-thread, then continue as a normal file apply
    s.applying = true;
    std::thread::spawn(move || {
        let full = cache::full_path(&url);
        let r = if full.is_file() {
            cache::touch(&full);
            Ok(())
        } else {
            client.download(&url, &full)
        };
        post(move |a| {
            // The state outlives the screen. Clear the busy flag even when the screen was
            // closed during the download: otherwise A does nothing after reopening.
            st().applying = false;
            if !is_open() {
                return;
            }
            let mut s = st();
            if s.gen == gen && idx < s.items.len() {
                s.items[idx].busy = false;
                set_row(&s, idx);
            }
            match r {
                Ok(()) if s.appid == id && s.t() == t => {
                    eprintln!("[deckshell] sgdb: apply asset {asset_id} ({}) to app {id}", t.key());
                    spawn_apply_file(&mut s, a, full)
                }
                Ok(()) => {}
                Err(e) => a.toast(&tr_args("sgdb.download_failed", &[("e", &e.to_string())])),
            }
            finish(&mut s, a);
        });
    });
    a.toast(&tr_args("sgdb.artwork_loading", &[("asset", t.label())]));
}

/// Steam side of an apply (worker thread): undo snapshot -> Clear -> wait 500 ms -> Set (+ logo position for shortcuts) -> refresh.
fn spawn_apply_file(s: &mut St, a: &mut App, file: PathBuf) {
    let (id, t, shortcut) = (s.appid, s.t(), s.shortcut);
    let (b, root, acct) = (crate::actions::bridge(), a.base.steam_root.clone(), a.lib.active_user);
    std::thread::spawn(move || {
        let grid = grid_for(b.as_ref(), &root, acct);
        let prev = grid.as_ref().and_then(|g| apply::current_custom(g, id, t));
        let r: Result<Option<PathBuf>, String> = (|| {
            if t == AType::Icon {
                if shortcut {
                    return Err(tr_ref("sgdb.shortcut_icon_path").into());
                }
                let expected = std::fs::read(&file).map_err(|e| e.to_string())?;
                let path = apply::set_steam_icon(b.as_ref(), &root, id, &file)?;
                if !apply::dry() { confirmed_icon(&path, &expected)?; }
                return Ok(Some(path));
            }
            if !apply::dry() && file.parent() != Some(cache::undo_dir().as_path()) {
                apply::save_undo(id, t, prev.as_deref());
            } else if !apply::dry() {
                // undo of an undo: current becomes the new snapshot after the copy below was read
                let data = std::fs::read(&file).map_err(|e| e.to_string())?;
                let tmp = cache::root().join(format!("undo_swap_{id}"));
                let _ = std::fs::write(&tmp, data);
                apply::save_undo(id, t, prev.as_deref());
                apply::set_art(b.as_ref(), id, t, &tmp)?;
                let _ = std::fs::remove_file(&tmp);
                return Ok(None);
            }
            apply::set_art(b.as_ref(), id, t, &file)?;
            if t == AType::Logo && shortcut && grid.as_ref().map(|g| !g.join(format!("{id}.json")).exists()).unwrap_or(true) {
                // plugin: force a position json for shortcuts or the logo stays blank
                apply::set_logo_position(b.as_ref(), id, 0, 50.0, 50.0)?;
            }
            Ok(None)
        })();
        post(move |a| {
            match &r {
                Ok(_) if t == AType::Icon && apply::dry() => a.toast(tr_ref("sgdb.icon_dry_run")),
                Ok(_) => a.toast(&tr_args("sgdb.artwork_applied", &[("asset", t.label())])),
                Err(e) => a.toast(&tr_args("sgdb.apply_failed", &[("e", &e.to_string())])),
            }
            if let Ok(icon) = r {
                if t == AType::Icon {
                    // the icon branch always yields the written path; `None` only comes from the art branch
                    if let Some(icon) = icon { if !apply::dry() { refresh_icon(a, id, icon); } }
                } else {
                    refresh_art(a, vec![id], expect_art(t, true));
                }
            }
        });
    });
}

/// What Steam must show for the art of `t` before the screens reload: the kind resolves to the custom file (`custom`) or to
/// Steam's own art. `None` for icons, which go through `refresh_icon`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Expect {
    t: AType,
    custom: bool,
}

fn expect_art(t: AType, custom: bool) -> Option<Expect> {
    art_kind(t).map(|_| Expect { t, custom })
}

fn art_kind(t: AType) -> Option<steam_bridge::ArtKind> {
    use steam_bridge::ArtKind;
    Some(match t {
        AType::Capsule => ArtKind::Capsule,
        AType::Wide => ArtKind::Header,
        AType::Hero => ArtKind::Hero,
        AType::Logo => ArtKind::Logo,
        AType::Icon => return None,
    })
}

/// Steam's art list has caught up with the write for this app. An app with no list at all (non-Steam shortcuts: Steam gives
/// none) cannot be checked through Steam, so it counts as settled; `fill_from_grid` then supplies the file.
fn art_settled(set: &steam_bridge::ArtSet, e: Expect) -> bool {
    let Some(kind) = art_kind(e.t) else { return true };
    let no_lists = set.capsule.is_none() && set.header.is_none() && set.hero.is_none() && set.logo.is_none() && set.icon.is_none();
    no_lists || set.custom.contains(&kind) == e.custom
}

/// Where Steam gave no file for a kind, the custom file in the grid is what the user chose. Steam's own files are never replaced.
/// Not used for a reset: after Clear the grid file stays on disk while Steam shows its stock art.
fn fill_from_grid(set: &mut steam_bridge::ArtSet, grid: &Path, id: u32, allowed: bool) {
    use steam_bridge::ArtKind;
    if !allowed {
        return;
    }
    for (t, kind) in [(AType::Capsule, ArtKind::Capsule), (AType::Wide, ArtKind::Header), (AType::Hero, ArtKind::Hero), (AType::Logo, ArtKind::Logo)] {
        let slot = match kind {
            ArtKind::Capsule => &mut set.capsule,
            ArtKind::Header => &mut set.header,
            ArtKind::Hero => &mut set.hero,
            _ => &mut set.logo,
        };
        if slot.is_none() {
            if let Some(p) = apply::current_custom(grid, id, t) {
                *slot = Some(p);
                if !set.custom.contains(&kind) {
                    set.custom.push(kind);
                }
            }
        }
    }
}

/// Steam's resolved art for `ids`, read once Steam shows what was written (`e`). Steam's art list can lag its own file write:
/// on the Deck the list still gave the stock hero first about 0.5 s after the write, and a single read then kept that stock hero
/// on screen until the next restart. So the read repeats until the list settles or `ART_SETTLE` runs out.
fn settled_sets(b: Option<&steam_bridge::Bridge>, root: &Path, acct: Option<u32>, ids: &[u32], e: Option<Expect>) -> Option<HashMap<u32, steam_bridge::ArtSet>> {
    let b = b?;
    let deadline = Instant::now() + ART_SETTLE;
    let mut sets = None;
    loop {
        let now = b.art_all(root, ids).ok().map(|s| s.1);
        let settled = match (&now, e) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(m), Some(e)) => ids.iter().all(|id| m.get(id).map_or(true, |s| art_settled(s, e))),
        };
        sets = now.or(sets);
        if settled {
            break;
        }
        if Instant::now() >= deadline {
            eprintln!("[deckshell] sgdb: Steam still shows the previous art for {ids:?}");
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if let (Some(m), Some(g)) = (sets.as_mut(), grid_for(Some(b), root, acct)) {
        let allowed = e.map_or(true, |e| e.custom);
        for id in ids {
            if let Some(s) = m.get_mut(id) {
                fill_from_grid(s, &g, *id, allowed);
            }
        }
    }
    sets
}

/// Upper bound for re-reading Steam's art list after a write (each read costs up to ~1 s on the Deck).
const ART_SETTLE: Duration = Duration::from_secs(10);

/// After Steam changed art: re-resolve art through steam-bridge once it shows the write, then refresh every screen.
fn refresh_art(a: &mut App, ids: Vec<u32>, e: Option<Expect>) {
    let (b, root, acct) = (crate::actions::bridge(), a.base.steam_root.clone(), a.lib.active_user);
    // dry run: one read, no expectation (nothing was written)
    let e = if apply::dry() { None } else { e };
    std::thread::spawn(move || {
        let sets = settled_sets(b.as_ref(), &root, acct, &ids, e);
        post(move |a| apply_art_refresh(a, ids, sets));
    });
}

// The icon writer returns the actual librarycache destination. Re-read it directly:
// Steam's resolver may still prefer a stale grid icon or be temporarily unavailable.
fn confirmed_icon(path: &Path, expected: &[u8]) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes != expected { return Err(tr_ref("sgdb.icon_unconfirmed").into()); }
    Ok(())
}

fn select_icon(art: &mut steam_data::Art, path: PathBuf) {
    art.icon = Some(path);
}

fn refresh_icon(a: &mut App, id: u32, path: PathBuf) {
    let mut lib = (*a.lib).clone();
    if let Some(g) = lib.games.iter_mut().find(|g| g.app_id == id) {
        select_icon(&mut g.art, path);
    }
    a.lib = Arc::new(lib);
    // Invalidate the local preview before scheduling a fresh read of the same path.
    if is_open() {
        let mut s = st();
        if s.appid == id {
            s.loc[Slot::Cur as usize] = None;
            let generation = s.gen;
            request_locals(&mut s, a, generation);
            sync_preview(&mut s, a);
        }
    }
    a.sync_lib();
}

/// Replace the art of `ids` in the shown library and force-reload every cached image of them (same path => same file name, new bytes).
fn apply_art_refresh(a: &mut App, ids: Vec<u32>, sets: Option<HashMap<u32, steam_bridge::ArtSet>>) {
    use crate::loader::Kind;
    let mut lib = (*a.lib).clone();
    for g in lib.games.iter_mut().filter(|g| ids.contains(&g.app_id)) {
        if let Some(s) = sets.as_ref().and_then(|m| m.get(&g.app_id)) {
            g.art = steam_data::Art { capsule: s.capsule.clone(), capsule_animated: s.capsule_animated.clone(), hero: s.hero.clone(), logo: s.logo.clone(), header: s.header.clone(), icon: s.icon.clone() };
        }
    }
    a.lib = Arc::new(lib);
    for id in &ids {
        for k in [Kind::Capsule, Kind::Hero, Kind::Logo, Kind::Header] {
            a.loader.invalidate(*id, k);
            a.imgs.remove(&(*id, k));
            a.requested.remove(&(*id, k));
            a.lru.retain(|x| *x != (*id, k));
        }
        a.anim_cache.remove(id);
        a.anim_requested.remove(id);
        a.hero_clips.forget(*id);
        a.logo_clips.forget(*id);
        if a.bg_game == Some(*id) { a.bg_game = None; }
        if let Some(&gi) = a.gidx.get(id) {
            for k in [Kind::Capsule, Kind::Hero, Kind::Logo, Kind::Header] {
                a.request(gi, k);
            }
        }
    }
    a.home_refresh();
    a.sync_lib();
    a.update_bg();
    a.update_game_art();
    a.logo_sync();
    a.anim_sync();
    // the open cover changer shows the new "now" image and the saved logo position
    if is_open() {
        let mut s = st();
        if ids.contains(&s.appid) {
            let gen_ = s.gen;
            request_locals(&mut s, a, gen_);
            sync_preview(&mut s, a);
        }
    }
    eprintln!("[deckshell] sgdb: art refreshed for {ids:?}");
}

// ------------------------------------------------------------------ batch: apply the top result to every game without art
fn art_missing(g: &steam_data::Game, t: AType) -> bool {
    match t {
        AType::Capsule => g.art.capsule.is_none(),
        AType::Wide => g.art.header.is_none(),
        AType::Hero => g.art.hero.is_none(),
        AType::Logo => g.art.logo.is_none(),
        AType::Icon => g.art.icon.is_none(),
    }
}

fn missing_for(a: &App, t: AType) -> Vec<(u32, String, bool)> {
    a.games().iter().filter(|g| art_missing(g, t) && !(t == AType::Icon && g.kind == GameKind::Shortcut)).map(|g| (g.app_id, g.name.clone(), g.kind == GameKind::Shortcut)).collect()
}

fn start_batch(s: &mut St, a: &mut App) {
    let t = s.t();
    let todo = missing_for(a, t);
    if todo.is_empty() {
        return a.toast(tr_ref("sgdb.batch_none"));
    }
    if s.client.key.is_none() {
        return a.toast(tr_ref("sgdb.key_required"));
    }
    let cancel = Arc::new(AtomicBool::new(false));
    s.batch_cancel = Some(cancel.clone());
    BATCH_RUNNING.store(true, Ordering::Relaxed);
    let (client, settings, filters) = (s.client.clone(), s.settings.clone(), s.filters.get(&t).cloned().unwrap_or_else(|| Filters::default_for(t)));
    let (b, root, acct) = (crate::actions::bridge(), a.base.steam_root.clone(), a.lib.active_user);
    let total = todo.len();
    a.toast(&tr_args("sgdb.batch_start", &[("total", &total.to_string()), ("asset", t.label())]));
    std::thread::spawn(move || {
        let (mut ok, mut fail) = (0usize, 0usize);
        let mut done_ids = vec![];
        let mut icons = Vec::new();
        let grid = grid_for(b.as_ref(), &root, acct);
        for (n, (id, name, shortcut)) in todo.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let r: Result<(), String> = (|| {
                let game = if shortcut {
                    match settings.game_for(id) {
                        Some(g) => Some(g.0),
                        None => Some(client.search_games(&name).map_err(|e| e.to_string())?.0.into_iter().next().ok_or(tr_ref("sgdb.batch_game_missing"))?.0),
                    }
                } else {
                    None
                };
                let (assets, _) = client.assets(t, id, game, &filters, 0).map_err(|e| e.to_string())?;
                let assets: Vec<Asset> = assets.into_iter().filter(|x| !filters.animated_only() || t.fits_tab(x)).collect();
                let pick = assets.iter().find(|x| !x.animated()).or(assets.first()).ok_or(tr_ref("sgdb.batch_no_artwork"))?;
                let full = cache::full_path(&pick.url);
                if !full.is_file() {
                    client.download(&pick.url, &full)?;
                }
                if t == AType::Icon {
                    if shortcut { return Err(tr_ref("sgdb.shortcut_icon_unsupported").into()); }
                    if apply::dry() { return Err(tr_ref("sgdb.batch_dry_icon").into()); }
                    let expected = std::fs::read(&full).map_err(|e| e.to_string())?;
                    let path = apply::set_steam_icon(b.as_ref(), &root, id, &full)?;
                    confirmed_icon(&path, &expected)?;
                    icons.push((id, path));
                    return Ok(());
                }
                if !apply::dry() {
                    let prev = grid.as_ref().and_then(|g| apply::current_custom(g, id, t));
                    apply::save_undo(id, t, prev.as_deref());
                }
                apply::set_art(b.as_ref(), id, t, &full)?;
                if t == AType::Logo && shortcut {
                    apply::set_logo_position(b.as_ref(), id, 0, 50.0, 50.0)?;
                }
                Ok(())
            })();
            match r {
                Ok(()) => {
                    ok += 1;
                    done_ids.push(id);
                    eprintln!("[deckshell] sgdb batch {}/{total}: {name} ({id}) ok", n + 1);
                }
                Err(e) => {
                    fail += 1;
                    eprintln!("[deckshell] sgdb batch {}/{total}: {name} ({id}) skipped: {e}", n + 1);
                }
            }
            let msg = tr_args("sgdb.batch_progress", &[("current", &(n + 1).to_string()), ("total", &total.to_string()), ("name", &name)]);
            post(move |a| a.toast(&msg));
            std::thread::sleep(Duration::from_millis(350));
        }
        BATCH_RUNNING.store(false, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(if apply::dry() { 0 } else { 1500 }));
        let sets = settled_sets(b.as_ref(), &root, acct, &done_ids, expect_art(t, true));
        post(move |a| {
            a.toast(&tr_args("sgdb.batch_done", &[("ok", &ok.to_string()), ("fail", &fail.to_string())]));
            if !done_ids.is_empty() {
                apply_art_refresh(a, done_ids, sets);
                for (id, path) in icons { refresh_icon(a, id, path); }
            }
            a.update_hint();
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_icon_refresh_regression() {
        let dir = std::env::temp_dir().join(format!("deckshell-refresh-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let grid = dir.join("grid");
        let icon = dir.join("librarycache/icon.jpg");
        let input = dir.join("input");
        let bak = dir.join("undo.bak");
        let none = dir.join("undo.none");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&grid, b"other artwork").unwrap();
        std::fs::write(&icon, b"old").unwrap();
        std::fs::write(&input, b"new").unwrap();
        let grid_before = stat_of(&grid);
        apply::replace_icon(&icon, &input, &bak, &none).unwrap();
        assert!(confirmed_icon(&icon, b"new").is_ok());
        assert_eq!(stat_of(&grid), grid_before);
        assert_eq!(std::fs::read(&grid).unwrap(), b"other artwork");
        let mut art = steam_data::Art::default();
        art.capsule = Some(grid.clone());
        art.header = Some(grid.clone());
        art.hero = Some(grid.clone());
        art.logo = Some(grid.clone());
        select_icon(&mut art, icon.clone());
        assert_eq!(art.icon.as_ref(), Some(&icon));
        assert!([art.capsule, art.header, art.hero, art.logo].iter().all(|p| p.as_ref() == Some(&grid)));
        apply::replace_icon(&icon, &bak, &bak, &none).unwrap();
        assert!(confirmed_icon(&icon, b"old").is_ok());
        assert!(confirmed_icon(&icon, b"new").is_err());
        assert!(confirmed_icon(&dir.join("missing"), b"new").is_err());
        assert!(apply::replace_icon(&icon, &dir.join("missing"), &bak, &none).is_err());
        assert!(apply::replace_icon(&icon, &icon, &bak, &none).is_err());
        assert_eq!(std::fs::read(&grid).unwrap(), b"other artwork");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filter_rows_cover_every_group() {
        let f = Filters::default_for(AType::Capsule);
        let r = filter_rows(AType::Capsule, &f);
        assert!(r.iter().any(|x| x.1 == FRow::Anim) && r.iter().any(|x| matches!(x.1, FRow::Dim(_))) && r.iter().any(|x| x.1 == FRow::Reset));
        let logo = filter_rows(AType::Logo, &Filters::default_for(AType::Logo));
        assert!(!logo.iter().any(|x| matches!(x.1, FRow::Dim(_))), "logos have no dimension filter");
    }

    #[test]
    fn blank_png_is_a_png() {
        let img = image::load_from_memory(BLANK_PNG).expect("decodes");
        assert_eq!((img.width(), img.height()), (1, 1));
    }
}

#[cfg(test)]
mod hero_focus_tests {
    use super::*;

    #[test]
    fn rows_cover_every_editor_button_once() {
        let total: usize = HERO_ROWS.iter().map(|r| r.1).sum();
        assert_eq!(total, HERO_BTNS);
        assert_eq!(HERO_ROWS[0].0, 0);
        for w in HERO_ROWS.windows(2) {
            assert_eq!(w[0].0 + w[0].1, w[1].0, "rows are contiguous");
        }
        assert_eq!((HERO_RESET, HERO_CANCEL, HERO_SAVE), (6, 7, 8), "actions sit in the last row, in on-screen order");
    }

    #[test]
    fn left_right_stay_inside_the_row() {
        assert_eq!(hero_focus_move(0, Action::Left), 0, "no wrap at the start of the row");
        assert_eq!(hero_focus_move(3, Action::Right), 3, "no wrap at the end of the row");
        assert_eq!(hero_focus_move(1, Action::Right), 2);
        assert_eq!(hero_focus_move(4, Action::Right), 5, "zoom row");
        assert_eq!(hero_focus_move(5, Action::Right), 5);
        assert_eq!(hero_focus_move(8, Action::Left), 7);
    }

    #[test]
    fn up_down_change_row_at_the_same_column_clamped() {
        assert_eq!(hero_focus_move(2, Action::Down), 4 + 1, "column 2 of the pan row -> column 1 of the zoom row");
        assert_eq!(hero_focus_move(3, Action::Down), 5, "column 3 clamps to the last zoom button");
        assert_eq!(hero_focus_move(7, Action::Up), 4 + 1, "centre of the action row -> the + button");
        assert_eq!(hero_focus_move(8, Action::Up), 5);
        assert_eq!(hero_focus_move(0, Action::Up), 0, "top row: up does nothing");
        assert_eq!(hero_focus_move(6, Action::Down), 6, "bottom row: down does nothing");
    }

    #[test]
    fn every_button_is_reachable_from_the_first() {
        let mut seen = [false; HERO_BTNS];
        let mut frontier = vec![0usize];
        seen[0] = true;
        while let Some(f) = frontier.pop() {
            for d in [Action::Left, Action::Right, Action::Up, Action::Down] {
                let n = hero_focus_move(f, d);
                if !seen[n] {
                    seen[n] = true;
                    frontier.push(n);
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "{seen:?}");
    }
}

#[cfg(test)]
mod apply_pick_tests {
    use super::*;

    fn asset(id: u64) -> Asset {
        Asset::from_json(&serde_json::json!({
            "id": id,
            "url": format!("https://cdn2.steamgriddb.com/hero/{id}.png"),
            "thumb": format!("https://cdn2.steamgriddb.com/hero_thumb/{id}.jpg"),
            "width": 1920,
            "height": 620,
        }))
        .unwrap()
    }

    fn item(id: u64) -> Item {
        Item { asset: asset(id), img: None, bytes: 0, requested: false, busy: false, clip: None, fi: usize::MAX }
    }

    #[test]
    fn apply_takes_the_focused_asset_not_the_first() {
        let items: Vec<Item> = [11, 22, 33, 44].into_iter().map(item).collect();
        let pick = apply_pick(&items, 2).unwrap();
        assert_eq!(pick, ApplyPick { idx: 2, id: 33, url: "https://cdn2.steamgriddb.com/hero/33.png".into() });
        assert_eq!(apply_pick(&items, 0).unwrap().id, 11, "focus 0 is still the first tile");
        assert_eq!(apply_pick(&items, 3).unwrap().id, 44, "last tile");
    }

    #[test]
    fn apply_without_a_focused_tile_does_nothing() {
        let items: Vec<Item> = [11, 22].into_iter().map(item).collect();
        assert_eq!(apply_pick(&items, 2), None, "focus past the end");
        assert_eq!(apply_pick(&[], 0), None, "empty list");
    }

    #[test]
    fn duplicates_are_dropped_within_and_across_pages() {
        let mut known: HashSet<u64> = [11u64].into_iter().collect();
        let fresh = fresh_assets(&mut known, vec![asset(11), asset(22), asset(22), asset(33)]);
        assert_eq!(fresh.iter().map(|a| a.id).collect::<Vec<_>>(), vec![22, 33]);
        let next = fresh_assets(&mut known, vec![asset(33), asset(44)]);
        assert_eq!(next.iter().map(|a| a.id).collect::<Vec<_>>(), vec![44]);
    }
}

/// Invalidation after apply / undo / reset: a read of Steam's art list counts only when it shows the write, and the grid file
/// stands in only where Steam gives nothing (non-Steam shortcuts).
#[cfg(test)]
mod settle_tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use steam_bridge::{ArtKind, ArtSet};

    fn p(s: &str) -> Option<PathBuf> {
        Some(PathBuf::from(s))
    }

    struct TempGrid(PathBuf);
    impl TempGrid {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let d = std::env::temp_dir().join(format!("deckshell-settle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }
    }
    impl Drop for TempGrid {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stock_hero_listed_first_is_not_settled_for_an_apply() {
        // the 1238860 read on the Deck: the write was on disk, Steam's list still had the stock file first
        let stale = ArtSet { hero: p("librarycache/1238860/library_hero.jpg"), ..Default::default() };
        assert!(!art_settled(&stale, Expect { t: AType::Hero, custom: true }));
        let fresh = ArtSet { hero: p("grid/1238860_hero.png"), custom: vec![ArtKind::Hero], ..Default::default() };
        assert!(art_settled(&fresh, Expect { t: AType::Hero, custom: true }));
    }

    #[test]
    fn reset_settles_only_when_steam_shows_its_own_hero() {
        let custom = ArtSet { hero: p("grid/578080_hero.png"), custom: vec![ArtKind::Hero], ..Default::default() };
        assert!(!art_settled(&custom, Expect { t: AType::Hero, custom: false }));
        let stock = ArtSet { hero: p("librarycache/578080/library_hero.jpg"), ..Default::default() };
        assert!(art_settled(&stock, Expect { t: AType::Hero, custom: false }));
    }

    #[test]
    fn only_the_applied_kind_decides() {
        // a custom capsule says nothing about a hero write
        let s = ArtSet { hero: p("librarycache/x/library_hero.jpg"), capsule: p("grid/1p.png"), custom: vec![ArtKind::Capsule], ..Default::default() };
        assert!(!art_settled(&s, Expect { t: AType::Hero, custom: true }));
        assert!(art_settled(&s, Expect { t: AType::Capsule, custom: true }));
        // Wide is the header kind (grid_l = <appid>.png)
        let w = ArtSet { header: p("grid/1.png"), custom: vec![ArtKind::Header], ..Default::default() };
        assert!(art_settled(&w, Expect { t: AType::Wide, custom: true }));
    }

    #[test]
    fn icons_and_app_without_steam_lists_are_not_checked() {
        assert!(art_settled(&ArtSet::default(), Expect { t: AType::Hero, custom: true }));
        assert!(art_settled(&ArtSet::default(), Expect { t: AType::Hero, custom: false }));
        assert_eq!(expect_art(AType::Icon, true), None);
        assert_eq!(expect_art(AType::Hero, true), Some(Expect { t: AType::Hero, custom: true }));
    }

    #[test]
    fn grid_file_fills_only_empty_slots_of_a_shortcut() {
        let g = TempGrid::new();
        let id = 2710347437u32;
        std::fs::write(g.0.join(format!("{id}_hero.png")), b"new hero").unwrap();
        // Steam gives nothing for this app: the chosen hero file is used
        let mut s = ArtSet::default();
        fill_from_grid(&mut s, &g.0, id, true);
        assert_eq!(s.hero, Some(g.0.join(format!("{id}_hero.png"))));
        assert!(s.custom.contains(&ArtKind::Hero));
        assert!(s.capsule.is_none() && s.header.is_none(), "no file, no fill");
        // Steam's own file is never replaced by the grid
        let mut own = ArtSet { hero: p("librarycache/x/library_hero.jpg"), ..Default::default() };
        fill_from_grid(&mut own, &g.0, id, true);
        assert_eq!(own.hero, p("librarycache/x/library_hero.jpg"));
        // after a reset the grid file (still on disk) must not come back
        let mut reset = ArtSet::default();
        fill_from_grid(&mut reset, &g.0, id, false);
        assert!(reset.hero.is_none());
    }
}
