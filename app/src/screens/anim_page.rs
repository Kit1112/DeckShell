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
pub const LABEL: &str = "Анимации";

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
        "boot" => "Запуск",
        "suspend" => "Сон",
        "throbber" => "Сон из загрузки",
        _ => "Выключение и перезагрузка",
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
            LibSort::Name => "По названию",
            LibSort::Newest => "Сначала новые",
            LibSort::Oldest => "Сначала старые",
            LibSort::Popular => "Популярные",
            LibSort::Liked => "Понравившиеся",
            LibSort::Size => "По размеру",
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
        return (if slot == "shutdown" { "Как для сна".into() } else { "Стандартная Steam".into() }, 0);
    }
    if c.resolve(&id).is_none() {
        return (format!("Файл не найден ({})", name_of(c, &id)), 0);
    }
    (name_of(c, &id), 0)
}

fn duration_ru(secs: f64) -> String {
    let s = secs.round() as i64;
    if s >= 60 { format!("{} мин {} с", s / 60, s % 60) } else { format!("{s} с") }
}

fn count_ru(n: usize) -> String {
    format!("{n} {}", repo::plural(n as i64, ["анимация", "анимации", "анимаций"]))
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

// ---------------------------------------------------------------- layout

fn tab_label(t: Tab, n_local: usize, _n_cat: Option<usize>) -> String {
    match t {
        Tab::Overview => "Выбор".into(),
        Tab::Library => format!("Мои анимации · {n_local}"),
        Tab::Catalog => "Каталог".into(), // the filtered count is in the status line under the bar
        Tab::Storage => "Хранилище".into(),
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
            hero.value = if slot == "boot" { "ЗАПУСК · СЕЙЧАС".into() } else { slot_title(slot).to_uppercase() };
            let meta = all.iter().find(|l| l.id == id);
            hero.label = if id.is_empty() { if slot == "shutdown" { "Как для сна".into() } else { "Стандартная анимация Steam".into() } } else { name_of(c, &id) };
            let mut lines = vec![];
            if let Some(m) = meta {
                let mut parts = vec![];
                if !m.author.is_empty() { parts.push(format!("Автор: {}", m.author)); }
                if m.size > 0 { parts.push(repo::size_ru(m.size)); }
                if !parts.is_empty() { lines.push(parts.join(" · ")); }
            } else if !id.is_empty() {
                lines.push("Файл анимации не найден: будет показана стандартная".into());
            }
            if slot == "boot" && boot_now != c.get("boot") {
                lines.push(format!("При следующем запуске: {}", if c.get("boot").is_empty() { "стандартная".into() } else { name_of(c, &c.get("boot")) }));
            }
            if slot == "boot" && c.x.boot_on_resume {
                lines.push("Эта же анимация играет при выходе из сна".into());
            }
            if !active {
                lines.push("Проигрыватель анимаций сейчас выключен: изменения применятся после перезапуска DeckShell".into());
            }
            hero.desc = lines.join("\n");
            hero.img = Some(format!("hero:{slot}"));
            v.push(hero);
            let mut y = BODY_Y + 226.0;
            let descs = [
                ("boot", "Ролик при включении. Выбор в слоте — на следующий запуск"),
                ("suspend", "Перед переходом в сон из меню или кнопкой питания"),
                ("throbber", "Сон, когда на экране индикатор загрузки"),
                ("shutdown", "По умолчанию — тот же ролик, что и для сна"),
            ];
            for (slot, d) in descs {
                let (val, on) = slot_value(c, slot);
                v.push(row(Key::Slot(slot), 3, &mut y, slot_title(slot), d).value(val).on(on));
            }
            v.push(section(&mut y, "Поведение", 901));
            for (slot, label, desc) in [
                (ShuffleSlot::Boot, "Перемешать для запуска", "Выбирать случайный ролик для следующего запуска"),
                (ShuffleSlot::Suspend, "Перемешать для сна", "Выбирать случайный ролик при каждом переходе в сон"),
            ] {
                v.push(row(Key::Shuffle(slot), 4, &mut y, label, desc).on(c.shuffle_enabled(slot) as i32));
            }
            v.push(row(Key::BootOnResume, 4, &mut y, "Анимация запуска при выходе из сна", "Показывать ролик запуска после пробуждения").on(c.x.boot_on_resume as i32));
            let mut vol = row(Key::Volume, 16, &mut y, "Громкость анимаций", "").value(format!("{} %", (c.x.volume * 100.0).round() as i32));
            vol.progress = (c.x.volume / 1.0).clamp(0.0, 1.0);
            v.push(vol);
            v.push(row(Key::Enabled, 4, &mut y, "Анимации DeckShell", "Проигрывать анимации запуска, сна и выключения. Действует после перезапуска").on(c.x.enabled as i32));
            v.push(row(Key::Ipv4, 4, &mut y, "Только IPv4 для каталога", "Если каталог steamdeckrepo.com не загружается").on(c.force_ipv4() as i32));
        }
        Tab::Library => {
            let bar = BODY_Y;
            v.push(Item::new(Key::Search, 7, X0, bar, 290.0, 44.0).label("Название или автор").value(s.lib_text.clone()));
            v.push(Item::new(Key::Sort, 8, X0 + 305.0, bar, 200.0, 44.0).value(s.lib_sort.label()));
            v.push(Item::new(Key::Type, 8, X0 + 520.0, bar, 150.0, 44.0).value(s.lib_kind.label()));
            v.push(Item::new(Key::FavOnly, 9, X0 + 685.0, bar + 2.0, 160.0, 40.0).label("★ Избранные").on(s.fav_only as i32));
            let idx = filter_locals(&all, &s.lib_text, s.lib_sort, s.lib_kind, s.fav_only);
            let total: u64 = all.iter().map(|l| l.size).sum();
            let status = if all.is_empty() {
                "Пока нет анимаций".to_string()
            } else if idx.len() == all.len() {
                format!("{} · {}", count_ru(all.len()), repo::size_ru(total))
            } else {
                format!("Найдено {} из {}", idx.len(), all.len())
            };
            text(&mut v, 10, bar + 54.0, 26.0, status, 0);
            let gy = bar + 92.0;
            if idx.is_empty() {
                let msg = if all.is_empty() { "Скачайте анимации на вкладке «Каталог» или импортируйте файлы Animation Changer на вкладке «Хранилище»" } else { "Ничего не найдено. Измените поиск или фильтры" };
                text(&mut v, 15, gy, 120.0, msg.into(), 0);
            }
            for (k, i) in idx.iter().enumerate() {
                let l = &all[*i];
                let (cx, cy) = (X0 + (k % COLS) as f32 * (card_w() + GAP), gy + (k / COLS) as f32 * (card_h() + GAP));
                let mut it = Item::new(Key::Card(l.id.clone()), 6, cx, cy, card_w(), card_h()).label(l.name.clone());
                it.badge = if l.target == "suspend" { "Сон".into() } else { "Запуск".into() };
                if !l.slots.is_empty() {
                    it.badge2 = "Выбрана".into();
                    it.on = 1;
                } else if l.favorite {
                    it.badge2 = "★".into();
                }
                let mut meta = vec![];
                if l.favorite && !l.slots.is_empty() { meta.push("★".to_string()); }
                if !l.author.is_empty() { meta.push(l.author.clone()); }
                meta.push(match l.origin {
                    Origin::Missing => "файл не найден".into(),
                    Origin::Local => "набор из папки".into(),
                    _ => repo::size_ru(l.size),
                });
                if l.excluded { meta.push("без перемешивания".into()); }
                it.desc = meta.join(" · ");
                it.value = if l.path.is_some() { "Загрузка кадра…".into() } else { "Нет файла".into() };
                it.img = Some(format!("local:{}", l.id));
                v.push(it);
            }
        }
        Tab::Catalog => {
            let bar = BODY_Y;
            v.push(Item::new(Key::Search, 7, X0, bar, 230.0, 44.0).label("Поиск в каталоге").value(s.cat_q.text.clone()));
            v.push(Item::new(Key::Sort, 8, X0 + 242.0, bar, 190.0, 44.0).value(s.cat_q.sort.label()));
            v.push(Item::new(Key::Type, 8, X0 + 444.0, bar, 130.0, 44.0).value(s.cat_q.kind.label()));
            v.push(Item::new(Key::DeckOnly, 9, X0 + 586.0, bar + 2.0, 135.0, 40.0).label("Steam Deck").on(s.cat_q.deck_only as i32));
            let busy = s.cat_state == Some(CatState::Loading);
            v.push(Item::new(Key::Refresh, 14, X0 + 733.0, bar, 112.0, 44.0).label(if busy { "Загрузка…" } else { "Обновить" }).on(if busy { -1 } else { 0 }));
            let (status, level) = match (&s.cat_state, &s.cat) {
                (None, _) => ("Каталог steamdeckrepo.com загрузится при открытии вкладки".to_string(), 0),
                (Some(CatState::Loading), None) => ("Загрузка каталога steamdeckrepo.com…".to_string(), 0),
                (Some(CatState::Loading), Some(_)) => (format!("{} · обновление…", count_ru(s.cat_view.len())), 0),
                (Some(CatState::Failed(e)), _) => (e.clone(), 3),
                (Some(CatState::Ready { fetched, offline }), _) => {
                    let age = fetched.elapsed().map(|d| d.as_secs() as i64).unwrap_or(0);
                    let when = repo::relative_ru(&chrono::DateTime::from_timestamp(now_unix() - age, 0).map(|d| d.to_rfc3339()).unwrap_or_default(), now_unix());
                    match offline {
                        Some(e) => (format!("{e}. Показан сохранённый каталог ({when})"), 2),
                        None => (format!("{} · каталог обновлён {when}", count_ru(s.cat_view.len())), 0),
                    }
                }
            };
            text(&mut v, 10, bar + 54.0, 26.0, status, level);
            let gy = bar + 92.0;
            if let Some(cat) = &s.cat {
                if s.cat_view.is_empty() {
                    text(&mut v, 15, gy, 120.0, "Ничего не найдено. Измените поиск или фильтры".into(), 0);
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
                    it.badge = if p.target == "suspend" { "Сон".into() } else { "Запуск".into() };
                    let have = c.entry(&p.id).is_some() && c.resolve(&p.id).is_some();
                    if let Some(d) = s.downloads.get(&p.id) {
                        it.progress = d.fraction();
                        it.badge2 = format!("{} %", (d.fraction() * 100.0) as i32);
                        it.on = 1;
                    } else if have {
                        it.badge2 = "Скачано".into();
                        it.on = 1;
                    } else if s.errors.contains_key(&p.id) {
                        it.badge2 = "Ошибка".into();
                    }
                    let mut meta = vec![];
                    if !p.author.is_empty() { meta.push(p.author.clone()); }
                    meta.push(format!("⬇ {}", repo::short_count(p.downloads)));
                    meta.push(format!("♥ {}", repo::short_count(p.likes)));
                    if let Some(d) = p.duration { meta.push(duration_ru(d)); }
                    it.desc = meta.join(" · ");
                    it.value = "Обложка…".into();
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
            let files = |(n, b): (usize, u64)| format!("{} {} · {}", n, repo::plural(n as i64, ["файл", "файла", "файлов"]), repo::size_ru(b));
            let mut info = |v: &mut Vec<Item>, y: &mut f32, label: &str, value: String| {
                n += 1;
                v.push(Item::new(Key::Text(n), 12, X0, *y, W, 48.0).label(label).value(value));
                *y += 52.0;
            };
            info(&mut v, &mut y, "Скачано в DeckShell", files(st.own));
            info(&mut v, &mut y, "Файлы Animation Changer (только чтение)", files(st.plugin));
            info(&mut v, &mut y, "Свободно на диске", st.free.map(repo::size_ru).unwrap_or_else(|| "неизвестно".into()));
            v.push(section(&mut y, "Перенос из Animation Changer", 902));
            v.push(row(Key::ImportFiles, 5, &mut y, "Импортировать файлы", "Скопировать скачанные плагином ролики в папку DeckShell. Файлы плагина не меняются").value("Импортировать"));
            v.push(row(Key::ImportSettings, 5, &mut y, "Перенести выбор плагина", "Взять из плагина анимации слотов, перемешивание, исключения и IPv4").value("Перенести"));
            v.push(row(Key::Rescan, 5, &mut y, "Перечитать папки", "Найти вручную добавленные ролики и наборы").value("Перечитать"));
            v.push(section(&mut y, "Кэш", 903));
            v.push(row(Key::ClearFrames, 5, &mut y, "Кадры для плавного воспроизведения", &format!("{} из {}. Пересоберётся автоматически", repo::size_ru(st.frames), repo::size_ru(st.frames_budget))).value("Очистить"));
            v.push(row(Key::ClearRepo, 5, &mut y, "Обложки и превью каталога", &format!("{}. Каталог для работы без сети сохраняется", repo::size_ru(st.repo))).value("Очистить"));
            v.push(section(&mut y, "Об этом разделе", 904));
            n += 1;
            v.push(Item::new(Key::Text(n), 13, X0, y, W, 120.0).label(
                "Раздел повторяет возможности Decky-плагина Animation Changer (SDH-AnimationChanger, Justin Marentette, лицензия BSD-3-Clause) и написан заново для DeckShell. Анимации — с сайта steamdeckrepo.com, права на ролики принадлежат их авторам. Скачивание выполняется только по вашему действию.",
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
        return vec![("a", "Ввод", 1, "Enter", Action::A), ("b", "Готово", 1, "Esc", Action::B)];
    }
    let tabs = ("lb", "Вкладки", 1, "[ ]", Action::LB);
    match modal {
        Some(Modal::Picker { .. }) => vec![("a", "Выбрать", 1, "Enter", Action::A), ("x", "Смотреть со звуком", 1, "F3", Action::X), ("y", "Избранное", 1, "F4", Action::Y), b],
        Some(Modal::Details { .. }) => vec![("a", "Выбрать", 1, "Enter", Action::A), ("x", "Смотреть со звуком", 1, "F3", Action::X), b],
        Some(Modal::Menu { .. }) => vec![("a", "Выбрать", 1, "Enter", Action::A), b],
        None => match (tab, key) {
            (Some(Tab::Library), Some(Key::Card(_))) => vec![("a", "Действия", 1, "Enter", Action::A), ("x", "Смотреть со звуком", 1, "F3", Action::X), ("y", "Избранное", 1, "F4", Action::Y), tabs, b],
            (Some(Tab::Catalog), Some(Key::Card(_))) => vec![("a", "Подробнее", 1, "Enter", Action::A), ("x", "Скачать", 1, "F3", Action::X), tabs, b],
            (_, Some(Key::Slot(_))) => vec![("a", "Выбрать", 1, "Enter", Action::A), ("x", "Смотреть со звуком", 1, "F3", Action::X), tabs, b],
            (_, Some(Key::Volume)) => vec![("a", "Изменить", 1, "Enter", Action::A), tabs, b],
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
            a.toast(if on { "Анимации включатся после перезапуска DeckShell" } else { "Анимации выключатся после перезапуска DeckShell" });
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
            open_menu(a, "Сортировка", opts.into_iter().enumerate().map(|(i, l)| (l, MenuAct::SetSort(i))).collect(), cur);
        }
        Key::Type => {
            let cur = with(|s| if s.tab == Some(Tab::Catalog) { s.cat_q.kind } else { s.lib_kind });
            let pos = repo::Kind::ALL.iter().position(|k| *k == cur).unwrap_or(0);
            open_menu(a, "Тип анимации", repo::Kind::ALL.iter().enumerate().map(|(i, k)| (k.label().to_string(), MenuAct::SetType(i))).collect(), pos);
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
                format!("Недостаточно места: скопировано {}", r.copied)
            } else if r.copied == 0 && r.failed == 0 {
                if r.present > 0 { "Все файлы Animation Changer уже в DeckShell".to_string() } else { "Файлы Animation Changer не найдены".to_string() }
            } else {
                format!("Скопировано {} ({}){}", count_ru(r.copied), repo::size_ru(r.bytes), if r.failed > 0 { format!(", ошибок: {}", r.failed) } else { String::new() })
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
            a.toast(if ok { "Выбор Animation Changer перенесён" } else { "Настройки Animation Changer не найдены" });
            rebuild(a);
        }
        Key::Rescan => {
            let n = anim::with_config(|c, _| {
                c.sync_downloads();
                c.scan_local();
                c.all().len()
            });
            anim::page_commit();
            a.toast(&format!("Найдено: {}", count_ru(n)));
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
            a.toast("Кэш кадров очищен");
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
            a.toast("Обложки и превью удалены");
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
    let mut v = vec![(String::new(), if slot == "shutdown" { "Как для сна".to_string() } else { "Стандартная Steam".to_string() })];
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
            o.push((format!("{}{}", if on { "✓ " } else { "Назначить: " }, slot_title(slot).to_lowercase()), MenuAct::Assign(l.id.clone(), slot)));
        }
    }
    if l.path.is_some() {
        o.push(("Смотреть со звуком".into(), MenuAct::Watch(l.id.clone(), l.target == "suspend")));
    }
    o.push((if l.favorite { "Убрать из избранного" } else { "Добавить в избранное" }.into(), MenuAct::Fav(l.id.clone())));
    o.push((if l.excluded { "Участвовать в перемешивании" } else { "Не участвовать в перемешивании" }.into(), MenuAct::Exclude(l.id.clone())));
    o.push(("Подробнее".into(), MenuAct::Details(l.id.clone())));
    if l.origin != Origin::Local {
        o.push(("Удалить".into(), MenuAct::AskDelete(l.id.clone())));
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
            b.push((format!("Отменить загрузку · {} %", (d.fraction() * 100.0) as i32), "cancel", 0, d.fraction()));
        } else if have {
            b.push(("Назначить…".into(), "assign", 0, -1.0));
        } else {
            b.push(("Скачать".into(), "download", 0, -1.0));
        }
        match &s.preview_dl {
            Some((id, f, cancel)) if *id == p.id && !cancel.load(Ordering::Relaxed) => {
                b.push((format!("Превью · {} %", (*f * 100.0) as i32), "preview-cancel", 0, *f));
            }
            Some(_) => b.push(("Предпросмотр".into(), "preview", -1, -1.0)),
            None if s.preview_id.as_ref() == Some(&p.id) && anim::player::catalog_playing() => {
                b.push(("Остановить".into(), "preview-stop", 0, -1.0));
            }
            None => b.push(("Предпросмотр".into(), "preview", 0, -1.0)),
        }
        if have {
            b.push(("Удалить".into(), "delete", 2, -1.0));
        }
    } else if let Some(id) = local {
        b.push(("Назначить…".into(), "assign", 0, -1.0));
        b.push(("Смотреть со звуком".into(), "watch", 0, -1.0));
        if c.origin(id) != Origin::Local {
            b.push(("Удалить".into(), "delete", 2, -1.0));
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
            prev.value = if fid.is_empty() { "Стандартная анимация".into() } else { "Загрузка превью…".into() };
            items.push(prev);
            let mut info = mk(22, 528.0, 385.0, 488.0, 170.0);
            anim::with_config(|c, _| {
                let l = locals(c).into_iter().find(|l| l.id == fid);
                info.label = options.get(*focus).map(|o| o.1.clone()).unwrap_or_default().into();
                info.value = l.as_ref().map(|l| [l.author.clone(), repo::size_ru(l.size)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")).unwrap_or_default().into();
                info.desc = l.map(|l| l.description).unwrap_or_else(|| if fid.is_empty() { "Steam покажет свою стандартную анимацию (или файл из uioverrides).".into() } else { String::new() }).into();
            });
            items.push(info);
            let mut hint = mk(24, 24.0, 584.0, 992.0, 32.0);
            hint.label = "A — выбрать · X — смотреть со звуком · Y — избранное · B — закрыть".into();
            items.push(hint);
            (format!("{}: выбор анимации", slot_title(slot)), MODAL_W, 640.0, *focus as i32, (24.0, 64.0, 480.0, 512.0, options.len() as f32 * 48.0))
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
                    title = if p.target == "suspend" { "Анимация сна" } else { "Анимация запуска" }.to_string();
                    if let Some(img) = s.imgs.get(&format!("thumb:{}", p.id)) {
                        prev.img = img.clone();
                        prev.has_img = true;
                    }
                    prev.value = "Обложка загружается…".into();
                    info.label = p.name.as_str().into();
                    let mut meta = vec![];
                    if !p.author.is_empty() { meta.push(format!("Автор: {}", p.author)); }
                    let when = repo::relative_ru(&p.updated, now_unix());
                    if !when.is_empty() { meta.push(format!("обновлено {when}")); }
                    let mut line2 = vec![format!("⬇ {}", repo::short_count(p.downloads)), format!("♥ {}", repo::short_count(p.likes))];
                    if let Some(d) = p.duration { line2.push(duration_ru(d)); }
                    info.value = format!("{}\n{}", meta.join(", "), line2.join(" · ")).into();
                    info.desc = if p.description.is_empty() { "Без описания".into() } else { p.description.as_str().into() };
                    let mut st = vec![];
                    if !p.for_deck() { st.push("Автор отметил ролик только для Steam Machine: на Steam Deck он может выглядеть иначе".to_string()); }
                    if let Some(e) = s.errors.get(&p.id) { st.push(e.clone()); status.on = 3; }
                    status.label = st.join(". ").into();
                } else if let Some(id) = local {
                    let l = locals(c).into_iter().find(|l| l.id == *id);
                    title = "Моя анимация".into();
                    if let Some(img) = s.imgs.get(&format!("local:{id}")) {
                        prev.img = img.clone();
                        prev.has_img = true;
                    }
                    if let Some(l) = l {
                        info.label = l.name.as_str().into();
                        let origin = match l.origin {
                            Origin::Own => "в папке DeckShell",
                            Origin::Plugin => "файл Animation Changer",
                            Origin::Local => "набор из папки",
                            Origin::Custom => "свой файл",
                            Origin::Missing => "файл не найден",
                        };
                        let mut meta = vec![];
                        if !l.author.is_empty() { meta.push(format!("Автор: {}", l.author)); }
                        meta.push(format!("{} · {}", repo::size_ru(l.size), origin));
                        info.value = meta.join("\n").into();
                        info.desc = l.description.as_str().into();
                        if !l.slots.is_empty() {
                            status.label = format!("Выбрана: {}", l.slots.iter().map(|s| slot_title(s).to_lowercase()).collect::<Vec<_>>().join(", ")).into();
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
                        None => a.toast("Сначала откройте предпросмотр или скачайте анимацию"),
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
            a.toast(if ex { "Не будет выбираться при перемешивании" } else { "Снова участвует в перемешивании" });
        }
        MenuAct::Details(id) => {
            open_details(a, None, Some(id));
            return;
        }
        MenuAct::AskDelete(id) => {
            let name = anim::with_config(|c, _| name_of(c, &id));
            open_menu(a, &format!("Удалить «{name}»?"), vec![("Удалить".into(), MenuAct::Delete(id)), ("Отмена".into(), MenuAct::Cancel)], 1);
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
                    let mut msg = if r.plugin_file_kept && !r.deleted_file { "Убрано из списка. Файл Animation Changer не удалён".to_string() } else { "Анимация удалена".to_string() };
                    if !r.cleared_slots.is_empty() {
                        msg.push_str(&format!(". Стандартная для: {}", r.cleared_slots.iter().map(|s| slot_title(s).to_lowercase()).collect::<Vec<_>>().join(", ")));
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
    a.toast(if on { "Добавлено в избранное" } else { "Убрано из избранного" });
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
            let msg = e.ru();
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
        a.toast("Эта анимация уже скачана");
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
    a.toast("Загрузка началась");
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
            a.toast(&format!("«{}» скачана", p.name));
        }
        Err(repo::NetErr::Cancelled) => a.toast("Загрузка отменена"),
        Err(e) => {
            let msg = e.ru();
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
                    a.toast("Не удалось воспроизвести предпросмотр");
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
                Err(e) if !cancel.load(Ordering::Relaxed) => a.toast(&e.ru()),
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
mod tests {
    use super::*;
    use crate::anim::config::Dirs;
    use crate::anim::repo::tests::{serve, webm, FIXTURE};
    use crate::router::Route;
    use crate::screens::settings::Page;
    use crate::AppWindow;
    use serde_json::json;

    /// One page test at a time: worker results go through the process-wide TEST_POSTS queue.
    static LOCK: Mutex<()> = Mutex::new(());

    fn fixture_dirs(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("anim-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let d = Dirs::under(&root);
        std::fs::create_dir_all(d.plugin_data.join("downloads")).unwrap();
        std::fs::create_dir_all(d.plugin_config.parent().unwrap()).unwrap();
        for (id, len) in [("PDkKY", 3_877_957), ("nvWjE", 838_027), ("ndbjY", 25_859), ("Extra", 1_000)] {
            std::fs::write(d.plugin_data.join("downloads").join(format!("{id}.webm")), webm(len)).unwrap();
        }
        std::fs::write(
            &d.plugin_config,
            serde_json::to_vec(&json!({
                "boot": "PDkKY", "suspend": "ndbjY", "throbber": "ndbjY", "randomize": "all", "force_ipv4": true,
                "downloads": [
                    {"id": "PDkKY", "name": "Playstation 2", "author": "Parimak", "target": "boot", "likes": 262, "downloads": 60373, "last_changed": "2022-10-11T21:25:37.000000Z", "description": "A (Unity remake of) Playstation 2's bootup sequence"},
                    {"id": "nvWjE", "name": "90's Valve Machine", "author": "PieMuncher", "target": "boot", "likes": 198, "downloads": 46690, "last_changed": "2022-10-12T20:18:21.000000Z"},
                    {"id": "ndbjY", "name": "minimal glitch logo suspend", "author": "x", "target": "suspend", "likes": 5, "downloads": 9, "last_changed": "2023-01-01T00:00:00.000000Z"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        d
    }

    struct H {
        ui: AppWindow,
        a: App,
        _g: std::sync::MutexGuard<'static, ()>,
        root: PathBuf,
    }

    impl Drop for H {
        fn drop(&mut self) {
            TEST_MODE.store(false, Ordering::Relaxed);
            *TEST_NET.lock().unwrap() = None;
            TEST_POSTS.lock().unwrap().clear();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn harness(name: &str) -> H {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        TEST_MODE.store(true, Ordering::Relaxed);
        TEST_POSTS.lock().unwrap().clear();
        let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            ..Default::default()
        })));
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        ui.global::<crate::Shell>().set_page(4);
        let st = ui.global::<SettingsState>();
        st.set_rail(ModelRc::new(VecModel::from(vec![crate::ListEntry { label: LABEL.into(), ..Default::default() }])));
        let ds = Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut a = App::new(ui.as_weak(), ds, Arc::new(steam_data::Library::default()), loader);
        let dirs = fixture_dirs(name);
        let root = dirs.own.parent().unwrap().to_path_buf();
        anim::install_page_config(Config::load_from(dirs));
        ST.with(|s| *s.borrow_mut() = St { lib_kind: repo::Kind::All, cat_q: repo::Query { deck_only: true, ..Default::default() }, ..Default::default() });
        a.se.pages = vec![Page { id: PAGE.into(), label: LABEL.into() }];
        a.router.replace(Route::Settings(PAGE.into()));
        a.sync_settings(PAGE);
        H { ui, a, _g: g, root }
    }

    fn drain_until(a: &mut App, what: &str, cond: impl Fn(&mut App) -> bool) {
        let t = Instant::now();
        loop {
            let posts: Vec<Post> = std::mem::take(&mut *TEST_POSTS.lock().unwrap());
            for f in posts {
                f(a);
            }
            if cond(a) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn focus() -> Option<Key> {
        with(|s| s.focus.clone())
    }

    fn cfg<R>(f: impl FnOnce(&mut Config) -> R) -> R {
        anim::with_config(|c, _| f(c))
    }

    fn catalog() -> Arc<Vec<repo::Post>> {
        Arc::new(repo::parse_catalog(FIXTURE.as_bytes()).unwrap().posts)
    }

    fn assert_clean_layout(items: &[Item], what: &str) {
        let o = overlaps(items);
        assert!(o.is_empty(), "{what}: overlapping items {:?}", o.iter().map(|(i, j)| (&items[*i].key, &items[*j].key)).collect::<Vec<_>>());
        for it in items {
            assert!(it.x >= X0 - 0.5 && it.x + it.w <= X0 + W + 0.5, "{what}: {:?} leaves the content column ({}..{})", it.key, it.x, it.x + it.w);
            assert!(it.y >= TABS_Y - 0.5, "{what}: {:?} above the tabs", it.key);
        }
    }

    #[test]
    fn every_tab_lays_out_without_overlap_inside_the_content_column() {
        let mut h = harness("layout");
        for tab in Tab::ALL {
            switch_tab(&mut h.a, tab);
            with(|s| {
                if tab == Tab::Catalog {
                    s.cat = Some(catalog());
                    s.cat_state = Some(CatState::Ready { fetched: SystemTime::now(), offline: None });
                    s.downloads.insert("MnZgE".into(), Dl { done: 50, total: Some(100), cancel: Arc::new(AtomicBool::new(false)) });
                    s.errors.insert("RuZ01".into(), "Сайт отклонил запрос (ошибка 404)".into());
                }
                if tab == Tab::Storage {
                    s.storage = Some(Storage { own: (3, 5 << 20), plugin: (31, 107 << 20), frames: 640 << 20, frames_budget: 3 << 30, repo: 45 << 20, free: Some(23 << 30) });
                }
            });
            rebuild(&mut h.a);
            let items = with(|s| s.items.clone());
            assert!(items.iter().any(|i| i.focusable && !matches!(i.key, Key::Tab(_))), "{tab:?} has controls");
            assert_clean_layout(&items, &format!("{tab:?}"));
            let tabs: Vec<&Item> = items.iter().filter(|i| matches!(i.key, Key::Tab(_))).collect();
            assert_eq!(tabs.len(), 4);
        }
        // filters narrowing the library to nothing and an empty library show a message, not an empty grid
        switch_tab(&mut h.a, Tab::Library);
        with(|s| s.lib_text = "нет такого".into());
        rebuild(&mut h.a);
        let items = with(|s| s.items.clone());
        assert!(items.iter().any(|i| i.kind == 15));
        assert_clean_layout(&items, "library empty");
    }

    #[test]
    fn gamepad_walk_assigns_slots_and_toggles_settings() {
        let mut h = harness("pad");
        let a = &mut h.a;
        assert_eq!(a.se.zone, 0);
        a.handle_pad(Action::Right);
        assert_eq!(a.se.zone, 1, "Right on the rail enters the page");
        assert_eq!(focus(), Some(Key::Slot("boot")));
        a.handle_pad(Action::Down);
        assert_eq!(focus(), Some(Key::Slot("suspend")));
        assert_eq!(with(|s| s.hero_slot), Some("suspend"), "the hero follows the focused slot");
        a.handle_pad(Action::Up);
        a.handle_pad(Action::Up);
        assert!(matches!(focus(), Some(Key::Tab(_))), "Up from the first slot reaches the tabs, got {:?}", focus());
        a.handle_pad(Action::Down);
        assert_eq!(focus(), Some(Key::Slot("boot")));
        // Y belongs to the page here: no global search
        a.handle_pad(Action::Y);
        assert!(!a.lb.search);
        // picker: preview before applying, then A assigns
        a.handle_pad(Action::A);
        let (n, first) = with(|s| match &s.modal {
            Some(Modal::Picker { options, focus, .. }) => (options.len(), options[*focus].0.clone()),
            m => panic!("picker expected, got {m:?}"),
        });
        assert_eq!(first, "PDkKY", "opens on the current choice");
        assert_eq!(n, 1 + 3, "default + three boot animations (suspend ones are not offered)");
        // options: default, then by name: 90's Valve Machine, Extra, Playstation 2 (current)
        a.handle_pad(Action::Up);
        assert_eq!(with(|s| match &s.modal { Some(Modal::Picker { options, focus, .. }) => options[*focus].0.clone(), _ => String::new() }), "Extra");
        a.handle_pad(Action::Up);
        a.handle_pad(Action::Up);
        a.handle_pad(Action::Up);
        assert!(crate::screens::chrome::hf() < 0, "the list edge does not escape to the header");
        a.handle_pad(Action::A);
        assert!(with(|s| s.modal.is_none()));
        assert_eq!(cfg(|c| c.get("boot")), "", "default chosen");
        assert_eq!(anim::with_config(|_, now| now.clone()), "", "boot_now follows like `anim set boot`");
        // B in a modal closes only the modal
        a.handle_pad(Action::A);
        assert!(with(|s| s.modal.is_some()));
        a.handle_pad(Action::B);
        assert!(with(|s| s.modal.is_none()));
        assert_eq!(a.se.zone, 1);
        // walk down to the behaviour rows
        for _ in 0..4 {
            a.handle_pad(Action::Down);
        }
        assert_eq!(focus(), Some(Key::Shuffle(ShuffleSlot::Boot)));
        a.handle_pad(Action::A);
        assert!(!cfg(|c| c.shuffle_enabled(ShuffleSlot::Boot)));
        assert!(cfg(|c| c.shuffle_enabled(ShuffleSlot::Suspend)));
        a.handle_pad(Action::Down);
        assert_eq!(focus(), Some(Key::Shuffle(ShuffleSlot::Suspend)));
        a.handle_pad(Action::A);
        assert!(!cfg(|c| c.shuffle_enabled(ShuffleSlot::Suspend)));
        assert!(!cfg(|c| c.shuffle_enabled(ShuffleSlot::Boot)));
        a.handle_pad(Action::Down);
        assert_eq!(focus(), Some(Key::BootOnResume));
        let before = cfg(|c| c.x.boot_on_resume);
        a.handle_pad(Action::A);
        assert_eq!(cfg(|c| c.x.boot_on_resume), !before);
        a.handle_pad(Action::Down);
        assert_eq!(focus(), Some(Key::Volume));
        a.handle_pad(Action::Left);
        a.handle_pad(Action::Left);
        assert!((cfg(|c| c.x.volume) - 0.8).abs() < 1e-6);
        assert_eq!(a.se.zone, 1, "Left on the volume row adjusts, it does not leave the page");
        // saved to disk
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(Config::config_path(&cfg(|c| c.dirs.clone()))).unwrap()).unwrap();
        assert_eq!(saved["randomize_boot"], false);
        assert_eq!(saved["randomize_suspend"], false);
        assert_eq!(saved["deckshell"]["volume"].as_f64().map(|v| (v * 10.0).round()), Some(8.0));
        // Left from a plain row returns to the rail
        a.handle_pad(Action::Down);
        a.handle_pad(Action::Left);
        assert_eq!(a.se.zone, 0);
    }

    #[test]
    fn library_grid_favorites_exclusions_and_delete() {
        let mut h = harness("lib");
        let a = &mut h.a;
        a.handle_pad(Action::Right);
        a.handle_pad(Action::RB);
        assert_eq!(with(|s| s.tab), Some(Tab::Library));
        assert_eq!(focus(), Some(Key::Tab(Tab::Library)));
        a.handle_pad(Action::Down);
        assert!(matches!(focus(), Some(Key::Search | Key::Sort | Key::Type | Key::FavOnly)), "{:?}", focus());
        a.handle_pad(Action::Down);
        let Some(Key::Card(first)) = focus() else { panic!("card expected, got {:?}", focus()) };
        assert_eq!(first, "nvWjE", "sorted by name: 90's Valve Machine first");
        a.handle_pad(Action::Right);
        let Some(Key::Card(second)) = focus() else { panic!() };
        assert_ne!(first, second);
        a.handle_pad(Action::Y);
        assert!(cfg(|c| c.is_favorite(&second)));
        // favorites filter
        with(|s| s.fav_only = true);
        rebuild(a);
        let cards: Vec<Key> = with(|s| s.items.iter().filter(|i| matches!(i.key, Key::Card(_))).map(|i| i.key.clone()).collect());
        assert_eq!(cards, vec![Key::Card(second.clone())]);
        with(|s| s.fav_only = false);
        rebuild(a);
        // card menu → shuffle exclusion
        with(|s| s.focus = Some(Key::Card("ndbjY".into())));
        a.handle_pad(Action::A);
        let opts = with(|s| match &s.modal {
            Some(Modal::Menu { options, .. }) => options.clone(),
            m => panic!("{m:?}"),
        });
        assert!(opts.iter().any(|o| matches!(o.1, MenuAct::Assign(_, "suspend"))));
        assert!(!opts.iter().any(|o| matches!(o.1, MenuAct::Assign(_, "boot"))), "a sleep animation is not offered for boot");
        let ex = opts.iter().position(|o| matches!(o.1, MenuAct::Exclude(_))).unwrap();
        for _ in 0..ex {
            a.handle_pad(Action::Down);
        }
        a.handle_pad(Action::A);
        assert!(cfg(|c| c.is_excluded("ndbjY")));
        // delete asks first; default answer is «Отмена»
        a.handle_pad(Action::A);
        let del = with(|s| match &s.modal {
            Some(Modal::Menu { options, .. }) => options.iter().position(|o| matches!(o.1, MenuAct::AskDelete(_))).unwrap(),
            _ => panic!(),
        });
        for _ in 0..del {
            a.handle_pad(Action::Down);
        }
        a.handle_pad(Action::A);
        assert!(with(|s| matches!(&s.modal, Some(Modal::Menu { focus: 1, .. }))));
        a.handle_pad(Action::Up);
        a.handle_pad(Action::A);
        assert!(cfg(|c| c.entry("ndbjY").is_none()));
        assert_eq!(cfg(|c| (c.get("suspend"), c.get("throbber"))), (String::new(), String::new()), "slots reset");
        assert!(cfg(|c| c.plugin_download("ndbjY").is_file()), "the plugin's file is not deleted");
        // grid edge: Left from the first column goes back to the rail
        with(|s| s.focus = Some(Key::Card(first.clone())));
        a.handle_pad(Action::Left);
        assert_eq!(a.se.zone, 0);
    }

    #[test]
    fn catalog_download_cancel_errors_and_offline_on_fixtures() {
        let mut h = harness("cat");
        let video = webm(300_000);
        let v2 = video.clone();
        let f = serve(move |p, base| match p {
            "/api/posts/all" => (200, vec![], FIXTURE.as_bytes().to_vec(), 0),
            "/post/download/MnZgE" => (302, vec![("Location".into(), format!("{base}/videos/MnZgE.webm?sig=1"))], vec![], 0),
            "/videos/MnZgE.webm?sig=1" => (200, vec![], v2.clone(), 0),
            "/post/download/Mach1" => (200, vec![], webm(2_000_000), 40),
            _ => (404, vec![], vec![], 0),
        });
        *TEST_NET.lock().unwrap() = Some((f.base.clone(), f.base.trim_start_matches("http://").into()));
        let a = &mut h.a;
        a.handle_pad(Action::Right);
        a.handle_pad(Action::RB);
        a.handle_pad(Action::RB);
        assert_eq!(with(|s| s.tab), Some(Tab::Catalog));
        drain_until(a, "catalog", |_| with(|s| matches!(s.cat_state, Some(CatState::Ready { offline: None, .. }))));
        assert_eq!(f.hits.lock().unwrap().iter().filter(|p| *p == "/api/posts/all").count(), 1);
        assert_eq!(with(|s| s.cat_view.len()), 5, "Steam Deck filter hides the Steam Machine-only post");
        let status = with(|s| s.items.iter().find(|i| i.kind == 10).map(|i| i.label.clone()).unwrap());
        assert!(status.starts_with("5 анимаций"), "{status}");
        assert!(cfg(|c| c.dirs.repo.join("posts.json").is_file()), "saved for offline use");
        // search through the shared keyboard target
        with(|s| s.focus = Some(Key::Search));
        a.handle_pad(Action::A);
        assert!(keyboard_target());
        for c in "star".chars() {
            type_char(a, Some(c));
        }
        assert_eq!(with(|s| s.cat_view.iter().map(|i| s.cat.as_ref().unwrap()[*i].id.clone()).collect::<Vec<_>>()), vec!["MnZgE"]);
        assert!(a.settings_back(), "B closes the keyboard first");
        assert!(!keyboard_target());
        for _ in 0..4 {
            type_char(a, None);
        }
        // an animation the plugin already downloaded is not fetched again
        with(|s| s.focus = Some(Key::Card("PDkKY".into())));
        a.handle_pad(Action::X);
        assert!(!with(|s| s.downloads.contains_key("PDkKY")));
        // download with progress → appears in «Мои анимации»
        with(|s| s.focus = Some(Key::Card("MnZgE".into())));
        a.handle_pad(Action::X);
        assert!(with(|s| s.downloads.contains_key("MnZgE")));
        drain_until(a, "download", |_| with(|s| !s.downloads.contains_key("MnZgE")));
        assert_eq!(std::fs::read(cfg(|c| c.own_download("MnZgE"))).unwrap(), video);
        assert_eq!(cfg(|c| c.origin("MnZgE")), Origin::Own);
        assert_eq!(cfg(|c| c.entry("MnZgE").unwrap()["download_url"].clone()), json!("https://steamdeckrepo.com/post/download/MnZgE"));
        let card = with(|s| s.items.iter().find(|i| i.key == Key::Card("MnZgE".into())).cloned().unwrap());
        assert_eq!(card.badge2, "Скачано");
        assert!(cfg(|c| locals(c).iter().any(|l| l.id == "MnZgE" && l.author == "Jaidek")));
        // 404 → a readable error on the card and in details
        with(|s| s.focus = Some(Key::Card("RuZ01".into())));
        a.handle_pad(Action::X);
        drain_until(a, "error", |_| with(|s| s.errors.contains_key("RuZ01")));
        assert!(with(|s| s.errors["RuZ01"].contains("404")));
        assert!(!cfg(|c| c.own_download("RuZ01")).exists());
        assert_eq!(with(|s| s.items.iter().find(|i| i.key == Key::Card("RuZ01".into())).map(|i| i.badge2.clone())), Some("Ошибка".into()));
        // cancel: X again on a running download
        with(|s| s.cat_q.deck_only = false);
        rebuild(a);
        with(|s| s.focus = Some(Key::Card("Mach1".into())));
        a.handle_pad(Action::X);
        drain_until(a, "progress", |_| with(|s| s.downloads.get("Mach1").is_some_and(|d| d.done > 0)));
        a.handle_pad(Action::X);
        drain_until(a, "cancel", |_| with(|s| !s.downloads.contains_key("Mach1")));
        let dest = cfg(|c| c.own_download("Mach1"));
        assert!(!dest.exists() && !repo::part_path(&dest).exists());
        assert!(cfg(|c| c.entry("Mach1")).is_none());
        // details modal of a catalog post: buttons and A → download state
        with(|s| s.focus = Some(Key::Card("RuZ01".into())));
        a.handle_pad(Action::A);
        assert!(with(|s| matches!(s.modal, Some(Modal::Details { .. }))));
        let labels = with(|s| anim::with_config(|c, _| detail_buttons(s, c, match &s.modal { Some(Modal::Details { post, .. }) => *post, _ => None }, &None)));
        assert_eq!(labels.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), vec!["Скачать", "Предпросмотр"]);
        a.handle_pad(Action::B);
        // offline: the saved catalog stays usable with a warning
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = dead.local_addr().unwrap().to_string();
        drop(dead);
        *TEST_NET.lock().unwrap() = Some((format!("http://{addr}"), addr));
        ensure_catalog(a, true);
        drain_until(a, "offline", |_| with(|s| matches!(s.cat_state, Some(CatState::Ready { offline: Some(_), .. }))));
        let status = with(|s| s.items.iter().find(|i| i.kind == 10).map(|i| (i.label.clone(), i.on)).unwrap());
        assert!(status.0.contains("Нет подключения") && status.0.contains("сохранённый каталог"), "{}", status.0);
        assert_eq!(status.1, 2);
        // a fresh start without network and without a saved catalog says so
        with(|s| {
            s.cat = None;
            s.cat_state = None;
        });
        let _ = std::fs::remove_file(cfg(|c| c.dirs.repo.join("posts.json")));
        ensure_catalog(a, false);
        drain_until(a, "failed", |_| with(|s| matches!(s.cat_state, Some(CatState::Failed(_)))));
        assert!(with(|s| s.items.iter().any(|i| i.kind == 10 && i.on == 3)));
    }

    #[test]
    fn catalog_preview_keeps_gamepad_focus_and_closing_stops_it() {
        let mut h = harness("catalog-preview-pad");
        let a = &mut h.a;
        a.handle_pad(Action::Right);
        with(|s| {
            s.tab = Some(Tab::Catalog);
            s.cat = Some(catalog());
            s.focus = Some(Key::Card("PDkKY".into()));
        });
        rebuild(a);
        a.handle_pad(Action::A);
        a.handle_pad(Action::Right);
        a.handle_pad(Action::A);
        assert!(anim::player::catalog_playing());
        assert_eq!(anim::player::INPUT_MODE.load(Ordering::Relaxed), 0);
        assert_eq!(with(|s| modal_focus(s)), 1);
        a.handle_pad(Action::B);
        assert!(!anim::player::catalog_playing());
        assert!(with(|s| s.modal.is_none() && s.preview_id.is_none()));
        assert_eq!(focus(), Some(Key::Card("PDkKY".into())));

        a.handle_pad(Action::A);
        a.handle_pad(Action::Right);
        a.handle_pad(Action::A);
        assert!(anim::player::catalog_playing());
        switch_tab(a, Tab::Library);
        assert!(!anim::player::catalog_playing());
        assert!(with(|s| s.modal.is_none() && s.preview_id.is_none()));
    }

    #[test]
    fn grid_moves_by_index_including_the_last_partial_row() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<String> = (0..7).map(|i| format!("c{i}")).collect();
        with(|s| {
            s.items = ids.iter().map(|id| Item::new(Key::Card(id.clone()), 6, 0.0, 0.0, 1.0, 1.0)).collect();
        });
        let mv = |from: &str, act| card_move(Tab::Library, &Some(Key::Card(from.into())), act);
        assert_eq!(mv("c0", Action::Right), Some(Key::Card("c1".into())));
        assert_eq!(mv("c2", Action::Right), None, "row end");
        assert_eq!(mv("c0", Action::Left), None, "leaves the grid (rail)");
        assert_eq!(mv("c1", Action::Down), Some(Key::Card("c4".into())));
        assert_eq!(mv("c5", Action::Down), Some(Key::Card("c6".into())), "to the last card of a shorter row");
        assert_eq!(mv("c6", Action::Down), None);
        assert_eq!(mv("c4", Action::Up), Some(Key::Card("c1".into())));
        assert_eq!(mv("c1", Action::Up), None, "first row: spatial move to the filters");
    }

    // ------------------------------------------------ PNG gallery of the key states (DECKSHELL_GALLERY_DIR=<dir> to save)

    fn solid(w: u32, h: u32, rgb: [u8; 3], stripe: [u8; 3]) -> Image {
        let mut b = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        for (i, px) in b.make_mut_slice().iter_mut().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let c = if (x + y) / 24 % 2 == 0 { rgb } else { stripe };
            *px = Rgba8Pixel { r: c[0], g: c[1], b: c[2], a: 255 };
        }
        Image::from_rgba8(b)
    }

    #[test]
    fn page_gallery_renders_key_states() {
        let mut h = harness("gallery");
        let dir = std::env::var_os("DECKSHELL_GALLERY_DIR").map(PathBuf::from);
        if let Some(d) = &dir {
            std::fs::create_dir_all(d).unwrap();
        }
        let ui = h.ui.clone_strong();
        let save = |name: &str| {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::mock_elapsed_time(Duration::from_secs(1));
            slint::platform::update_timers_and_animations();
            let img = ui.window().take_snapshot().unwrap();
            assert_eq!((img.width(), img.height()), (1280, 800));
            // something is drawn in the content column (not an empty page)
            let px = img.as_bytes();
            let lit = (200..700).filter(|y| (400..1240).any(|x| { let o = (y * 1280 + x) * 4; px[o] > 120 })).count();
            assert!(lit > 20, "{name}: content column is empty");
            if let Some(d) = &dir {
                image::save_buffer(d.join(format!("anim-{name}.png")), img.as_bytes(), img.width(), img.height(), image::ColorType::Rgba8).unwrap();
            }
        };
        let a = &mut h.a;
        let palette = [([28, 74, 120], [36, 90, 140]), ([110, 40, 60], [130, 52, 74]), ([40, 100, 60], [52, 120, 74]), ([90, 70, 20], [110, 88, 30]), ([60, 40, 110], [74, 52, 130]), ([20, 90, 100], [30, 110, 120])];
        let ids = ["PDkKY", "nvWjE", "ndbjY", "Extra", "MnZgE", "RuZ01", "Mach1"];
        with(|s| {
            for (i, id) in ids.iter().enumerate() {
                let (c1, c2) = palette[i % palette.len()];
                s.imgs.insert(format!("local:{id}"), solid(288, 180, c1, c2));
                s.imgs.insert(format!("thumb:{id}"), solid(288, 180, c2, c1));
            }
        });
        let hero = solid(336, 210, [24, 60, 100], [30, 76, 120]);
        a.handle_pad(Action::Right);
        {
            let ui = a.ui();
            let g = ui.global::<AnimPageState>();
            g.set_preview(hero.clone());
            g.set_has_preview(true);
        }
        save("overview");
        a.handle_pad(Action::A);
        {
            let ui = a.ui();
            ui.global::<AnimPageState>().set_preview(solid(488, 305, [80, 30, 50], [100, 40, 64]));
        }
        save("picker");
        a.handle_pad(Action::B);
        cfg(|c| c.set_shuffle(ShuffleSlot::Boot, false));
        rebuild(a);
        for _ in 0..7 {
            a.handle_pad(Action::Down);
        }
        save("overview-scrolled");
        a.handle_pad(Action::RB);
        a.handle_pad(Action::Down);
        a.handle_pad(Action::Down);
        save("library");
        a.handle_pad(Action::A);
        save("library-menu");
        a.handle_pad(Action::B);
        cfg(|c| c.toggle_favorite("PDkKY"));
        rebuild(a);
        a.handle_pad(Action::RB);
        with(|s| {
            s.cat = Some(catalog());
            s.cat_state = Some(CatState::Ready { fetched: SystemTime::now() - Duration::from_secs(300), offline: None });
            s.downloads.insert("MnZgE".into(), Dl { done: 45, total: Some(100), cancel: Arc::new(AtomicBool::new(false)) });
            s.errors.insert("RuZ01".into(), repo::NetErr::Status(404).ru());
            s.focus = Some(Key::Card("MnZgE".into()));
        });
        rebuild(a);
        save("catalog");
        with(|s| s.focus = Some(Key::Card("PDkKY".into())));
        a.handle_pad(Action::A);
        a.handle_pad(Action::Right);
        a.handle_pad(Action::A);
        let g = ui.global::<AnimPageState>();
        g.set_preview(solid(560, 350, [24, 60, 100], [30, 76, 120]));
        g.set_has_preview(true);
        save("catalog-preview");
        a.handle_pad(Action::B);
        with(|s| s.focus = Some(Key::Card("RuZ01".into())));
        a.handle_pad(Action::A);
        save("catalog-details-error");
        a.handle_pad(Action::B);
        with(|s| {
            s.cat_state = Some(CatState::Ready { fetched: SystemTime::now() - Duration::from_secs(3 * 86_400), offline: Some(repo::NetErr::Offline(String::new()).ru()) });
            s.focus = Some(Key::Search);
            s.cat_q.text = "star".into();
        });
        rebuild(a);
        save("catalog-offline-search");
        with(|s| {
            s.cat = None;
            s.cat_state = Some(CatState::Failed(repo::NetErr::RateLimited.ru()));
            s.cat_q.text.clear();
        });
        rebuild(a);
        save("catalog-error");
        a.handle_pad(Action::RB);
        with(|s| s.storage = Some(Storage { own: (3, 5 << 20), plugin: (31, 107 << 20), frames: 640 << 20, frames_budget: 3 << 30, repo: 45 << 20, free: Some(23 << 30) }));
        rebuild(a);
        save("storage");
    }
}
