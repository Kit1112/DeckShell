//! Глобальный поиск. Ранжирование работает со снимком библиотеки, Slint получает только видимые ряды.
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc, LazyLock};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::app::App;
use crate::assets::{t, t_or};
use crate::input::Action;
use crate::loader::Kind;
use crate::model::{self, Extra, Sort, SORTS};
use crate::router::Route;
use crate::screens::library::collate_key;
use crate::screens::settings::{parse_rows, to_grow, PRow, SC};
use crate::sound::{play, Snd};
use crate::{AppWindow, GPageRow, SearchState};

const PAGE: &str = include_str!("../../ui/generated/search/page.json");
static TEMPLATE: LazyLock<Vec<PRow>> =
    LazyLock::new(|| parse_rows(&serde_json::from_str(PAGE).expect("геометрия поиска")));

const COLS: usize = 5;
const X0: f32 = 30.0;
const PX: f32 = 204.0;
const Y0: f32 = 98.0;
const PY: f32 = 140.0;
const VIEW_HEIGHT: f32 = 578.0;
const RESULT_LIMIT: usize = 300;
const DEBOUNCE: Duration = Duration::from_millis(70);

#[derive(Default)]
pub struct State {
    /// 0 поле, 1 вкладки, 2 результаты.
    zone: u8,
    tab: usize,
    focus: usize,
    caret: usize,
    revision: u64,
    displayed: Option<u64>,
    model_generation: i32,
    pending: bool,
    focus_on_results: bool,
    timer: Timer,
    worker: Option<(u64, mpsc::Receiver<GameHits>)>,
    game_hits: GameHits,
    hits: Vec<Hit>,
    first: Option<usize>,
    rows: Rc<VecModel<GPageRow>>,
    /// Обложки живут вместе со строками видимой области и переживают вытеснение из общего кэша.
    artwork_rows: HashMap<u32, usize>,
}

#[derive(Default)]
struct GameHits {
    games: Vec<u32>,
    tools: Vec<u32>,
}

#[derive(Clone, Copy)]
enum Hit {
    Game(u32),
    Friend(u32),
    Store,
}

fn is_tool(name: &str) -> bool {
    ["Proton", "Steam Linux Runtime", "Steamworks Common", "SteamVR", "Steam Controller Configs"]
        .iter().any(|p| name.starts_with(p))
        || name.ends_with("SDK") || name.ends_with("Dedicated Server")
}

fn fuzzy(q: &str, name: &str) -> Option<i32> {
    let n = name.to_lowercase();
    if let Some(p) = n.find(q) {
        let word_start = p == 0 || n.as_bytes()[p - 1] == b' ';
        return Some(2000 - p as i32 * 4 - n.len() as i32 + if word_start { 400 } else { 0 });
    }
    let mut score = 1000;
    let mut it = n.chars().enumerate();
    let mut last: Option<usize> = None;
    for qc in q.chars() {
        loop {
            let (i, c) = it.next()?;
            if c == qc {
                match last {
                    Some(l) if i == l + 1 => score += 20,
                    Some(l) => score -= (i - l) as i32 * 3,
                    None => score -= i as i32,
                }
                last = Some(i);
                break;
            }
        }
    }
    Some(score)
}

fn rank(lib: &steam_data::Library, extras: &HashMap<u32, Extra>, sort: Sort, query: &str) -> GameHits {
    let query = query.trim().to_lowercase();
    let default = Extra::default();
    let mut matches: Vec<_> = lib.games.iter().filter_map(|game| {
        let score = if query.is_empty() { 0 } else { fuzzy(&query, &game.name)? };
        let extra = extras.get(&game.app_id).unwrap_or(&default);
        Some((game, score, collate_key(&game.sort_name), extra))
    }).collect();
    matches.sort_by(|(a, sa, ka, ea), (b, sb, kb, eb)| {
        let order = if query.is_empty() { model::primary(sort, a, b, ea, eb) } else { sb.cmp(sa) };
        order.then_with(|| ka.cmp(kb))
    });
    let mut result = GameHits::default();
    for (game, ..) in matches {
        let list = if is_tool(&game.name) { &mut result.tools } else { &mut result.games };
        if list.len() < RESULT_LIMIT {
            list.push(game.app_id);
        }
    }
    result
}

