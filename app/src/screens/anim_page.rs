//! Settings → «Анимации» (route `/routes/settings/customization`, task 412). Replaces the generated stock «Кастомизация» page.
//!
//! Everything the Decky plugin Animation Changer offers (docs/animation-changer-analysis-2026-11-18.md), drawn in DeckShell's
//! settings style (`ui/screens/anim_page.slint`):
//! * «Выбор»: what plays at boot / sleep / sleep from the loading screen / shutdown, with a live muted preview; shuffle mode
//!   (off / all / sets), shuffle now, boot animation on wake, volume, the master switch, IPv4-only for the catalog.
//! * «Мои анимации»: the user's animations (DeckShell downloads, the plugin's files, folder sets) with search, sort, type and
//!   favorites filters; the focused card plays; actions: assign, watch with sound, favorite, shuffle exclusion, details, delete.
//! * «Каталог»: steamdeckrepo.com (`anim::repo`): search, the plugin's sorts, type and device filters, thumbnails, details with
//!   a video preview, download with progress / cancel / errors, offline copy of the catalog.
//! * «Хранилище»: sizes, import of the plugin's files and choices, rescan, cache cleanup, attribution.
//!
//! Geometry is absolute window px (content x 395..1240), computed here so tests can lint overlaps and walk the pad focus.
//! Network and decoding run on worker threads; results come back through `slint::invoke_from_event_loop` into `apply_*`
//! functions that tests call directly.
use steam_assets::i18n;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::anim::config::{Config, Origin, ShuffleSlot};
use crate::anim::{self, repo};
use crate::app::App;
use crate::input::Action;
use crate::{AnimItem, AnimPageState, SettingsState};

pub const PAGE: &str = "customization";
pub const LABEL: &str = "anim_page.animations";

pub const X0: f32 = 395.0;
pub const W: f32 = 845.0;
pub const TABS_Y: f32 = 128.0;
pub const BODY_Y: f32 = 188.0;
const GAP: f32 = 15.0;
pub const COLS: usize = 3;
/// top of the settings viewport in window px (below the top bar)
const VIEW_TOP: f32 = 47.94;

pub fn card_w() -> f32 {
    (W - GAP * (COLS as f32 - 1.0)) / COLS as f32
}
pub fn card_h() -> f32 {
    card_w() * 0.625 + 62.0
}
const THUMB_W: u32 = 288;
const THUMB_H: u32 = 180;

// ---------------------------------------------------------------- model

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Tab {
    Overview,
    Library,
    Catalog,
    Storage,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Library, Tab::Catalog, Tab::Storage];
    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Slot keys of the config: the plugin's three plus DeckShell's shutdown.
pub const SLOTS: [&str; 4] = ["boot", "suspend", "throbber", "shutdown"];

pub fn slot_title(slot: &str) -> &'static str {
    match slot {
        "boot" => i18n::tr_ref("anim_page.launching"),
        "suspend" => i18n::tr_ref("anim_page.son"),
        "throbber" => i18n::tr_ref("anim_page.sleep_from_loading"),
        _ => i18n::tr_ref("anim_page.shutdown_and_restart"),
    }
}

