//! /routes/library/tab/<id> - tabs, sort (Y), grid. Search mode state lives here too (see screens/search.rs).
use steam_assets::i18n;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use steam_data::{Game, GameKind, Library};

use crate::actions::Act;
use crate::app::{fmt_ago, App};
use crate::nav::{self, Button, Edges, FocusSource, Layout, NavEvent, NodeId, Props, Rect as NRect, ScrollAnim, ScrollBox, ScrollTarget, ScrollWhenChild};
use crate::assets::{t, t_or, tu};
use crate::input::Action;
use crate::loader::Kind;
use crate::model::{self, Extra, Sort, SORTS};
use crate::router::Route;
use crate::sound::{play, Snd};
use crate::{AppWindow, CardData, LibRow, LibraryState};

pub const COLS_DEFAULT: usize = 5;
const HEADER_H: f32 = 64.0;
const ROW_H: f32 = 339.0;
// Geometry shared with ui/generated/library/library.slint (GLib: compile-library.mjs from docs/cdp/library-all.json), device px.
const LIST_Y: f32 = 137.0; // content origin (tile row 0 at +28)
const VIEW_TOP: f32 = 48.0; // scroller top: under the header, rows slide below the pill
const TILE_W: f32 = 201.0;
const TILE_H: f32 = 301.5;
const PITCH_X: f32 = 246.06;
const TILE_DY: f32 = 28.0;
const FOOTER_H: f32 = 62.7;
const PILL_Y: f32 = 78.0;
const PILL_H: f32 = 51.0;

#[derive(Clone, PartialEq, Debug)]
pub enum TabKind {
    All,
    Installed,
    NonSteam,
    Collections,
    Favorites,
    Custom(String),
}

impl TabKind {
    /// Route id (`/routes/library/tab/<id>`), the ids Steam uses.
    pub fn id(&self) -> String {
        match self {
            TabKind::All => "AllGames".into(),
            TabKind::Installed => "Installed".into(),
            TabKind::NonSteam => "DesktopApps".into(),
            TabKind::Collections => "Collections".into(),
            TabKind::Favorites => "Favorites".into(),
            TabKind::Custom(s) => format!("custom:{s}"),
        }
    }
    pub fn title(&self) -> String {
        match self {
            TabKind::All => tu("#LibraryTab_AllGames"),
            TabKind::Installed => tu("#LibraryTab_Installed"),
            TabKind::NonSteam => tu("#LibraryTab_NonSteam"),
            TabKind::Collections => tu("#LibraryTab_Collections"),
            TabKind::Favorites => tu("#LibraryTab_Favorites"),
            TabKind::Custom(s) => s.to_uppercase(),
        }
    }
}

