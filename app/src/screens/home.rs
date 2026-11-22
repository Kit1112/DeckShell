//! /routes/library/home - recents carousel.
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::{fmt_ago, fmt_playtime, App};
use crate::assets::{t, t_or};
use crate::input::Action;
use crate::loader::Kind;
use crate::router::Route;
use crate::actions::Act;
use crate::feed::{parse_feed, HomeFeed, HOME_FEED};
use crate::sound::{play, Snd};
use crate::{AppWindow, CardData, HomeRow, HomeState};
use steam_control::SteamPage;

pub struct State {
    /// indices into `App::lib.games`
    pub recent: Vec<usize>,
    pub recent_model: Rc<VecModel<CardData>>,
    pub idx: usize,
    /// 0 recents row, 1 tab pills, 2 content list
    pub zone: i32,
    pub tab: usize,
    pub item: usize,
    pub items: Vec<Item>,
    pub scroll: f32,
    pub feed: Option<HomeFeed>,
    pub feed_at: Option<std::time::Instant>,
    diag_recent: Vec<u32>,
    diag_rows: Vec<RowDiag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RowDiag {
    id: u32,
    focused: bool,
    capsule: bool,
    capsule_source: String,
    wide: bool,
    wide_source: String,
}
impl State {
    /// Move among focusable feed rows, preserving headings as non-focusable separators.
    fn step_items(&mut self, up: bool) {
        if up {
            match (0..self.item).rev().find(|&i| self.items[i].kind != 0) {
                Some(i) => self.item = i,
                None => self.zone = 1,
            }
        } else if let Some(i) = (self.item + 1..self.items.len()).find(|&i| self.items[i].kind != 0) {
            self.item = i;
        } else {
            play(Snd::FailedNav);
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State { recent: vec![], recent_model: Rc::new(VecModel::default()), idx: 0, zone: 0, tab: 0, item: 0, items: vec![], scroll: 0.0, feed: None, feed_at: None, diag_recent: vec![], diag_rows: vec![] }
    }
}

#[derive(Clone, Debug)]
pub enum ItemAct {
    None,
    /// open the in-library game page
    Game(u32),
    /// not in the library: open the store page
    Store(u32),
    Discovery,
}
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: i32,
    pub title: String,
    pub sub: String,
    pub tag: String,
    pub gi: Option<usize>,
    pub act: ItemAct,
}
const H_HEADER: f32 = 58.0;
const H_ITEM: f32 = 68.0;
const H_BUTTON: f32 = 64.0;
const LIST_VIEW_H: f32 = 612.0;

pub fn wire(ui: &AppWindow) {
    ui.global::<HomeState>().on_tab_clicked(|i| super::defer(move |a| {
        a.hm.zone = 1;
        a.home_set_tab(i as usize);
    }));
    ui.global::<HomeState>().on_item_clicked(|i| super::defer(move |a| {
        a.hm.zone = 2;
        a.hm.item = i as usize;
        a.home_sync_lower();
        a.home_activate_item();
    }));
    ui.global::<HomeState>().on_activate(|i| {
        super::defer(move |a| {
            a.hm.idx = i as usize;
            a.home_refresh();
            a.handle(Action::A);
        })
    });
}

pub fn wire_models(app: &App, ui: &AppWindow) {
    ui.global::<HomeState>().set_recent(ModelRc::from(app.hm.recent_model.clone()));
}

pub fn init_strings(ui: &AppWindow) {
    ui.global::<HomeState>().set_lib_label(t_or("#GamepadHome_GoToLibrary", "Открыть библиотеку").into());
}

fn first_focusable_item(items: &[Item]) -> Option<usize> {
    items.iter().position(|item| item.kind != 0)
}

fn selected_id(recent: &[usize], idx: usize, id_at: impl Fn(usize) -> Option<u32>) -> Option<u32> {
    recent.get(idx).and_then(|&gi| id_at(gi))
}

fn selected_position(recent: &[usize], id: u32, id_at: impl Fn(usize) -> Option<u32>) -> Option<usize> {
    recent.iter().position(|&gi| id_at(gi) == Some(id))
}

impl App {
    /// Home "recent" row: running game first (resume), then bridge recents (installed or played), else last-played.
    pub fn recompute_recent(&mut self) {
        let keep = self.home_selected_id();
        self.recompute_recent_keeping(keep);
    }