fn header_rows() -> impl Iterator<Item = &'static PRow> {
    TEMPLATE.iter().filter(|r| r.k == 0 || r.k == 9)
}

pub fn init_strings(ui: &AppWindow) {
    ui.global::<SearchState>().set_no_results(t_or("#Search_NoResults", "").into());
}

pub fn wire(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.global::<SearchState>().on_activated(move |i| {
        if let Some(ui) = weak.upgrade() {
            let state = ui.global::<SearchState>();
            let first = state.get_first() as usize;
            let generation = state.get_model_generation();
            super::defer(move |a| a.search_row_clicked(i as usize, first, generation));
        }
    });
    ui.global::<SearchState>().on_scrolled(|| super::defer(|a| a.search_render(false)));
}

impl App {
    /// Используется и обычным входом, и прямым переходом на маршрут.
    pub fn enter_search(&mut self) {
        if self.lb.search {
            return;
        }
        self.lb.search = true;
        self.lb.query.clear();
        self.search.zone = 0;
        self.search.tab = 0;
        self.search.caret = 0;
        self.search.focus = 0;
        let ui = self.ui();
        let st = ui.global::<SearchState>();
        st.set_rows(ModelRc::from(self.search.rows.clone()));
        st.set_vy(0.0);
        self.sync_caret();
        self.search_render(true);
        self.search_changed(true);
        let started = Instant::now();
        self.osk_set_open(true);
        crate::framestats::search_span("open_osk", started);
    }

    pub fn open_search(&mut self) {
        let started = Instant::now();
        self.sm.open = false;
        self.qm.open = false;
        self.lb.sort_open = false;
        self.header_clear();
        self.router.push(Route::Search);
        self.enter_route();
        crate::framestats::search_span("open", started);
    }

    pub fn close_search(&mut self) {
        if !self.lb.search {
            return;
        }
        self.osk_set_open(false);
        self.lb.search = false;
        self.lb.query.clear();
        self.sync_caret();
        self.search.revision = self.search.revision.wrapping_add(1);
        self.search.displayed = None;
        self.search.pending = false;
        self.search.focus_on_results = false;
        self.search.timer.stop();
        self.search.first = None;
        self.search.game_hits = GameHits::default();
        self.search.hits.clear();
        self.search.artwork_rows.clear();
        self.search.rows.set_vec(Vec::new());
        self.bump_search_model();
        self.set_search_pending(false);
        self.update_hint();
    }

    pub fn search_focus_field(&mut self) {
        self.search.zone = 0;
        self.search.focus_on_results = false;
        self.search_sync_focus();
    }

    fn caret(&self) -> usize {
        self.search.caret.min(self.lb.query.chars().count())
    }

    fn byte_at(&self, ci: usize) -> usize {
        self.lb.query.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(self.lb.query.len())
    }

    pub fn sync_caret(&self) {
        let (a, b) = self.lb.query.split_at(self.byte_at(self.caret()));
        let ui = self.ui();
        let tb = ui.global::<crate::SearchCaret>();
        tb.set_before(a.into());
        tb.set_after(b.into());
        let tb = ui.global::<crate::TopBarState>();
        tb.set_search_active(self.lb.search);
        tb.set_search_query(self.lb.query.as_str().into());
    }

    pub fn search_char(&mut self, c: char) {
        let at = self.byte_at(self.caret());
        self.lb.query.insert(at, c);
        self.search.caret = self.caret() + 1;
        self.search_query_changed();
    }

    pub fn search_backspace(&mut self) {
        let c = self.caret();
        if c > 0 {
            let at = self.byte_at(c - 1);
            self.lb.query.remove(at);
            self.search.caret = c - 1;
            self.search_query_changed();
        }
    }

