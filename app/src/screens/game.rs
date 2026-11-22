//! /routes/library/app/<id> - game page (hero, play bar, plugin elements, tab pills + content).
//! Layout/colours are generated from the Steam dumps (ui/generated/game, tools/compile-ui/game.mjs); this file holds the behaviour
//! from docs/cdp/behavior/handlers.json + semantics.md "Game page":
//!   A play button  -> resume running game / Apps.RunGame / OpenInstallWizard      X (running) -> stop
//!   controller btn -> controller configurator (no Act yet: falls back to Properties)       gear / START -> context menu
//!   Down from the bar -> tab pills (page scrolls), Down again -> content, Up -> back; L1/R1 or Left/Right switch tabs; B from tabs -> bar
//! Plugin elements (read-only): HLTB bar = Decky `hltb-for-deck` IndexedDB cache (via the bridge), player badge = Steam public API (curl).
use steam_assets::i18n;
use std::time::{Duration, Instant};

use chrono::{Datelike, TimeZone};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use steam_data::GameKind;

use crate::actions::Act;
use crate::app::{fmt_playtime, fmt_size, now_secs, App};
use crate::assets::{t_or, tu};
use crate::input::Action;
use crate::loader::Kind;
use crate::model::DS_UPDATE;
use crate::router::Route;
use crate::sound::{play, Snd};
use crate::{AppWindow, GHltbRow, GRow, GameState};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RowAct {
    News,
    Achievements,
    Store,
    Properties,
}

pub(crate) fn can_resume_running_game(running: &[u32], id: u32) -> bool {
    running.contains(&id)
}

// Keep the primary button's routing testable without launching Steam or a UI.
trait PrimaryTarget {
    fn resume(&mut self, id: u32);
    fn launch(&mut self, id: u32, rungame: u64);
    fn install(&mut self, id: u32);
}

fn dispatch_primary(target: &mut impl PrimaryTarget, primary: Primary, id: u32, rungame: u64) {
    match primary {
        Primary::Resume => target.resume(id),
        Primary::Play => target.launch(id, rungame),
        Primary::Install | Primary::Update => target.install(id),
    }
}

impl PrimaryTarget for App {
    fn resume(&mut self, id: u32) { self.resume_running_game(id); }
    fn launch(&mut self, id: u32, rungame: u64) {
        play(Snd::LaunchGame);
        self.act(Act::Launch { app_id: id, rungame });
    }
    fn install(&mut self, id: u32) { self.act(Act::Install(id)); }
}

#[derive(Default)]
pub struct State {
    /// bar focus (zone 0): 0 primary, 1 controller, 2 gear
    pub focus: i32,
    /// 0 bar, 1 tab pills (page scrolled under the top bar), 2 tab content
    pub zone: i32,
    pub tab: i32,
    pub crow: i32,
    rows: Vec<(i32, String, String, String, Option<RowAct>)>,
    pub mn: super::manage::Menu,
    pub badge_timer: Timer,
    badge_id: u32,
    badge_at: Option<Instant>,
}

const TABS: [(&str, &str); 4] = [
    ("#menu_activity", "game.activity"),
    ("#appdetails_tab_yourstuff", "game.your_stuff"),
    ("#menu_community", "game.community"),
    ("#appdetails_gameinfo", "game.game_info"),
];

fn row_h(kind: i32) -> f32 {
    match kind {
        0 => 46.0,
        1 => 30.0,
        2 => 34.0,
        _ => 52.0,
    }
}

pub fn wire(ui: &AppWindow) {
    let g = ui.global::<GameState>();
    g.set_label_scroll_to_top(i18n::tr("game.scroll_to_top").into());
    g.on_button(|i| super::defer(move |a| a.game_button(i)));
    g.on_tab_clicked(|i| super::defer(move |a| {
        a.gm.zone = a.gm.zone.max(1);
        a.game_set_tab(i);
    }));
    g.on_row_clicked(|i| super::defer(move |a| {
        a.gm.zone = 2;
        a.gm.crow = i;
        a.game_row_action(i as usize);
        a.sync_game();
    }));
    g.on_menu_clicked(|i| super::defer(move |a| {
        if i < 0 {
            a.gmenu_back();
        } else {
            a.gmenu_set_idx(i);
            a.gmenu_activate();
        }
    }));
    g.on_scroll_top(|| super::defer(|a| {
        a.gm.zone = 0;
        a.gm.focus = 0;
        a.sync_game();
    }));
    g.on_badge_clicked(|| super::defer(|a| {
        if let Some(gi) = a.current_game() {
            let id = a.games()[gi].app_id;
            a.act(Act::OpenStore(id));
        }
    }));
}