    pub(crate) fn home_selected_id(&self) -> Option<u32> {
        selected_id(&self.hm.recent, self.hm.idx, |gi| self.games().get(gi).map(|g| g.app_id))
    }

    pub(crate) fn recompute_recent_keeping(&mut self, keep: Option<u32>) {
        let games = &self.lib.games;
        let mut r: Vec<usize> = Vec::new();
        if !self.bridge_recent.is_empty() {
            for id in &self.bridge_recent {
                if let Some(&gi) = self.gidx.get(id) {
                    let g = &games[gi];
                    if g.installed || g.playtime_minutes > 0 || g.last_played.is_some() {
                        r.push(gi);
                    }
                }
            }
        }
        if r.is_empty() {
            r = (0..games.len()).filter(|&i| games[i].last_played.is_some()).collect();
            r.sort_by_key(|&i| std::cmp::Reverse(games[i].last_played));
            if r.is_empty() {
                r = (0..games.len()).filter(|&i| games[i].installed).collect();
            }
        }
        // the running game is pinned first (Steam: "Resume" featured tile)
        for id in self.live.running.iter().rev() {
            if let Some(&gi) = self.gidx.get(id) {
                r.retain(|&x| x != gi);
                r.insert(0, gi);
            }
        }
        r.truncate(if self.bridge_recent.is_empty() { 15 } else { 20 });
        let recent_ids: Vec<u32> = r.iter().filter_map(|&gi| games.get(gi).map(|g| g.app_id)).collect();
        if crate::diag::enabled() && recent_ids != self.hm.diag_recent {
            crate::diag::log("home_recent_changed", serde_json::json!({
                "previous": self.hm.diag_recent,
                "current": recent_ids,
                "bridge_recent": self.bridge_recent,
                "running": self.live.running,
            }));
            self.hm.diag_recent = recent_ids;
        }
        self.hm.recent = r;
        if let Some(id) = keep {
            if let Some(p) = selected_position(&self.hm.recent, id, |gi| self.games().get(gi).map(|g| g.app_id)) {
                self.hm.idx = p;
            }
        }
        self.hm.idx = self.hm.idx.min(self.hm.recent.len()); // == len: the trailing "open library" tile
    }