    pub fn search_caret_move(&mut self, d: i32) {
        let n = self.lb.query.chars().count() as i32;
        self.search.caret = (self.caret() as i32 + d).clamp(0, n) as usize;
        self.sync_caret();
    }

    pub fn search_paste(&mut self) {
        let revision = self.search.revision;
        std::thread::spawn(move || {
            let out = clipboard_text();
            super::defer(move |a| {
                if a.lb.search && revision == a.search.revision {
                    a.search_paste_done(out);
                }
            });
        });
    }

    fn search_paste_done(&mut self, out: Option<String>) {
        match out {
            Some(t) => {
                let text: String = paste_chars(&t).into_iter().collect();
                self.search.caret = self.caret();
                let at = self.byte_at(self.search.caret);
                self.lb.query.insert_str(at, &text);
                self.search.caret += text.chars().count();
                self.search_query_changed();
            }
            None => self.toast(&t_or("#DeckShell_ClipboardEmpty", "Буфер обмена пуст")),
        }
    }

    fn search_query_changed(&mut self) {
        let started = Instant::now();
        self.search.focus = 0;
        self.search.focus_on_results = false;
        self.sync_caret();
        self.search_changed(false);
        play(Snd::Typing);
        crate::framestats::search_span("input", started);
    }

    pub fn search_catalog_changed(&mut self) {
        if self.lb.search {
            self.search_changed(true);
        }
    }

    fn set_search_pending(&self, pending: bool) {
        self.ui().global::<SearchState>().set_pending(pending);
    }

    fn search_changed(&mut self, immediate: bool) {
        self.search.revision = self.search.revision.wrapping_add(1);
        self.search.pending = false;
        self.set_search_pending(true);
        if immediate {
            self.search_submit();
        } else {
            self.search.timer.start(TimerMode::SingleShot, DEBOUNCE, || {
                crate::with_app(|a| a.search_submit());
            });
        }
    }

    fn search_submit(&mut self) {
        self.search.timer.stop();
        self.search.pending = self.lb.search
            && self.search.displayed != Some(self.search.revision)
            && !self.search.worker.as_ref().is_some_and(|(rev, _)| *rev == self.search.revision);
        self.search_start_worker();
    }