pub fn init_strings(ui: &AppWindow) {
    let g = ui.global::<GameState>();
    let labels: Vec<SharedString> = TABS.iter().map(|(tok, fb)| t_or(tok, i18n::tr_ref(fb)).into()).collect();
    let toks: Vec<SharedString> = TABS.iter().map(|(tok, _)| (*tok).into()).collect();
    g.set_tabs(ModelRc::new(VecModel::from(labels)));
    g.set_tab_tokens(ModelRc::new(VecModel::from(toks)));
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Primary {
    Play,
    Resume,
    Install,
    Update,
}

const MONTHS: [&str; 12] = ["game.jan", "game.feb", "game.mar", "game.apr", "game.may", "game.jun", "game.jul", "game.aug", "game.sep", "game.oct", "game.nov", "game.dec"];

/// "Сегодня" / "13 мая 2024 г." like Steam's last-played block.
fn fmt_date(t: u64) -> String {
    let days = now_secs().saturating_sub(t) / 86_400;
    if days == 0 {
        return i18n::tr_ref("game.today").into();
    }
    match chrono::Local.timestamp_opt(t as i64, 0).single() {
        Some(d) => i18n::tr_args("game.last_played_date", &[("day", &(d.day()).to_string()), ("month", i18n::tr_ref(MONTHS[d.month0() as usize])), ("year", &(d.year()).to_string())]),
        None => "-".into(),
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            o.push(',');
        }
        o.push(c);
    }
    o
}

impl App {
    pub fn current_game(&self) -> Option<usize> {
        match self.router.current() {
            Route::App(id) => self.gidx.get(id).copied(),
            _ => None,
        }
    }

    /// Push /library/app/<id> (Navigator.App) and show it.
    pub fn open_game(&mut self, gi: usize) {
        if self.lb.search {
            self.close_search();
            if matches!(self.router.current(), Route::Search) {
                // search results -> game: B should return to the page the search was opened from
                self.router.back();
            }
        }
        let id = self.games()[gi].app_id;
        self.navigate(Route::App(id));
    }

    pub fn primary_kind(&self, gi: usize) -> Primary {
        let g = &self.games()[gi];
        let ds = self.extras.get(&g.app_id).map(|e| e.display_status).unwrap_or(0);
        if self.is_running(g.app_id) {
            Primary::Resume
        } else if g.kind == GameKind::Shortcut || (g.installed && !g.update_pending && ds != DS_UPDATE) {
            Primary::Play
        } else if g.installed {
            Primary::Update
        } else {
            Primary::Install
        }
    }

    fn install_download_pending(&self, id: u32) -> bool {
        self.live.downloads.as_ref().is_some_and(|d| d.items.iter().any(|item| item.appid == id && !item.completed))
    }

    pub(crate) fn primary_label_pub(&self, gi: usize) -> String {
        if !self.is_running(self.games()[gi].app_id) && self.install_download_pending(self.games()[gi].app_id) {
            i18n::tr_ref("game.uploading").into()
        } else {
            self.primary_label(self.primary_kind(gi))
        }
    }

    fn primary_label(&self, p: Primary) -> String {
        match p {
            Primary::Play => t_or("#sale_playnow", &t_or("#GameAction_Play", i18n::tr_ref("game.play"))),
            Primary::Resume => t_or("#GameAction_ResumeGameInProgress", i18n::tr_ref("game.resume")),
            Primary::Install => t_or("#GameAction_Install", i18n::tr_ref("game.install")),
            Primary::Update => t_or("#GameAction_Update", i18n::tr_ref("game.update")),
        }
    }

    pub fn fill_game_page(&mut self, gi: usize) {
        let id = self.games()[gi].app_id;
        self.gm.focus = 0;
        self.gm.zone = 0;
        self.gm.tab = 0;
        self.gm.crow = 0;
        self.gm.mn.open = false;
        self.request(gi, Kind::Hero);
        self.request(gi, Kind::Logo);
        self.request(gi, Kind::Capsule);
        self.refresh_game_page_for(gi);
        self.update_game_art();
        self.game_plugins(id, gi);
        self.sync_game();
    }