    pub fn home_refresh(&mut self) {
        let idx = self.hm.idx;
        let mut row_diag = Vec::with_capacity(self.hm.recent.len());
        let mut cards = Vec::with_capacity(self.hm.recent.len());
        for (i, &gi) in self.hm.recent.iter().enumerate() {
            let g = &self.games()[gi];
            let anim = self.anim_play.contains_key(&g.app_id);
            let capsule_loaded = self.imgs.contains_key(&(g.app_id, Kind::Capsule));
            let header_loaded = self.imgs.contains_key(&(g.app_id, Kind::Header));
            let hero_loaded = self.imgs.contains_key(&(g.app_id, Kind::Hero));
            row_diag.push(RowDiag {
                id: g.app_id,
                focused: i == idx,
                capsule: anim || capsule_loaded,
                capsule_source: if anim { "anim_frame".into() } else if capsule_loaded { format!("capsule:{}", g.art.capsule.as_ref().map(|p| p.display().to_string()).unwrap_or_default()) } else { "none".into() },
                wide: header_loaded || hero_loaded,
                wide_source: if header_loaded { format!("header:{}", g.art.header.as_ref().or(g.art.hero.as_ref()).map(|p| p.display().to_string()).unwrap_or_default()) } else if hero_loaded { format!("hero:{}", g.art.hero.as_ref().map(|p| p.display().to_string()).unwrap_or_default()) } else { "none".into() },
            });
            let mut c = self.card(gi);
            let (x, w) = if i < idx {
                (36.0 - (idx - i) as f32 * 175.0, 161.0)
            } else if i == idx {
                (36.0, 516.0)
            } else {
                (36.0 + 516.0 + 14.0 + (i - idx - 1) as f32 * 175.0, 161.0)
            };
            c.x = x;
            c.w = w;
            c.caption = "".into();
            cards.push(c);
        }
        if crate::diag::enabled() && row_diag != self.hm.diag_rows {
            let describe = |rows: &[RowDiag]| rows.iter().enumerate().map(|(row,r)| serde_json::json!({
                "row":row, "wide_mode":row==0, "effective":if row==0 && r.wide {"wide"} else if r.capsule {"capsule"} else {"placeholder"},
                "id": r.id, "focused": r.focused,
                "capsule": {"present": r.capsule, "source": r.capsule_source},
                "wide": {"present": r.wide, "source": r.wide_source},
            })).collect::<Vec<_>>();
            crate::diag::log("home_rows_changed", serde_json::json!({
                "focus_idx": idx,
                "previous": describe(&self.hm.diag_rows),
                "current": describe(&row_diag),
            }));
            self.hm.diag_rows = row_diag;
        }
        if self.hm.recent_model.row_count() != cards.len() {
            self.hm.recent_model.set_vec(cards);
        } else {
            for (i, c) in cards.into_iter().enumerate() {
                self.hm.recent_model.set_row_data(i, c);
            }
        }
        let ui = self.ui();
        let st = ui.global::<HomeState>();
        st.set_idx(idx as i32);
        if let Some(&gi) = self.hm.recent.get(idx) {
            let g = self.games()[gi].clone();
            st.set_hero_title(g.name.as_str().into());
            let fp = self.friends_playing(g.app_id);
            let sub = if fp > 0 {
                format!("{} {fp}", t_or("#LibraryHome_FriendsPlaying", "Друзей играет:"))
            } else if self.is_running(g.app_id) {
                t_or("#GameAction_ResumeGameInProgress", "Возобновить").to_uppercase()
            } else if g.playtime_minutes == 0 {
                t_or("#AppBox_NoPlayTimeYet", "Нет проведённого в игре времени").to_uppercase()
            } else {
                format!("{}: {}   {}: {}", t("#AppDetails_SectionTitle_PlayTime").to_uppercase(), fmt_playtime(g.playtime_minutes).to_uppercase(), t("#AppDetails_SectionTitle_LastPlayed").to_uppercase(), fmt_ago(g.last_played).to_uppercase())
            };
            st.set_hero_sub(sub.into());
        } else {
            st.set_hero_title("".into());
            st.set_hero_sub("".into());
        }
    }

    pub fn home_request_all(&mut self) {
        let idx = self.hm.idx;
        for i in 0..self.hm.recent.len() {
            let gi = self.hm.recent[i];
            let id = self.games()[gi].app_id;
            if crate::diag::enabled() {
                crate::diag::log("home_request_all", serde_json::json!({"idx": idx, "row": i, "id": id, "kind": "Capsule"}));
            }
            self.request(gi, Kind::Capsule);
            // tile 0 is always the wide (header) capsule: keep requesting it so a cache eviction after scrolling far
            // right doesn't leave the portrait capsule cropped into the wide slot (test 3).
            if i == 0 || (i + 1 >= idx && i <= idx + 1) {
                if crate::diag::enabled() {
                    crate::diag::log("home_request_all", serde_json::json!({"idx": idx, "row": i, "id": id, "kind": "Header"}));
                }
                self.request(gi, Kind::Header);
            }
            // The background hero follows focus (update_bg hides the background until the hero is cached). Prefetch the
            // focused tile's neighbours so a Right press finds its hero already decoded (task 143: each press otherwise
            // blanked the background for the decode time).
            if i + 1 >= idx && i <= idx + 1 {
                self.request(gi, Kind::Hero);
            }
        }
    }

    /// B in the lower page goes back up to the recents row before leaving Home.
    pub fn home_back(&mut self) -> bool {
        if self.hm.zone != 0 {
            self.hm.zone = 0;
            self.home_sync_lower();
            return true;
        }
        false
    }