    fn search_start_worker(&mut self) {
        if !self.search.pending || self.search.worker.is_some() {
            return;
        }
        self.search.pending = false;
        let lib = Arc::clone(&self.lib);
        let extras = Arc::clone(&self.extras);
        let sort = SORTS[self.lb.sort];
        let query = self.lb.query.clone();
        let (tx, rx) = mpsc::channel();
        self.search.worker = Some((self.search.revision, rx));
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = rank(&lib, &extras, sort, &query);
            crate::framestats::search_span("rank_worker", started);
            let _ = tx.send(result);
            super::defer(|a| {
                if let Some((revision, rx)) = a.search.worker.as_ref() {
                    if let Ok(result) = rx.try_recv() {
                        a.search_complete(*revision, result);
                    }
                }
            });
        });
    }

    fn search_complete(&mut self, revision: u64, result: GameHits) {
        let started = Instant::now();
        self.search.worker = None;
        if self.lb.search && revision == self.search.revision {
            self.search.displayed = Some(revision);
            self.search.game_hits = result;
            self.set_search_pending(false);
            self.search_show_hits();
        }
        self.search_start_worker();
        crate::framestats::search_span("results_apply", started);
    }

    pub fn search_to_results(&mut self) {
        if self.search.displayed != Some(self.search.revision) {
            self.search.focus_on_results = true;
            self.search_submit();
        } else {
            self.search.zone = if self.search.hits.is_empty() { 1 } else { 2 };
            self.search_sync_focus();
        }
    }

    fn search_show_hits(&mut self) {
        let q = self.lb.query.trim().to_lowercase();
        let friends = || self.search_friend_ids(&q).map(Hit::Friend);
        let hits = match self.search.tab {
            0 => self.search.game_hits.games.iter().copied().map(Hit::Game).chain(friends().take(10)).collect(),
            1 => self.search.game_hits.games.iter().copied().map(Hit::Game).collect(),
            2 => friends().take(60).collect(),
            3 if !q.is_empty() => vec![Hit::Store],
            3 => vec![],
            _ => self.search.game_hits.tools.iter().copied().map(Hit::Game).collect(),
        };
        let s = &mut self.search;
        s.hits = hits;
        if s.focus_on_results || s.zone == 2 {
            s.zone = if s.hits.is_empty() { 1 } else { 2 };
        }
        s.focus_on_results = false;
        s.focus = s.focus.min(s.hits.len().saturating_sub(1));
        let ui = self.ui();
        let st = ui.global::<SearchState>();
        st.set_result_count(self.search.hits.len() as i32);
        st.set_content_h(((Y0 + self.search.hits.len().div_ceil(COLS) as f32 * PY).max(VIEW_HEIGHT) * SC).into());
        st.set_vy((-self.search_scroll(0.0) * SC).into());
        self.search_render(true);
    }

    fn bump_search_model(&mut self) {
        self.search.model_generation = self.search.model_generation.wrapping_add(1);
        self.ui().global::<SearchState>().set_model_generation(self.search.model_generation);
    }

    fn search_render(&mut self, reset: bool) {
        if !self.lb.search {
            return;
        }
        let ui = self.ui();
        let st = ui.global::<SearchState>();
        let y = -f32::from(st.get_vy()) / SC;
        let first = ((y + 40.0 - Y0) / PY).floor().max(0.0) as usize * COLS;
        if !reset && self.search.first == Some(first) {
            return;
        }
        let started = Instant::now();
        if reset {
            self.bump_search_model();
        }
        let mut visible_art = std::mem::take(&mut self.search.artwork_rows);
        let mut rows: Vec<_> = header_rows().scan(0, |tab, r| {
            let on = r.k == 9 && *tab == self.search.tab;
            *tab += usize::from(r.k == 9);
            Some(to_grow(r, i32::from(on)))
        }).collect();
        let card = TEMPLATE.iter().find(|r| r.k == 8).expect("шаблон карточки");
        let img = TEMPLATE.iter().find(|r| r.k == 6).expect("шаблон обложки");
        let in_library = t(&card.tok).to_uppercase();
        // Пять видимых рядов и один запасной ряд для быстрого скролла.
        let end = (first + COLS * 6).min(self.search.hits.len());
        for n in first..end {
            let x = X0 + (n % COLS) as f32 * PX;
            let y = Y0 + (n / COLS) as f32 * PY;
            let (cap, name, id) = match self.search.hits[n] {
                Hit::Game(id) => {
                    let name = self.gidx.get(&id).map(|&gi| self.games()[gi].name.clone()).unwrap_or_default();
                    (in_library.clone(), name, Some(id))
                }
                Hit::Friend(id) => {
                    let (name, status) = self.search_friend_label(id).unwrap_or_default();
                    (status.to_uppercase(), name, None)
                }
                Hit::Store => (t_or("#Menu_Store", "МАГАЗИН").to_uppercase(), format!("{}: {}", t_or("#Menu_Store", "Магазин"), self.lb.query.trim()), None),
            };
            let mut c = to_grow(&PRow { k: 15, role: "button".into(), x, y, w: card.w, h: card.h, pad: card.pad.clone(), tok: if id.is_some() { card.tok.clone() } else { String::new() }, lab: cap, ..Default::default() }, 0);
            c.sub = name.as_str().into();
            rows.push(c);
            let mut im = to_grow(&PRow { k: 6, role: "image".into(), x, y: y + 6.0, w: img.w, h: img.h, pad: img.pad.clone(), lab: name, ..Default::default() }, 0);
            if let Some(id) = id {
                let artwork = visible_art.remove(&id).filter(|_| !reset)
                    .and_then(|row| self.search.rows.row_data(row))
                    .filter(|row| row.has_img).map(|row| row.img)
                    .or_else(|| self.imgs.get(&(id, Kind::Header)).cloned());
                if let Some(artwork) = artwork {
                    im.img = artwork;
                    im.has_img = true;
                } else if let Some(&gi) = self.gidx.get(&id) {
                    self.request(gi, Kind::Header);
                }
                self.search.artwork_rows.insert(id, rows.len());
            }
            rows.push(im);
        }
        self.search.first = Some(first);
        st.set_first(first as i32);
        self.search.rows.set_vec(rows);
        self.search_sync_focus();
        crate::framestats::search_span("viewport", started);
    }

    pub fn search_header_ready(&mut self, id: u32) {
        let Some(&index) = self.search.artwork_rows.get(&id) else { return };
        let Some(mut row) = self.search.rows.row_data(index) else { return };
        let Some(img) = self.imgs.get(&(id, Kind::Header)) else { return };
        let started = Instant::now();
        row.img = img.clone();
        row.has_img = true;
        self.search.rows.set_row_data(index, row);
        crate::framestats::search_span("header_apply", started);
    }

    fn search_sync_focus(&self) {
        let s = &self.search;
        let focus = match s.zone {
            1 => header_rows().enumerate().filter(|(_, r)| r.k == 9).nth(s.tab).map(|(i, _)| i),
            2 => s.first.and_then(|first| s.focus.checked_sub(first))
                .map(|n| header_rows().count() + 2 * n)
                .filter(|&row| row < s.rows.row_count()),
            _ => None,
        };
        self.ui().global::<SearchState>().set_rfocus(focus.map_or(-1, |i| i as i32));
    }

    fn search_set_tab(&mut self, tab: usize) {
        if tab != self.search.tab {
            self.search.tab = tab;
            self.search.focus = 0;
            self.search.focus_on_results = false;
            if self.search.displayed == Some(self.search.revision) {
                self.search_show_hits();
                return;
            }
            self.search.hits.clear();
            self.search_render(true);
        }
        self.search_sync_focus();
    }

    fn search_row_clicked(&mut self, i: usize, first: usize, generation: i32) {
        if !self.lb.search || self.search.model_generation != generation {
            return;
        }
        let head = header_rows().count();
        if i < head {
            if let Some(tab) = header_rows().enumerate().filter(|(_, r)| r.k == 9).position(|(n, _)| n == i) {
                self.search.zone = 1;
                self.search_set_tab(tab);
            }
            return;
        }
        self.search_open_hit(first + (i - head) / 2);
    }

    fn search_open_hit(&mut self, n: usize) {
        if !self.lb.search || self.search.displayed != Some(self.search.revision) {
            return;
        }
        match self.search.hits.get(n).copied() {
            Some(Hit::Game(id)) => {
                if let Some(&gi) = self.gidx.get(&id) {
                    self.open_game(gi);
                }
            }
            Some(Hit::Friend(id)) => self.act(crate::actions::Act::OpenChat(id)),
            Some(Hit::Store) => self.act(crate::actions::Act::StoreSearch(self.lb.query.trim().to_string())),
            None => {}
        }
    }

    pub fn handle_search(&mut self, a: Action) {
        if self.osk_handle(a) {
            return;
        }
        let zone = self.search.zone;
        match a {
            Action::B | Action::Search | Action::Steam | Action::Qam => {
                self.close_search();
                if self.router.current() == &Route::Search {
                    self.go_back();
                }
                match a {
                    Action::Steam => self.toggle_side(),
                    Action::Qam => self.toggle_qam(),
                    _ => {}
                }
            }
            Action::Y if zone != 0 => {
                self.close_search();
                self.go_back();
            }
            Action::Char(c) => self.search_char(c),
            Action::Backspace | Action::X => self.search_backspace(),
            Action::A if zone == 0 => self.osk_set_open(true),
            Action::Left if zone == 0 => self.search_caret_move(-1),
            Action::Right if zone == 0 => self.search_caret_move(1),
            Action::LB => self.search_set_tab(self.search.tab.saturating_sub(1)),
            Action::RB => self.search_set_tab((self.search.tab + 1).min(4)),
            _ => self.search_nav(a),
        }
    }

    fn search_nav(&mut self, a: Action) {
        let n = if self.search.displayed == Some(self.search.revision) { self.search.hits.len() } else { 0 };
        let zone = self.search.zone;
        let tab = self.search.tab;
        let focus = self.search.focus;
        match (zone, a) {
            (1, Action::Left) => return self.search_set_tab(tab.saturating_sub(1)),
            (1, Action::Right) => return self.search_set_tab((tab + 1).min(4)),
            (2, Action::A) => return self.search_open_hit(focus),
            _ => {}
        }
        let s = &mut self.search;
        match (zone, a) {
            (0, Action::Down) => s.zone = 1,
            (1, Action::Up) => s.zone = 0,
            (1, Action::Down | Action::A) if n > 0 => {
                s.zone = 2;
                s.focus = 0;
            }
            (2, Action::Left) if s.focus % COLS > 0 => s.focus -= 1,
            (2, Action::Right) if s.focus % COLS + 1 < COLS && s.focus + 1 < n => s.focus += 1,
            (2, Action::Down) if s.focus + COLS < n => s.focus += COLS,
            (2, Action::Down) if s.focus / COLS < n.saturating_sub(1) / COLS => s.focus = n - 1,
            (2, Action::Up) if s.focus >= COLS => s.focus -= COLS,
            (2, Action::Up) => s.zone = 1,
            _ => return,
        }
        let ui = self.ui();
        let st = ui.global::<SearchState>();
        let vy = self.search_scroll(-f32::from(st.get_vy()) / SC);
        st.set_vy((-vy * SC).into());
        self.search_render(false);
        self.search_sync_focus();
    }

    pub fn search_focus_report(&self) -> serde_json::Value {
        let s = &self.search;
        let id = if s.zone == 2 {
            match s.hits.get(s.focus) { Some(Hit::Game(id)) => Some(*id), _ => None }
        } else {
            None
        };
        let name = id.and_then(|id| self.gidx.get(&id).map(|&gi| self.games()[gi].name.as_str()));
        serde_json::json!({
            "kind": (["query", "tab", "grid-card"][s.zone as usize]),
            "row": s.focus / COLS, "col": s.focus % COLS, "zone": s.zone, "tab": s.tab,
            "appid": id, "name": name,
        })
    }

    pub fn search_focus_index(&mut self, index: usize) -> bool {
        if index >= self.search.hits.len() || self.search.displayed != Some(self.search.revision) {
            return false;
        }
        self.osk_set_open(false);
        self.search.zone = 2;
        self.search.focus = index;
        let ui = self.ui();
        let st = ui.global::<SearchState>();
        st.set_vy((-self.search_scroll(-f32::from(st.get_vy()) / SC) * SC).into());
        self.search_render(false);
        self.search_sync_focus();
        true
    }

    pub fn search_focus_app(&mut self, id: u32) -> bool {
        self.search.hits.iter().position(|hit| matches!(hit, Hit::Game(appid) if *appid == id))
            .is_some_and(|i| self.search_focus_index(i))
    }

    fn search_scroll(&self, vy: f32) -> f32 {
        if self.search.zone != 2 {
            return 0.0;
        }
        let top = Y0 + (self.search.focus / COLS) as f32 * PY;
        vy.clamp((top + PY - 571.0).max(0.0), (top - 60.0).max(0.0))
    }
}