/// Engine node ids of the library (Steam focus engine port, src/nav): one persistent column [pill row, grid].
pub struct NavIds {
    pub col: NodeId,
    pub pill: NodeId,
    pub tabs: Vec<NodeId>,
    pub grid: Option<NodeId>,
    /// per card row (parallel to `card_rows`)
    pub tiles: Vec<Vec<NodeId>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuKind {
    Sort,
    Filter,
    Tile(usize),
    Manage(u32),
    Tab,
    Media,
    Download,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuAct {
    Sort(usize),
    Filter(usize),
    Advanced,
    Launch(usize),
    Install(usize),
    Favorite(usize),
    AddTo(usize),
    Manage(usize),
    Uninstall(u32),
    TileBack(u32),
    ChangeArt,
    Props(usize),
    TabMaster,
    Media(super::media::MediaOp),
    Dl(super::downloads_menu::Op),
    Cancel,
}

pub struct MItem {
    pub label: String,
    /// 0 normal, 1 primary (green), 2 primary blue
    pub kind: i32,
    pub sep: bool,
    pub arrow: bool,
    pub act: MenuAct,
}

pub struct Menu {
    pub kind: MenuKind,
    pub title: String,
    pub items: Vec<MItem>,
    pub focus: usize,
    pub current: Option<usize>,
}

pub struct State {
    pub tabs: Vec<TabKind>,
    pub tab: usize,
    /// index into `model::SORTS`
    pub sort: usize,
    pub sort_focus: usize,
    pub sort_open: bool,
    pub search: bool,
    pub query: String,
    pub view: Vec<usize>,
    pub cols: usize,
    /// 0 tabs, 1 grid
    pub zone: i32,
    pub frow: usize,
    pub fcol: usize,
    pub rows: Rc<VecModel<LibRow>>,
    pub flat: Vec<Vec<usize>>,
    pub flat_models: Vec<Option<Rc<VecModel<CardData>>>>,
    pub flat_y: Vec<f32>,
    pub flat_first: Vec<bool>,
    pub card_rows: Vec<usize>,
    pub view_pos: HashMap<u32, (usize, usize)>,
    pub nav: Option<NavIds>,
    /// engine scroll target / currently displayed scroll (device px), content height, running tween
    pub scroll: f32,
    pub scroll_cur: f32,
    pub content_h: f32,
    pub anim: Option<ScrollAnim>,
    pub timer: slint::Timer,
    pub menu: Option<Menu>,
    /// X filter: 0 Verified, 1 +Playable, 2 all (default, collectionFilterGamepad)
    pub filter: usize,
    pub filter_read: bool,
}

fn clock_ms() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// ru-locale Intl.Collator order as Steam sorts (library.json): punctuation < digits < Cyrillic < Latin, case-insensitive.
pub(super) fn collate_key(s: &str) -> Vec<(u8, char)> {
    s.chars()
        .filter(|c| !matches!(c, '\u{2122}' | '\u{ae}' | '\u{a9}'))
        .flat_map(|c| c.to_lowercase())
        .map(|c| {
            let class = if c.is_ascii_digit() {
                1
            } else if ('\u{0400}'..='\u{052F}').contains(&c) {
                2
            } else if c.is_alphabetic() {
                3
            } else {
                0
            };
            (class, c)
        })
        .collect()
}

impl State {
    pub fn new(tabs: Vec<TabKind>) -> State {
        State {
            tabs,
            tab: 0,
            sort: 0,
            sort_focus: 0,
            sort_open: false,
            search: false,
            query: String::new(),
            view: vec![],
            cols: COLS_DEFAULT,
            zone: 1,
            frow: 0,
            fcol: 0,
            rows: Rc::new(VecModel::default()),
            flat: vec![],
            flat_models: vec![],
            flat_y: vec![],
            flat_first: vec![],
            card_rows: vec![],
            view_pos: HashMap::new(),
            nav: None,
            scroll: 0.0,
            scroll_cur: 0.0,
            content_h: 0.0,
            anim: None,
            timer: slint::Timer::default(),
            menu: None,
            filter: 2,
            filter_read: false,
        }
    }
    pub fn tab_id(&self) -> String {
        self.tabs.get(self.tab).map(|t| t.id()).unwrap_or_else(|| "AllGames".into())
    }
}

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<LibraryState>();
    st.on_tab_clicked(|i| super::defer(move |a| {
        a.lb.zone = 0;
        a.set_tab(i as usize);
        a.nav_sync_focus();
        a.lib_focus_changed();
    }));
    st.on_card_clicked(|ri, ci| super::defer(move |a| {
        let f = ri as usize;
        if let Some(r) = a.lb.card_rows.iter().position(|&x| x == f) {
            a.lb.zone = 1;
            a.lb.frow = r;
            a.lb.fcol = ci as usize;
            a.nav_sync_focus();
            a.lib_focus_changed();
            a.handle(Action::A);
        }
    }));
    // touch press-and-hold on a tile = the START menu (tile options), not the game page
    st.on_card_menu(|ri, ci| super::defer(move |a| {
        let f = ri as usize;
        if let Some(r) = a.lb.card_rows.iter().position(|&x| x == f) {
            a.lb.zone = 1;
            a.lb.frow = r;
            a.lb.fcol = ci as usize;
            a.nav_sync_focus();
            a.lib_focus_changed();
            a.open_menu_for_focus();
        }
    }));
    st.on_row_visible(|r| super::defer(move |a| a.row_visible(r as usize)));
    st.on_sort_picked(|i| super::defer(move |a| {
        a.lb.sort_focus = i as usize;
        a.handle(Action::A);
    }));
    st.on_menu_picked(|i| super::defer(move |a| {
        if let Some(m) = a.lb.menu.as_mut() {
            m.focus = (i.max(0) as usize).min(m.items.len().saturating_sub(1));
        }
        a.handle(Action::A);
    }));
    st.on_cols_changed(|c| super::defer(move |a| a.set_cols(c.max(1) as usize)));
    st.on_user_scrolled(|y| super::defer(move |a| a.lib_user_scrolled(y)));
    st.on_close_sort(|| super::defer(|a| a.close_menu()));
    // Steam focus engine events (real gamepad presses reach the engine through input.rs -> nav::intercept)
    nav::set_sink(|e| {
        let _ = crate::with_app(|a| a.lib_nav_event(vec![e], true));
    });
}

pub fn wire_models(app: &App, ui: &AppWindow) {
    let st = ui.global::<LibraryState>();
    st.set_rows(ModelRc::from(app.lb.rows.clone()));
    st.set_tabs(ModelRc::new(VecModel::from(app.lb.tabs.iter().map(|t| SharedString::from(t.title())).collect::<Vec<_>>())));
    st.set_sort_options(ModelRc::new(VecModel::from(SORTS.iter().map(|s| SharedString::from(t(s.token()))).collect::<Vec<_>>())));
}

pub fn init_strings(ui: &AppWindow) {
    let st = ui.global::<LibraryState>();
    st.set_sort_title(tu("#ActionButtonLabelSort").into());
    st.set_empty_text(t_or("#GameList_View_NothingSelected_HelpText", "").into());
}

/// Preserve a TabMaster configuration when present; otherwise use the supported
/// native tabs in their source order. Device-specific tabs are not implemented.
fn default_tabs(has_collections: bool) -> Vec<TabKind> {
    let mut tabs = vec![TabKind::All, TabKind::Installed];
    if has_collections { tabs.push(TabKind::Collections); }
    tabs.push(TabKind::NonSteam);
    tabs
}

/// Tabs: TabMaster user configuration when present, otherwise native defaults.
pub fn load_tabs(lib: &Library) -> Vec<TabKind> {
    let try_load = || -> Option<Vec<TabKind>> {
        let home = std::env::var("HOME").ok()?;
        let txt = std::fs::read_to_string(format!("{home}/homebrew/settings/TabMaster/settings.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
        let users = v.get("usersDict")?.as_object()?;
        let uid = lib.active_user.map(|u| u.to_string())?;
        let tabs = users.get(&uid)?.get("tabs")?.as_object()?;
        let mut list: Vec<(i64, TabKind)> = Vec::new();
        for t in tabs.values() {
            let pos = t.get("position")?.as_i64()?;
            if pos < 0 {
                continue;
            }
            let id = t.get("id")?.as_str()?;
            let kind = match id {
                "AllGames" => TabKind::All,
                "Installed" => TabKind::Installed,
                "DesktopApps" => TabKind::NonSteam,
                "Collections" => TabKind::Collections,
                "Favorites" => TabKind::Favorites,
                "GreatOnDeck" | "Soundtracks" => continue,
                _ => TabKind::Custom(t.get("title")?.as_str()?.to_string()),
            };
            list.push((pos, kind));
        }
        list.sort_by_key(|x| x.0);
        Some(list.into_iter().map(|x| x.1).collect())
    };
    let t = try_load().unwrap_or_default();
    if t.is_empty() { default_tabs(!lib.collections.is_empty()) } else { t }
}

impl App {
    pub fn set_cols(&mut self, c: usize) {
        if c != self.lb.cols && c > 0 {
            self.lb.cols = c;
            let keep = self.focused_lib_game().map(|i| self.games()[i].app_id);
            self.rebuild_view(keep);
            self.sync_lib();
        }
    }

    pub fn focused_lib_game(&self) -> Option<usize> {
        let f = *self.lb.card_rows.get(self.lb.frow)?;
        self.lb.flat.get(f)?.get(self.lb.fcol).copied()
    }

    fn tab_matches(&self, g: &Game) -> bool {
        match self.lb.tabs.get(self.lb.tab).unwrap_or(&TabKind::All) {
            TabKind::All => true,
            TabKind::Installed => g.installed,
            TabKind::NonSteam => g.kind == GameKind::Shortcut,
            TabKind::Collections => !g.collections.is_empty(),
            TabKind::Favorites => Self::is_favorite(g),
            TabKind::Custom(t) => g.collections.iter().any(|c| c.eq_ignore_ascii_case(t)),
        }
    }

    /// X-dialog compat filter (collectionFilterGamepad): 0 Verified only, 1 Verified + Playable, 2 everything. The Non-Steam tab
    /// ignores it (ignoreFiltering, library.json). Unknown/unsupported apps are hidden by levels 0 and 1 like Steam.
    fn filter_ok(&self, g: &Game) -> bool {
        if self.lb.filter >= 2 || matches!(self.lb.tabs.get(self.lb.tab), Some(TabKind::NonSteam)) {
            return true;
        }
        let c = self.extras.get(&g.app_id).map(|e| e.compat).unwrap_or(0);
        match self.lb.filter {
            0 => c >= 3,
            _ => c >= 2,
        }
    }

    /// Once the bridge is up: adopt Steam's own current gamepad filter (25 verified / 15 +playable / 24 all).
    pub fn lib_read_filter(&mut self) {
        if self.lb.filter_read {
            return;
        }
        self.lb.filter_read = true;
        crate::setters::read(crate::setters::READ_GAMEPAD_FILTER.into(), |a, v| {
            let lvl = match v.as_i64() {
                Some(25) => 0,
                Some(15) => 1,
                Some(24) => 2,
                _ => return,
            };
            if lvl != a.lb.filter {
                a.lb.filter = lvl;
                let keep = a.focused_lib_game().map(|g| a.games()[g].app_id);
                a.rebuild_view(keep);
                a.update_hint();
            }
        });
    }

    /// Footer X chip text: the active compat filter, else "no filter" (Library_FilteredBy / _None).
    pub fn filter_token(&self) -> &'static str {
        match self.lb.filter {
            0 => "#HardwareVariant_SteamDeck_Verified",
            1 => "#DeckVerified_CategoryLabel_Playable",
            _ => "#Library_FilteredBy_None",
        }
    }

    pub fn is_favorite(g: &Game) -> bool {
        g.collections.iter().any(|name| is_favorite_collection(name))
    }

    /// Tile menu "Add to favorites" / "Remove": real collectionStore write (DRYRUN-gated), local view updated at once.
    fn toggle_favorite(&mut self, gi: usize) {
        let (id, fav) = (self.games()[gi].app_id, Self::is_favorite(&self.games()[gi]));
        crate::setters::set_favorite(id, !fav);
        let lib = Arc::make_mut(&mut self.lib);
        let g = &mut lib.games[gi];
        if fav {
            g.collections.retain(|name| !is_favorite_collection(name));
        } else {
            g.collections.push(t_or("#GameList_Category_Favorites", i18n::tr_ref("library.favorites")));
        }
        let keep = Some(id);
        self.rebuild_view(keep);
        self.sync_lib();
    }

    /// L2/R2 fast scroll (engine library_grid_fast_scroll): alphabetical -> previous/next first letter; other sorts with
    /// sections -> previous/next section header. Focus lands on the first tile of the target row.
    fn fast_scroll(&mut self, dir: i32) {
        if self.lb.zone != 1 || self.lb.card_rows.is_empty() {
            return;
        }
        let games = &self.lib.games;
        let alpha = SORTS[self.lb.sort] == Sort::Alphabetical || SORTS[self.lb.sort] == Sort::FriendsPlaying || SORTS[self.lb.sort] == Sort::PctAchievementsComplete;
        let key = |gi: usize| -> String {
            if alpha {
                match games[gi].sort_name.chars().find(|c| !c.is_whitespace()) {
                    Some(c) if c.is_alphabetic() => c.to_uppercase().to_string(),
                    _ => "#".into(),
                }
            } else {
                let e = self.extras.get(&games[gi].app_id).cloned().unwrap_or_default();
                model::section(SORTS[self.lb.sort], &games[gi], &e)
            }
        };
        // first card row of every distinct key group
        let mut starts: Vec<usize> = vec![];
        let mut prev: Option<String> = None;
        for (r, &f) in self.lb.card_rows.iter().enumerate() {
            let k = self.lb.flat[f].first().map(|&gi| key(gi)).unwrap_or_default();
            let new_group = prev.as_ref() != Some(&k);
            if new_group {
                starts.push(r);
            }
            // a group that begins mid-row still counts for the row it starts in
            let last_k = self.lb.flat[f].last().map(|&gi| key(gi)).unwrap_or_default();
            prev = Some(last_k);
        }
        let cur = self.lb.frow;
        let target = if dir > 0 {
            starts.iter().copied().find(|&r| r > cur)
        } else {
            // previous group start strictly before the current row's group start (or the current group's start when mid-group)
            let here = starts.iter().copied().rev().find(|&r| r <= cur);
            match here {
                Some(h) if h < cur => Some(h),
                Some(h) => starts.iter().copied().rev().find(|&r| r < h),
                None => None,
            }
        };
        let Some(r) = target else {
            play(Snd::FailedNav);
            return;
        };
        self.lb.frow = r;
        self.lb.fcol = 0;
        self.lib_focus_changed();
        self.lib_ensure_visible_legacy();
        self.ensure_visible_cards();
        play(Snd::BasicNav);
    }

    /// Caption under a focused tile: relative last-played when sorting by it.
    pub fn caption(&self, g: &Game) -> SharedString {
        if SORTS[self.lb.sort] == Sort::LastPlayed && g.last_played.is_some() { fmt_ago(g.last_played).into() } else { "".into() }
    }

    pub fn rebuild_view(&mut self, keep: Option<u32>) {
        let lib = self.lib.clone();
        let games = &lib.games;
        let dflt = Extra::default();
        let ex = |gi: usize| self.extras.get(&games[gi].app_id).unwrap_or(&dflt);
        let mut v: Vec<usize> = (0..games.len())
            .filter(|&i| self.tab_matches(&games[i]) && self.filter_ok(&games[i]))
            .collect();
        let sort = SORTS[self.lb.sort];
        v.sort_by(|&a, &b| model::primary(sort, &games[a], &games[b], ex(a), ex(b))
            .then_with(|| collate_key(&games[a].sort_name).cmp(&collate_key(&games[b].sort_name))));
        let by_collection = matches!(self.lb.tabs.get(self.lb.tab), Some(TabKind::Collections));
        if by_collection {
            v.sort_by(|&a, &b| games[a].collections.first().cmp(&games[b].collections.first()));
        }
        let section = |gi: usize| -> Option<String> {
            if by_collection {
                games[gi].collections.first().map(|c| c.to_uppercase())
            } else {
                let s = model::section(sort, &games[gi], ex(gi));
                (!s.is_empty()).then(|| s.to_uppercase())
            }
        };
        let cols = self.lb.cols.max(1);
        self.lb.view_pos.clear();
        self.lb.flat.clear();
        self.lb.flat_models.clear();
        self.lb.flat_y.clear();
        self.lb.flat_first.clear();
        self.lb.card_rows.clear();
        let mut out: Vec<LibRow> = Vec::new();
        let mut y = 0.0f32;
        let mut i = 0;
        while i < v.len() {
            let sec = section(v[i]);
            let mut j = i;
            while j < v.len() && section(v[j]) == sec {
                j += 1;
            }
            if let Some(h) = &sec {
                out.push(LibRow { is_header: true, header: h.as_str().into(), cards: ModelRc::default(), y, h: HEADER_H });
                self.lb.flat.push(vec![]);
                self.lb.flat_models.push(None);
                self.lb.flat_y.push(y);
                self.lb.flat_first.push(false);
                y += HEADER_H;
            }
            let mut first = true;
            for chunk in v[i..j].chunks(cols) {
                let m = Rc::new(VecModel::from(chunk.iter().map(|&g| self.card(g)).collect::<Vec<_>>()));
                let fi = out.len();
                for (c, &g) in chunk.iter().enumerate() {
                    self.lb.view_pos.insert(games[g].app_id, (self.lb.card_rows.len(), c));
                }
                out.push(LibRow { is_header: false, header: "".into(), cards: ModelRc::from(m.clone()), y, h: ROW_H });
                self.lb.card_rows.push(fi);
                self.lb.flat.push(chunk.to_vec());
                self.lb.flat_models.push(Some(m));
                self.lb.flat_y.push(y);
                self.lb.flat_first.push(first && sec.is_some());
                first = false;
                y += ROW_H;
            }
            i = j;
        }
        self.lb.view = v;
        self.lb.rows.set_vec(out);
        let pos = keep.and_then(|id| self.lb.view_pos.get(&id).copied()).unwrap_or((0, 0));
        self.lb.frow = pos.0;
        self.lb.fcol = pos.1;
        self.lb.content_h = y;
        // a new view starts at the top (Steam re-mounts the grid); a column change keeps the focused tile in view through the engine
        self.lb.anim = None;
        self.lb.timer.stop();
        self.lb.scroll = 0.0;
        self.lb.scroll_cur = 0.0;
        let ui = self.ui();
        let st = ui.global::<LibraryState>();
        st.set_content_h(y);
        st.set_scroll_y(0.0);
        st.set_count(self.lb.view.len() as i32);
        self.nav_build();
        self.ensure_visible_cards();
    }

    pub fn refresh_lib_card(&mut self, id: u32, gi: usize) {
        if let Some(&(r, c)) = self.lb.view_pos.get(&id) {
            if let Some(Some(m)) = self.lb.card_rows.get(r).and_then(|&f| self.lb.flat_models.get(f)) {
                if c < m.row_count() {
                    m.set_row_data(c, self.card(gi));
                }
            }
        }
    }

    pub fn ensure_visible_cards(&mut self) {
        let top = self.lb.scroll_cur.min(self.lb.scroll);
        let bot = self.lb.scroll_cur.max(self.lb.scroll) + self.view_h();
        let margin = 1500.0;
        let y_min = (top - margin).max(0.0);
        let y_max = bot + margin;

        let card_rows = self.lb.card_rows.clone();
        for &f in &card_rows {
            let y = self.lb.flat_y.get(f).copied().unwrap_or(0.0);
            if y + ROW_H >= y_min && y <= y_max {
                if let Some(games) = self.lb.flat.get(f).cloned() {
                    let model = self.lb.flat_models.get(f).and_then(|m| m.clone());
                    for (c, &gi) in games.iter().enumerate() {
                        let app_id = self.games()[gi].app_id;
                        let has_img = model.as_ref()
                            .and_then(|m| m.row_data(c))
                            .map(|cd| cd.has_img)
                            .unwrap_or(false);
                        if !has_img {
                            if self.imgs.contains_key(&(app_id, Kind::Capsule)) {
                                if let Some(ref m) = model {
                                    if c < m.row_count() {
                                        m.set_row_data(c, self.card(gi));
                                    }
                                }
                            } else {
                                self.requested.remove(&(app_id, Kind::Capsule));
                                self.request(gi, Kind::Capsule);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn row_visible(&mut self, r: usize) {
        let Some(games) = self.lb.flat.get(r).cloned() else { return };
        let model = self.lb.flat_models.get(r).and_then(|m| m.clone());
        for (c, &gi) in games.iter().enumerate() {
            let app_id = self.games()[gi].app_id;
            let has_img = model.as_ref()
                .and_then(|m| m.row_data(c))
                .map(|cd| cd.has_img)
                .unwrap_or(false);
            if !has_img {
                if self.imgs.contains_key(&(app_id, Kind::Capsule)) {
                    if let Some(ref m) = model {
                        if c < m.row_count() {
                            m.set_row_data(c, self.card(gi));
                        }
                    }
                } else {
                    self.requested.remove(&(app_id, Kind::Capsule));
                    self.request(gi, Kind::Capsule);
                }
            }
        }
    }

    pub fn sync_lib(&self) {
        let ui = self.ui();
        let st = ui.global::<LibraryState>();
        st.set_tab(self.lb.tab as i32);
        st.set_sort_idx(self.lb.sort as i32);
        st.set_sort_focus(self.lb.sort_focus as i32);
        st.set_zone(self.lb.zone);
        let flat = self.lb.card_rows.get(self.lb.frow).copied().unwrap_or(0);
        st.set_focus_row(flat as i32);
        st.set_focus_col(self.lb.fcol as i32);
        let y = self.lb.flat_y.get(flat).copied().unwrap_or(0.0);
        let first = self.lb.flat_first.get(flat).copied().unwrap_or(false);
        let top = if self.lb.frow == 0 || first { (y - if first { HEADER_H } else { 0.0 }).max(0.0) } else { y };
        st.set_focus_top(top);
        st.set_focus_bottom(y + ROW_H);
        let tb = ui.global::<crate::TopBarState>();
        tb.set_search_active(self.lb.search);
        tb.set_search_query(self.lb.query.as_str().into());
    }

    pub fn lib_focus_changed(&mut self) {
        self.nav_sync_focus();
        self.sync_lib();
        let id = if self.router.current().is_grid() && self.lb.zone == 1 { self.focused_lib_game().map(|i| self.games()[i].app_id) } else { None };
        self.focus_changed(id);
    }

    /// L1/R1 or pill click: history.replace(/library/tab/<id>)
    pub fn set_tab(&mut self, t: usize) {
        let t = t.min(self.lb.tabs.len().saturating_sub(1));
        if t != self.lb.tab {
            self.lb.tab = t;
            self.rebuild_view(None);
            if matches!(self.router.current(), Route::Library(_)) {
                let id = self.lb.tab_id();
                self.router.replace(Route::Library(id));
            }
        }
    }

    /// TabBar L1/R1 (sp.js:347290: wrapAround defaults to true, so TabMaster's tab list wraps).
    /// `handle_library` returns early for the bumpers, so the pill state must be pushed here too (`sync_lib`);
    /// otherwise the indicator stays on the old tab until a D-pad press runs `lib_focus_changed`.
    fn step_tab(&mut self, delta: i32) {
        let n = self.lb.tabs.len();
        if let Some(t) = nav::tab_step(self.lb.tab, delta, n, None) {
            if t != self.lb.tab {
                self.set_tab(t);
                if self.lb.zone == 1 {
                    self.lb.frow = 0;
                    self.lb.fcol = 0;
                }
                self.lib_focus_changed();
            }
        }
    }

    pub fn select_tab_by_id(&mut self, id: &str) {
        if let Some(p) = self.lb.tabs.iter().position(|t| t.id() == id) {
            if p != self.lb.tab {
                self.lb.tab = p;
                self.rebuild_view(None);
            }
        }
    }

    fn set_sort(&mut self, s: usize) {
        if s != self.lb.sort {
            self.lb.sort = s;
            self.rebuild_view(None);
        }
    }

    // ------------------------------------------------------------------ Steam focus engine (src/nav)

    /// The library grid/pill are driven by the engine only while nothing else owns the pad (no menu/QAM/search/other page).
    pub fn nav_active(&self) -> bool {
        // `Scope::Page` already excludes every surface above the page and header focus; without it the grid
        // behind the power menu, a confirmation, Properties, artwork or the focused header took D-pad presses.
        nav::enabled() && matches!(self.router.current(), Route::Library(_)) && self.focus_scope() == nav::scope::Scope::Page
    }

    fn win_size(&self) -> (f32, f32) {
        let ui = self.ui();
        let w = ui.window();
        let sz = w.size();
        let sf = w.scale_factor().max(0.1);
        let (x, y) = (sz.width as f32 / sf, sz.height as f32 / sf);
        if x < 100.0 || y < 100.0 { (1280.0, 800.0) } else { (x, y) }
    }

    /// height of the scroller viewport
    fn view_h(&self) -> f32 {
        self.win_size().1 - VIEW_TOP - FOOTER_H
    }

    fn max_scroll(&self) -> f32 {
        (self.lb.content_h + (LIST_Y - VIEW_TOP) + 2.0 - self.view_h()).max(0.0)
    }

    fn tile_rect(&self, flat: usize, col: usize, scroll: f32, w: f32) -> NRect {
        let cols = self.lb.cols.max(1) as f32;
        let x = (w - (cols * PITCH_X - (PITCH_X - TILE_W))) / 2.0 + col as f32 * PITCH_X;
        NRect::xywh(x, LIST_Y + TILE_DY + self.lb.flat_y.get(flat).copied().unwrap_or(0.0) - scroll, TILE_W, TILE_H)
    }

    /// (Re)create the engine nodes of the grid (and the pill row when the tab list changed) for the current view.
    pub fn nav_build(&mut self) {
        if !nav::enabled() {
            return;
        }
        let (w, _) = self.win_size();
        let view_h = self.view_h();
        let active: Rc<dyn Fn(NodeId) -> bool> = Rc::new(|_| crate::with_app(|a| a.nav_active()).unwrap_or(true));
        let start: nav::tree::Handler = Rc::new(|id| {
            if !crate::with_app(|a| a.nav_active()).unwrap_or(true) {
                return false;
            }
            super::defer(move |a| a.lib_start(id));
            true
        });
        let mut ids = self.lb.nav.take();
        let tab_titles: Vec<String> = self.lb.tabs.iter().map(|t| t.title()).collect();
        let rects: Vec<Vec<(usize, NRect, bool)>> = self
            .lb
            .card_rows
            .iter()
            .map(|&f| {
                let n = self.lb.flat[f].len();
                (0..n).map(|c| (self.lb.flat[f][c], self.tile_rect(f, c, self.lb.scroll, w), c == 0 && self.lb.flat_first[f])).collect()
            })
            .collect();
        let app_ids: Vec<Vec<u32>> = rects.iter().map(|r| r.iter().map(|x| self.games()[x.0].app_id).collect()).collect();
        let cur_scroll = self.lb.scroll;
        let max_top = self.max_scroll();
        let built = nav::with(|t| {
            let root = t.root();
            let mut n = match ids.take() {
                Some(n) => n,
                None => {
                    let col = t.add_node(root, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(0.0, 0.0, w, 800.0)));
                    let pill = t.add_node(col, Props { layout: Layout::Row, ..Default::default() }, Some(NRect::xywh(0.0, PILL_Y, w, PILL_H)));
                    NavIds { col, pill, tabs: vec![], grid: None, tiles: vec![] }
                }
            };
            if n.tabs.len() != tab_titles.len() {
                for id in n.tabs.drain(..) {
                    t.remove_node(id);
                }
                // tab geometry only matters for geometric moves; the row layout moves by order
                let widths: Vec<f32> = tab_titles.iter().map(|s| 40.0 + 10.5 * s.chars().count() as f32).collect();
                let total: f32 = widths.iter().sum::<f32>() + 2.3 * widths.len() as f32;
                let mut x = (w - total) / 2.0;
                for (i, title) in tab_titles.iter().enumerate() {
                    let id = t.add_node(
                        n.pill,
                        Props {
                            focusable: true,
                            nav_key: Some(format!("tab:{title}")),
                            can_take_focus: Some(active.clone()),
                            on_button: vec![(Button::Start, start.clone())],
                            ..Default::default()
                        },
                        Some(NRect::xywh(x, PILL_Y, widths[i], PILL_H)),
                    );
                    x += widths[i] + 2.3;
                    n.tabs.push(id);
                }
            }
            if let Some(g) = n.grid.take() {
                t.remove_node(g);
            }
            let grid = t.add_node(
                n.col,
                Props { layout: Layout::Grid, scroll_into_view_when_child_focused: ScrollWhenChild::Yes, can_take_focus: Some(active.clone()), ..Default::default() },
                Some(NRect::xywh(0.0, VIEW_TOP, w, view_h)),
            );
            let mut sb = ScrollBox::vertical(max_top);
            sb.padding = Edges { left: 0.0, right: 0.0, top: LIST_Y - VIEW_TOP + TILE_DY, bottom: 2.0 };
            sb.top = cur_scroll;
            t.set_scroll_box(grid, sb);
            n.tiles.clear();
            for (r, row) in rects.iter().enumerate() {
                let mut v = vec![];
                for (c, (_, rect, first)) in row.iter().enumerate() {
                    let id = t.add_node(
                        grid,
                        Props {
                            focusable: true,
                            nav_key: Some(app_ids[r][c].to_string()),
                            can_take_focus: Some(active.clone()),
                            scroll_margin: Edges { left: 0.0, right: 0.0, top: if *first { HEADER_H } else { 0.0 }, bottom: 0.0 },
                            on_button: vec![(Button::Start, start.clone())],
                            ..Default::default()
                        },
                        Some(*rect),
                    );
                    v.push(id);
                }
                n.tiles.push(v);
            }
            n.grid = Some(grid);
            t.flush_deferred_focus();
            let _ = t.drain_events(); // focus moves caused by removing the old nodes are bookkeeping, not input
            n
        });
        self.lb.nav = Some(built);
        self.nav_sync_focus();
    }

    /// Re-lay the tile rects for the engine's scroll position (the UI layer's job after a ScrollReq).
    fn nav_update_rects(&mut self) {
        let Some(n) = self.lb.nav.as_ref() else { return };
        let (w, _) = self.win_size();
        let scroll = self.lb.scroll;
        let max_top = self.max_scroll();
        let grid = n.grid;
        let mut upd: Vec<(NodeId, NRect)> = vec![];
        for (r, row) in n.tiles.iter().enumerate() {
            let Some(&f) = self.lb.card_rows.get(r) else { continue };
            for (c, &id) in row.iter().enumerate() {
                upd.push((id, self.tile_rect(f, c, scroll, w)));
            }
        }
        nav::with(|t| {
            if let Some(g) = grid {
                t.set_scroll(Some(g), scroll, 0.0);
                let _ = max_top;
            }
            for (id, r) in upd {
                t.set_rect(id, r);
            }
        });
    }

    fn pos_of(&self, id: NodeId) -> Option<(i32, usize, usize)> {
        let n = self.lb.nav.as_ref()?;
        if let Some(i) = n.tabs.iter().position(|&x| x == id) {
            return Some((0, i, 0));
        }
        for (r, row) in n.tiles.iter().enumerate() {
            if let Some(c) = row.iter().position(|&x| x == id) {
                return Some((1, r, c));
            }
        }
        None
    }

    fn node_of(&self, zone: i32, row: usize, col: usize) -> Option<NodeId> {
        let n = self.lb.nav.as_ref()?;
        if zone == 0 { n.tabs.get(self.lb.tab).copied() } else { n.tiles.get(row)?.get(col).copied() }
    }

    /// Engine focus <- lb.zone/frow/fcol (clicks, tab changes, rebuilds, legacy search-mode moves). Silent: only scroll is applied.
    pub fn nav_sync_focus(&mut self) {
        if !nav::enabled() || self.lb.nav.is_none() {
            return;
        }
        let want = if self.lb.zone == 1 && !self.lb.card_rows.is_empty() { self.node_of(1, self.lb.frow, self.lb.fcol) } else { self.node_of(0, 0, 0) };
        let Some(want) = want else { return };
        let cur = nav::with(|t| t.focused());
        if cur == Some(want) {
            return;
        }
        let evs = nav::with(|t| {
            t.now_ms = clock_ms();
            t.transfer_focus(FocusSource::Application, Some(want), None);
            t.drain_events()
        });
        self.lib_nav_event(evs, false);
    }

    /// Consume engine events. `from_engine` = the press came through the engine (apply its focus + sounds); false = silent resync.
    pub fn lib_nav_event(&mut self, evs: Vec<NavEvent>, from_engine: bool) {
        if from_engine && !self.nav_active() {
            return;
        }
        let mut moved = false;
        for e in evs {
            match e {
                NavEvent::Focus { to: Some(to), .. } if from_engine => {
                    if let Some((zone, r, c)) = self.pos_of(to) {
                        self.lb.zone = zone;
                        if zone == 0 {
                            if r != self.lb.tab {
                                self.set_tab(r);
                            }
                        } else {
                            self.lb.frow = r;
                            self.lb.fcol = c;
                        }
                        moved = true;
                    }
                }
                NavEvent::Scroll(req) => {
                    if let (ScrollTarget::Node(_), Some(top)) = (req.target, req.top) {
                        let smooth = req.behavior == nav::Behavior::Smooth;
                        self.lib_scroll_to(top, smooth);
                    }
                }
                NavEvent::BasicNav if from_engine => play(Snd::BasicNav),
                NavEvent::FailedNav if from_engine => play(Snd::FailedNav),
                _ => {}
            }
        }
        if moved {
            self.lib_focus_changed();
        }
    }

    /// Apply a scroll request of the engine: instant (`Auto`) or the engine's 200 ms sine tween (linear when retriggered).
    fn lib_scroll_to(&mut self, top: f32, smooth: bool) {
        let top = top.clamp(0.0, self.max_scroll());
        self.lb.scroll = top;
        self.nav_update_rects();
        let ui = self.ui();
        let st = ui.global::<LibraryState>();
        if !smooth {
            self.lb.anim = None;
            self.lb.timer.stop();
            self.lb.scroll_cur = top;
            st.set_scroll_y(top);
            self.ensure_visible_cards();
            return;
        }
        let running = self.lb.anim.is_some();
        match ScrollAnim::start(clock_ms(), (0.0, self.lb.scroll_cur), (0.0, top), running) {
            Some(a) => {
                self.lb.anim = Some(a);
                self.lb.timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(8), || {
                    let _ = crate::with_app(|a| a.lib_scroll_tick());
                });
            }
            None => {
                self.lb.scroll_cur = top;
                st.set_scroll_y(top);
                self.ensure_visible_cards();
            }
        }
    }

    pub fn lib_scroll_tick(&mut self) {
        let Some(a) = self.lb.anim else {
            self.lb.timer.stop();
            return;
        };
        let ((_, y), done) = a.at(clock_ms());
        self.lb.scroll_cur = y;
        self.ui().global::<LibraryState>().set_scroll_y(y);
        self.ensure_visible_cards();
        if done {
            self.lb.anim = None;
            self.lb.timer.stop();
        }
    }

    /// Finger drag / wheel on the grid (Flickable): the engine's scroll position follows.
    pub fn lib_user_scrolled(&mut self, y: f32) {
        if (y - self.lb.scroll_cur).abs() < 0.5 {
            return;
        }
        let y = y.clamp(0.0, self.max_scroll());
        self.lb.anim = None;
        self.lb.timer.stop();
        self.lb.scroll_cur = y;
        self.lb.scroll = y;
        self.ui().global::<LibraryState>().set_scroll_y(y);
        self.nav_update_rects();
        self.ensure_visible_cards();
    }

    /// Engine `Start` handler of a tile / tab (the START button is ours before the legacy router sees it).
    pub fn lib_start(&mut self, id: NodeId) {
        if !self.nav_active() {
            return;
        }
        match self.pos_of(id) {
            Some((0, ..)) => self.open_menu(MenuKind::Tab),
            Some((1, r, c)) => {
                self.lb.zone = 1;
                self.lb.frow = r;
                self.lb.fcol = c;
                if let Some(gi) = self.focused_lib_game() {
                    self.open_menu(MenuKind::Tile(gi));
                }
            }
            _ => {}
        }
    }

    pub fn open_menu_for_focus(&mut self) {
        if self.lb.zone == 1 {
            if let Some(gi) = self.focused_lib_game() {
                self.open_menu(MenuKind::Tile(gi));
                return;
            }
        }
        self.open_menu(MenuKind::Tab);
    }

    // ------------------------------------------------------------------ input

    pub fn handle_library(&mut self, a: Action) {
        // gamepad moves go through the Steam engine (grid layout, focus memory, scroll-into-view) when it owns the screen
        if self.nav_active() && self.lb.nav.is_some() {
            if let Some(b) = nav::button_of(a) {
                if matches!(b, Button::DirUp | Button::DirDown | Button::DirLeft | Button::DirRight) {
                    let evs = nav::with(|t| {
                        t.now_ms = clock_ms();
                        t.handle_button(b, false);
                        t.drain_events()
                    });
                    self.lib_nav_event(evs, false);
                    // focus part of the events (silent resync above ignored it): derive from the engine
                    if let Some(f) = nav::with(|t| t.focused()) {
                        if let Some((zone, r, c)) = self.pos_of(f) {
                            self.lb.zone = zone;
                            if zone == 0 {
                                if r != self.lb.tab {
                                    self.set_tab(r);
                                }
                            } else {
                                self.lb.frow = r;
                                self.lb.fcol = c;
                            }
                        }
                    }
                    self.lib_focus_changed();
                    return;
                }
            }
        }
        let nrows = self.lb.card_rows.len();
        match a {
            Action::LB => return self.step_tab(-1),
            Action::RB => return self.step_tab(1),
            Action::L2 => return self.fast_scroll(-1),
            Action::R2 => return self.fast_scroll(1),
            Action::X if !self.lb.search => {
                self.open_menu(MenuKind::Filter);
                return;
            }
            Action::Y => {
                self.open_sort();
                return;
            }
            Action::Menu => return self.open_menu_for_focus(),
            _ if self.lb.zone == 0 => match a {
                Action::Left => self.set_tab(self.lb.tab.saturating_sub(1)),
                Action::Right => self.set_tab(self.lb.tab + 1),
                Action::Down if !self.lb.view.is_empty() => self.lb.zone = 1,
                _ => return,
            },
            Action::Left => self.lb.fcol = self.lb.fcol.saturating_sub(1),
            Action::Right => {
                let len = self.lb.card_rows.get(self.lb.frow).and_then(|&f| self.lb.flat.get(f)).map(|r| r.len()).unwrap_or(0);
                if self.lb.fcol + 1 < len {
                    self.lb.fcol += 1
                }
            }
            Action::Up => {
                if self.lb.frow > 0 {
                    self.lb.frow -= 1
                } else if !self.lb.search {
                    self.lb.zone = 0;
                }
            }
            Action::Down => {
                if self.lb.frow + 1 < nrows {
                    self.lb.frow += 1;
                    let len = self.lb.flat[self.lb.card_rows[self.lb.frow]].len();
                    self.lb.fcol = self.lb.fcol.min(len.saturating_sub(1));
                }
            }
            Action::A => {
                if let Some(gi) = self.focused_lib_game() {
                    self.open_game(gi)
                }
            }
            _ => return,
        }
        self.lib_focus_changed();
        // legacy (search mode) moves: keep the grid in view with the engine's scroll rules too
        self.lib_ensure_visible_legacy();
    }

    /// Search mode has no engine ownership: scroll the focused row into view (engine "nearest" rule, 200 ms sine).
    fn lib_ensure_visible_legacy(&mut self) {
        let Some(&f) = self.lb.card_rows.get(self.lb.frow) else { return };
        let y = self.lb.flat_y.get(f).copied().unwrap_or(0.0);
        let first = self.lb.flat_first.get(f).copied().unwrap_or(false);
        let pad = LIST_Y - VIEW_TOP;
        let top = if first { y - HEADER_H } else { y }.max(0.0);
        let bottom = y + ROW_H + pad;
        let vh = self.view_h();
        let cur = self.lb.scroll;
        if top < cur {
            self.lib_scroll_to(top, true);
        } else if bottom > cur + vh {
            self.lib_scroll_to(bottom - vh, true);
        }
    }

    // ------------------------------------------------------------------ popup menus (Y sort, X filter, START tile options, TabMaster)

    fn open_sort(&mut self) {
        self.open_menu(MenuKind::Sort);
    }

    pub fn open_menu(&mut self, kind: MenuKind) {
        let mut items: Vec<MItem> = vec![];
        let mut title = String::new();
        let mut focus = 0usize;
        let mut current = None;
        let it = |label: String, kind: i32, sep: bool, arrow: bool, act: MenuAct| MItem { label, kind, sep, arrow, act };
        match kind {
            MenuKind::Sort => {
                title = tu("#ActionButtonLabelSort");
                for (i, s) in SORTS.iter().enumerate() {
                    items.push(it(t(s.token()), 0, false, false, MenuAct::Sort(i)));
                }
                focus = self.lb.sort;
                current = Some(self.lb.sort);
                self.lb.sort_focus = self.lb.sort;
            }
            MenuKind::Filter => {
                title = t_or("#Library_FilterCollectionTitle", i18n::tr_ref("library.library_filters"));
                for (i, tok) in ["#HardwareVariant_SteamDeck_Verified", "#DeckVerified_CategoryLabel_Playable", "#DeckVerified_FilterLabel_AllGames"].iter().enumerate() {
                    items.push(it(t_or(tok, tok), 0, false, false, MenuAct::Filter(i)));
                }
                items.push(it(t_or("#Library_FilterCollection_Advanced", i18n::tr_ref("library.show_advanced")), 0, true, false, MenuAct::Advanced));
                focus = self.lb.filter;
                current = Some(self.lb.filter);
            }
            MenuKind::Tile(gi) => {
                let g = &self.games()[gi];
                let installed = g.installed || g.kind == GameKind::Shortcut;
                if installed {
                    items.push(it(t_or("#GameAction_Play", i18n::tr_ref("library.play")), 1, false, false, MenuAct::Launch(gi)));
                } else {
                    items.push(it(t_or("#GameAction_Install", i18n::tr_ref("library.install")), 2, false, false, MenuAct::Install(gi)));
                }
                let fav = Self::is_favorite(g);
                items.push(it(if fav { t_or("#GameAction_Unfavorite", i18n::tr_ref("library.remove_from_favorites")) } else { t_or("#GameAction_Favorite", i18n::tr_ref("library.add_to_favorites")) }, 0, false, false, MenuAct::Favorite(gi)));
                items.push(it(t_or("#GameAction_AddToCollection", i18n::tr_ref("library.add_to")), 0, false, true, MenuAct::AddTo(gi)));
                items.push(it(t_or("#Button_Manage", i18n::tr_ref("library.manage")), 0, false, true, MenuAct::Manage(gi)));
                items.push(it(t_or("#GameAction_ChangeArt", i18n::tr_ref("library.change_the_picture")), 0, false, false, MenuAct::ChangeArt));
                items.push(it(t_or("#GameAction_GameProperties", i18n::tr_ref("library.properties")), 0, true, false, MenuAct::Props(gi)));
                items.push(it(t_or("#Button_Cancel", i18n::tr_ref("library.cancel")), 0, true, false, MenuAct::Cancel));
            }
            MenuKind::Manage(id) => {
                title = t_or("#Button_Manage", i18n::tr_ref("library.manage"));
                if let Some(&gi) = self.gidx.get(&id) {
                    if self.can_uninstall(gi) {
                        items.push(it(t_or("#GameAction_Uninstall", i18n::tr_ref("library.uninstall")), 0, false, false, MenuAct::Uninstall(id)));
                    }
                }
                items.push(it(t_or("#Button_Back", i18n::tr_ref("library.back")), 0, true, false, MenuAct::TileBack(id)));
            }
            MenuKind::Media | MenuKind::Download => {}
            MenuKind::Tab => {
                title = "Tab Master".into();
                items.push(it(t_or("#Settings_Plugins_Open", i18n::tr_ref("library.set_up_tabs")), 0, false, false, MenuAct::TabMaster));
                items.push(it(t_or("#Button_Cancel", i18n::tr_ref("library.cancel")), 0, true, false, MenuAct::Cancel));
            }
        }
        self.sm.open = false;
        self.qm.open = false;
        self.lb.menu = Some(Menu { kind, title, items, focus, current });
        self.lb.sort_open = true;
        self.sync_menu();
        self.sync_overlays();
        self.sync_lib();
        self.update_hint();
        play(Snd::ShowModal);
    }

    /// A popup built by another screen (media filters / options) in the same window-level menu.
    pub fn open_custom_menu(&mut self, kind: MenuKind, title: String, items: Vec<MItem>, focus: usize) {
        self.sm.open = false;
        self.qm.open = false;
        self.lb.menu = Some(Menu { kind, title, items, focus, current: Some(focus) });
        self.lb.sort_open = true;
        self.sync_menu();
        self.sync_overlays();
        self.sync_lib();
        self.update_hint();
        play(Snd::ShowModal);
    }

    pub fn close_menu(&mut self) {
        if self.lb.menu.is_none() && !self.lb.sort_open {
            return;
        }
        self.lb.menu = None;
        self.lb.sort_open = false;
        self.sync_overlays();
        self.sync_lib();
        self.update_hint();
        play(Snd::HideModal);
    }

    fn sync_menu(&self) {
        let ui = self.ui();
        let st = ui.global::<LibraryState>();
        let Some(m) = self.lb.menu.as_ref() else { return };
        st.set_menu_labels(ModelRc::new(VecModel::from(m.items.iter().map(|i| SharedString::from(i.label.as_str())).collect::<Vec<_>>())));
        st.set_menu_flags(ModelRc::new(VecModel::from(
            m.items.iter().map(|i| i.kind + if i.sep { 4 } else { 0 } + if i.arrow { 8 } else { 0 }).collect::<Vec<i32>>(),
        )));
        st.set_menu_focus(m.focus as i32);
        st.set_menu_current(m.current.map(|c| c as i32).unwrap_or(-1));
        st.set_menu_title(m.title.as_str().into());
        st.set_sort_focus(m.focus as i32);
        // geometry from the ledger rows: sort list-items x 375 css / w 317 / first at y 55; tile dialog x 394 / w 280, items from y 166; filter button x 235 / w 597
        let (x, y, w, role) = match m.kind {
            MenuKind::Sort => (450.0, 8.4, 380.4, "list-item"),
            MenuKind::Filter => (282.0, 72.0, 716.4, "menu-item"),
            MenuKind::Tile(_) | MenuKind::Manage(_) | MenuKind::Tab | MenuKind::Media | MenuKind::Download => (472.8, 181.2, 336.0, "menu-item"),
        };
        let tile = matches!(m.kind, MenuKind::Tile(_) | MenuKind::Manage(_) | MenuKind::Tab | MenuKind::Media | MenuKind::Download);
        st.set_menu_pad_top(if tile { 18.0 } else { 0.0 });
        st.set_menu_pad_bottom(if tile { 48.0 } else { 0.0 });
        st.set_menu_page(matches!(m.kind, MenuKind::Filter));
        st.set_menu_x(x);
        st.set_menu_y(y);
        st.set_menu_w(w);
        st.set_menu_role(role.into());
    }

    fn activate_menu(&mut self, act: MenuAct) {
        match act {
            MenuAct::Sort(i) => {
                self.close_menu();
                self.set_sort(i);
                self.lib_focus_changed();
                return;
            }
            MenuAct::Filter(i) => {
                self.lb.filter = i;
                crate::setters::set_gamepad_filter(i);
                let keep = self.focused_lib_game().map(|g| self.games()[g].app_id);
                self.rebuild_view(keep);
                self.update_hint();
                if let Some(m) = self.lb.menu.as_mut() {
                    m.current = Some(i);
                }
                self.sync_menu();
                return;
            }
            MenuAct::Favorite(gi) => {
                self.close_menu();
                self.toggle_favorite(gi);
                return;
            }
            MenuAct::ChangeArt => {
                let gi = match self.lb.menu.as_ref().map(|m| m.kind) {
                    Some(MenuKind::Tile(gi)) => Some(gi),
                    _ => None,
                };
                self.close_menu();
                if let Some(gi) = gi {
                    let id = self.games()[gi].app_id;
                    crate::sgdb::open(self, id);
                }
                return;
            }
            MenuAct::AddTo(gi) => {
                self.close_menu();
                self.gmenu_open_add_to(gi);
                return;
            }
            MenuAct::Advanced => {
                self.close_menu();
                self.toast(&t_or("#Button_Manage", i18n::tr_ref("library.unavailable_at_deckshell")));
                return;
            }
            MenuAct::Launch(gi) => {
                self.close_menu();
                let g = &self.games()[gi];
                self.act(Act::Launch { app_id: g.app_id, rungame: g.rungame_id() });
                return;
            }
            MenuAct::Install(gi) => {
                self.close_menu();
                let id = self.games()[gi].app_id;
                self.act(Act::Install(id));
                return;
            }
            MenuAct::Props(gi) => {
                self.close_menu();
                let id = self.games()[gi].app_id;
                self.act(Act::Properties(id));
                return;
            }
            MenuAct::Manage(gi) => {
                let id = self.games()[gi].app_id;
                self.open_menu(MenuKind::Manage(id));
                return;
            }
            MenuAct::Uninstall(id) => {
                self.close_menu();
                self.confirm_uninstall(id);
                return;
            }
            MenuAct::TileBack(id) => {
                if let Some(&gi) = self.gidx.get(&id) { self.open_menu(MenuKind::Tile(gi)); }
                else { self.close_menu(); }
                return;
            }
            MenuAct::Dl(op) => {
                self.close_menu();
                self.dl_op(op);
                return;
            }
            MenuAct::Media(super::media::MediaOp::Delete) => {
                self.close_menu();
                self.media_menu_delete();
                return;
            }
            MenuAct::Media(op) => {
                self.close_menu();
                self.media_op(op);
                return;
            }
            MenuAct::TabMaster => {
                self.close_menu();
                self.toggle_qam();
                return;
            }
            MenuAct::Cancel => {}
        }
        self.close_menu();
    }

    /// Input while a library popup is open (app.rs routes everything here while `sort_open`).
    pub fn handle_sort(&mut self, a: Action) {
        let Some(m) = self.lb.menu.as_mut() else {
            self.lb.sort_open = false;
            self.sync_overlays();
            self.update_hint();
            return;
        };
        match a {
            Action::Up => m.focus = m.focus.saturating_sub(1),
            Action::Down => m.focus = (m.focus + 1).min(m.items.len().saturating_sub(1)),
            Action::A => {
                let act = m.items[m.focus].act;
                return self.activate_menu(act);
            }
            Action::B | Action::Left if matches!(m.kind, MenuKind::Manage(_)) => {
                let MenuKind::Manage(id) = m.kind else { unreachable!() };
                return self.activate_menu(MenuAct::TileBack(id));
            }
            Action::B | Action::Y | Action::X | Action::Menu => return self.close_menu(),
            _ => return,
        }
        self.sync_menu();
        let _ = play as fn(Snd);
    }
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;

/// Steam collections may still have their stored Russian name after a locale change.
pub(super) fn is_favorite_collection(name: &str) -> bool {
    let name = name.to_lowercase();
    let russian_prefix = i18n::by_code("ru").expect("Russian locale").tr("library.favorite_collection_prefix");
    name.contains("favorite") || name.contains(&russian_prefix)
}