    /// L1/R1 on Home: the section tabs (wrap), never the library.
    fn home_step_tab(&mut self, d: i32) {
        let n = 3i32;
        let t = (self.hm.tab as i32 + d).rem_euclid(n) as usize;
        self.home_set_tab(t);
    }

    pub fn home_set_tab(&mut self, t: usize) {
        if t != self.hm.tab {
            play(Snd::ChangeTabs);
        }
        self.hm.tab = t.min(2);
        self.hm.item = 0;
        self.hm.scroll = 0.0;
        self.home_build_items();
        self.home_sync_lower();
    }

    /// Fetch the feed (events, completed downloads, trending, friends, play-next) at most once a minute while Home is shown.
    pub fn home_fetch_feed(&mut self) {
        if self.hm.feed_at.map(|t| t.elapsed().as_secs() < 60).unwrap_or(false) {
            return;
        }
        self.hm.feed_at = Some(std::time::Instant::now());
        crate::setters::read(HOME_FEED.to_string(), |a, v| {
            if let Some(f) = parse_feed(&v) {
                a.hm.feed = Some(f);
                a.home_build_items();
                a.home_sync_lower();
                a.home_refresh();
            }
        });
    }

    pub fn friends_in_game(&self) -> usize {
        self.hm.feed.as_ref().map(|f| f.friends.iter().filter(|x| x.gameid != 0 || !x.game.is_empty()).count()).unwrap_or(0)
    }

    fn friends_playing(&self, appid: u32) -> usize {
        self.hm.feed.as_ref().map(|f| f.friends.iter().filter(|x| x.gameid == appid).count()).unwrap_or(0)
    }

    fn ago(_now: i64, t: i64) -> String {
        if t <= 0 {
            return String::new();
        }
        crate::app::fmt_ago(Some(t as u64))
            .to_string()
            .replace("Сегодня", "сегодня")
            .replace("Вчера", "вчера")
    }