/// Clipboard text from the first clipboard tool that answers. Blocking: call from a worker thread.
fn clipboard_text() -> Option<String> {
    [("wl-paste", &["-n"][..]), ("xclip", &["-o", "-selection", "clipboard"][..]), ("xsel", &["-b", "-o"][..])]
        .iter()
        .find_map(|(c, a)| std::process::Command::new(c).args(*a).output().ok().filter(|o| o.status.success()))
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
}

/// Characters a paste inserts into the query: no control characters (newlines, tabs), at most 200.
fn paste_chars(text: &str) -> Vec<char> {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

#[cfg(test)]
mod paste_tests {
    use super::paste_chars;

    #[test]
    fn paste_drops_control_characters_and_caps_length() {
        assert_eq!(paste_chars("ab\ncd\t\u{7}e"), "abcde".chars().collect::<Vec<_>>());
        let long = "x".repeat(500);
        assert_eq!(paste_chars(&long).len(), 200);
        assert!(paste_chars("").is_empty());
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn library(count: u32, art: bool) -> steam_data::Library {
        use steam_data::{Art, Game, GameKind, Library};
        Library {
            games: (1..=count)
                .map(|id| {
                    let title = if id <= 5 { "ZX" } else { "Игровая коллекция Adventure" };
                    let name = format!("{title} {:06}", (id * 919) % 65537);
                    Game {
                        app_id: id, kind: GameKind::Steam, name: name.clone(), sort_name: name.to_lowercase(),
                        installed: true, install_dir: None, size_on_disk: 0, playtime_minutes: 0,
                        last_played: None, update_pending: false, collections: vec![],
                        art: Art { header: art.then(|| "fixture.png".into()), ..Default::default() },
                    }
                })
                .collect(),
            ..Default::default()
        }
    }

    fn apply_results(app: &mut App) {
        while let Some((rev, rx)) = app.search.worker.as_ref() {
            let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            app.search_complete(*rev, result);
        }
    }

    fn snapshot(ui: &AppWindow) -> Duration {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(16));
        let start = Instant::now();
        assert_eq!(ui.window().take_snapshot().unwrap().width(), 1280);
        start.elapsed()
    }

    fn artwork(ui: &AppWindow, app: &mut App) -> (usize, Duration, Duration) {
        let img = slint::Image::from_rgba8(slint::SharedPixelBuffer::new(460, 215));
        let mut count = 0;
        let mut ui_cost = Duration::ZERO;
        let mut frame_cost = Duration::ZERO;
        // Один ответ загрузчика на программный кадр. Ограничение позволяет измерить и цикл без конца.
        while count < 80 {
            let Some(id) = app.loader.take_header_job() else { break };
            let start = Instant::now();
            app.on_still(id, Kind::Header, Some((img.clone(), [0, 0, 0])));
            ui_cost += start.elapsed();
            frame_cost += snapshot(ui);
            count += 1;
        }
        (count, ui_cost, frame_cost)
    }

    #[test]
    fn search_debounce_replaces_pending_input_and_enter_ignores_stale_results() {
        let (ui, mut app) = super::super::osk::field_tests::app_with_library(library(108, false));
        app.open_search();
        apply_results(&mut app);
        crate::APP.with(|cell| cell.replace(Some(app)));
        let advance = i_slint_backend_testing::mock_elapsed_time;
        crate::with_app(|a| a.search_char('z'));
        advance(Duration::from_millis(45));
        crate::with_app(|a| a.search_char('x'));
        advance(Duration::from_millis(45));
        crate::with_app(|a| assert!(a.search.worker.is_none(), "таймер перезапускается от последней клавиши"));
        advance(Duration::from_millis(35));
        crate::with_app(|a| {
            assert!(a.search.worker.is_some());
            apply_results(a);
            assert_eq!(ui.global::<SearchState>().get_result_count(), 5);
            assert_eq!(ui.global::<crate::SearchCaret>().get_before(), "zx");
            a.search_char('!');
            a.search_submit();
            a.search_backspace();
            a.handle(Action::R2);
            assert!(!a.osk_is_open());
            apply_results(a);
            assert_eq!(a.search.zone, 2);
            assert_eq!(ui.global::<SearchState>().get_result_count(), 5);
            a.search_char('!');
            a.search_submit();
            a.handle(Action::B);
            assert_eq!(a.router.current(), &Route::Home);
            apply_results(a);
            assert_eq!(ui.global::<SearchState>().get_rows().row_count(), 0);
            assert!(!ui.global::<SearchState>().get_pending());
        });
        crate::APP.with(|cell| cell.take());
    }

    #[test]
    fn search_scroll_and_click_use_the_same_hit_and_return_to_library() {
        let (ui, mut app) = super::super::osk::field_tests::app_with_library(library(108, false));
        let library_route = Route::Library("AllGames".into());
        app.navigate(library_route.clone());
        app.rebuild_view(None);
        let library_view = app.lb.view.clone();
        app.open_search();
        apply_results(&mut app);
        app.handle(Action::B);
        assert_eq!(app.search.zone, 0);
        app.handle(Action::Down);
        app.handle(Action::Down);
        for _ in 0..11 {
            app.handle(Action::Down);
        }
        let st = ui.global::<SearchState>();
        let first = app.search.first.unwrap();
        assert!(first > 0);
        let index = app.search.focus;
        let Hit::Game(id) = app.search.hits[index] else { panic!("ожидается игра") };
        let row = st.get_rfocus() as usize;
        assert_eq!(row, header_rows().count() + (index - first) * 2);
        assert_eq!(st.get_rows().row_data(row).unwrap().kind, 15);
        let generation = st.get_model_generation();
        app.search_set_tab(1);
        app.search_row_clicked(row, first, generation);
        assert_eq!(app.router.current(), &Route::Search, "старый клик не выбирает другой результат");
        assert!(app.search_focus_app(id));
        app.search_row_clicked(st.get_rfocus() as usize, app.search.first.unwrap(), st.get_model_generation());
        assert_eq!(app.router.current(), &Route::App(id));
        app.go_back();
        assert_eq!(app.router.current(), &library_route);
        assert_eq!(app.lb.view, library_view);
        assert!(!app.lb.search && !app.osk_is_open());
    }

    #[test]
    fn search_profile_fixture() {
        for count in [108, 20_000] {
            for art in [true, false] {
                let (ui, mut app) = super::super::osk::field_tests::app_with_library(library(count, art));
                let _ = snapshot(&ui);
                assert!(app.header_try_enter());
                let start = Instant::now();
                app.handle(Action::A);
                let open = start.elapsed();
                assert!(app.osk_is_open());
                let first = snapshot(&ui);
                apply_results(&mut app);
                let empty_results_frame = snapshot(&ui);
                let empty = artwork(&ui, &mut app);
                let start = Instant::now();
                app.search_char('z');
                app.search_char('x');
                let input = start.elapsed();
                let typed = snapshot(&ui);
                assert!(ui.global::<SearchState>().get_pending());
                assert!(app.search.worker.is_none(), "два символа только перезапускают debounce");
                let started = Instant::now();
                app.search_submit();
                apply_results(&mut app);
                let filtered_results = started.elapsed();
                let filtered_frame = snapshot(&ui);
                let filtered = artwork(&ui, &mut app);
                assert_eq!(ui.global::<SearchState>().get_result_count(), 5);
                eprintln!("search fixture {count} art={art}: open={open:?} first={first:?} empty_results_frame={empty_results_frame:?} empty={empty:?} input={input:?} typed={typed:?} filtered_results={filtered_results:?} filtered_frame={filtered_frame:?} filtered={filtered:?}");
                assert_eq!(empty.0, if art { 30 } else { 0 });
                assert_eq!(filtered.0, if art { 5 } else { 0 });
                assert!(app.loader.take_header_job().is_none());
                if art {
                    let rows = ui.global::<SearchState>().get_rows();
                    assert_eq!((0..rows.row_count()).filter(|&i| rows.row_data(i).unwrap().has_img).count(), 5);
                    assert!(empty.2 / (empty.0 as u32) < Duration::from_millis(65),
                        "средний программный кадр: {:?}", empty.2 / empty.0 as u32);
                }
                assert!(input < Duration::from_millis(50), "синхронный ввод: {input:?}");
                // После первого запуска библиотек шрифтов открытие не зависит от объёма каталога.
                if count > 108 || !art {
                    assert!(open < Duration::from_millis(100), "синхронное открытие: {open:?}");
                }
                assert!(empty.1 < Duration::from_millis(50), "сумма обработчиков обложек: {:?}", empty.1);
            }
        }
    }
}