    pub fn refresh_game_page(&mut self) {
        if let Some(gi) = self.current_game() {
            self.refresh_game_page_for(gi);
            self.sync_game();
        }
    }

    fn refresh_game_page_for(&mut self, gi: usize) {
        let g = self.games()[gi].clone();
        let ui = self.ui();
        let st = ui.global::<GameState>();
        st.set_name(g.name.as_str().into());
        let layout = crate::sgdb::logo_layout::get(g.app_id);
        st.set_logo_layout(crate::HeroFrame { zoom: layout.zoom, pan_x: layout.pan_x, pan_y: layout.pan_y });
        let p = self.primary_kind(gi);
        st.set_primary(self.primary_label_pub(gi).into());
        st.set_playable(matches!(p, Primary::Play | Primary::Resume));
        // info blocks (Steam order): required disk space (not installed), last played, played time
        let mut b: Vec<(String, String)> = vec![];
        if !g.installed && g.kind != GameKind::Shortcut && g.size_on_disk > 0 {
            b.push((tu("#AppDetails_SectionTitle_DiskSpaceRequired"), fmt_size(g.size_on_disk)));
        }
        if let Some(t) = g.last_played.filter(|t| *t > 0) {
            b.push((tu("#AppDetails_SectionTitle_LastPlayed"), fmt_date(t)));
        }
        if g.playtime_minutes > 0 {
            b.push((tu("#AppDetails_SectionTitle_PlayTime"), fmt_playtime(g.playtime_minutes)));
        }
        b.resize(3, (String::new(), String::new()));
        st.set_k1(b[0].0.as_str().into());
        st.set_v1(b[0].1.as_str().into());
        st.set_k2(b[1].0.as_str().into());
        st.set_v2(b[1].1.as_str().into());
        st.set_k3(b[2].0.as_str().into());
        st.set_v3(b[2].1.as_str().into());
        self.game_build_rows(gi);
    }

    pub fn update_game_art(&self) {
        let Some(gi) = self.current_game() else { return };
        let id = self.games()[gi].app_id;
        let ui = self.ui();
        let st = ui.global::<GameState>();
        // a logo clip that is playing owns the image; the still is only its first frame
        if !self.logo_clips.play.as_ref().is_some_and(|p| p.id == id) {
            let l = self.imgs.get(&(id, Kind::Logo));
            st.set_logo_has(l.is_some());
            st.set_logo(l.cloned().unwrap_or_default());
        }
        let c = self.imgs.get(&(id, Kind::Capsule));
        st.set_capsule_has(c.is_some());
        st.set_capsule(c.cloned().unwrap_or_default());
    }

    // ------------------------------------------------------------------ tab content
    fn game_build_rows(&mut self, gi: usize) {
        let g = self.games()[gi].clone();
        let steam = g.kind == GameKind::Steam;
        let mut r: Vec<(i32, String, String, String, Option<RowAct>)> = vec![];
        let played = g.last_played.filter(|t| *t > 0).map(fmt_date).unwrap_or_else(|| "-".into());
        let kv = |r: &mut Vec<_>, k: String, v: String| r.push((2, k, v, String::new(), None));
        match self.gm.tab {
            0 => {
                r.push((0, i18n::tr(TABS[0].1), String::new(), String::new(), None));
                kv(&mut r, t_or("#AppDetails_SectionTitle_LastPlayed", i18n::tr_ref("game.last_played")), played);
                if steam {
                    r.push((3, t_or("#AppActivity_ViewLatestNews", i18n::tr_ref("game.view_the_latest_news")), String::new(), "#appactivity_viewlatestnews".into(), Some(RowAct::News)));
                }
            }
            1 => {
                r.push((0, i18n::tr(TABS[1].1), String::new(), String::new(), None));
                kv(&mut r, t_or("#AppDetails_SectionTitle_PlayTime", i18n::tr_ref("game.play_time")), fmt_playtime(g.playtime_minutes));
                kv(&mut r, t_or("#AppDetails_SectionTitle_LastPlayed", i18n::tr_ref("game.last_played")), played);
                if steam {
                    r.push((3, t_or("#AppDetails_ViewAchievements", i18n::tr_ref("game.achievements")), String::new(), "#appdetails_achievements".into(), Some(RowAct::Achievements)));
                }
            }
            2 => {
                r.push((0, i18n::tr(TABS[2].1), String::new(), String::new(), None));
                if steam {
                    r.push((3, t_or("#AppDetails_StorePage", i18n::tr_ref("game.store_page")), String::new(), "#appdetails_storepage".into(), Some(RowAct::Store)));
                } else {
                    r.push((1, t_or("#AppDetails_NoCommunity", i18n::tr_ref("game.no_community_data")), String::new(), String::new(), None));
                }
            }
            _ => {
                r.push((0, i18n::tr(TABS[3].1), String::new(), String::new(), None));
                kv(&mut r, t_or("#AppDetails_Name", i18n::tr_ref("game.title")), g.name.clone());
                kv(&mut r, "AppID".into(), g.app_id.to_string());
                kv(&mut r, t_or("#AppDetails_SectionTitle_DiskSpaceRequired", i18n::tr_ref("game.size_on_the_disc")), fmt_size(g.size_on_disk));
                r.push((3, t_or("#Menu_Properties", i18n::tr_ref("game.properties")), String::new(), "#appdetails_properties".into(), Some(RowAct::Properties)));
            }
        }
        self.gm.rows = r;
        let first = self.gm.rows.iter().position(|x| x.4.is_some());
        self.gm.crow = first.map(|i| i as i32).unwrap_or(-1);
    }