    /// The lower page rows of the selected tab (sp.js 392300 tabs; data = feed.rs).
    pub fn home_build_items(&mut self) {
        let mut v: Vec<Item> = vec![];
        let hdr = |t: String| Item { kind: 0, title: t, sub: String::new(), tag: String::new(), gi: None, act: ItemAct::None };
        let f = self.hm.feed.clone();
        let now = f.as_ref().map(|f| f.now).unwrap_or(0);
        let game_of = |a: &App, id: u32| a.gidx.get(&id).copied();
        match self.hm.tab {
            0 => {
                if let Some(f) = &f {
                    for e in f.events.iter().take(8) {
                        let gi = game_of(self, e.appid);
                        let kind = if e.category.is_empty() { "НОВОСТЬ".to_string() } else { e.category.to_uppercase() };
                        let game = gi.map(|g| self.games()[g].name.clone()).unwrap_or_default();
                        v.push(Item { kind: 1, title: if e.name.is_empty() { game.clone() } else { e.name.clone() }, sub: if game.is_empty() { e.summary.clone() } else { format!("{game} - {}", e.summary) }, tag: format!("{kind}  {}", Self::ago(now, e.post)), gi, act: ItemAct::Game(e.appid) });
                    }
                    if !f.completed.is_empty() {
                        v.push(hdr(t_or("#LibraryHome_RecentlyCompleted", "Недавно обновлено на этом устройстве")));
                        for c in f.completed.iter().take(8) {
                            let gi = game_of(self, c.appid);
                            let name = gi.map(|g| self.games()[g].name.clone()).unwrap_or_else(|| format!("AppID {}", c.appid));
                            let gb = if c.bytes > 0 { format!("{:.1} ГБ", c.bytes as f64 / 1e9) } else { String::new() };
                            v.push(Item { kind: 1, title: name, sub: gb, tag: Self::ago(now, c.time), gi, act: ItemAct::Game(c.appid) });
                        }
                    }
                    let st: Vec<_> = f.store.iter().filter(|a| a.ready && !a.name.is_empty()).take(10).collect();
                    if !st.is_empty() {
                        v.push(hdr(t_or("#LibraryHome_TrendingWithFriends", "Популярно у друзей")));
                        for a in st {
                            let tag = if a.owned { String::new() } else if a.soon { "СКОРО".to_string() } else if a.free { "БЕСПЛАТНО".to_string() } else if a.pct > 0 { format!("-{}%  {}", a.pct, a.price) } else { a.price.clone() };
                            v.push(Item { kind: 1, title: a.name.clone(), sub: if a.owned { "В БИБЛИОТЕКЕ".into() } else { String::new() }, tag, gi: game_of(self, a.appid), act: if a.owned { ItemAct::Game(a.appid) } else { ItemAct::Store(a.appid) } });
                        }
                    }
                }
            }
            1 => {
                if let Some(f) = &f {
                    let mut fr: Vec<_> = f.friends.iter().filter(|x| x.state > 0 || x.gameid != 0).collect();
                    fr.sort_by_key(|x| (x.gameid == 0 && x.game.is_empty(), x.name.to_lowercase()));
                    let playing = fr.iter().filter(|x| x.gameid != 0 || !x.game.is_empty()).count();
                    if playing > 0 {
                        v.push(hdr(format!("{}: {playing}", t_or("#FriendsUI_InGame", "Сейчас в игре"))));
                    }
                    let mut shown_online = false;
                    for x in fr.iter().take(40) {
                        let in_game = x.gameid != 0 || !x.game.is_empty();
                        if !in_game && !shown_online {
                            v.push(hdr("В сети".into()));
                            shown_online = true;
                        }
                        let gi = game_of(self, x.gameid);
                        v.push(Item { kind: 1, title: x.name.clone(), sub: if in_game { x.game.clone() } else { String::new() }, tag: if in_game { String::new() } else { "В СЕТИ".into() }, gi, act: if x.gameid != 0 { ItemAct::Game(x.gameid) } else { ItemAct::None } });
                    }
                }
            }
            _ => {
                v.push(Item { kind: 2, title: t_or("#GamepadHome_RecommendationsButton", "ОТКРЫТЬ НОВЫЙ СПИСОК РЕКОМЕНДАЦИЙ"), sub: String::new(), tag: String::new(), gi: None, act: ItemAct::Discovery });
                if let Some(f) = &f {
                    if !f.play_next.is_empty() {
                        v.push(hdr(t_or("#LibraryHome_PlayNext", "Во что поиграть дальше")));
                        for id in f.play_next.iter().take(12) {
                            if let Some(gi) = game_of(self, *id) {
                                let g = &self.games()[gi];
                                v.push(Item { kind: 1, title: g.name.clone(), sub: if g.playtime_minutes > 0 { fmt_playtime(g.playtime_minutes) } else { String::new() }, tag: String::new(), gi: Some(gi), act: ItemAct::Game(*id) });
                            }
                        }
                    }
                }
            }
        }
        self.hm.items = v;
        // focus lands on the first focusable row
        if self.hm.items.get(self.hm.item).map(|i| i.kind == 0).unwrap_or(true) {
            match first_focusable_item(&self.hm.items) {
                Some(item) => self.hm.item = item,
                None => {
                    self.hm.item = 0;
                    if self.hm.zone == 2 { self.hm.zone = 1; }
                }
            }
        }
        let gis: Vec<usize> = self.hm.items.iter().filter_map(|i| i.gi).collect();
        for gi in gis {
            self.request(gi, Kind::Capsule);
        }
    }

    fn item_y(&self, idx: usize) -> (f32, f32) {
        let mut y = 8.0;
        for (i, it) in self.hm.items.iter().enumerate() {
            let h = match it.kind { 0 => H_HEADER, 2 => H_BUTTON, _ => H_ITEM };
            if i == idx {
                return (y, h);
            }
            y += h;
        }
        (y, 0.0)
    }

