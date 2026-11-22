//! Steam main menu (STEAM button): Home, Library, Store, Friends, Media, Downloads, Settings, Power.
//! A on an item = history.push(route) + close menu (handlers.json "steam menu"); Power opens sleep/restart/shutdown.
use std::collections::HashMap;
use std::path::PathBuf;

use slint::{ComponentHandle, Image, ModelRc, VecModel};
use steam_control::SteamPage;

use crate::actions::Act;
use crate::app::App;
use crate::assets::{try_icon_image, t, t_or};
use crate::data::PowerAction;
use crate::input::Action;
use crate::nav::{self, FocusSource, Layout, NavEvent, NodeId, Props, Rect as NRect, Tree};
use crate::router::Route;
use crate::{AppWindow, MenuItem, SteamMenuState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAct {
    Home,
    Library,
    Store,
    Friends,
    Media,
    Downloads,
    Settings,
    Power,
    Sleep,
    Restart,
    Shutdown,
    Desktop,
    StockSteam,
    /// running game at the top of the menu (A resumes it)
    Running(u32),
    ExitRunning(u32),
}

#[derive(Default)]
pub struct State {
    pub open: bool,
    pub acts: Vec<MenuAct>,
    pub focus: usize,
    scroll_y: f32,
    pub game_column: bool,
    pub game_focus: usize,
    /// Power action waiting for the confirmation dialog (Steam: ConfirmModal with the item name as title) + focused button (0 OK, 1 cancel).
    pub confirm: Option<MenuAct>,
    pub confirm_focus: usize,
    icons: HashMap<&'static str, Image>,
    game_icons: HashMap<u32, (PathBuf, Option<Image>)>,
    game_icons_pending: HashMap<u32, PathBuf>,
    /// Steam focus engine for the menu column (private instance: the global tree belongs to the page behind).
    tree: Option<Tree>,
    ids: Vec<NodeId>,
}

#[derive(Debug, PartialEq, Eq)]
enum GameNav { Focus(usize), Back, Activate(usize), Ignore }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GameMenuAction { Resume, Details, Exit }
// Supported subset of sp.js:402460–402540, in its native order. Do not add
// Guides/Notes/Recording as fake buttons or route them to unrelated screens.
const GAME_MENU: [(GameMenuAction, &str, &str); 3] = [
    (GameMenuAction::Resume, "#AppOverlay_Resume", "Продолжить игру"),
    (GameMenuAction::Details, "#AppOverlay_GameDetails", "Информация об игре"),
    (GameMenuAction::Exit, "#AppOverlay_ExitGame", "Выйти из игры"),
];

fn game_nav(focus: usize, action: Action) -> GameNav {
    let last = GAME_MENU.len() - 1;
    let focus = focus.min(last);
    match action {
        Action::Up => GameNav::Focus(focus.saturating_sub(1)),
        Action::Down => GameNav::Focus((focus + 1).min(last)),
        Action::Left | Action::B => GameNav::Back,
        Action::A => GameNav::Activate(focus),
        _ => GameNav::Ignore,
    }
}

// Gaming Mode measurements at DPR 1.5: 360x681 column, 72px rows.
// With multiple running games we extend/scroll this layout; that case has not
// been measured in Steam and is not claimed as exact native parity.
const COLUMN_HEIGHT: f32 = 681.0;
const ROW_HEIGHT: f32 = 72.0;
fn row_y(i: usize, running: usize) -> f32 {
    if running == 0 { 52.5 + ROW_HEIGHT * i as f32 }
    else { 15.0 + ROW_HEIGHT * i as f32 + if i >= running { 3.0 } else { 0.0 } }
}
fn content_height(n: usize, running: usize) -> f32 {
    if n == 0 { COLUMN_HEIGHT }
    else { COLUMN_HEIGHT.max(row_y(n - 1, running) + ROW_HEIGHT + 15.0) }
}
fn focus_scroll(scroll: f32, i: usize, n: usize, running: usize) -> f32 {
    let top = row_y(i, running);
    let scroll = if top < scroll { top }
        else if top + ROW_HEIGHT > scroll + COLUMN_HEIGHT { top + ROW_HEIGHT - COLUMN_HEIGHT }
        else { scroll };
    scroll.clamp(0.0, (content_height(n, running) - COLUMN_HEIGHT).max(0.0))
}

fn menu_edge_wrap(focus: usize, count: usize, action: Action, is_repeat: bool) -> Option<usize> {
    if is_repeat || count == 0 { return None; }
    match action {
        Action::Up if focus == 0 => Some(count - 1),
        Action::Down if focus == count - 1 => Some(0),
        _ => None,
    }
}

fn clock_ms() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<SteamMenuState>();
    st.on_activate(|i| super::defer(move |a| {
        a.sm.focus = i as usize;
        a.menu_activate(i as usize);
    }));
    st.on_game_activate(|i| super::defer(move |a| a.menu_game_activate(i as usize)));
    st.on_game_hover(|i| super::defer(move |a| { a.sm.game_column = true; a.sm.game_focus = (i as usize).min(GAME_MENU.len() - 1); a.menu_sync_game(); }));
    st.on_user_scrolled(|y| super::defer(move |a| {
        let running = a.sm.acts.iter().take_while(|a| matches!(a, MenuAct::Running(_))).count();
        a.sm.scroll_y = y.clamp(0.0, (content_height(a.sm.acts.len(), running) - COLUMN_HEIGHT).max(0.0));
        a.ui().global::<SteamMenuState>().set_scroll_y(a.sm.scroll_y);
        a.menu_tree();
    }));
    st.on_close(|| super::defer(|a| a.close_overlays()));
    st.on_confirm_pick(|i| super::defer(move |a| a.menu_confirm_pick(i as usize)));
    // pointer hover moves the cursor like Steam's onMouseEnter focus
    st.on_hover(|i| super::defer(move |a| a.menu_set_focus(i as usize)));
}