    fn game_set_tab(&mut self, i: i32) {
        let i = i.rem_euclid(TABS.len() as i32);
        if i == self.gm.tab {
            self.sync_game();
            return;
        }
        self.gm.tab = i;
        if let Some(gi) = self.current_game() {
            self.game_build_rows(gi);
        }
        if self.gm.zone == 2 && self.gm.crow < 0 {
            self.gm.zone = 1;
        }
        play(Snd::ChangeTabs);
        self.sync_game();
    }

    fn game_row_action(&mut self, i: usize) {
        let Some(gi) = self.current_game() else { return };
        let id = self.games()[gi].app_id;
        match self.gm.rows.get(i).and_then(|r| r.4) {
            Some(RowAct::News) | Some(RowAct::Store) => self.act(Act::OpenStore(id)),
            Some(RowAct::Achievements) => self.act(Act::Achievements(id)),
            Some(RowAct::Properties) => self.act(Act::Properties(id)),
            None => {}
        }
    }

    /// Push the whole page state to Slint.
    pub fn sync_game(&mut self) {
        let ui = self.ui();
        let st = ui.global::<GameState>();
        st.set_focus(self.gm.focus);
        st.set_zone(self.gm.zone);
        st.set_tab(self.gm.tab);
        st.set_crow(self.gm.crow);
        let has_cap = self.current_game().map(|gi| self.imgs.contains_key(&(self.games()[gi].app_id, Kind::Capsule))).unwrap_or(false);
        let mut y = if self.gm.tab == 3 && has_cap { 93.6f32 } else { 20.0f32 };
        for (i, r) in self.gm.rows.iter().enumerate() {
            if i as i32 == self.gm.crow {
                break;
            }
            y += row_h(r.0);
        }
        st.set_crow_y(y);
        let rows: Vec<GRow> = self
            .gm
            .rows
            .iter()
            .map(|r| GRow { kind: r.0, a: r.1.as_str().into(), b: r.2.as_str().into(), token: r.3.as_str().into() })
            .collect();
        st.set_rows(ModelRc::new(VecModel::from(rows)));
        st.set_menu_open(self.gm.mn.open);
        let (title, labels, subs, dis, idx) = self.gmenu_view();
        st.set_menu_wide(labels.iter().any(|l| l.chars().count() > 24));
        st.set_menu_idx(idx);
        st.set_menu_title(title.into());
        st.set_menu_items(ModelRc::new(VecModel::from(labels.into_iter().map(SharedString::from).collect::<Vec<_>>())));
        st.set_menu_sub(ModelRc::new(VecModel::from(subs)));
        st.set_menu_dis(ModelRc::new(VecModel::from(dis)));
    }

    /// START on the game page (with the primary action) / A on the gear (without): see manage.rs.
    pub fn game_menu_open(&mut self) {
        self.gmenu_open_with(true);
    }

    /// B hook (call from the global B handling before `go_back`): returns true when the game page consumed it
    /// (menu -> close; tabs/content -> focus returns to the Play button = FocusActionButton()).
    pub fn game_back(&mut self) -> bool {
        if !matches!(self.router.current(), Route::App(_)) {
            return false;
        }
        if self.gm.mn.open {
            self.gmenu_back();
            return true;
        }
        if self.gm.zone > 0 {
            self.gm.zone = 0;
            self.gm.focus = 0;
            self.sync_game();
            return true;
        }
        false
    }