/// Target pool of a slot (the plugin: boot -> boot, suspend/throbber -> suspend). Shutdown accepts both.
fn slot_accepts(slot: &str, target: &str) -> bool {
    match slot {
        "boot" => target == "boot",
        "suspend" | "throbber" => target == "suspend",
        _ => true,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LibSort {
    #[default]
    Name,
    Newest,
    Oldest,
    Popular,
    Liked,
    Size,
}

impl LibSort {
    pub const ALL: [LibSort; 6] = [LibSort::Name, LibSort::Newest, LibSort::Oldest, LibSort::Popular, LibSort::Liked, LibSort::Size];
    pub fn label(self) -> &'static str {
        match self {
            LibSort::Name => i18n::tr_ref("anim_page.by_name"),
            LibSort::Newest => i18n::tr_ref("anim_page.newest_first"),
            LibSort::Oldest => i18n::tr_ref("anim_page.oldest_first"),
            LibSort::Popular => i18n::tr_ref("anim_page.popular_dlc_first"),
            LibSort::Liked => i18n::tr_ref("anim_page.liked_ones"),
            LibSort::Size => i18n::tr_ref("anim_page.fit"),
        }
    }
}

/// What an item does on A (and identifies it across rebuilds: the focus is kept by key).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Tab(Tab),
    Slot(&'static str),
    Shuffle(ShuffleSlot),
    BootOnResume,
    Volume,
    Enabled,
    Ipv4,
    Search,
    Sort,
    Type,
    FavOnly,
    DeckOnly,
    Refresh,
    Card(String),
    ImportFiles,
    ImportSettings,
    Rescan,
    ClearFrames,
    ClearRepo,
    Text(usize),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub key: Key,
    pub kind: i32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub value: String,
    pub desc: String,
    pub badge: String,
    pub badge2: String,
    pub on: i32,
    pub progress: f32,
    pub img: Option<String>,
    pub focusable: bool,
}

impl Item {
    fn new(key: Key, kind: i32, x: f32, y: f32, w: f32, h: f32) -> Item {
        let focusable = !matches!(kind, 2 | 10 | 11 | 12 | 13 | 15);
        Item { key, kind, x, y, w, h, label: String::new(), value: String::new(), desc: String::new(), badge: String::new(), badge2: String::new(), on: 0, progress: -1.0, img: None, focusable }
    }
    fn label(mut self, s: impl Into<String>) -> Item {
        self.label = s.into();
        self
    }
    fn value(mut self, s: impl Into<String>) -> Item {
        self.value = s.into();
        self
    }
    fn desc(mut self, s: impl Into<String>) -> Item {
        self.desc = s.into();
        self
    }
    fn on(mut self, on: i32) -> Item {
        self.on = on;
        self
    }
}

/// One entry of «Мои анимации».
#[derive(Clone, Debug)]
pub struct Local {
    pub id: String,
    pub name: String,
    pub target: String,
    pub author: String,
    pub description: String,
    pub updated: String,
    pub likes: i64,
    pub downloads: i64,
    pub size: u64,
    pub path: Option<PathBuf>,
    pub origin: Origin,
    pub favorite: bool,
    pub excluded: bool,
    pub slots: Vec<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CatState {
    Loading,
    Ready { fetched: SystemTime, offline: Option<String> },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Dl {
    pub done: u64,
    pub total: Option<u64>,
    pub cancel: Arc<AtomicBool>,
}

impl Dl {
    pub fn fraction(&self) -> f32 {
        self.total.filter(|t| *t > 0).map(|t| (self.done as f64 / t as f64) as f32).unwrap_or(0.0).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuAct {
    Assign(String, &'static str),
    Watch(String, bool),
    Fav(String),
    Exclude(String),
    Details(String),
    AskDelete(String),
    Delete(String),
    SetSort(usize),
    SetType(usize),
    Cancel,
}

#[derive(Clone, Debug)]
pub enum Modal {
    /// choose the animation of a slot: (id, label, target-ok) options, focused option
    Picker { slot: &'static str, options: Vec<(String, String)>, focus: usize },
    /// short list of actions / options
    Menu { title: String, options: Vec<(String, MenuAct)>, focus: usize },
    /// catalog post (index into the catalog) or local animation (id)
    Details { post: Option<usize>, local: Option<String>, focus: usize },
}

#[derive(Default)]
pub struct St {
    pub tab: Option<Tab>,
    pub items: Vec<Item>,
    pub focus: Option<Key>,
    pub vy: f32,
    pub viewport_h: f32,
    pub lib_text: String,
    pub lib_sort: LibSort,
    pub lib_kind: repo::Kind,
    pub fav_only: bool,
    pub cat: Option<Arc<Vec<repo::Post>>>,
    pub cat_state: Option<CatState>,
    pub cat_q: repo::Query,
    pub cat_view: Vec<usize>,
    pub downloads: HashMap<String, Dl>,
    pub errors: HashMap<String, String>,
    pub modal: Option<Modal>,
    pub modal_vy: f32,
    pub keyboard: Option<Key>,
    pub hero_slot: Option<&'static str>,
    pub imgs: HashMap<String, Image>,
    pub img_wanted: HashSet<String>,
    pub img_failed: HashSet<String>,
    pub preview_id: Option<String>,
    pub preview_dl: Option<(String, f32, Arc<AtomicBool>)>,
    pub storage: Option<Storage>,
}

#[derive(Clone, Debug, Default)]
pub struct Storage {
    pub own: (usize, u64),
    pub plugin: (usize, u64),
    pub frames: u64,
    pub frames_budget: u64,
    pub repo: u64,
    pub free: Option<u64>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St { lib_kind: repo::Kind::All, cat_q: repo::Query { deck_only: true, ..Default::default() }, ..Default::default() });
    static CLIP: RefCell<Option<ClipPlay>> = const { RefCell::new(None) };
}

struct ClipPlay {
    key: String,
    clip: Arc<media::Clip>,
    start: Instant,
    last: usize,
    buf: SharedPixelBuffer<Rgba8Pixel>,
    _timer: slint::Timer,
}

pub fn with<R>(f: impl FnOnce(&mut St) -> R) -> R {
    ST.with(|s| f(&mut s.borrow_mut()))
}

/// Focus/state signature for the app's "did the press change anything" check (sounds, Up → header).
pub fn sig() -> i64 {
    use std::hash::{Hash, Hasher};
    with(|s| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.tab.hash(&mut h);
        s.focus.hash(&mut h);
        s.keyboard.hash(&mut h);
        (modal_focus(s), s.modal.is_some(), s.lib_text.len(), s.cat_q.text.len(), s.vy as i64).hash(&mut h);
        (h.finish() >> 9) as i64
    })
}

/// A page modal or the on-screen keyboard owns the pad: the header must not take an Up from a list edge.
pub fn modal_or_keyboard_open() -> bool {
    with(|s| s.modal.is_some() || s.keyboard.is_some())
}

pub fn is_page(a: &App) -> bool {
    matches!(a.router.current(), crate::router::Route::Settings(p) if p == PAGE)
}

// ---------------------------------------------------------------- data from the config

fn file_len(p: &Option<PathBuf>) -> u64 {
    p.as_ref().and_then(|p| p.metadata().ok()).map(|m| m.len()).unwrap_or(0)
}

pub fn locals(c: &Config) -> Vec<Local> {
    let slots: Vec<(&'static str, String)> = SLOTS.iter().map(|s| (*s, if *s == "shutdown" { c.x.shutdown.clone() } else { c.get(s) })).collect();
    c.all()
        .into_iter()
        .map(|a| {
            let e = c.entry(&a.id).unwrap_or(serde_json::Value::Null);
            let st = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let n = |k: &str| e.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
            let path = c.resolve(&a.id);
            Local {
                name: if a.name.is_empty() { a.id.clone() } else { a.name.clone() },
                target: a.target.clone(),
                author: st("author"),
                description: st("description"),
                updated: st("last_changed"),
                likes: n("likes"),
                downloads: n("downloads"),
                size: file_len(&path),
                origin: c.origin(&a.id),
                favorite: c.is_favorite(&a.id),
                excluded: c.is_excluded(&a.id),
                slots: slots.iter().filter(|(_, v)| *v == a.id).map(|(s, _)| *s).collect(),
                path,
                id: a.id,
            }
        })
        .collect()
}

pub fn filter_locals(all: &[Local], text: &str, sort: LibSort, kind: repo::Kind, fav_only: bool) -> Vec<usize> {
    let mut idx: Vec<usize> = all.iter().enumerate().filter(|(_, l)| kind.accepts(&l.target) && (!fav_only || l.favorite) && repo::matches_text(text, &l.name, &l.author)).map(|(i, _)| i).collect();
    match sort {
        LibSort::Name => idx.sort_by_cached_key(|i| all[*i].name.to_lowercase()),
        LibSort::Newest => idx.sort_by(|a, b| all[*b].updated.cmp(&all[*a].updated)),
        LibSort::Oldest => idx.sort_by(|a, b| (all[*a].updated.is_empty(), &all[*a].updated).cmp(&(all[*b].updated.is_empty(), &all[*b].updated))),
        LibSort::Popular => idx.sort_by(|a, b| all[*b].downloads.cmp(&all[*a].downloads)),
        LibSort::Liked => idx.sort_by(|a, b| all[*b].likes.cmp(&all[*a].likes)),
        LibSort::Size => idx.sort_by(|a, b| all[*b].size.cmp(&all[*a].size)),
    }
    idx
}

fn name_of(c: &Config, id: &str) -> String {
    let n = c.name_of(id);
    if n.is_empty() { id.to_string() } else { n }
}

fn slot_id(c: &Config, slot: &str) -> String {
    if slot == "shutdown" { c.x.shutdown.clone() } else { c.get(slot) }
}

fn slot_value(c: &Config, slot: &str) -> (String, i32) {
    let id = slot_id(c, slot);
    if id.is_empty() {
        return (if slot == "shutdown" { i18n::tr_ref("anim_page.like_sleeping").into() } else { i18n::tr_ref("anim_page.standard_steam").into() }, 0);
    }
    if c.resolve(&id).is_none() {
        return (i18n::tr_args("anim_page.file_not_found", &[("id", &(name_of(c, &id)).to_string())]), 0);
    }
    (name_of(c, &id), 0)
}

fn duration_label(secs: f64) -> String {
    let s = secs.round() as i64;
    if s >= 60 { i18n::tr_args("anim_page.min_sec", &[("seconds", &(s / 60).to_string()), ("seconds_2", &(s % 60).to_string())]) } else { i18n::tr_args("anim_page.sec", &[("seconds", &(s).to_string())]) }
}

fn animation_count(n: usize) -> String {
    i18n::tr_plural("anim_page.animation_count", n as i64, &[])
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------- layout

fn tab_label(t: Tab, n_local: usize, _n_cat: Option<usize>) -> String {
    match t {
        Tab::Overview => i18n::tr_ref("anim_page.choice").into(),
        Tab::Library => i18n::tr_args("anim_page.my_animations", &[("n_local", &(n_local).to_string())]),
        Tab::Catalog => i18n::tr_ref("anim_page.catalogue").into(), // the filtered count is in the status line under the bar
        Tab::Storage => i18n::tr_ref("anim_page.storage").into(),
    }
}

fn row(key: Key, kind: i32, y: &mut f32, label: &str, desc: &str) -> Item {
    let h = if desc.is_empty() { 60.0 } else { 92.0 };
    let it = Item::new(key, kind, X0, *y, W, h).label(label).desc(desc);
    *y += h + 4.0;
    it
}

fn section(y: &mut f32, label: &str, n: usize) -> Item {
    *y += 12.0;
    let it = Item::new(Key::Text(n), 2, X0, *y, W, 40.0).label(label);
    *y += 48.0;
    it
}

/// Builds the items of the current tab. Pure: everything comes from the arguments (tests call it directly).
pub fn build(s: &St, c: &Config, boot_now: &str, active: bool) -> Vec<Item> {
    let tab = s.tab.unwrap_or(Tab::Overview);
    let all = locals(c);
    let mut v = vec![];
    let mut x = X0;
    for t in Tab::ALL {
        let label = tab_label(t, all.len(), s.cat.as_ref().map(|c| c.len()));
        let w = (label.chars().count() as f32 * 10.0 + 36.0).max(110.0);
        v.push(Item::new(Key::Tab(t), 1, x, TABS_Y, w, 40.0).label(label).on((t == tab) as i32));
        x += w + 10.0;
    }
    let mut n = 0usize;
    let mut text = |v: &mut Vec<Item>, kind: i32, y: f32, h: f32, label: String, on: i32| {
        n += 1;
        v.push(Item::new(Key::Text(n), kind, X0, y, W, h).label(label).on(on));
    };
    match tab {
        Tab::Overview => {
            let slot = s.hero_slot.unwrap_or("boot");
            let id = if slot == "boot" { boot_now.to_string() } else { slot_id(c, slot) };
            let mut hero = Item::new(Key::Text(900), 11, X0, BODY_Y, W, 210.0);
            hero.value = if slot == "boot" { i18n::tr_ref("anim_page.running_now").into() } else { slot_title(slot).to_uppercase() };
            let meta = all.iter().find(|l| l.id == id);
            hero.label = if id.is_empty() { if slot == "shutdown" { i18n::tr_ref("anim_page.like_sleeping").into() } else { i18n::tr_ref("anim_page.standard_steam_animation").into() } } else { name_of(c, &id) };
            let mut lines = vec![];
            if let Some(m) = meta {
                let mut parts = vec![];
                if !m.author.is_empty() { parts.push(i18n::tr_args("anim_page.author", &[("author", &(m.author).to_string())])); }
                if m.size > 0 { parts.push(repo::size_label(m.size)); }
                if !parts.is_empty() { lines.push(parts.join(" · ")); }
            } else if !id.is_empty() {
                lines.push(i18n::tr_ref("anim_page.animation_file_not_found_standard_image_will").into());
            }
            if slot == "boot" && boot_now != c.get("boot") {
                lines.push(i18n::tr_args("anim_page.next_launch", &[("boot", &(if c.get("boot").is_empty() { i18n::tr_ref("anim_page.standard").into() } else { name_of(c, &c.get("boot")) }).to_string())]));
            }
            if slot == "boot" && c.x.boot_on_resume {
                lines.push(i18n::tr_ref("anim_page.this_same_animation_plays_when_you_get").into());
            }
            if !active {
                lines.push(i18n::tr_ref("anim_page.animation_player_is_now_off_changes_will").into());
            }
            hero.desc = lines.join("\n");
            hero.img = Some(format!("hero:{slot}"));
            v.push(hero);
            let mut y = BODY_Y + 226.0;
            let descs = [
                ("boot", i18n::tr_ref("anim_page.select_in_the_slot_for_the_next")),
                ("suspend", i18n::tr_ref("anim_page.before_going_to_sleep_from_the_menu")),
                ("throbber", i18n::tr_ref("anim_page.sleep_when_the_loading_indicator_is_on")),
                ("shutdown", i18n::tr_ref("anim_page.by_default_the_same_video_as_for")),
            ];
            for (slot, d) in descs {
                let (val, on) = slot_value(c, slot);
                v.push(row(Key::Slot(slot), 3, &mut y, slot_title(slot), d).value(val).on(on));
            }
            v.push(section(&mut y, i18n::tr_ref("anim_page.behavior"), 901));
            for (slot, label, desc) in [
                (ShuffleSlot::Boot, i18n::tr_ref("anim_page.stir_to_launch"), i18n::tr_ref("anim_page.select_a_random_roller_for_the_next")),
                (ShuffleSlot::Suspend, i18n::tr_ref("anim_page.stir_for_sleep"), i18n::tr_ref("anim_page.select_a_random_roller_at_each_transition")),
            ] {
                v.push(row(Key::Shuffle(slot), 4, &mut y, label, desc).on(c.shuffle_enabled(slot) as i32));
            }
            v.push(row(Key::BootOnResume, 4, &mut y, i18n::tr_ref("anim_page.animation_of_the_launch_when_leaving_sleep"), i18n::tr_ref("anim_page.show_the_launch_video_after_waking_up")).on(c.x.boot_on_resume as i32));
            let mut vol = row(Key::Volume, 16, &mut y, i18n::tr_ref("anim_page.volume_of_animations"), "").value(format!("{} %", (c.x.volume * 100.0).round() as i32));
            vol.progress = (c.x.volume / 1.0).clamp(0.0, 1.0);
            v.push(vol);
            v.push(row(Key::Enabled, 4, &mut y, i18n::tr_ref("anim_page.deckshell_animations"), i18n::tr_ref("anim_page.play_animations_of_start_sleep_and_shut")).on(c.x.enabled as i32));
            v.push(row(Key::Ipv4, 4, &mut y, i18n::tr_ref("anim_page.ipv4_only_for_directory"), i18n::tr_ref("anim_page.if_the_steamdeckrepo_com_directory_does_not")).on(c.force_ipv4() as i32));
        }
        Tab::Library => {
            let bar = BODY_Y;
            v.push(Item::new(Key::Search, 7, X0, bar, 290.0, 44.0).label(i18n::tr_ref("anim_page.title_or_author")).value(s.lib_text.clone()));
            v.push(Item::new(Key::Sort, 8, X0 + 305.0, bar, 200.0, 44.0).value(s.lib_sort.label()));
            v.push(Item::new(Key::Type, 8, X0 + 520.0, bar, 150.0, 44.0).value(s.lib_kind.label()));
            v.push(Item::new(Key::FavOnly, 9, X0 + 685.0, bar + 2.0, 160.0, 40.0).label(i18n::tr_ref("anim_page.the_chosen")).on(s.fav_only as i32));
            let idx = filter_locals(&all, &s.lib_text, s.lib_sort, s.lib_kind, s.fav_only);
            let total: u64 = all.iter().map(|l| l.size).sum();
            let status = if all.is_empty() {
                i18n::tr_ref("anim_page.no_animations_yet").to_string()
            } else if idx.len() == all.len() {
                format!("{} · {}", animation_count(all.len()), repo::size_label(total))
            } else {
                i18n::tr_args("anim_page.found_of", &[("len", &(idx.len()).to_string()), ("len_2", &(all.len()).to_string())])
            };
            text(&mut v, 10, bar + 54.0, 26.0, status, 0);
            let gy = bar + 92.0;
            if idx.is_empty() {
                let msg = if all.is_empty() { i18n::tr_ref("anim_page.download_animations_in_the_catalog_tab_or") } else { i18n::tr_ref("anim_page.nothing_found_change_the_search_or_filters") };
                text(&mut v, 15, gy, 120.0, msg.into(), 0);
            }
            for (k, i) in idx.iter().enumerate() {
                let l = &all[*i];
                let (cx, cy) = (X0 + (k % COLS) as f32 * (card_w() + GAP), gy + (k / COLS) as f32 * (card_h() + GAP));
                let mut it = Item::new(Key::Card(l.id.clone()), 6, cx, cy, card_w(), card_h()).label(l.name.clone());
                it.badge = if l.target == "suspend" { i18n::tr_ref("anim_page.son").into() } else { i18n::tr_ref("anim_page.launching").into() };
                if !l.slots.is_empty() {
                    it.badge2 = i18n::tr_ref("anim_page.selected").into();
                    it.on = 1;
                } else if l.favorite {
                    it.badge2 = "★".into();
                }
                let mut meta = vec![];
                if l.favorite && !l.slots.is_empty() { meta.push("★".to_string()); }
                if !l.author.is_empty() { meta.push(l.author.clone()); }
                meta.push(match l.origin {
                    Origin::Missing => i18n::tr_ref("anim_page.file_not_found_2").into(),
                    Origin::Local => i18n::tr_ref("anim_page.folder").into(),
                    _ => repo::size_label(l.size),
                });
                if l.excluded { meta.push(i18n::tr_ref("anim_page.stirlessly").into()); }
                it.desc = meta.join(" · ");
                it.value = if l.path.is_some() { i18n::tr_ref("anim_page.uploading_the_frame").into() } else { i18n::tr_ref("anim_page.no_file").into() };
                it.img = Some(format!("local:{}", l.id));
                v.push(it);
            }
        }
        Tab::Catalog => {
            let bar = BODY_Y;
            v.push(Item::new(Key::Search, 7, X0, bar, 230.0, 44.0).label(i18n::tr_ref("anim_page.catalogue_search")).value(s.cat_q.text.clone()));
            v.push(Item::new(Key::Sort, 8, X0 + 242.0, bar, 190.0, 44.0).value(s.cat_q.sort.label()));
            v.push(Item::new(Key::Type, 8, X0 + 444.0, bar, 130.0, 44.0).value(s.cat_q.kind.label()));
            v.push(Item::new(Key::DeckOnly, 9, X0 + 586.0, bar + 2.0, 135.0, 40.0).label("Steam Deck").on(s.cat_q.deck_only as i32));
            let busy = s.cat_state == Some(CatState::Loading);
            v.push(Item::new(Key::Refresh, 14, X0 + 733.0, bar, 112.0, 44.0).label(if busy { i18n::tr_ref("anim_page.uploading") } else { i18n::tr_ref("anim_page.update") }).on(if busy { -1 } else { 0 }));
            let (status, level) = match (&s.cat_state, &s.cat) {
                (None, _) => (i18n::tr_ref("anim_page.steamdeckrepo_com_directory_will_load_when_you").to_string(), 0),
                (Some(CatState::Loading), None) => (i18n::tr_ref("anim_page.download_the_steamdeckrepo_com_directory").to_string(), 0),
                (Some(CatState::Loading), Some(_)) => (i18n::tr_args("anim_page.update_2", &[("len", &(animation_count(s.cat_view.len())).to_string())]), 0),
                (Some(CatState::Failed(e)), _) => (e.clone(), 3),
                (Some(CatState::Ready { fetched, offline }), _) => {
                    let age = fetched.elapsed().map(|d| d.as_secs() as i64).unwrap_or(0);
                    let when = repo::relative_age(&chrono::DateTime::from_timestamp(now_unix() - age, 0).map(|d| d.to_rfc3339()).unwrap_or_default(), now_unix());
                    match offline {
                        Some(e) => (i18n::tr_args("anim_page.showing_the_cached_catalog", &[("error", &(e).to_string()), ("when", &(when).to_string())]), 2),
                        None => (i18n::tr_args("anim_page.catalog_updated", &[("len", &(animation_count(s.cat_view.len())).to_string()), ("when", &(when).to_string())]), 0),
                    }
                }
            };
            text(&mut v, 10, bar + 54.0, 26.0, status, level);
            let gy = bar + 92.0;
            if let Some(cat) = &s.cat {
                if s.cat_view.is_empty() {
                    text(&mut v, 15, gy, 120.0, i18n::tr_ref("anim_page.nothing_found_change_the_search_or_filters").into(), 0);
                }
                // only the rows around the viewport exist (the catalog has ~9000 posts)
                let pitch = card_h() + GAP;
                let first = ((s.vy + VIEW_TOP - gy) / pitch).floor().max(0.0) as usize;
                let rows_vis = ((s.viewport_h.max(400.0)) / pitch).ceil() as usize + 1;
                let (r0, r1) = (first.saturating_sub(2), first + rows_vis + 2);
                let focus_row = match &s.focus {
                    Some(Key::Card(id)) => s.cat_view.iter().position(|i| cat[*i].id == *id).map(|k| k / COLS),
                    _ => None,
                };
                for (k, i) in s.cat_view.iter().enumerate() {
                    let r = k / COLS;
                    if (r < r0 || r > r1) && Some(r) != focus_row {
                        continue;
                    }
                    let p = &cat[*i];
                    let (cx, cy) = (X0 + (k % COLS) as f32 * (card_w() + GAP), gy + r as f32 * pitch);
                    let mut it = Item::new(Key::Card(p.id.clone()), 6, cx, cy, card_w(), card_h()).label(p.name.clone());
                    it.badge = if p.target == "suspend" { i18n::tr_ref("anim_page.son").into() } else { i18n::tr_ref("anim_page.launching").into() };
                    let have = c.entry(&p.id).is_some() && c.resolve(&p.id).is_some();
                    if let Some(d) = s.downloads.get(&p.id) {
                        it.progress = d.fraction();
                        it.badge2 = format!("{} %", (d.fraction() * 100.0) as i32);
                        it.on = 1;
                    } else if have {
                        it.badge2 = i18n::tr_ref("anim_page.downloaded").into();
                        it.on = 1;
                    } else if s.errors.contains_key(&p.id) {
                        it.badge2 = i18n::tr_ref("anim_page.error").into();
                    }
                    let mut meta = vec![];
                    if !p.author.is_empty() { meta.push(p.author.clone()); }
                    meta.push(format!("⬇ {}", repo::short_count(p.downloads)));
                    meta.push(format!("♥ {}", repo::short_count(p.likes)));
                    if let Some(d) = p.duration { meta.push(duration_label(d)); }
                    it.desc = meta.join(" · ");
                    it.value = i18n::tr_ref("anim_page.cover").into();
                    it.img = Some(format!("thumb:{}", p.id));
                    v.push(it);
                }
                // keep the scroll height of the whole grid
                let rows = s.cat_view.len().div_ceil(COLS);
                if rows > 0 {
                    n += 1;
                    v.push(Item::new(Key::Text(n), 13, X0, gy + rows as f32 * pitch - GAP, W, 1.0));
                }
            }
        }
        Tab::Storage => {
            let st = s.storage.clone().unwrap_or_default();
            let mut y = BODY_Y;
            let files = |(n, b): (usize, u64)| format!("{} · {}", i18n::tr_plural("anim_page.file_count", n as i64, &[]), repo::size_label(b));
            let mut info = |v: &mut Vec<Item>, y: &mut f32, label: &str, value: String| {
                n += 1;
                v.push(Item::new(Key::Text(n), 12, X0, *y, W, 48.0).label(label).value(value));
                *y += 52.0;
            };
            info(&mut v, &mut y, i18n::tr_ref("anim_page.downloaded_at_deckshell"), files(st.own));
            info(&mut v, &mut y, i18n::tr_ref("anim_page.animation_changer_files_read_only"), files(st.plugin));
            info(&mut v, &mut y, i18n::tr_ref("anim_page.free_on_disk"), st.free.map(repo::size_label).unwrap_or_else(|| i18n::tr_ref("anim_page.unknown").into()));
            v.push(section(&mut y, i18n::tr_ref("anim_page.transfer_from_animation_changer"), 902));
            v.push(row(Key::ImportFiles, 5, &mut y, i18n::tr_ref("anim_page.import_files"), i18n::tr_ref("anim_page.copy_the_videos_downloaded_by_the_plugin")).value(i18n::tr_ref("anim_page.import")));
            v.push(row(Key::ImportSettings, 5, &mut y, i18n::tr_ref("anim_page.move_the_selection_of_the_plugin"), i18n::tr_ref("anim_page.take_from_the_plugin_slot_animations_stirring")).value(i18n::tr_ref("anim_page.move_over")));
            v.push(row(Key::Rescan, 5, &mut y, i18n::tr_ref("anim_page.reread_the_folders"), i18n::tr_ref("anim_page.find_manually_added_rollers_and_sets")).value(i18n::tr_ref("anim_page.read_it_again")));
            v.push(section(&mut y, i18n::tr_ref("anim_page.cash"), 903));
            v.push(row(Key::ClearFrames, 5, &mut y, i18n::tr_ref("anim_page.footage_for_smooth_reproduction"), &i18n::tr_args("anim_page.of_it_will_rebuild_automatically", &[("frames", &(repo::size_label(st.frames)).to_string()), ("frames_budget", &(repo::size_label(st.frames_budget)).to_string())])).value(i18n::tr_ref("anim_page.clear")));
            v.push(row(Key::ClearRepo, 5, &mut y, i18n::tr_ref("anim_page.catalogue_covers_and_previews"), &i18n::tr_args("anim_page.catalog_for_working_without_a_network_is", &[("repo", &(repo::size_label(st.repo)).to_string())])).value(i18n::tr_ref("anim_page.clear")));
            v.push(section(&mut y, i18n::tr_ref("anim_page.about_this_section"), 904));
            n += 1;
            v.push(Item::new(Key::Text(n), 13, X0, y, W, 120.0).label(
                i18n::tr_ref("anim_page.the_section_replicates_the_capabilities_of_the"),
            ));
        }
    }
    v
}

// ---------------------------------------------------------------- navigation

/// Nearest focusable item in a direction (0 up, 1 down, 2 left, 3 right), Steam-style: candidates must lie in that
/// direction; distance weights the cross axis double.
pub fn spatial(items: &[Item], from: usize, dir: u8) -> Option<usize> {
    let c = &items[from];
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let mut best: Option<(f32, usize)> = None;
    for (i, r) in items.iter().enumerate() {
        if i == from || !r.focusable {
            continue;
        }
        let (x, y) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let ok = match dir {
            0 => r.y + r.h <= c.y + 1.0,
            1 => r.y >= c.y + c.h - 1.0,
            2 => r.x + r.w <= c.x + 1.0 && (r.y < c.y + c.h && r.y + r.h > c.y),
            _ => r.x >= c.x + c.w - 1.0 && (r.y < c.y + c.h && r.y + r.h > c.y),
        };
        if !ok {
            continue;
        }
        let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
        let d = if dir < 2 { dy + dx * 2.0 } else { dx + dy * 2.0 };
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, i));
        }
    }
    best.map(|b| b.1)
}

/// Pairs of items whose rectangles overlap (layout lint).
#[cfg_attr(not(test), allow(dead_code))]
pub fn overlaps(items: &[Item]) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for i in 0..items.len() {
        for j in i + 1..items.len() {
            let (a, b) = (&items[i], &items[j]);
            if a.h <= 1.0 || b.h <= 1.0 {
                continue;
            }
            if a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5 {
                out.push((i, j));
            }
        }
    }
    out
}

fn focus_index(s: &St) -> Option<usize> {
    let k = s.focus.as_ref()?;
    s.items.iter().position(|i| &i.key == k)
}

fn first_body(s: &St) -> Option<Key> {
    s.items.iter().find(|i| i.focusable && !matches!(i.key, Key::Tab(_))).or_else(|| s.items.iter().find(|i| i.focusable)).map(|i| i.key.clone())
}

// ---------------------------------------------------------------- page lifecycle

/// Rail → page: build the tab (first visit: «Выбор»).
pub fn open(a: &mut App) {
    with(|s| {
        if s.tab.is_none() {
            s.tab = Some(Tab::Overview);
        }
        s.vy = 0.0;
        s.modal = None;
        s.keyboard = None;
        s.focus = None;
    });
    refresh_tab_data(a);
    rebuild(a);
}

/// A / Right on the rail: into the page content.
pub fn enter(a: &mut App) {
    a.se.zone = 1;
    with(|s| {
        if s.focus.is_none() || focus_index(s).is_none() {
            s.focus = first_body(s);
        }
    });
    scroll_to_focus(a);
    rebuild(a);
}

/// B inside the page: closes the keyboard or modal first. False = the settings page handles it (back to the rail).
pub fn back(a: &mut App) -> bool {
    if with(|s| s.keyboard.is_some()) {
        close_keyboard(a);
        return true;
    }
    if with(|s| s.modal.is_some()) {
        close_modal(a);
        return true;
    }
    stop_preview();
    false
}

fn refresh_tab_data(a: &mut App) {
    let tab = with(|s| s.tab.unwrap_or(Tab::Overview));
    match tab {
        Tab::Catalog => ensure_catalog(a, false),
        Tab::Storage => refresh_storage(),
        _ => {}
    }
}

pub fn switch_tab(a: &mut App, t: Tab) {
    stop_preview();
    with(|s| {
        s.tab = Some(t);
        s.modal = None;
        s.vy = 0.0;
        s.focus = Some(Key::Tab(t));
    });
    refresh_tab_data(a);
    rebuild(a);
}

/// Recompute items from the config, push to Slint, request images / preview.
pub fn rebuild(a: &mut App) {
    let viewport = a.ui().global::<SettingsState>().get_viewport_h();
    let active = anim::active();
    let items = anim::with_config(|c, now| {
        with(|s| {
            s.viewport_h = if viewport > 0.0 { viewport } else { 690.0 };
            if s.tab == Some(Tab::Catalog) {
                if let Some(cat) = s.cat.clone() {
                    s.cat_view = repo::filter(&cat, &s.cat_q);
                }
            }
            build(s, c, now, active)
        })
    });
    with(|s| {
        s.items = items;
        if s.focus.as_ref().is_some_and(|k| !s.items.iter().any(|i| &i.key == k)) {
            s.focus = first_body(s);
        }
    });
    publish_wanted();
    request_images(a);
    sync_preview(a);
    push(a);
}

fn img_for(s: &St, key: &Option<String>) -> Option<Image> {
    key.as_ref().and_then(|k| s.imgs.get(k)).cloned()
}

fn to_slint(s: &St, it: &Item) -> AnimItem {
    let img = img_for(s, &it.img);
    AnimItem {
        kind: it.kind,
        x: it.x,
        y: it.y,
        w: it.w,
        h: it.h,
        label: it.label.as_str().into(),
        value: it.value.as_str().into(),
        desc: it.desc.as_str().into(),
        badge: it.badge.as_str().into(),
        badge2: it.badge2.as_str().into(),
        on: it.on,
        progress: it.progress,
        has_img: img.is_some(),
        img: img.unwrap_or_default(),
    }
}

/// Push the current state to Slint (no rebuild).
pub fn push(a: &App) {
    let ui = a.ui();
    let g = ui.global::<AnimPageState>();
    let st = ui.global::<SettingsState>();
    with(|s| {
        let rows: Vec<AnimItem> = s.items.iter().map(|it| to_slint(s, it)).collect();
        g.set_items(ModelRc::new(VecModel::from(rows)));
        g.set_focus(if a.se.zone == 1 { focus_index(s).map(|i| i as i32).unwrap_or(-1) } else { -1 });
        g.set_active(a.se.zone == 1 && s.keyboard.is_none());
        let bottom = s.items.iter().map(|i| i.y + i.h).fold(0.0, f32::max);
        st.set_content_h((bottom - VIEW_TOP + 40.0).max(s.viewport_h).into());
        st.set_vy((-s.vy).into());
        st.set_is_anim(true);
        st.set_is_recording(false);
        st.set_is_storage(false);
        st.set_storage_modal(false);
        st.set_content(ModelRc::new(VecModel::from(Vec::<crate::GPageRow>::new())));
        st.set_cfocus(-1);
        push_modal(a, s);
    });
}

fn scroll_to_focus(a: &App) {
    let viewport = a.ui().global::<SettingsState>().get_viewport_h();
    with(|s| {
        let vh = if viewport > 0.0 { viewport } else { 690.0 };
        let Some(i) = focus_index(s) else { return };
        let it = &s.items[i];
        let (top, bot) = (it.y - s.vy, it.y + it.h - s.vy);
        if matches!(it.key, Key::Tab(_)) || it.y <= BODY_Y + 60.0 {
            s.vy = 0.0;
        } else if top < VIEW_TOP + 70.0 {
            s.vy = (it.y - VIEW_TOP - 70.0).max(0.0);
        } else if bot > VIEW_TOP + vh - 24.0 {
            s.vy = it.y + it.h - (VIEW_TOP + vh - 24.0);
        }
        let bottom = s.items.iter().map(|i| i.y + i.h).fold(0.0, f32::max) + 40.0;
        let maxv = (bottom - VIEW_TOP - vh).max(0.0);
        s.vy = s.vy.clamp(0.0, maxv);
    });
}

// ---------------------------------------------------------------- input

pub fn hints() -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    let b = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    let (modal, key, tab, kb) = with(|s| (s.modal.clone(), s.focus.clone(), s.tab, s.keyboard.is_some()));
    if kb {
        return vec![("a", i18n::tr_ref("anim_page.input"), 1, "Enter", Action::A), ("b", i18n::tr_ref("anim_page.done"), 1, "Esc", Action::B)];
    }
    let tabs = ("lb", i18n::tr_ref("anim_page.tabs"), 1, "[ ]", Action::LB);
    match modal {
        Some(Modal::Picker { .. }) => vec![("a", i18n::tr_ref("anim_page.select"), 1, "Enter", Action::A), ("x", i18n::tr_ref("anim_page.watch_with_sound"), 1, "F3", Action::X), ("y", i18n::tr_ref("anim_page.favorites"), 1, "F4", Action::Y), b],
        Some(Modal::Details { .. }) => vec![("a", i18n::tr_ref("anim_page.select"), 1, "Enter", Action::A), ("x", i18n::tr_ref("anim_page.watch_with_sound"), 1, "F3", Action::X), b],
        Some(Modal::Menu { .. }) => vec![("a", i18n::tr_ref("anim_page.select"), 1, "Enter", Action::A), b],
        None => match (tab, key) {
            (Some(Tab::Library), Some(Key::Card(_))) => vec![("a", i18n::tr_ref("anim_page.activity"), 1, "Enter", Action::A), ("x", i18n::tr_ref("anim_page.watch_with_sound"), 1, "F3", Action::X), ("y", i18n::tr_ref("anim_page.favorites"), 1, "F4", Action::Y), tabs, b],
            (Some(Tab::Catalog), Some(Key::Card(_))) => vec![("a", i18n::tr_ref("anim_page.more_information"), 1, "Enter", Action::A), ("x", i18n::tr_ref("anim_page.download"), 1, "F3", Action::X), tabs, b],
            (_, Some(Key::Slot(_))) => vec![("a", i18n::tr_ref("anim_page.select"), 1, "Enter", Action::A), ("x", i18n::tr_ref("anim_page.watch_with_sound"), 1, "F3", Action::X), tabs, b],
            (_, Some(Key::Volume)) => vec![("a", i18n::tr_ref("anim_page.edit"), 1, "Enter", Action::A), tabs, b],
            _ => vec![("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A), tabs, b],
        },
    }
}

/// Pad / keyboard input while the page content has focus.
pub fn handle(a: &mut App, act: Action) {
    if with(|s| s.keyboard.is_some()) {
        match act {
            Action::Char(c) => type_char(a, Some(c)),
            Action::Backspace => type_char(a, None),
            _ => {
                if a.osk_is_open() {
                    a.osk_handle(act);
                } else {
                    close_keyboard(a);
                }
            }
        }
        return;
    }
    if with(|s| s.modal.is_some()) {
        return handle_modal(a, act);
    }
    let tab = with(|s| s.tab.unwrap_or(Tab::Overview));
    let focus = with(|s| s.focus.clone());
    match act {
        Action::LB | Action::RB | Action::L2 | Action::R2 => {
            let i = tab.index();
            let n = Tab::ALL.len();
            let next = if matches!(act, Action::LB | Action::L2) { (i + n - 1) % n } else { (i + 1) % n };
            switch_tab(a, Tab::ALL[next]);
            crate::sound::play(crate::sound::Snd::ChangeTabs);
        }
        Action::Up | Action::Down | Action::Left | Action::Right => {
            if focus == Some(Key::Volume) && matches!(act, Action::Left | Action::Right) {
                return step_volume(a, act == Action::Right);
            }
            if let Some(Key::Card(_)) = &focus {
                if let Some(next) = card_move(tab, &focus, act) {
                    with(|s| s.focus = Some(next));
                    after_move(a);
                    return;
                }
            }
            let dir = match act {
                Action::Up => 0,
                Action::Down => 1,
                Action::Left => 2,
                _ => 3,
            };
            let next = with(|s| focus_index(s).and_then(|i| spatial(&s.items, i, dir)).map(|n| s.items[n].key.clone()));
            match next {
                Some(k) => {
                    with(|s| s.focus = Some(k));
                    after_move(a);
                }
                None if dir == 2 => {
                    a.se.zone = 0;
                    a.se.cursor = a.se.focus;
                    stop_preview();
                    a.push_settings();
                }
                None => crate::sound::play(crate::sound::Snd::FailedNav),
            }
        }
        Action::A => {
            if let Some(k) = focus {
                activate(a, k);
            }
        }
        Action::X => match focus {
            Some(Key::Slot(slot)) => {
                let id = anim::with_config(|c, now| if slot == "boot" { now.clone() } else { slot_id(c, slot) });
                watch(a, &id, slot != "boot");
            }
            Some(Key::Card(id)) if tab == Tab::Library => {
                let suspend = anim::with_config(|c, _| locals(c).iter().find(|l| l.id == id).map(|l| l.target == "suspend").unwrap_or(false));
                watch(a, &id, suspend);
            }
            Some(Key::Card(id)) if tab == Tab::Catalog => start_download_id(a, &id),
            _ => {}
        },
        Action::Y => {
            if let (Tab::Library, Some(Key::Card(id))) = (tab, focus) {
                toggle_fav(a, &id);
            }
        }
        _ => {}
    }
}

/// Grid moves by index (the catalog grid only materializes the rows around the viewport).
fn card_move(tab: Tab, focus: &Option<Key>, act: Action) -> Option<Key> {
    let Some(Key::Card(id)) = focus else { return None };
    let ids: Vec<String> = match tab {
        Tab::Catalog => with(|s| s.cat.as_ref().map(|c| s.cat_view.iter().map(|i| c[*i].id.clone()).collect()).unwrap_or_default()),
        _ => with(|s| s.items.iter().filter_map(|i| if let Key::Card(id) = &i.key { Some(id.clone()) } else { None }).collect()),
    };
    let k = ids.iter().position(|x| x == id)?;
    let n = ids.len();
    let target = match act {
        Action::Left if k % COLS > 0 => k - 1,
        Action::Right if k % COLS + 1 < COLS && k + 1 < n => k + 1,
        Action::Down if k + COLS < n => k + COLS,
        // last partial row: Down from a column past its end goes to the last card
        Action::Down if k / COLS + 1 < n.div_ceil(COLS) => n - 1,
        Action::Up if k >= COLS => k - COLS,
        _ => return None,
    };
    Some(Key::Card(ids[target].clone()))
}

fn after_move(a: &mut App) {
    crate::sound::play(crate::sound::Snd::BasicNav);
    with(|s| {
        if let Some(Key::Slot(slot)) = s.focus {
            s.hero_slot = Some(slot);
        }
    });
    scroll_to_focus(a);
    rebuild(a);
}

fn step_volume(a: &mut App, up: bool) {
    anim::with_config(|c, _| {
        let v = ((c.x.volume * 10.0).round() + if up { 1.0 } else { -1.0 }).clamp(0.0, 10.0) / 10.0;
        c.set_extra(|x| x.volume = v);
    });
    anim::page_commit();
    crate::sound::play(crate::sound::Snd::BasicNav);
    rebuild(a);
}

fn activate(a: &mut App, k: Key) {
    crate::sound::play(crate::sound::Snd::DefaultOk);
    match k {
        Key::Tab(t) => switch_tab(a, t),
        Key::Slot(slot) => open_picker(a, slot),
        Key::Shuffle(slot) => {
            anim::with_config(|c, _| c.set_shuffle(slot, !c.shuffle_enabled(slot)));
            anim::page_commit();
            rebuild(a);
        }
        Key::BootOnResume => {
            anim::with_config(|c, _| c.set_extra(|x| x.boot_on_resume = !x.boot_on_resume));
            anim::page_commit();
            rebuild(a);
        }
        Key::Volume => step_volume(a, true),
        Key::Enabled => {
            let on = anim::with_config(|c, _| {
                c.set_extra(|x| x.enabled = !x.enabled);
                c.x.enabled
            });
            anim::page_commit();
            a.toast(if on { i18n::tr_ref("anim_page.animations_will_turn_on_after_deckshell_restart") } else { i18n::tr_ref("anim_page.animations_will_shut_down_after_deckshell_restarts") });
            rebuild(a);
        }
        Key::Ipv4 => {
            anim::with_config(|c, _| {
                let v = !c.force_ipv4();
                c.v.insert("force_ipv4".into(), serde_json::json!(v));
            });
            anim::page_commit();
            rebuild(a);
        }
        Key::Search => open_keyboard(a),
        Key::Sort => {
            let tab = with(|s| s.tab);
            let (opts, cur): (Vec<String>, usize) = if tab == Some(Tab::Catalog) {
                (repo::Sort::ALL.iter().map(|s| s.label().to_string()).collect(), with(|s| repo::Sort::ALL.iter().position(|x| *x == s.cat_q.sort).unwrap_or(0)))
            } else {
                (LibSort::ALL.iter().map(|s| s.label().to_string()).collect(), with(|s| LibSort::ALL.iter().position(|x| *x == s.lib_sort).unwrap_or(0)))
            };
            open_menu(a, i18n::tr_ref("anim_page.sort"), opts.into_iter().enumerate().map(|(i, l)| (l, MenuAct::SetSort(i))).collect(), cur);
        }
        Key::Type => {
            let cur = with(|s| if s.tab == Some(Tab::Catalog) { s.cat_q.kind } else { s.lib_kind });
            let pos = repo::Kind::ALL.iter().position(|k| *k == cur).unwrap_or(0);
            open_menu(a, i18n::tr_ref("anim_page.type_of_animation"), repo::Kind::ALL.iter().enumerate().map(|(i, k)| (k.label().to_string(), MenuAct::SetType(i))).collect(), pos);
        }
        Key::FavOnly => {
            with(|s| s.fav_only = !s.fav_only);
            rebuild(a);
        }
        Key::DeckOnly => {
            with(|s| s.cat_q.deck_only = !s.cat_q.deck_only);
            rebuild(a);
        }
        Key::Refresh => ensure_catalog(a, true),
        Key::Card(id) => {
            if with(|s| s.tab) == Some(Tab::Catalog) {
                let post = with(|s| s.cat.as_ref().and_then(|c| c.iter().position(|p| p.id == id)));
                if let Some(p) = post {
                    open_details(a, Some(p), None);
                }
            } else {
                open_card_menu(a, &id);
            }
        }
        Key::ImportFiles => {
            let r = anim::with_config(|c, _| c.import_files(&repo::free_space));
            anim::page_commit();
            let msg = if r.no_space {
                i18n::tr_args("anim_page.not_enough_space_copied", &[("copied", &(r.copied).to_string())])
            } else if r.copied == 0 && r.failed == 0 {
                if r.present > 0 { i18n::tr_ref("anim_page.all_animation_changer_files_are_already_in").to_string() } else { i18n::tr_ref("anim_page.animation_changer_files_not_found").to_string() }
            } else {
                i18n::tr_args("anim_page.copied", &[("copied", &(animation_count(r.copied)).to_string()), ("bytes", &(repo::size_label(r.bytes)).to_string()), ("new", &(if r.failed > 0 { i18n::tr_args("anim_page.errors", &[("count", &r.failed.to_string())]) } else { String::new() }).to_string())])
            };
            a.toast(&msg);
            refresh_storage();
            rebuild(a);
        }
        Key::ImportSettings => {
            let ok = anim::with_config(|c, now| {
                let ok = c.import_plugin_settings();
                if ok {
                    *now = c.get("boot");
                }
                ok
            });
            anim::page_commit();
            a.toast(if ok { i18n::tr_ref("anim_page.animation_changer_selection_delayed") } else { i18n::tr_ref("anim_page.animation_changer_settings_not_found") });
            rebuild(a);
        }
        Key::Rescan => {
            let n = anim::with_config(|c, _| {
                c.sync_downloads();
                c.scan_local();
                c.all().len()
            });
            anim::page_commit();
            a.toast(&i18n::tr_args("anim_page.found", &[("count", &(animation_count(n)).to_string())]));
            refresh_storage();
            rebuild(a);
        }
        Key::ClearFrames => {
            let dir = anim::with_config(|c, _| c.dirs.cache.clone());
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "dsa")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            anim::page_commit(); // rebuilds the caches of the chosen slots in the background
            a.toast(i18n::tr_ref("anim_page.cash_personnel_cleared"));
            refresh_storage();
            rebuild(a);
        }
        Key::ClearRepo => {
            let dir = anim::with_config(|c, _| c.dirs.repo.clone());
            repo::RepoCache { dir }.clear_media();
            with(|s| {
                s.imgs.retain(|k, _| !k.starts_with("thumb:"));
                s.img_failed.clear();
            });
            a.toast(i18n::tr_ref("anim_page.covers_and_previews_removed"));
            refresh_storage();
            rebuild(a);
        }
        Key::Text(_) => {}
    }
}

// ---------------------------------------------------------------- modals

const MODAL_W: f32 = 1040.0;

fn picker_options(c: &Config, slot: &str) -> Vec<(String, String)> {
    let mut all: Vec<Local> = locals(c).into_iter().filter(|l| slot_accepts(slot, &l.target) && l.path.is_some()).collect();
    all.sort_by_cached_key(|l| (!l.favorite, l.name.to_lowercase()));
    let mut v = vec![(String::new(), if slot == "shutdown" { i18n::tr_ref("anim_page.like_sleeping").to_string() } else { i18n::tr_ref("anim_page.standard_steam").to_string() })];
    v.extend(all.into_iter().map(|l| (l.id, l.name)));
    v
}

fn open_picker(a: &mut App, slot: &'static str) {
    let (options, cur) = anim::with_config(|c, _| (picker_options(c, slot), slot_id(c, slot)));
    let focus = options.iter().position(|o| o.0 == cur).unwrap_or(0);
    with(|s| {
        s.modal = Some(Modal::Picker { slot, options, focus });
        s.modal_vy = 0.0;
    });
    modal_changed(a);
}

fn open_menu(a: &mut App, title: &str, options: Vec<(String, MenuAct)>, focus: usize) {
    with(|s| {
        s.modal = Some(Modal::Menu { title: title.into(), options, focus });
        s.modal_vy = 0.0;
    });
    modal_changed(a);
}

fn open_card_menu(a: &mut App, id: &str) {
    let Some(l) = anim::with_config(|c, _| locals(c).into_iter().find(|l| l.id == id)) else { return };
    let mut o = vec![];
    for slot in SLOTS {
        if slot_accepts(slot, &l.target) && l.path.is_some() {
            let on = l.slots.contains(&slot);
            o.push((format!("{}{}", if on { "✓ " } else { i18n::tr_ref("anim_page.designate") }, slot_title(slot).to_lowercase()), MenuAct::Assign(l.id.clone(), slot)));
        }
    }
    if l.path.is_some() {
        o.push((i18n::tr_ref("anim_page.watch_with_sound").into(), MenuAct::Watch(l.id.clone(), l.target == "suspend")));
    }
    o.push((if l.favorite { i18n::tr_ref("anim_page.remove_from_favorites") } else { i18n::tr_ref("anim_page.add_to_favorites") }.into(), MenuAct::Fav(l.id.clone())));
    o.push((if l.excluded { i18n::tr_ref("anim_page.participate_in_stirring") } else { i18n::tr_ref("anim_page.do_not_engage_in_stirring") }.into(), MenuAct::Exclude(l.id.clone())));
    o.push((i18n::tr_ref("anim_page.more_information").into(), MenuAct::Details(l.id.clone())));
    if l.origin != Origin::Local {
        o.push((i18n::tr_ref("anim_page.delete").into(), MenuAct::AskDelete(l.id.clone())));
    }
    open_menu(a, &l.name, o, 0);
}

fn open_details(a: &mut App, post: Option<usize>, local: Option<String>) {
    stop_preview_ui(a);
    with(|s| {
        s.modal = Some(Modal::Details { post, local, focus: 0 });
        s.modal_vy = 0.0;
    });
    modal_changed(a);
}

fn close_modal(a: &mut App) {
    with(|s| s.modal = None);
    stop_preview();
    crate::sound::play(crate::sound::Snd::HideModal);
    rebuild(a);
}

fn modal_changed(a: &mut App) {
    sync_preview(a);
    with(|s| push_modal(a, s));
}

/// Details buttons in order (label, action id, state: -1 disabled / 2 danger, progress).
fn detail_buttons(s: &St, c: &Config, post: Option<usize>, local: &Option<String>) -> Vec<(String, &'static str, i32, f32)> {
    let mut b = vec![];
    if let Some(p) = post.and_then(|i| s.cat.as_ref()?.get(i)) {
        let have = c.entry(&p.id).is_some() && c.resolve(&p.id).is_some();
        if let Some(d) = s.downloads.get(&p.id) {
            b.push((i18n::tr_args("anim_page.cancel_download", &[("i32", &((d.fraction() * 100.0) as i32).to_string())]), "cancel", 0, d.fraction()));
        } else if have {
            b.push((i18n::tr_ref("anim_page.appoint").into(), "assign", 0, -1.0));
        } else {
            b.push((i18n::tr_ref("anim_page.download").into(), "download", 0, -1.0));
        }
        match &s.preview_dl {
            Some((id, f, cancel)) if *id == p.id && !cancel.load(Ordering::Relaxed) => {
                b.push((i18n::tr_args("anim_page.preview", &[("i32", &((*f * 100.0) as i32).to_string())]), "preview-cancel", 0, *f));
            }
            Some(_) => b.push((i18n::tr_ref("anim_page.preview_2").into(), "preview", -1, -1.0)),
            None if s.preview_id.as_ref() == Some(&p.id) && anim::player::catalog_playing() => {
                b.push((i18n::tr_ref("anim_page.stop").into(), "preview-stop", 0, -1.0));
            }
            None => b.push((i18n::tr_ref("anim_page.preview_2").into(), "preview", 0, -1.0)),
        }
        if have {
            b.push((i18n::tr_ref("anim_page.delete").into(), "delete", 2, -1.0));
        }
    } else if let Some(id) = local {
        b.push((i18n::tr_ref("anim_page.appoint").into(), "assign", 0, -1.0));
        b.push((i18n::tr_ref("anim_page.watch_with_sound").into(), "watch", 0, -1.0));
        if c.origin(id) != Origin::Local {
            b.push((i18n::tr_ref("anim_page.delete").into(), "delete", 2, -1.0));
        }
    }
    b
}

fn push_modal(a: &App, s: &St) {
    let ui = a.ui();
    let g = ui.global::<AnimPageState>();
    let Some(m) = &s.modal else {
        g.set_modal(false);
        g.set_preview_in_modal(false);
        return;
    };
    let mut items: Vec<AnimItem> = vec![];
    let mk = |kind: i32, x: f32, y: f32, w: f32, h: f32| AnimItem { kind, x, y, w, h, progress: -1.0, ..Default::default() };
    let (title, mw, mh, focus, list) = match m {
        Modal::Picker { slot, options, focus } => {
            let (cur, favs) = anim::with_config(|c, _| (slot_id(c, slot), c.id_list("favorites")));
            for (i, (id, label)) in options.iter().enumerate() {
                let mut it = mk(20, 0.0, i as f32 * 48.0, 480.0, 48.0);
                it.label = label.as_str().into();
                it.value = if *id == cur { "✓".into() } else if favs.contains(id) { "★".into() } else { "".into() };
                it.on = (*id == cur) as i32;
                items.push(it);
            }
            let fid = options.get(*focus).map(|o| o.0.clone()).unwrap_or_default();
            let mut prev = mk(21, 528.0, 64.0, 488.0, 305.0);
            prev.value = if fid.is_empty() { i18n::tr_ref("anim_page.standard_animation").into() } else { i18n::tr_ref("anim_page.download_previews").into() };
            items.push(prev);
            let mut info = mk(22, 528.0, 385.0, 488.0, 170.0);
            anim::with_config(|c, _| {
                let l = locals(c).into_iter().find(|l| l.id == fid);
                info.label = options.get(*focus).map(|o| o.1.clone()).unwrap_or_default().into();
                info.value = l.as_ref().map(|l| [l.author.clone(), repo::size_label(l.size)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")).unwrap_or_default().into();
                info.desc = l.map(|l| l.description).unwrap_or_else(|| if fid.is_empty() { i18n::tr_ref("anim_page.steam_will_show_its_standard_animation_or").into() } else { String::new() }).into();
            });
            items.push(info);
            let mut hint = mk(24, 24.0, 584.0, 992.0, 32.0);
            hint.label = i18n::tr_ref("anim_page.a_select_x_watch_with_sound_y").into();
            items.push(hint);
            (i18n::tr_args("anim_page.choose_an_animation", &[("slot", &(slot_title(slot)).to_string())]), MODAL_W, 640.0, *focus as i32, (24.0, 64.0, 480.0, 512.0, options.len() as f32 * 48.0))
        }
        Modal::Menu { title, options, focus } => {
            for (i, (label, _)) in options.iter().enumerate() {
                let mut it = mk(20, 0.0, i as f32 * 48.0, 496.0, 48.0);
                it.label = label.as_str().into();
                items.push(it);
            }
            let lh = (options.len() as f32 * 48.0).min(480.0);
            (title.clone(), 520.0, lh + 64.0 + 20.0, *focus as i32, (12.0, 64.0, 496.0, lh, options.len() as f32 * 48.0))
        }
        Modal::Details { post, local, focus } => {
            let (buttons, title) = anim::with_config(|c, _| {
                let b = detail_buttons(s, c, *post, local);
                let mut prev = mk(21, 24.0, 64.0, 560.0, 350.0);
                let mut info = mk(22, 608.0, 64.0, 408.0, 350.0);
                let mut status = mk(24, 24.0, 498.0, 992.0, 52.0);
                let title;
                if let Some(p) = post.and_then(|i| s.cat.as_ref()?.get(i)) {
                    title = if p.target == "suspend" { i18n::tr_ref("anim_page.sleep_animation") } else { i18n::tr_ref("anim_page.launch_animations") }.to_string();
                    if let Some(img) = s.imgs.get(&format!("thumb:{}", p.id)) {
                        prev.img = img.clone();
                        prev.has_img = true;
                    }
                    prev.value = i18n::tr_ref("anim_page.the_cover_is_downloading").into();
                    info.label = p.name.as_str().into();
                    let mut meta = vec![];
                    if !p.author.is_empty() { meta.push(i18n::tr_args("anim_page.author", &[("author", &(p.author).to_string())])); }
                    let when = repo::relative_age(&p.updated, now_unix());
                    if !when.is_empty() { meta.push(i18n::tr_args("anim_page.updated", &[("when", &(when).to_string())])); }
                    let mut line2 = vec![format!("⬇ {}", repo::short_count(p.downloads)), format!("♥ {}", repo::short_count(p.likes))];
                    if let Some(d) = p.duration { line2.push(duration_label(d)); }
                    info.value = format!("{}\n{}", meta.join(", "), line2.join(" · ")).into();
                    info.desc = if p.description.is_empty() { i18n::tr_ref("anim_page.no_description").into() } else { p.description.as_str().into() };
                    let mut st = vec![];
                    if !p.for_deck() { st.push(i18n::tr_ref("anim_page.the_author_noted_the_video_only_for").to_string()); }
                    if let Some(e) = s.errors.get(&p.id) { st.push(e.clone()); status.on = 3; }
                    status.label = st.join(". ").into();
                } else if let Some(id) = local {
                    let l = locals(c).into_iter().find(|l| l.id == *id);
                    title = i18n::tr_ref("anim_page.my_animation").into();
                    if let Some(img) = s.imgs.get(&format!("local:{id}")) {
                        prev.img = img.clone();
                        prev.has_img = true;
                    }
                    if let Some(l) = l {
                        info.label = l.name.as_str().into();
                        let origin = match l.origin {
                            Origin::Own => i18n::tr_ref("anim_page.deckshell_folder"),
                            Origin::Plugin => i18n::tr_ref("anim_page.animation_changer_file"),
                            Origin::Local => i18n::tr_ref("anim_page.folder"),
                            Origin::Custom => i18n::tr_ref("anim_page.file_4"),
                            Origin::Missing => i18n::tr_ref("anim_page.file_not_found_2"),
                        };
                        let mut meta = vec![];
                        if !l.author.is_empty() { meta.push(i18n::tr_args("anim_page.author", &[("author", &(l.author).to_string())])); }
                        meta.push(format!("{} · {}", repo::size_label(l.size), origin));
                        info.value = meta.join("\n").into();
                        info.desc = l.description.as_str().into();
                        if !l.slots.is_empty() {
                            status.label = i18n::tr_args("anim_page.selected_2", &[("join", &(l.slots.iter().map(|s| slot_title(s).to_lowercase()).collect::<Vec<_>>().join(", ")).to_string())]).into();
                        }
                    }
                } else {
                    title = String::new();
                }
                items.push(prev);
                items.push(info);
                items.push(status);
                (b, title)
            });
            let bw = (992.0 - 16.0 * (buttons.len().max(1) as f32 - 1.0)) / buttons.len().max(1) as f32;
            let base = items.len();
            for (i, (label, _, on, progress)) in buttons.iter().enumerate() {
                let mut it = mk(23, 24.0 + i as f32 * (bw + 16.0), 430.0, bw, 52.0);
                it.label = label.as_str().into();
                it.on = *on;
                it.progress = *progress;
                items.push(it);
            }
            (title, MODAL_W, 574.0, (base + (*focus).min(buttons.len().saturating_sub(1))) as i32, (0.0, 0.0, 0.0, 0.0, 0.0))
        }
    };
    g.set_modal(true);
    g.set_modal_title(title.into());
    g.set_mw(mw);
    g.set_mh(mh);
    let (ww, wh) = (1280.0, 800.0);
    g.set_mx((ww - mw) / 2.0);
    g.set_my(((wh - mh) / 2.0).max(56.0));
    g.set_lx(list.0);
    g.set_ly(list.1);
    g.set_lw(list.2);
    g.set_lh(list.3);
    g.set_list_h(list.4);
    g.set_modal_vy(s.modal_vy);
    g.set_modal_items(ModelRc::new(VecModel::from(items)));
    g.set_modal_focus(focus);
    g.set_preview_in_modal(true);
}

fn modal_len(s: &St) -> usize {
    match &s.modal {
        Some(Modal::Picker { options, .. }) => options.len(),
        Some(Modal::Menu { options, .. }) => options.len(),
        Some(Modal::Details { post, local, .. }) => anim::with_config(|c, _| detail_buttons(s, c, *post, local).len()),
        None => 0,
    }
}

fn set_modal_focus(s: &mut St, f: usize) {
    match &mut s.modal {
        Some(Modal::Picker { focus, .. }) | Some(Modal::Menu { focus, .. }) | Some(Modal::Details { focus, .. }) => *focus = f,
        None => {}
    }
    // keep the focused list option inside the list area
    let lh = match &s.modal {
        Some(Modal::Picker { .. }) => 512.0,
        Some(Modal::Menu { .. }) => 480.0,
        _ => return,
    };
    let top = f as f32 * 48.0;
    if top < s.modal_vy {
        s.modal_vy = top;
    } else if top + 48.0 > s.modal_vy + lh {
        s.modal_vy = top + 48.0 - lh;
    }
}

fn modal_focus(s: &St) -> usize {
    match &s.modal {
        Some(Modal::Picker { focus, .. }) | Some(Modal::Menu { focus, .. }) | Some(Modal::Details { focus, .. }) => *focus,
        None => 0,
    }
}

pub fn handle_modal(a: &mut App, act: Action) {
    let (n, f, details) = with(|s| (modal_len(s), modal_focus(s), matches!(s.modal, Some(Modal::Details { .. }))));
    let (prev, next) = if details { (Action::Left, Action::Right) } else { (Action::Up, Action::Down) };
    match act {
        x if x == prev || x == next => {
            let nf = if x == next { (f + 1).min(n.saturating_sub(1)) } else { f.saturating_sub(1) };
            if nf == f {
                crate::sound::play(crate::sound::Snd::FailedNav);
                return;
            }
            with(|s| set_modal_focus(s, nf));
            crate::sound::play(crate::sound::Snd::BasicNav);
            modal_changed(a);
        }
        Action::A => modal_activate(a, f),
        Action::B => {
            close_modal(a);
        }
        Action::X => {
            let m = with(|s| s.modal.clone());
            match m {
                Some(Modal::Picker { slot, options, focus }) => {
                    if let Some((id, _)) = options.get(focus) {
                        watch(a, id, slot != "boot");
                    }
                }
                Some(Modal::Details { local: Some(id), .. }) => {
                    let suspend = anim::with_config(|c, _| locals(c).iter().find(|l| l.id == id).map(|l| l.target == "suspend").unwrap_or(false));
                    watch(a, &id, suspend);
                }
                Some(Modal::Details { post: Some(p), .. }) => {
                    let id = with(|s| s.cat.as_ref().map(|c| c[p].id.clone()).unwrap_or_default());
                    let path = anim::with_config(|c, _| c.resolve(&id));
                    let prev = repo_cache().preview_path(&id);
                    match path.or(prev.is_file().then_some(prev)) {
                        Some(p) => {
                            stop_preview_ui(a);
                            if let Err(e) = anim::preview_file(p) {
                                a.toast(&e);
                            }
                        }
                        None => a.toast(i18n::tr_ref("anim_page.first_open_the_preview_or_download_animation")),
                    }
                }
                _ => {}
            }
        }
        Action::Y => {
            let m = with(|s| s.modal.clone());
            if let Some(Modal::Picker { options, focus, .. }) = m {
                if let Some((id, _)) = options.get(focus).filter(|o| !o.0.is_empty()) {
                    anim::with_config(|c, _| c.toggle_favorite(id));
                    anim::page_commit();
                    modal_changed(a);
                }
            }
        }
        _ => {}
    }
}

pub fn modal_activate(a: &mut App, i: usize) {
    let m = with(|s| s.modal.clone());
    match m {
        Some(Modal::Picker { slot, options, .. }) => {
            let Some((id, label)) = options.get(i).cloned() else { return };
            assign(a, &id, slot);
            with(|s| {
                s.modal = None;
                s.hero_slot = Some(slot);
            });
            stop_preview();
            a.toast(&format!("{}: {}", slot_title(slot), label));
            rebuild(a);
        }
        Some(Modal::Menu { options, .. }) => {
            let Some((_, act)) = options.get(i).cloned() else { return };
            with(|s| s.modal = None);
            menu_act(a, act);
        }
        Some(Modal::Details { post, local, .. }) => {
            let b = with(|s| anim::with_config(|c, _| detail_buttons(s, c, post, &local)));
            let Some((_, what, on, _)) = b.get(i).cloned() else { return };
            if on < 0 {
                return;
            }
            let id = post.and_then(|p| with(|s| s.cat.as_ref().map(|c| c[p].id.clone()))).or(local.clone()).unwrap_or_default();
            match what {
                "download" => start_download_id(a, &id),
                "cancel" => cancel_download(&id),
                "preview" => start_preview_download(a, post.unwrap_or(0)),
                "preview-stop" => stop_preview_ui(a),
                "preview-cancel" => {
                    with(|s| {
                        if let Some((_, _, c)) = &s.preview_dl {
                            c.store(true, Ordering::Relaxed);
                        }
                    });
                }
                "assign" => {
                    with(|s| s.modal = None);
                    open_card_menu(a, &id);
                    return;
                }
                "watch" => {
                    let suspend = anim::with_config(|c, _| locals(c).iter().find(|l| l.id == id).map(|l| l.target == "suspend").unwrap_or(false));
                    watch(a, &id, suspend);
                }
                "delete" => {
                    with(|s| s.modal = None);
                    menu_act(a, MenuAct::AskDelete(id));
                    return;
                }
                _ => {}
            }
            modal_changed(a);
            rebuild(a);
        }
        None => {}
    }
}

fn assign(_a: &mut App, id: &str, slot: &'static str) {
    anim::with_config(|c, now| {
        if slot == "shutdown" {
            c.set_extra(|x| x.shutdown = id.to_string());
        } else {
            c.set(slot, id);
            if slot == "boot" {
                // like `anim set boot`: also what plays on wake in this session
                *now = id.to_string();
            }
        }
    });
    anim::page_commit();
}

fn menu_act(a: &mut App, act: MenuAct) {
    match act {
        MenuAct::Assign(id, slot) => {
            assign(a, &id, slot);
            let name = anim::with_config(|c, _| name_of(c, &id));
            a.toast(&format!("{}: {}", slot_title(slot), name));
        }
        MenuAct::Watch(id, suspend) => watch(a, &id, suspend),
        MenuAct::Fav(id) => toggle_fav(a, &id),
        MenuAct::Exclude(id) => {
            let ex = anim::with_config(|c, _| c.toggle_exclusion(&id));
            anim::page_commit();
            a.toast(if ex { i18n::tr_ref("anim_page.it_won_t_get_out_when_stirring") } else { i18n::tr_ref("anim_page.getting_involved_in_stirring_again") });
        }
        MenuAct::Details(id) => {
            open_details(a, None, Some(id));
            return;
        }
        MenuAct::AskDelete(id) => {
            let name = anim::with_config(|c, _| name_of(c, &id));
            open_menu(a, &i18n::tr_args("anim_page.delete_2", &[("name", &(name).to_string())]), vec![(i18n::tr_ref("anim_page.delete").into(), MenuAct::Delete(id)), (i18n::tr_ref("anim_page.cancel").into(), MenuAct::Cancel)], 1);
            return;
        }
        MenuAct::Delete(id) => {
            let r = anim::with_config(|c, now| {
                let r = c.remove(&id);
                if *now == id {
                    *now = c.get("boot");
                }
                r
            });
            anim::page_commit();
            match r {
                Ok(r) => {
                    let mut msg = if r.plugin_file_kept && !r.deleted_file { i18n::tr_ref("anim_page.removed_from_the_list_animation_changer_file").to_string() } else { i18n::tr_ref("anim_page.animation_deleted").to_string() };
                    if !r.cleared_slots.is_empty() {
                        msg.push_str(&i18n::tr_args("anim_page.default_for", &[("join", &(r.cleared_slots.iter().map(|s| slot_title(s).to_lowercase()).collect::<Vec<_>>().join(", ")).to_string())]));
                    }
                    a.toast(&msg);
                }
                Err(e) => a.toast(&e),
            }
            with(|s| {
                s.imgs.remove(&format!("local:{id}"));
            });
            refresh_storage();
        }
        MenuAct::SetSort(i) => with(|s| {
            if s.tab == Some(Tab::Catalog) {
                s.cat_q.sort = repo::Sort::ALL[i.min(repo::Sort::ALL.len() - 1)];
            } else {
                s.lib_sort = LibSort::ALL[i.min(LibSort::ALL.len() - 1)];
            }
            s.vy = 0.0;
        }),
        MenuAct::SetType(i) => with(|s| {
            let k = repo::Kind::ALL[i.min(2)];
            if s.tab == Some(Tab::Catalog) {
                s.cat_q.kind = k;
            } else {
                s.lib_kind = k;
            }
            s.vy = 0.0;
        }),
        MenuAct::Cancel => {}
    }
    stop_preview();
    rebuild(a);
}

fn toggle_fav(a: &mut App, id: &str) {
    let on = anim::with_config(|c, _| c.toggle_favorite(id));
    anim::page_commit();
    a.toast(if on { i18n::tr_ref("anim_page.added_to_selected") } else { i18n::tr_ref("anim_page.removed_from_the_elect") });
    rebuild(a);
}

fn watch(a: &mut App, id: &str, suspend: bool) {
    stop_preview();
    if let Err(e) = anim::preview(id, suspend) {
        a.toast(&e);
    }
}

// ---------------------------------------------------------------- search keyboard (shared OSK)

pub fn keyboard_target() -> bool {
    with(|s| s.keyboard.is_some())
}

fn open_keyboard(a: &mut App) {
    with(|s| s.keyboard = Some(Key::Search));
    a.osk_set_open(true);
    push(a);
}

pub fn close_keyboard(a: &mut App) {
    with(|s| s.keyboard = None);
    if a.osk_is_open() {
        a.osk_set_open(false);
    }
    rebuild(a);
}

/// A typed character (None = backspace) from the OSK or a keyboard.
pub fn type_char(a: &mut App, c: Option<char>) {
    with(|s| {
        let q = if s.tab == Some(Tab::Catalog) { &mut s.cat_q.text } else { &mut s.lib_text };
        match c {
            Some(c) if !c.is_control() && q.chars().count() < 60 => q.push(c),
            None => {
                q.pop();
            }
            _ => {}
        }
        s.vy = 0.0;
    });
    rebuild(a);
}

// ---------------------------------------------------------------- workers

type Job = Box<dyn FnOnce() + Send>;

fn pool() -> &'static Mutex<std::sync::mpsc::Sender<Job>> {
    static P: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<Job>>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..3 {
            let rx = rx.clone();
            let _ = std::thread::Builder::new().name(format!("anim-page-{i}")).spawn(move || loop {
                let job = rx.lock().unwrap().recv();
                match job {
                    Ok(j) => j(),
                    Err(_) => return,
                }
            });
        }
        Mutex::new(tx)
    })
}

fn spawn(j: impl FnOnce() + Send + 'static) {
    let _ = pool().lock().unwrap().send(Box::new(j));
}

type Post = Box<dyn FnOnce(&mut App) + Send>;

/// Tests: worker results are queued here instead of the Slint event loop and drained by the test, and nothing reaches the
/// internet (catalog / downloads go to the local fixture server in `TEST_NET`, thumbnails are never fetched).
static TEST_MODE: AtomicBool = AtomicBool::new(false);
static TEST_POSTS: Mutex<Vec<Post>> = Mutex::new(Vec::new());
static TEST_NET: Mutex<Option<(String, String)>> = Mutex::new(None);

fn post(f: impl FnOnce(&mut App) + Send + 'static) {
    if TEST_MODE.load(Ordering::Relaxed) {
        TEST_POSTS.lock().unwrap().push(Box::new(f));
        return;
    }
    super::defer(f);
}

fn repo_cache() -> repo::RepoCache {
    repo::RepoCache { dir: anim::with_config(|c, _| c.dirs.repo.clone()) }
}

fn net() -> repo::Net {
    let ipv4 = anim::with_config(|c, _| c.force_ipv4());
    if TEST_MODE.load(Ordering::Relaxed) {
        // never the real site from tests: the fixture server, or an address that refuses at once
        let (base, host) = TEST_NET.lock().unwrap().clone().unwrap_or_else(|| ("http://127.0.0.1:9".into(), "127.0.0.1:9".into()));
        return repo::Net { base, ipv4, test_hosts: vec![host], timeout: Duration::from_secs(5) };
    }
    repo::Net::new(ipv4)
}

/// Pixel buffer -> Slint image (UI thread).
fn image_of(buf: SharedPixelBuffer<Rgba8Pixel>) -> Image {
    Image::from_rgba8(buf)
}

fn decode_png(p: &Path, w: u32, h: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::open(p).ok()?;
    let img = if img.width() > w || img.height() > h { img.thumbnail(w, h) } else { img };
    let rgba = img.to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}

/// A still frame of a video, cached as PNG next to the catalog cache (keyed by file size + mtime).
fn video_still(video: &Path, cache: &Path, w: u32, h: u32) -> Option<PathBuf> {
    let m = video.metadata().ok()?;
    let stamp = m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let name = video.file_stem()?.to_string_lossy().replace('/', "_");
    let out = cache.join("stills").join(format!("{name}-{}-{stamp}-{w}.png", m.len()));
    if out.is_file() {
        return Some(out);
    }
    let ff = media::find_ffmpeg()?;
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension("tmp.png");
    for ss in ["1.0", "0"] {
        let ok = std::process::Command::new(&ff)
            .args(["-v", "error", "-nostdin", "-y", "-ss", ss, "-i"])
            .arg(video)
            .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}:force_original_aspect_ratio=decrease"), "-f", "image2", "-c:v", "png"])
            .arg(&tmp)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && tmp.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            std::fs::rename(&tmp, &out).ok()?;
            return Some(out);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    None
}

/// Image keys used by the visible items: decode cached files, fetch catalog thumbnails (catalog tab only).
fn request_images(a: &App) {
    let (wanted, tab) = with(|s| {
        let vis_top = s.vy;
        let vis_bot = s.vy + s.viewport_h + VIEW_TOP + 200.0;
        let keys: Vec<String> = s
            .items
            .iter()
            .filter(|i| i.y + i.h >= vis_top - 100.0 && i.y <= vis_bot)
            .filter_map(|i| i.img.clone())
            .filter(|k| !k.starts_with("hero:") && !s.imgs.contains_key(k) && !s.img_failed.contains(k))
            .collect();
        (keys, s.tab)
    });
    let _ = a;
    let new: Vec<String> = with(|s| wanted.into_iter().filter(|k| s.img_wanted.insert(k.clone())).collect());
    if new.is_empty() {
        return;
    }
    let cache = repo_cache();
    let cat = with(|s| s.cat.clone());
    for key in new {
        let dir = cache.dir.clone();
        if let Some(id) = key.strip_prefix("local:") {
            let path = anim::with_config(|c, _| c.resolve(id));
            let thumb = repo::RepoCache { dir: dir.clone() }.thumb_path(id);
            let key2 = key.clone();
            spawn(move || {
                let buf = if thumb.is_file() {
                    decode_png(&thumb, THUMB_W, THUMB_H)
                } else {
                    path.as_deref().and_then(|p| video_still(p, &dir, THUMB_W, THUMB_H)).and_then(|p| decode_png(&p, THUMB_W, THUMB_H))
                };
                post(move |a| apply_image(a, key2, buf));
            });
        } else if let Some(id) = key.strip_prefix("thumb:") {
            let Some(p) = cat.as_ref().and_then(|c| c.iter().find(|p| p.id == id)).cloned() else { continue };
            let online = tab == Some(Tab::Catalog) && !TEST_MODE.load(Ordering::Relaxed);
            let key2 = key.clone();
            let net = net();
            spawn(move || {
                // still wanted? (the user may have scrolled away)
                if !with_wanted(&key2) {
                    post(move |a| apply_image_dropped(a, key2));
                    return;
                }
                let rc = repo::RepoCache { dir };
                let path = if rc.thumb_path(&p.id).is_file() || !online { Ok(rc.thumb_path(&p.id)) } else { repo::fetch_thumb(&net, &rc, &p.id, &p.thumbnail, THUMB_W, THUMB_H) };
                let buf = path.ok().and_then(|p| decode_png(&p, THUMB_W, THUMB_H));
                post(move |a| apply_image(a, key2, buf));
            });
        }
    }
}

/// Worker check that an image is still wanted (shared with the UI thread through a snapshot).
static WANTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn with_wanted(k: &str) -> bool {
    WANTED.lock().unwrap().as_ref().map(|w| w.contains(k)).unwrap_or(true)
}

fn publish_wanted() {
    let visible: HashSet<String> = with(|s| {
        let (top, bot) = (s.vy - 300.0, s.vy + s.viewport_h + VIEW_TOP + 300.0);
        s.items.iter().filter(|i| i.y + i.h >= top && i.y <= bot).filter_map(|i| i.img.clone()).collect()
    });
    *WANTED.lock().unwrap() = Some(visible);
}

fn apply_image_dropped(_a: &mut App, key: String) {
    with(|s| {
        s.img_wanted.remove(&key);
    });
}

pub fn apply_image(a: &mut App, key: String, buf: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    with(|s| {
        s.img_wanted.remove(&key);
        match buf {
            Some(b) => {
                s.imgs.insert(key, image_of(b));
                // bound the memory: ~200 KB per thumbnail
                if s.imgs.len() > 400 {
                    let keep: HashSet<String> = s.items.iter().filter_map(|i| i.img.clone()).collect();
                    s.imgs.retain(|k, _| keep.contains(k));
                }
            }
            None => {
                s.img_failed.insert(key);
            }
        }
    });
    if is_page(a) {
        push(a);
    }
}

// ---------------------------------------------------------------- catalog

pub fn ensure_catalog(a: &mut App, force: bool) {
    let (state, have) = with(|s| (s.cat_state.clone(), s.cat.is_some()));
    if state == Some(CatState::Loading) {
        return;
    }
    if !force && have && matches!(state, Some(CatState::Ready { .. })) {
        return;
    }
    with(|s| s.cat_state = Some(CatState::Loading));
    let cache = repo_cache();
    let net = net();
    spawn(move || {
        // the saved copy first: the page is usable offline at once
        if !force {
            if let Some((c, age)) = cache.load_catalog() {
                let fetched = SystemTime::now() - Duration::from_secs(age);
                let fresh = age < 6 * 3600;
                let posts = Arc::new(c.posts);
                post(move |a| apply_catalog(a, Ok((posts, fetched)), !fresh));
                if fresh {
                    return;
                }
            }
        }
        let r = net.fetch_catalog().and_then(|bytes| {
            let c = repo::parse_catalog(&bytes)?;
            if let Err(e) = cache.store_catalog(&bytes) {
                eprintln!("[anim-page] cannot store catalog: {e}");
            }
            Ok((Arc::new(c.posts), SystemTime::now()))
        });
        post(move |a| apply_catalog(a, r, false));
    });
    rebuild(a);
}

/// Result of a catalog load. `more` = a network refresh is still running after this cached copy.
pub fn apply_catalog(a: &mut App, r: Result<(Arc<Vec<repo::Post>>, SystemTime), repo::NetErr>, more: bool) {
    with(|s| match r {
        Ok((posts, fetched)) => {
            s.cat = Some(posts);
            s.cat_state = Some(if more { CatState::Loading } else { CatState::Ready { fetched, offline: None } });
        }
        Err(e) => {
            let msg = e.message();
            eprintln!("[anim-page] catalog: {e:?}");
            s.cat_state = Some(match s.cat_state.clone() {
                _ if s.cat.is_some() => {
                    let fetched = repo_cache_age(&repo_cache()).unwrap_or(SystemTime::now());
                    CatState::Ready { fetched, offline: Some(msg) }
                }
                _ => CatState::Failed(msg),
            });
        }
    });
    if is_page(a) {
        rebuild(a);
    }
}

fn repo_cache_age(c: &repo::RepoCache) -> Option<SystemTime> {
    c.catalog_path().metadata().ok()?.modified().ok()
}

// ---------------------------------------------------------------- downloads

fn start_download_id(a: &mut App, id: &str) {
    let p = with(|s| s.cat.as_ref().and_then(|c| c.iter().find(|p| p.id == id).cloned()));
    let Some(p) = p else { return };
    let have = anim::with_config(|c, _| c.entry(&p.id).is_some() && c.resolve(&p.id).is_some());
    if have {
        a.toast(i18n::tr_ref("anim_page.this_animation_is_already_downloaded"));
        return;
    }
    if with(|s| s.downloads.contains_key(&p.id)) {
        return cancel_download(&p.id);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    with(|s| {
        s.errors.remove(&p.id);
        s.downloads.insert(p.id.clone(), Dl { done: 0, total: None, cancel: cancel.clone() });
    });
    let dest = anim::with_config(|c, _| c.own_download(&p.id));
    let net = net();
    let url = p.download_url(&net.base);
    let id = p.id.clone();
    spawn(move || {
        let mut last = Instant::now() - Duration::from_secs(1);
        let id2 = id.clone();
        let r = net.download(&url, &dest, &cancel, &repo::free_space, &mut |done, total| {
            if last.elapsed() >= Duration::from_millis(120) || total == Some(done) {
                last = Instant::now();
                let id3 = id2.clone();
                post(move |a| apply_progress(a, &id3, done, total));
            }
        });
        post(move |a| apply_download(a, p, r));
    });
    a.toast(i18n::tr_ref("anim_page.download_started"));
    rebuild(a);
}

fn cancel_download(id: &str) {
    with(|s| {
        if let Some(d) = s.downloads.get(id) {
            d.cancel.store(true, Ordering::Relaxed);
        }
    });
}

pub fn apply_progress(a: &mut App, id: &str, done: u64, total: Option<u64>) {
    let changed = with(|s| match s.downloads.get_mut(id) {
        Some(d) => {
            d.done = done;
            d.total = total;
            true
        }
        None => false,
    });
    if changed && is_page(a) {
        rebuild(a);
    }
}

pub fn apply_download(a: &mut App, p: repo::Post, r: Result<u64, repo::NetErr>) {
    with(|s| {
        s.downloads.remove(&p.id);
    });
    match r {
        Ok(_) => {
            anim::with_config(|c, _| c.add_download(p.to_entry(repo::BASE)));
            anim::page_commit();
            // the catalog thumbnail doubles as the card picture of «Мои анимации»
            with(|s| {
                if let Some(img) = s.imgs.get(&format!("thumb:{}", p.id)).cloned() {
                    s.imgs.insert(format!("local:{}", p.id), img);
                }
            });
            a.toast(&i18n::tr_args("anim_page.downloaded_2", &[("name", &(p.name).to_string())]));
        }
        Err(repo::NetErr::Cancelled) => a.toast(i18n::tr_ref("anim_page.download_cancelled")),
        Err(e) => {
            let msg = e.message();
            eprintln!("[anim-page] download {}: {e:?}", p.id);
            a.toast(&msg);
            with(|s| {
                s.errors.insert(p.id.clone(), msg);
            });
        }
    }
    refresh_storage();
    if is_page(a) {
        rebuild(a);
    }
}

fn start_catalog_preview(a: &mut App, id: String, path: PathBuf) {
    stop_preview_ui(a);
    let generation = CLIP_GEN.load(Ordering::Relaxed);
    let result = anim::preview_catalog(&a.ui(), path, move |outcome| {
        post(move |a| {
            if is_page(a) && CLIP_GEN.load(Ordering::Relaxed) == generation {
                if outcome == anim::player::Outcome::Failed {
                    a.toast(i18n::tr_ref("anim_page.we_were_unable_to_reproduce_the_preview"));
                }
                modal_changed(a);
            }
        });
    });
    match result {
        Ok(()) => with(|s| s.preview_id = Some(id)),
        Err(e) => a.toast(&e),
    }
    modal_changed(a);
}

fn start_preview_download(a: &mut App, post_idx: usize) {
    let Some(p) = with(|s| s.cat.as_ref().and_then(|c| c.get(post_idx).cloned())) else { return };
    let cache = repo_cache();
    let dest = cache.preview_path(&p.id);
    let local = anim::with_config(|c, _| c.resolve(&p.id));
    if local.is_some() || dest.is_file() {
        let path = local.unwrap_or(dest);
        start_catalog_preview(a, p.id, path);
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    with(|s| s.preview_dl = Some((p.id.clone(), 0.0, cancel.clone())));
    let net = net();
    let mut p = p;
    if TEST_MODE.load(Ordering::Relaxed) {
        p.video = format!("{}/videos/{}.webm", net.base, p.id);
    }
    spawn(move || {
        let id = p.id.clone();
        let mut last = Instant::now();
        let r = net.download(&p.video, &dest, &cancel, &repo::free_space, &mut |d, t| {
            if last.elapsed() >= Duration::from_millis(150) {
                last = Instant::now();
                let f = t.filter(|t| *t > 0).map(|t| d as f32 / t as f32).unwrap_or(0.0);
                let id2 = id.clone();
                post(move |_a| {
                    with(|s| {
                        if let Some(pd) = s.preview_dl.as_mut().filter(|pd| pd.0 == id2) {
                            pd.1 = f;
                        }
                    });
                    crate::with_app(|a| if is_page(a) { modal_changed(a) });
                });
            }
        });
        cache.prune(256 << 20);
        post(move |a| {
            with(|s| s.preview_dl = None);
            match r {
                Ok(_) => {
                    let still_open = with(|s| match &s.modal {
                        Some(Modal::Details { post: Some(i), .. }) =>
                            s.cat.as_ref().and_then(|c| c.get(*i)).is_some_and(|p| p.id == id),
                        _ => false,
                    });
                    if is_page(a) && still_open && !cancel.load(Ordering::Relaxed) {
                        start_catalog_preview(a, id, dest);
                    }
                }
                Err(repo::NetErr::Cancelled) => {}
                Err(e) if !cancel.load(Ordering::Relaxed) => a.toast(&e.message()),
                Err(_) => {}
            }
            if is_page(a) {
                modal_changed(a);
            }
        });
    });
    modal_changed(a);
}

// ---------------------------------------------------------------- live preview (muted clip in the page)

/// Which preview the current state wants: hero slot, focused library card, or picker option.
fn wanted_preview(a: &App) -> Option<(String, PathBuf, u32, u32, i32, bool)> {
    let (tab, focus, modal, hero_slot, items) = with(|s| (s.tab, s.focus.clone(), s.modal.clone(), s.hero_slot, s.items.iter().map(|i| (i.key.clone(), i.kind)).collect::<Vec<_>>()));
    match modal {
        Some(Modal::Picker { options, focus, .. }) => {
            let id = options.get(focus)?.0.clone();
            let path = anim::with_config(|c, _| c.resolve(&id))?;
            return Some((format!("pick:{id}"), path, 488, 305, -1, true));
        }
        Some(Modal::Details { local: Some(id), .. }) => {
            let path = anim::with_config(|c, _| c.resolve(&id))?;
            return Some((format!("local-detail:{id}"), path, 560, 350, -1, true));
        }
        Some(_) => return None,
        None => {}
    }
    match tab? {
        Tab::Overview => {
            let slot = hero_slot.unwrap_or("boot");
            let path = anim::with_config(|c, now| {
                let id = if slot == "boot" { now.clone() } else { slot_id(c, slot) };
                c.resolve(&id).or_else(|| c.fallback(if slot == "shutdown" { "suspend" } else { slot }))
            })?;
            let owner = items.iter().position(|i| i.1 == 11)? as i32;
            Some((format!("hero:{slot}:{}", path.display()), path, 336, 210, owner, false))
        }
        Tab::Library if a.se.zone == 1 => {
            let Some(Key::Card(id)) = focus else { return None };
            let owner = items.iter().position(|i| i.0 == Key::Card(id.clone()))? as i32;
            let path = anim::with_config(|c, _| c.resolve(&id))?;
            Some((format!("card:{id}"), path, THUMB_W, THUMB_H, owner, false))
        }
        _ => None,
    }
}

fn sync_preview(a: &mut App) {
    let ui = a.ui();
    let g = ui.global::<AnimPageState>();
    let catalog_preview = with(|s| match &s.modal {
        Some(Modal::Details { post: Some(i), .. }) => s.cat.as_ref().and_then(|c| c.get(*i))
            .is_some_and(|p| s.preview_id.as_ref() == Some(&p.id)),
        _ => false,
    });
    if catalog_preview {
        g.set_preview_owner(-1);
        g.set_preview_in_modal(true);
        return;
    }
    if with(|s| s.preview_id.is_some()) {
        reset_preview();
    }
    let want = wanted_preview(a);
    match want {
        None => {
            stop_muted_clip();
            g.set_has_preview(false);
        }
        Some((key, path, w, h, owner, in_modal)) => {
            g.set_preview_owner(owner);
            g.set_preview_in_modal(in_modal);
            let same = CLIP.with(|c| c.borrow().as_ref().is_some_and(|p| p.key == key));
            if !same {
                start_clip(a, key, path, w, h);
            }
        }
    }
}

static CLIP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn start_clip(a: &mut App, key: String, path: PathBuf, w: u32, h: u32) {
    if TEST_MODE.load(Ordering::Relaxed) {
        return;
    }
    stop_preview_ui(a);
    let gen = CLIP_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    spawn(move || {
        // ~30 fps, scaled to the box; built once into ~/.cache/deckshell/clips and reused
        let clip = media::open_clip(&path, w, h, 33).ok().filter(|c| !c.is_empty()).map(Arc::new);
        post(move |a| {
            if CLIP_GEN.load(Ordering::Relaxed) != gen || !is_page(a) {
                return;
            }
            let Some(clip) = clip else { return };
            let buf = SharedPixelBuffer::<Rgba8Pixel>::new(clip.width, clip.height);
            let timer = slint::Timer::default();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), tick);
            CLIP.with(|c| *c.borrow_mut() = Some(ClipPlay { key, clip, start: Instant::now(), last: usize::MAX, buf, _timer: timer }));
            tick();
        });
    });
}

fn tick() {
    let img = CLIP.with(|c| {
        let mut c = c.borrow_mut();
        let p = c.as_mut()?;
        let ms = p.start.elapsed().as_millis() as u64 % p.clip.total_ms().max(1);
        let (i, _) = p.clip.at(ms);
        if i == p.last {
            return None;
        }
        p.last = i;
        p.clip.frame_into(i, p.buf.make_mut_bytes()).ok()?;
        Some(Image::from_rgba8(p.buf.clone()))
    });
    if let Some(img) = img {
        crate::with_app(|a| {
            if !is_page(a) {
                stop_preview_ui(a);
                return;
            }
            let ui = a.ui();
            let g = ui.global::<AnimPageState>();
            g.set_preview(img);
            g.set_has_preview(true);
        });
    }
}

fn stop_muted_clip() {
    CLIP_GEN.fetch_add(1, Ordering::Relaxed);
    CLIP.with(|c| c.borrow_mut().take());
}

fn reset_preview() {
    stop_muted_clip();
    anim::player::cancel_catalog();
    with(|s| {
        s.preview_id = None;
        if let Some((_, _, cancel)) = &s.preview_dl {
            cancel.store(true, Ordering::Relaxed);
        }
    });
}

pub fn stop_preview() {
    reset_preview();
    crate::with_app(|a| a.ui().global::<AnimPageState>().set_has_preview(false));
}

fn stop_preview_ui(a: &App) {
    reset_preview();
    a.ui().global::<AnimPageState>().set_has_preview(false);
}

pub fn leave() {
    reset_preview();
    with(|s| {
        s.keyboard = None;
        s.modal = None;
    });
}

// ---------------------------------------------------------------- storage numbers

fn refresh_storage() {
    let (own, plugin, frames, budget, repo_dir) = anim::with_config(|c, _| (c.dirs.own.join("downloads"), c.dirs.plugin_data.join("downloads"), c.dirs.cache.clone(), c.x.cache_budget_mb << 20, c.dirs.repo.clone()));
    let count = |d: &Path| -> (usize, u64) {
        std::fs::read_dir(d)
            .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "webm")).fold((0, 0), |(n, b), e| (n + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0))))
            .unwrap_or((0, 0))
    };
    let st = Storage { own: count(&own), plugin: count(&plugin), frames: repo::dir_size(&frames), frames_budget: budget, repo: repo::dir_size(&repo_dir.join("thumbs")) + repo::dir_size(&repo_dir.join("previews")) + repo::dir_size(&repo_dir.join("stills")), free: repo::free_space(own.parent().unwrap_or(&own)).or_else(|| repo::free_space(Path::new("/"))) };
    with(|s| s.storage = Some(st));
}