    pub fn home_sync_lower(&mut self) {
        // scroll so the focused row is visible (headers above stay with the first row)
        let mut top = 0.0;
        if self.hm.zone == 2 {
            let (mut y, h) = self.item_y(self.hm.item);
            if self.hm.item > 0 && self.hm.items.get(self.hm.item - 1).map(|i| i.kind == 0).unwrap_or(false) {
                y -= H_HEADER;
            }
            top = self.hm.scroll;
            if y < top {
                top = y;
            } else if y + h + 12.0 > top + LIST_VIEW_H {
                top = y + h + 12.0 - LIST_VIEW_H;
            }
        }
        self.hm.scroll = top.max(0.0);
        let mut rows = Vec::with_capacity(self.hm.items.len());
        let mut y = 8.0;
        let items = self.hm.items.clone();
        for it in &items {
            let h = match it.kind { 0 => H_HEADER, 2 => H_BUTTON, _ => H_ITEM };
            let (img, has) = match it.gi {
                Some(gi) => {
                    let c = self.card(gi);
                    (c.img, c.has_img)
                }
                None => (Default::default(), false),
            };
            rows.push(HomeRow { kind: it.kind, title: it.title.as_str().into(), sub: it.sub.as_str().into(), tag: it.tag.as_str().into(), img, has_img: has, y, h });
            y += h;
        }
        let in_game = self.friends_in_game();
        let tabs = vec![
            ("#LibraryHome_WhatsNew", "Что нового", String::new()),
            ("#FriendsUI_Friends", "Друзья", if in_game > 0 { in_game.to_string() } else { String::new() }),
            ("#LibraryHome_Recommended", "Рекомендации", String::new()),
        ];
        let tabs: Vec<crate::GTab> = tabs.into_iter().enumerate().map(|(i, (tok, fb, badge))| crate::GTab { label: t_or(tok, fb).into(), badge: badge.into(), selected: i == self.hm.tab }).collect();
        let ui = self.ui();
        let st = ui.global::<HomeState>();
        st.set_tabs(ModelRc::new(VecModel::from(tabs)));
        st.set_zone(self.hm.zone);
        st.set_tab(self.hm.tab as i32);
        st.set_item_focus(self.hm.item as i32);
        st.set_items(ModelRc::new(VecModel::from(rows)));
        st.set_list_scroll(self.hm.scroll);
        st.set_content_h(y);
        let empty_txt = match self.hm.tab {
            1 => t_or("#FriendsUI_NoFriendsOnline", "Нет друзей в сети"),
            2 => t_or("#LibraryHome_Recommendations_Empty", "Рекомендации пока недоступны"),
            _ => t_or("#LibraryHome_WhatsNew_Empty", "Здесь пока ничего нет"),
        };
        st.set_empty_text(empty_txt.into());
    }

    fn home_activate_item(&mut self) {
        let Some(it) = self.hm.items.get(self.hm.item).cloned() else { return };
        match it.act {
            ItemAct::Game(id) => match self.gidx.get(&id).copied() {
                Some(gi) => self.open_game(gi),
                None => self.act(Act::OpenStore(id)),
            },
            ItemAct::Store(id) => self.act(Act::OpenStore(id)),
            // DiscoveryQueue wizard lives in the store; the closest in-shell equivalent is the store front page
            ItemAct::Discovery => self.act(Act::Open(SteamPage::Store)),
            ItemAct::None => {}
        }
    }

    /// START on a recents tile: the game context menu (Играть/Возобновить, Добавить в избранное, ..., Отмена).
    pub fn home_start(&mut self) {
        if self.hm.zone == 0 {
            if let Some(&gi) = self.hm.recent.get(self.hm.idx) {
                self.open_menu(super::library::MenuKind::Tile(gi));
            }
        } else if self.hm.zone == 2 {
            if let Some(gi) = self.hm.items.get(self.hm.item).and_then(|i| i.gi) {
                self.open_menu(super::library::MenuKind::Tile(gi));
            }
        }
    }