impl App {
    fn running_game_icon(&mut self, id: u32) -> Option<Image> {
        let path = self.gidx.get(&id).and_then(|&gi| self.games()[gi].art.icon.clone())?;
        if let Some((cached_path, image)) = self.sm.game_icons.get(&id) {
            if *cached_path == path { return image.clone(); }
        }
        if self.sm.game_icons_pending.get(&id) == Some(&path) { return None; }
        self.sm.game_icons_pending.insert(id, path.clone());
        let ds = self.ds.clone();
        std::thread::spawn(move || {
            let pixels = ds.thumbnail(&path, 96, 96).and_then(|frame| {
                if frame.width == 0 || frame.height == 0 || frame.rgba.len() != frame.width as usize * frame.height as usize * 4 { return None; }
                Some(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&frame.rgba, frame.width, frame.height))
            });
            super::defer(move |app| {
                if app.sm.game_icons_pending.get(&id) != Some(&path) { return; }
                app.sm.game_icons_pending.remove(&id);
                let current = app.gidx.get(&id).and_then(|&gi| app.games()[gi].art.icon.as_ref());
                if current != Some(&path) { return; }
                app.sm.game_icons.insert(id, (path, pixels.map(Image::from_rgba8)));
                if app.sm.open { app.build_menu(); }
            });
        });
        None
    }

    fn menu_icon(&mut self, name: &'static str) -> Image {
        if let Some(icon) = self.sm.icons.get(name) { return icon.clone(); }
        let Some(icon) = try_icon_image(name) else { return Image::default() };
        self.sm.icons.insert(name, icon.clone());
        icon
    }

    pub(crate) fn menu_icons_ready(&mut self) {
        self.sm.icons.clear();
        if self.sm.open { self.build_menu(); }
    }

    pub fn build_menu(&mut self) {
        let mut v: Vec<(String, &'static str, MenuAct)> = vec![];
        // running games first (Steam: RunningApps block, A resumes)
        for &id in &self.live.running {
            if let Some(&gi) = self.gidx.get(&id) {
                v.push((self.games()[gi].name.clone(), "library", MenuAct::Running(id)));
            }
        }
        v.extend([
            (t_or("#MainTabsHome", "Главная"), "home", MenuAct::Home),
            (t_or("#MainTabsLibrary", "Библиотека"), "library", MenuAct::Library),
            (t_or("#MainTabsStore", "Магазин"), "store", MenuAct::Store),
            (t_or("#MainTabsChat", "Друзья и чат"), "friends", MenuAct::Friends),
            (t_or("#MainTabsMedia", "Медиа"), "media", MenuAct::Media),
            (t_or("#MainTabsDownload", "Загрузки"), "downloads", MenuAct::Downloads),
            (t_or("#MainTabsSettings", "Настройки"), "settings", MenuAct::Settings),
            (t_or("#Power", "Выключение"), "power", MenuAct::Power),
        ]);
        let route = self.router.current().clone();
        let mut items: Vec<MenuItem> = Vec::with_capacity(v.len());
        for (label, ic, a) in &v {
            let game_icon = match a { MenuAct::Running(id) => self.running_game_icon(*id), _ => None };
            let color_icon = game_icon.is_some();
            let icon = game_icon.unwrap_or_else(|| self.menu_icon(ic));
            items.push(MenuItem {
                label: label.as_str().into(),
                icon,
                color_icon,
                indent: false,
                current: match a {
                    MenuAct::Home => route == Route::Home,
                    MenuAct::Library => matches!(route, Route::Library(_) | Route::App(_) | Route::Search),
                    MenuAct::Downloads => route == Route::Downloads,
                    MenuAct::Settings => matches!(route, Route::Settings(_)),
                    MenuAct::Media => route == Route::Media,
                    _ => false,
                },
            });
        }
        self.sm.acts = v.into_iter().map(|x| x.2).collect();
        let ui = self.ui();
        let st = ui.global::<SteamMenuState>();
        st.set_running_count(self.live.running.iter().filter(|id| self.gidx.contains_key(id)).count() as i32);
        let running = self.sm.acts.iter().take_while(|a| matches!(a, MenuAct::Running(_))).count();
        st.set_row_y(ModelRc::new(VecModel::from((0..items.len()).map(|i| row_y(i, running)).collect::<Vec<_>>())));
        st.set_content_h(content_height(items.len(), running));
        st.set_items(ModelRc::new(VecModel::from(items)));
        self.sm.focus = self.sm.focus.min(self.sm.acts.len() - 1);
        st.set_focus(if self.pad_mode { self.sm.focus as i32 } else { -1 });
        self.menu_focus_visible();
        self.menu_tree();
        self.menu_sync_game();
    }

    fn menu_sync_game(&self) {
        let ui = self.ui();
        let st = ui.global::<SteamMenuState>();
        let running = matches!(self.sm.acts.get(self.sm.focus), Some(MenuAct::Running(_)));
        st.set_game_visible(self.sm.open && running);
        let title = match self.sm.acts.get(self.sm.focus) {
            Some(MenuAct::Running(id)) => self.gidx.get(id).map(|&gi| self.games()[gi].name.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        st.set_game_title(title.into());
        let logo = match self.sm.acts.get(self.sm.focus) {
            Some(MenuAct::Running(id)) => self.imgs.get(&(*id, crate::loader::Kind::Logo)).cloned(),
            _ => None,
        };
        st.set_game_has_logo(logo.is_some());
        st.set_game_logo(logo.unwrap_or_default());
        st.set_game_focus(if self.sm.game_column && running { self.sm.game_focus as i32 } else { -1 });
        st.set_game_items(ModelRc::new(VecModel::from(GAME_MENU.iter().map(|(_, token, fallback)| t_or(token, fallback).into()).collect::<Vec<_>>())));
    }

    fn menu_game_activate(&mut self, i: usize) {
        let Some(MenuAct::Running(id)) = self.sm.acts.get(self.sm.focus).copied() else { return };
        if !self.live.running.contains(&id) { return; }
        let Some(&(action, ..)) = GAME_MENU.get(i) else { return };
        match action {
            GameMenuAction::Resume => { self.resume_running_game(id); self.sm.open = false; self.sm.game_column = false; self.sync_overlays(); self.update_hint(); }
            GameMenuAction::Details => {
                if let Some(&gi) = self.gidx.get(&id) {
                    self.sm.game_column = false;
                    self.open_game(gi);
                }
            }
            GameMenuAction::Exit => self.menu_confirm_show(MenuAct::ExitRunning(id)),
        }
    }

    /// Column of focusable rows in the Steam focus engine (focus memory, FailedNav at the ends).
    fn menu_tree(&mut self) {
        let n = self.sm.acts.len();
        let mut t = Tree::new((1280.0, 800.0));
        let root = t.root();
        let running_count = self.sm.acts.iter().take_while(|a| matches!(a, MenuAct::Running(_))).count();
        let col = t.add_node(root, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(0.0, 60.0, 360.0, COLUMN_HEIGHT)));
        let ids: Vec<NodeId> = (0..n)
            .map(|i| {
                let rect = NRect::xywh(0.0, 60.0 + row_y(i, running_count) - self.sm.scroll_y, 360.0, ROW_HEIGHT);
                t.add_node(col, Props { focusable: true, nav_key: Some(format!("menu:{i}")), ..Default::default() }, Some(rect))
            })
            .collect();
        t.now_ms = clock_ms();
        t.transfer_focus(FocusSource::Application, ids.get(self.sm.focus).copied(), None);
        let _ = t.drain_events();
        self.sm.tree = Some(t);
        self.sm.ids = ids;
    }

    fn menu_focus_visible(&mut self) {
        let running = self.sm.acts.iter().take_while(|a| matches!(a, MenuAct::Running(_))).count();
        self.sm.scroll_y = focus_scroll(self.sm.scroll_y, self.sm.focus, self.sm.acts.len(), running);
        self.ui().global::<SteamMenuState>().set_scroll_y(self.sm.scroll_y);
    }

    pub fn menu_set_focus(&mut self, i: usize) {
        if i >= self.sm.acts.len() { return; }
        if i == self.sm.focus {
            if self.sm.game_column { self.sm.game_column = false; self.menu_sync_game(); }
            return;
        }
        self.sm.focus = i;
        self.sm.game_column = false;
        self.sm.game_focus = 0;
        self.menu_sync_game();
        if let (Some(t), Some(&id)) = (self.sm.tree.as_mut(), self.sm.ids.get(i)) {
            t.now_ms = clock_ms();
            t.transfer_focus(FocusSource::Application, Some(id), None);
            let _ = t.drain_events();
        }
        self.ui().global::<SteamMenuState>().set_focus(i as i32);
    }

    /// Drop an open confirmation dialog. Every surface that opens over it must call this:
    /// the dialog is the topmost input scope and would keep the pad while hidden behind the new surface.
    pub(crate) fn dismiss_confirm(&mut self) {
        if self.sm.confirm.take().is_some() {
            self.ui().global::<SteamMenuState>().set_confirm_open(false);
        }
    }

    pub fn close_overlays(&mut self) {
        self.dismiss_confirm();
        self.sm.open = false;
        self.sm.game_column = false;
        self.menu_sync_game();
        self.qm.open = false;
        self.lb.sort_open = false;
        self.close_search();
        if matches!(self.router.current(), Route::Search) {
            self.go_back();
        }
        self.sync_overlays();
        self.update_hint();
    }

    pub fn toggle_side(&mut self) {
        self.header_clear();
        self.dl_menu_reset();
        self.dismiss_confirm();
        self.sm.open = !self.sm.open;
        if self.sm.open {
            self.qm.open = false;
            self.lb.sort_open = false;
            self.sm.game_column = false;
            self.sm.game_focus = 0;
            // opens with the cursor on the current page's item (live reference)
            self.build_menu();
            let cur = self.ui().global::<SteamMenuState>().get_items();
            use slint::Model;
            self.sm.focus = if matches!(self.sm.acts.first(), Some(MenuAct::Running(_))) { 0 } else { (0..cur.row_count()).find(|&i| cur.row_data(i).map(|m| m.current).unwrap_or(false)).unwrap_or(0) };
            self.build_menu();
        }
        self.sync_overlays();
        self.update_hint();
    }

    /// Confirmation dialog of a destructive power item (Steam ConfirmModal: title = the item's name, OK / cancel).
    pub(crate) fn menu_confirm_show(&mut self, act: MenuAct) {
        if act == MenuAct::StockSteam && !self.live.running.is_empty() {
            self.toast(&t_or("#DeckShell_StockSteamRunning", "Сначала завершите игру: возврат к стандартному Steam перезапустит клиент"));
            return;
        }
        let (tok, fb) = match act {
            MenuAct::ExitRunning(_) => ("#GameAction_ConfirmExitGameTitle", "Выйти из игры?"),
            MenuAct::Shutdown => ("#Quit_Shutdown", "Выключить"),
            MenuAct::Restart => ("#Quit_Restart", "Перезагрузка"),
            MenuAct::StockSteam => ("#DeckShell_StockSteam", "Вернуть стандартный Steam"),
            _ => ("#SwitchToDesktop", "На рабочий стол"),
        };
        self.sm.confirm = Some(act);
        self.sm.confirm_focus = 0;
        let ui = self.ui();
        let st = ui.global::<SteamMenuState>();
        st.set_confirm_title(t_or(tok, fb).into());
        let exit = matches!(act, MenuAct::ExitRunning(_));
        st.set_confirm_exit(exit);
        st.set_confirm_description(if exit {
            t_or("#AppOverlay_UnsavedDataWarning", "Несохранённые данные могут быть утеряны.").into()
        } else if act == MenuAct::StockSteam {
            t_or("#DeckShell_StockSteamDescription", "DeckShell закроется, клиент Steam перезапустится со стандартным интерфейсом. Игры и сохранения не удаляются.").into()
        } else { "".into() });
        st.set_confirm_ok(if exit { t_or("#Button_Confirm", "Подтвердить").into() } else { t_or("#Button_OK", "ОК").into() });
        st.set_confirm_cancel(t_or("#Button_Cancel", "Отмена").into());
        st.set_confirm_focus(0);
        st.set_confirm_open(true);
        crate::sound::play(crate::sound::Snd::ShowModal);
        self.update_hint();
    }

    fn menu_confirm_close(&mut self) {
        self.sm.confirm = None;
        self.ui().global::<SteamMenuState>().set_confirm_open(false);
        crate::sound::play(crate::sound::Snd::HideModal);
        // A dialog over a game gives the game back when nothing else is open.
        self.sync_overlays();
        self.update_hint();
    }

    /// OK (0) or cancel (1) in the confirmation dialog.
    pub fn menu_confirm_pick(&mut self, i: usize) {
        let Some(act) = self.sm.confirm else { return };
        self.menu_confirm_close();
        if i != 0 { return; }
        // Recheck after the dialog: a game could have started while it was open.
        if act == MenuAct::StockSteam && !self.live.running.is_empty() {
            self.toast(&t_or("#DeckShell_StockSteamRunning", "Сначала завершите игру: возврат к стандартному Steam перезапустит клиент"));
            return;
        }
        match act {
            MenuAct::ExitRunning(id) => self.exit_running_game(id),
            MenuAct::Shutdown => self.act(Act::Power(PowerAction::Shutdown)),
            MenuAct::Restart => self.act(Act::Power(PowerAction::Restart)),
            MenuAct::Desktop => self.act(Act::Power(PowerAction::Desktop)),
            MenuAct::StockSteam => self.act(Act::Power(PowerAction::StockSteam)),
            _ => {}
        }
        self.sm.open = false;
        self.sync_overlays();
        self.update_hint();
    }

    pub fn handle_side(&mut self, a: Action, is_repeat: bool) {
        if self.sm.confirm.is_some() {
            match a {
                Action::Left | Action::Right | Action::Up | Action::Down => {
                    self.sm.confirm_focus = 1 - self.sm.confirm_focus.min(1);
                    self.ui().global::<SteamMenuState>().set_confirm_focus(self.sm.confirm_focus as i32);
                }
                Action::A => self.menu_confirm_pick(self.sm.confirm_focus),
                Action::B => self.menu_confirm_pick(1),
                _ => {}
            }
            return;
        }
        if self.dm.is_open() {
            self.handle_dl_menu(a);
            return;
        }
        if self.sm.game_column {
            match game_nav(self.sm.game_focus, a) {
                GameNav::Focus(i) => self.sm.game_focus = i,
                GameNav::Back => self.sm.game_column = false,
                GameNav::Activate(i) => { self.menu_game_activate(i); return; }
                GameNav::Ignore => return,
            }
            self.menu_sync_game();
            return;
        }
        match a {
            Action::Up | Action::Down => {
                // engine move (Column layout); unhandled at the ends -> app.rs plays FailedNav (focus unchanged)
                let Some(b) = nav::button_of(a) else { return };
                let mut to = None;
                if let Some(t) = self.sm.tree.as_mut() {
                    t.now_ms = clock_ms();
                    t.handle_button(b, is_repeat);
                    for ev in t.drain_events() {
                        if let NavEvent::Focus { to: Some(n), .. } = ev {
                            to = self.sm.ids.iter().position(|&x| x == n);
                        }
                    }
                } else {
                    to = match a {
                        Action::Up => self.sm.focus.checked_sub(1),
                        Action::Down if self.sm.focus + 1 < self.sm.acts.len() => Some(self.sm.focus + 1),
                        _ => None,
                    };
                }
                // Steam wraps only a fresh directional press at a column edge.
                if to.is_none() {
                    to = menu_edge_wrap(self.sm.focus, self.sm.acts.len(), a, is_repeat);
                }
                if let Some(i) = to {
                    self.sm.focus = i;
                    self.sm.game_focus = 0;
                    self.menu_focus_visible();
                    self.menu_tree();
                    self.menu_sync_game();
                }
            }
            Action::B | Action::Left => {
                self.sm.open = false;
                self.sync_overlays();
                self.update_hint();
                return;
            }
            Action::A | Action::Right => {
                self.menu_activate(self.sm.focus);
                return;
            }
            _ => return,
        }
        self.ui().global::<SteamMenuState>().set_focus(if self.pad_mode { self.sm.focus as i32 } else { -1 });
    }

    pub fn menu_activate(&mut self, i: usize) {
        let Some(&act) = self.sm.acts.get(i) else { return };
        match act {
            MenuAct::Power => {
                self.hk_power_from_menu();
                return;
            }
            MenuAct::Home => self.navigate(Route::Home),
            MenuAct::Library => {
                let id = self.lb.tab_id();
                self.navigate(Route::Library(id))
            }
            // Opens the downloads panel next to the rail; the full page is a row inside it.
            MenuAct::Downloads => {
                self.sm.focus = i;
                self.dl_menu_open();
                return;
            }
            MenuAct::Media => self.navigate(Route::Media),
            MenuAct::Settings => {
                let p = self.se.page_id();
                self.navigate(Route::Settings(p))
            }
            // delegated to native Steam windows (routes.json "delegated"): Store, Friends & chat
            // /steamweb is an in-Steam web page; DeckShell has no web view, so the store is handed to Steam's own window
            MenuAct::Store => {
                self.act(Act::Open(SteamPage::Store));
                self.toast(&format!("{}: {}", t_or("#DeckShell_OpenedInSteam", "Открыто в Steam"), t("#MainTabsStore")));
            }
            // Steam opens Friends with an expanded conversation area from the main menu.
            MenuAct::Friends => {
                self.sm.open = false;
                self.open_qam_tab(1);
                self.ui().global::<crate::QamState>().set_expanded(true);
                self.qam_refresh_expanded_title();
                return;
            }
            MenuAct::Running(_) => {
                self.sm.focus = i;
                self.sm.game_column = true;
                self.sm.game_focus = 0;
                self.menu_sync_game();
                return;
            }
            MenuAct::ExitRunning(_) => return,
            // Sleep is immediate in Steam; Shutdown / Restart / Switch to desktop ask first
            MenuAct::Sleep => self.act(Act::Power(PowerAction::Sleep)),
            MenuAct::Restart | MenuAct::Shutdown | MenuAct::Desktop | MenuAct::StockSteam => {
                self.menu_confirm_show(act);
                return;
            }
        }
        self.sm.open = false;
        self.sync_overlays();
        self.update_hint();
    }
}

#[cfg(test)]
mod game_nav_tests {
    use super::*;
    // Implementation regression table. Steam's exact lateral/boundary rules are not yet
    // established (context/reference/steamui/game-submenu.md); do not call this native parity.
    #[test]
    fn game_submenu_focus_graph() {
        use Action::{Up, Down, Left, Right, A, B};
        let buttons = [Up, Down, Left, Right, A, B];
        let graph = [
            [GameNav::Focus(0), GameNav::Focus(1), GameNav::Back, GameNav::Ignore, GameNav::Activate(0), GameNav::Back],
            [GameNav::Focus(0), GameNav::Focus(2), GameNav::Back, GameNav::Ignore, GameNav::Activate(1), GameNav::Back],
            [GameNav::Focus(1), GameNav::Focus(2), GameNav::Back, GameNav::Ignore, GameNav::Activate(2), GameNav::Back],
        ];
        for (node, targets) in graph.into_iter().enumerate() {
            for (button, target) in buttons.into_iter().zip(targets) {
                assert_eq!(game_nav(node, button), target, "node {node}, {button:?}");
            }
        }
    }

    #[test]
    fn game_column_stops_at_ends_and_back_does_not_activate() {
        assert_eq!(game_nav(0, Action::Up), GameNav::Focus(0));
        assert_eq!(game_nav(0, Action::Down), GameNav::Focus(1));
        assert_eq!(game_nav(1, Action::Down), GameNav::Focus(2));
        assert_eq!(game_nav(2, Action::Down), GameNav::Focus(2));
        assert_eq!(game_nav(2, Action::A), GameNav::Activate(2));
        assert_eq!(game_nav(1, Action::B), GameNav::Back);
        assert_eq!(game_nav(0, Action::Left), GameNav::Back);
    }

    #[test]
    fn every_visible_game_action_is_reachable_in_native_relative_order() {
        let mut cursor = 0;
        let mut seen = Vec::new();
        loop {
            let GameNav::Activate(i) = game_nav(cursor, Action::A) else { panic!("A must select the row") };
            seen.push(GAME_MENU[i].0);
            let GameNav::Focus(next) = game_nav(cursor, Action::Down) else { panic!("Down must move or stop") };
            if next == cursor { break; }
            cursor = next;
        }
        assert_eq!(seen, vec![GameMenuAction::Resume, GameMenuAction::Details, GameMenuAction::Exit]);
        assert_eq!(game_nav(usize::MAX, Action::A), GameNav::Activate(2));
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;
    #[test]
    fn fresh_edge_press_wraps_but_repeat_does_not() {
        assert_eq!(menu_edge_wrap(0, 8, Action::Up, false), Some(7));
        assert_eq!(menu_edge_wrap(7, 8, Action::Down, false), Some(0));
        assert_eq!(menu_edge_wrap(0, 8, Action::Up, true), None);
        assert_eq!(menu_edge_wrap(7, 8, Action::Down, true), None);
        assert_eq!(menu_edge_wrap(3, 8, Action::Down, false), None);
        assert_eq!(menu_edge_wrap(0, 0, Action::Up, false), None);
    }
    #[test]
    fn native_zero_and_one_game_positions_stay_measured() {
        assert_eq!(60.0 + row_y(0, 0), 112.5);
        assert_eq!(60.0 + row_y(7, 0), 616.5);
        assert_eq!(60.0 + row_y(0, 1), 75.0);
        assert_eq!(60.0 + row_y(1, 1), 150.0);
        assert_eq!(content_height(9, 1), COLUMN_HEIGHT);
    }
    #[test]
    fn two_running_games_do_not_overlap_home() {
        assert!(row_y(0, 2) + ROW_HEIGHT <= row_y(1, 2));
        assert!(row_y(1, 2) + ROW_HEIGHT <= row_y(2, 2));
        assert_eq!(60.0 + row_y(2, 2), 222.0);
    }
    #[test]
    fn focus_reveals_power_and_returns_to_first_game() {
        let scroll = focus_scroll(0.0, 9, 10, 2);
        assert!(scroll > 0.0);
        assert!(row_y(9, 2) >= scroll);
        assert!(row_y(9, 2) + ROW_HEIGHT <= scroll + COLUMN_HEIGHT);
        assert_eq!(focus_scroll(scroll, 0, 10, 2), row_y(0, 2));
        assert_eq!(focus_scroll(9000.0, 0, 8, 0), 0.0);
    }
}

#[cfg(test)]
mod composition_tests {
    use super::*;
    use slint::platform::{Platform, PlatformError, WindowAdapter, software_renderer::{MinimalSoftwareWindow, RepaintBufferType, PremultipliedRgbaColor, TargetPixel}};
    use std::{cell::Cell, rc::Rc, time::Duration};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Rgb8Pixel { r: u8, g: u8, b: u8, a: u8 }
    impl TargetPixel for Rgb8Pixel {
        fn blend(&mut self, c: PremultipliedRgbaColor) {
            let inv = 255 - c.alpha as u32;
            self.r = (c.red as u32 + self.r as u32 * inv / 255).min(255) as u8;
            self.g = (c.green as u32 + self.g as u32 * inv / 255).min(255) as u8;
            self.b = (c.blue as u32 + self.b as u32 * inv / 255).min(255) as u8;
            self.a = (c.alpha as u32 + self.a as u32 * inv / 255).min(255) as u8;
        }
        fn from_rgb(r: u8, g: u8, b: u8) -> Self { Self { r, g, b, a: 255 } }
    }

    struct TestPlatform { window: Rc<MinimalSoftwareWindow>, time: Rc<Cell<u64>> }
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> { Ok(self.window.clone()) }
        fn duration_since_start(&self) -> Duration { Duration::from_millis(self.time.get()) }
    }

    #[test]
    fn side_menu_composition_regression() {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        let time = Rc::new(Cell::new(0));
        slint::platform::set_platform(Box::new(TestPlatform { window: window.clone(), time: time.clone() })).unwrap();
        window.set_size(slint::PhysicalSize::new(1280, 800));
        let ui = AppWindow::new().unwrap();
        // Keep the normal page behind the menu so dimming and bar backing are observable.
        ui.global::<crate::Shell>().set_game_overlay(false);
        let state = ui.global::<SteamMenuState>();
        let mut red = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(8, 8);
        red.make_mut_slice().fill(slint::Rgba8Pixel { r: 240, g: 20, b: 20, a: 255 });
        state.set_items(ModelRc::new(VecModel::from(vec![MenuItem { label: "Game".into(), icon: Image::from_rgba8(red), color_icon: true, indent: false, current: false }])));
        state.set_row_y(ModelRc::new(VecModel::from(vec![15.0])));
        state.set_content_h(680.0);
        state.set_focus(0);
        state.set_game_visible(true);
        state.set_game_items(ModelRc::new(VecModel::from(vec!["Resume".into(), "Details".into()])));
        state.set_game_focus(-1);
        ui.show().unwrap();
        let render = || {
            // Render the composition without creating a desktop window.
            let mut pixels = vec![Rgb8Pixel { r: 0, g: 0, b: 0, a: 0 }; 1280 * 800];
            window.request_redraw();
            window.draw_if_needed(|r| { r.render(&mut pixels, 1280); });
            pixels
        };
        let save = |name: &str, pixels: &[Rgb8Pixel]| {
            if let Ok(dir) = std::env::var("DECKSHELL_PANEL_RENDER_DIR") {
                let bytes: Vec<u8> = pixels.iter().flat_map(|p| {
                    let inv = 255 - p.a as u16;
                    [(p.r as u16 + 180 * inv / 255).min(255) as u8, (p.g as u16 + 140 * inv / 255).min(255) as u8, (p.b as u16 + 100 * inv / 255).min(255) as u8]
                }).collect();
                image::save_buffer(format!("{dir}/{name}.png"), &bytes, 1280, 800, image::ColorType::Rgb8).unwrap();
            }
        };
        let closed = render();
        state.set_open(true);
        render();
        time.set(1000); slint::platform::update_timers_and_animations();
        let open = render();
        save("open", &open);
        state.set_running_count(1);
        let with_separator = render();
        assert_ne!(open[148 * 1280 + 180], with_separator[148 * 1280 + 180], "running-app separator appears beneath the row");
        state.set_running_count(0);
        assert!(open.iter().any(|p| p.r > 200 && p.g < 50 && p.b < 50), "game artwork retains its color");
        assert_ne!(closed[200 * 1280 + 400], open[200 * 1280 + 400], "game panel appears");
        assert_ne!(closed[10 * 1280 + 600], open[10 * 1280 + 600], "header backing covers the page");
        assert_ne!(closed[200 * 1280 + 1000], open[200 * 1280 + 1000], "dimmer covers the page");
        assert_eq!(open[200 * 1280 + 400], open[200 * 1280 + 700], "opaque game surface");
        state.set_game_focus(0);
        render();
        time.set(2000); slint::platform::update_timers_and_animations();
        let sub = render();
        save("submenu", &sub);
        assert_eq!(sub[200 * 1280 + 100], open[200 * 1280 + 400], "game surface shifts behind the rail");
        assert_ne!(open[100 * 1280 + 10], sub[100 * 1280 + 10], "main-column focus is cleared");
        assert_ne!(open[280 * 1280 + 375], sub[280 * 1280 + 81], "game row receives focus");
        assert_ne!(sub[200 * 1280 + 600], sub[200 * 1280 + 100], "game surface ends at rail + 456");
        assert_eq!(sub[10 * 1280 + 600], Rgb8Pixel { r: 0, g: 0, b: 0, a: 255 });
        assert_eq!(sub[790 * 1280 + 600], Rgb8Pixel { r: 0, g: 0, b: 0, a: 255 });
        state.set_open(false);
        let qam = ui.global::<crate::QamState>();
        qam.set_title("Быстрые настройки".into());
        qam.set_open(true);
        render(); time.set(3000); slint::platform::update_timers_and_animations();
        let quick = render();
        save("qam", &quick);
        assert_eq!(quick[10 * 1280 + 600], Rgb8Pixel { r: 0, g: 0, b: 0, a: 255 });
        assert_ne!(quick[200 * 1280 + 1100], closed[200 * 1280 + 1100], "QAM panel appears");
    }
}