// ---------------------------------------------------------------- touch

pub fn wire(ui: &crate::AppWindow) {
    let g = ui.global::<AnimPageState>();
    g.set_label_standard_steam_animation(i18n::tr("anim_page.standard_steam_animation").into());
    g.on_activated(|i| {
        super::defer(move |a| {
            let key = with(|s| s.items.get(i as usize).filter(|it| it.focusable).map(|it| it.key.clone()));
            if let Some(k) = key {
                a.se.zone = 1;
                with(|s| s.focus = Some(k.clone()));
                if let Key::Slot(slot) = k {
                    with(|s| s.hero_slot = Some(slot));
                }
                activate(a, k);
            }
        })
    });
    g.on_modal_activated(|i| {
        super::defer(move |a| {
            let idx = with(|s| match &s.modal {
                // details: preview, info, status, then the buttons
                Some(Modal::Details { .. }) => (i as usize).checked_sub(3),
                Some(_) => Some(i as usize),
                None => None,
            });
            if let Some(idx) = idx {
                with(|s| set_modal_focus(s, idx));
                modal_activate(a, idx);
            }
        })
    });
    g.on_modal_dismiss(|| super::defer(close_modal));
}

// ---------------------------------------------------------------- tests (headless, fixtures, no internet)

#[cfg(test)]
#[path = "anim_page_tests.rs"]
mod tests;