    pub fn handle_home(&mut self, a: Action) {
        self.home_fetch_feed();
        match a {
            Action::LB => return self.home_step_tab(-1),
            Action::RB => return self.home_step_tab(1),
            Action::Menu => return self.home_start(),
            _ => {}
        }
        match self.hm.zone {
            1 => {
                match a {
                    Action::Left => self.home_set_tab(self.hm.tab.saturating_sub(1)),
                    Action::Right => self.home_set_tab((self.hm.tab + 1).min(2)),
                    Action::Up => self.hm.zone = 0,
                    Action::Down => {
                        if let Some(item) = self.hm.items.get(self.hm.item).filter(|item| item.kind != 0)
                            .map(|_| self.hm.item).or_else(|| first_focusable_item(&self.hm.items)) {
                            self.hm.item = item;
                            self.hm.zone = 2;
                        } else {
                            play(Snd::FailedNav);
                            return;
                        }
                    },
                    Action::A => {}
                    _ => return,
                }
                play(Snd::BasicNav);
                self.home_sync_lower();
                return;
            }
            2 => {
                match a {
                    Action::Up => self.hm.step_items(true),
                    Action::Down => self.hm.step_items(false),
                    Action::A => return self.home_activate_item(),
                    _ => return,
                }
                self.home_sync_lower();
                return;
            }
            _ => {}
        }
        match a {
            Action::Left => self.hm.idx = self.hm.idx.saturating_sub(1),
            Action::Right => self.hm.idx = (self.hm.idx + 1).min(self.hm.recent.len()),
            Action::Down => {
                // Down from the row -> the tab pills (sp.js: tabs are always present)
                self.hm.zone = 1;
                self.home_build_items();
                self.home_sync_lower();
                play(Snd::BasicNav);
                return;
            }
            Action::A => {
                if self.hm.idx >= self.hm.recent.len() {
                    self.navigate(Route::Library("AllGames".into()));
                    return;
                }
                if let Some(&gi) = self.hm.recent.get(self.hm.idx) {
                    self.open_game(gi)
                }
            }
            _ => return,
        }
        self.home_request_all();
        self.home_refresh();
        self.update_bg();
        self.focus_changed_home();
    }

    pub fn focus_changed_home(&mut self) {
        let id = if matches!(self.router.current(), Route::Home) { self.hm.recent.get(self.hm.idx).map(|&i| self.games()[i].app_id) } else { None };
        self.focus_changed(id);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn lower_content_focus_skips_headings_and_empty_lists() {
        let item = |kind| Item { kind, title: String::new(), sub: String::new(), tag: String::new(), gi: None, act: ItemAct::None };
        for (kinds, expected) in [(&[][..], None), (&[0, 0][..], None), (&[0, 1, 2][..], Some(1)), (&[2][..], Some(0))] {
            let items: Vec<_> = kinds.iter().copied().map(item).collect();
            assert_eq!(first_focusable_item(&items), expected);
        }
    }
    #[test]
    fn long_feed_walk_returns_through_every_row_before_tabs() {
        let item = |kind| Item { kind, title: String::new(), sub: String::new(), tag: String::new(), gi: None, act: ItemAct::None };
        let mut s = State::default();
        s.items = [0, 1, 1, 0, 1, 1, 1, 0, 1].into_iter().map(item).collect();
        s.zone = 2;
        s.item = 1;
        for expected in [2, 4, 5, 6, 8] { s.step_items(false); assert_eq!(s.item, expected); }
        for expected in [6, 5, 4, 2, 1] { s.step_items(true); assert_eq!((s.zone, s.item), (2, expected)); }
        s.step_items(true);
        assert_eq!(s.zone, 1);
    }
    #[test]
    fn selected_id_survives_library_reorder() {
        let old_library = [11, 22, 33];
        let old_recent = [2, 0, 1];
        let keep = selected_id(&old_recent, 1, |i| old_library.get(i).copied()).unwrap();
        let new_library = [33, 22, 11];
        let new_recent = [0, 1, 2];
        let idx = selected_position(&new_recent, keep, |i| new_library.get(i).copied()).unwrap();
        assert_eq!(selected_id(&new_recent, idx, |i| new_library.get(i).copied()), Some(11));
        assert_eq!(idx, 2);
    }
}