    /// Resume an existing game; Play launches and Install/Update opens the wizard.
    pub(crate) fn primary_action(&mut self, gi: usize) {
        let g = self.games()[gi].clone();
        if !self.is_running(g.app_id) && self.install_download_pending(g.app_id) {
            self.navigate(Route::Downloads);
            return;
        }
        let p = self.primary_kind(gi);
        dispatch_primary(self, p, g.app_id, g.rungame_id());
    }

    fn game_button(&mut self, which: i32) {
        self.gm.zone = 0;
        self.gm.focus = which;
        self.game_btn(which);
        self.sync_game();
    }

    fn game_btn(&mut self, which: i32) {
        let Some(gi) = self.current_game() else { return };
        let id = self.games()[gi].app_id;
        match which {
            0 => self.primary_action(gi),
            // controller button: ControllerConfigurator.Main(appid)
            1 => self.act(Act::Controller(id)),
            _ => self.gmenu_open_with(false),
        }
    }

    pub fn handle_game(&mut self, a: Action) {
        if self.gm.mn.open {
            return self.gmenu_handle(a);
        }
        match a {
            // X = stop the running game (footer ЗАКРЫТЬ)
            Action::X => {
                if let Some(gi) = self.current_game() {
                    let id = self.games()[gi].app_id;
                    if self.is_running(id) {
                        self.act(Act::Stop(id));
                    }
                }
                return;
            }
            Action::Menu => return self.game_menu_open(),
            Action::B => {
                self.game_back();
                return;
            }
            Action::LB | Action::RB => {
                self.gm.zone = self.gm.zone.max(1);
                let d = if a == Action::LB { -1 } else { 1 };
                return self.game_set_tab(self.gm.tab + d);
            }
            _ => {}
        }
        match self.gm.zone {
            0 => match a {
                Action::Left => self.gm.focus = (self.gm.focus - 1).max(0),
                Action::Right => self.gm.focus = (self.gm.focus + 1).min(2),
                Action::Down => {
                    self.gm.zone = 1;
                    play(Snd::BasicNav);
                }
                Action::A => self.game_btn(self.gm.focus),
                _ => return,
            },
            1 => match a {
                Action::Left => return self.game_set_tab((self.gm.tab - 1).max(0)),
                Action::Right => return self.game_set_tab((self.gm.tab + 1).min(TABS.len() as i32 - 1)),
                Action::Up => {
                    self.gm.zone = 0;
                    self.gm.focus = 0;
                }
                Action::Down => {
                    if self.gm.rows.iter().any(|r| r.4.is_some()) {
                        self.gm.zone = 2;
                    }
                }
                _ => return,
            },
            _ => {
                let cur = self.gm.crow.max(0) as usize;
                match a {
                    Action::Up => match self.gm.rows[..cur].iter().rposition(|r| r.4.is_some()) {
                        Some(i) => self.gm.crow = i as i32,
                        None => self.gm.zone = 1,
                    },
                    Action::Down => {
                        if let Some(i) = self.gm.rows[cur + 1..].iter().position(|r| r.4.is_some()) {
                            self.gm.crow = (cur + 1 + i) as i32;
                        }
                    }
                    Action::A => self.game_row_action(cur),
                    _ => return,
                }
            }
        }
        self.sync_game();
    }

    // ------------------------------------------------------------------ plugin elements (read-only)
    /// HLTB bar from the Decky plugin's own cache (IndexedDB hltb-for-deck, key = appid) + PlayCount badge from the Steam web API.
    fn game_plugins(&mut self, id: u32, gi: usize) {
        let ui = self.ui();
        let st = ui.global::<GameState>();
        st.set_hltb(ModelRc::new(VecModel::from(Vec::<GHltbRow>::new())));
        st.set_badge("".into());
        self.gm.badge_timer.stop();
        self.gm.badge_id = id;
        if let Some(b) = crate::actions::bridge() {
            std::thread::spawn(move || {
                let js = format!(
                    "(async()=>{{try{{const db=await new Promise((res,rej)=>{{const r=indexedDB.open('hltb-for-deck');r.onsuccess=()=>res(r.result);r.onerror=()=>rej(r.error)}});const v=await new Promise(r=>{{const q=db.transaction('keyvaluepairs').objectStore('keyvaluepairs').get('{id}');q.onsuccess=()=>r(q.result);q.onerror=()=>r(null)}});db.close();return v||null}}catch(e){{return null}}}})()"
                );
                let Ok(v) = b.eval_json(&js) else { return };
                let rows: Vec<(String, String)> = [("mainStat", i18n::tr_ref("game.main_story")), ("mainPlusStat", i18n::tr_ref("game.plot_additional_assignments")), ("completeStat", i18n::tr_ref("game.complete_passage")), ("allStylesStat", i18n::tr_ref("game.all_styles"))]
                    .iter()
                    .filter(|_| v["showStats"].as_bool().unwrap_or(true))
                    .filter_map(|(k, l)| {
                        let s = v[*k].as_str()?.trim().to_string();
                        (!s.is_empty() && s != "--" && s != "0").then(|| (i18n::tr_args("game.hrs", &[("replace", &(crate::app::comma(s.to_owned())).to_string())]), (*l).to_string()))
                    })
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| {
                        if a.gm.badge_id == id {
                            let m: Vec<GHltbRow> = rows.into_iter().map(|(t, l)| GHltbRow { time: t.into(), label: l.into() }).collect();
                            a.ui().global::<GameState>().set_hltb(ModelRc::new(VecModel::from(m)));
                        }
                    });
                });
            });
        }
        if self.games()[gi].kind == GameKind::Steam {
            self.game_badge_fetch(id);
            self.gm.badge_timer.start(TimerMode::Repeated, Duration::from_secs(30), || {
                crate::with_app(|a| {
                    if let Route::App(id) = *a.router.current() {
                        a.game_badge_fetch(id);
                    }
                });
            });
        }
    }

    fn game_badge_fetch(&mut self, id: u32) {
        if self.gm.badge_at.map(|t| t.elapsed() < Duration::from_secs(20)).unwrap_or(false) && self.gm.badge_id == id {
            return;
        }
        self.gm.badge_at = Some(Instant::now());
        std::thread::spawn(move || {
            let url = format!("https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid={id}");
            let Ok(o) = std::process::Command::new("curl").args(["-s", "-m", "6", &url]).output() else { return };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&o.stdout) else { return };
            let Some(n) = v["response"]["player_count"].as_u64() else { return };
            let _ = slint::invoke_from_event_loop(move || {
                crate::with_app(|a| {
                    if a.gm.badge_id == id {
                        a.ui().global::<GameState>().set_badge(format!("{} Online", thousands(n)).into());
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod primary_tests {
    use super::*;

    #[derive(Default)]
    struct Target {
        running: Vec<u32>,
        resume_requests: Vec<u32>,
        resumes: Vec<u32>,
        launches: Vec<(u32, u64)>,
        installs: Vec<u32>,
    }

    impl PrimaryTarget for Target {
        fn resume(&mut self, id: u32) {
            self.resume_requests.push(id);
            if can_resume_running_game(&self.running, id) { self.resumes.push(id); }
        }
        fn launch(&mut self, id: u32, rungame: u64) { self.launches.push((id, rungame)); }
        fn install(&mut self, id: u32) { self.installs.push(id); }
    }

    #[test]
    fn resume_running_game_does_not_launch() {
        let mut target = Target { running: vec![42], ..Default::default() };
        assert!(can_resume_running_game(&target.running, 42));
        dispatch_primary(&mut target, Primary::Resume, 42, 420);
        assert_eq!(target.resumes, [42]);
        assert!(target.launches.is_empty());

        // A stale Resume selection must not fall back to launching a missing game.
        assert!(!can_resume_running_game(&[42], 99));
        assert!(!can_resume_running_game(&[], 42));
        dispatch_primary(&mut target, Primary::Resume, 99, 990);
        assert_eq!(target.resumes, [42]);
        assert!(target.launches.is_empty());

        target.running.clear();
        dispatch_primary(&mut target, Primary::Resume, 42, 420);
        assert_eq!(target.resume_requests, [42, 99, 42]);
        assert_eq!(target.resumes, [42]);
        assert!(target.launches.is_empty());

        dispatch_primary(&mut target, Primary::Play, 42, 420);
        assert_eq!(target.launches, [(42, 420)]);
        assert_eq!(target.resumes, [42]);
        assert!(target.installs.is_empty());
    }
}
