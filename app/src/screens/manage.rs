//! The game page's gear / START context menu (sp.js AppActionsMenu `uU`, render at 498930, Manage submenu `BuildManageSubmenu` at 498880):
//!   [primary action when opened with START]  Избранное  Добавить в... ▸  [Убрать из... ▸]  Управление ▸  Сменить картинку... (SteamGridDB)  Свойства...  Отмена
//! Управление ▸ (gamepad build): Скрыть игру / Убрать из скрытого, Ключи активации (when the app has legacy keys), Удалить стороннюю игру из
//! библиотеки (non-Steam), Удалить с устройства (confirm dialog). "Browse local files", backup and move live on the Properties "Installed files" page
//! (Steam hides them in the gamepad menu). Writes: collectionStore.SetAppsAsFavorite / SetAppsAsHidden / collection.AddApps / RemoveApps,
//! SteamClient.Apps.RemoveShortcut (+ removal from user collections after 500 ms like sp.js:498507), Act::Uninstall.
//! All writes go through `setters::write` (DRYRUN-gated); the local view is patched at once.
use serde_json::Value;
use steam_data::GameKind;

use crate::actions::Act;
use crate::app::App;
use crate::assets::t_or;
use crate::input::Action;
use crate::setters::{self, js_str};
use crate::sound::{play, Snd};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sub {
    AddTo,
    RemoveFrom,
    Manage,
}

#[derive(Clone, PartialEq, Debug)]
pub enum MAct {
    Primary,
    Store,
    Achievements,
    ToggleFav,
    Sub(Sub),
    Artwork,
    Properties,
    Cancel,
    AddTo(String),
    RemoveFrom(String),
    NewCollection,
    Hide(bool),
    CdKeys,
    RemoveShortcut,
    Uninstall,
    /// Family View: block-list state to set (`SetAppBlockedState(apps, blocked)`)
    Family(bool),
    Private(bool),
    RemoveLicense,
    Loading,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub act: MAct,
    pub sub: bool,
    pub dis: bool,
}

#[derive(Default)]
struct Level {
    title: String,
    items: Vec<Item>,
    idx: i32,
}

/// (collection id, name, contains the app)
type Coll = (String, String, bool);

#[derive(Default)]
pub struct Menu {
    pub open: bool,
    levels: Vec<Level>,
    colls: Vec<Coll>,
    loaded: bool,
    hidden: bool,
    fav: Option<bool>,
    cdkeys: bool,
    /// (Family View active and unlocked, app in block list); None when the parental store is not reachable
    fam: Option<(bool, bool)>,
    with_primary: bool,
}

fn it(label: String, act: MAct) -> Item {
    Item { label, act, sub: false, dis: false }
}
fn sub(label: String, s: Sub) -> Item {
    Item { label, act: MAct::Sub(s), sub: true, dis: false }
}

impl App {
    fn mn_game(&self) -> Option<(u32, usize)> {
        let gi = self.current_game()?;
        Some((self.games()[gi].app_id, gi))
    }

    fn mn_top(&self, gi: usize) -> Vec<Item> {
        let g = &self.games()[gi];
        let steam = g.kind == GameKind::Steam;
        let m = &self.gm.mn;
        let mut v = vec![];
        if m.with_primary {
            v.push(it(self.menu_primary_label(gi), MAct::Primary));
            if steam {
                v.push(it(t_or("#AppDetails_StorePage", "Страница в магазине"), MAct::Store));
                v.push(it(t_or("#AppDetails_ViewAchievements", "Достижения"), MAct::Achievements));
            }
        }
        let fav = m.fav.unwrap_or_else(|| Self::is_favorite(g));
        v.push(it(
            if fav { t_or("#GameAction_RemoveFromFavorites", "Убрать из избранного") } else { t_or("#GameAction_AddToFavorites", "Добавить в избранное") },
            MAct::ToggleFav,
        ));
        v.push(sub(t_or("#GameAction_AddToCollection", "Добавить в..."), Sub::AddTo));
        if m.colls.iter().any(|c| c.2) {
            v.push(sub(t_or("#GameAction_RemoveFromCollection", "Убрать из..."), Sub::RemoveFrom));
        }
        v.push(sub(t_or("#Button_Manage", "Управление"), Sub::Manage));
        v.push(it(t_or("#GameAction_ChangeArt", "Сменить картинку..."), MAct::Artwork));
        v.push(it(t_or("#GameAction_GameProperties", "Свойства..."), MAct::Properties));
        v.push(it(t_or("#GameAction_Cancel", "Отмена"), MAct::Cancel));
        v
    }

    fn mn_sub(&self, gi: usize, s: Sub) -> (String, Vec<Item>) {
        let g = &self.games()[gi];
        let m = &self.gm.mn;
        match s {
            Sub::AddTo => {
                let mut v: Vec<Item> = m.colls.iter().map(|c| Item { label: c.1.to_uppercase(), act: MAct::AddTo(c.0.clone()), sub: false, dis: c.2 }).collect();
                if !m.loaded {
                    v.push(Item { label: "...".into(), act: MAct::Loading, sub: false, dis: true });
                }
                v.push(it(t_or("#GameAction_AddToCollectionOption_NewCollection", "Создать коллекцию..."), MAct::NewCollection));
                (t_or("#GameAction_AddToCollection", "Добавить в..."), v)
            }
            Sub::RemoveFrom => {
                let v = m.colls.iter().filter(|c| c.2).map(|c| it(c.1.to_uppercase(), MAct::RemoveFrom(c.0.clone()))).collect();
                (t_or("#GameAction_RemoveFromCollection", "Убрать из..."), v)
            }
            Sub::Manage => {
                use super::manage_rules::{allowed, entries, Flags, ManageEntry as E, AT_GAME, AT_SHORTCUT};
                let ex = self.extras.get(&g.app_id);
                let at = match g.kind {
                    GameKind::Shortcut => AT_SHORTCUT,
                    _ => ex.map(|e| e.app_type).filter(|t| *t != 0).unwrap_or(AT_GAME),
                };
                let ds = ex.map(|e| e.display_status).unwrap_or(if g.installed { 11 } else { 0 });
                let mut a = allowed(at, ds, g.installed, false);
                a.uninstall &= self.can_uninstall(gi);
                let v = entries(
                    Flags {
                        hidden: m.hidden,
                        cdkeys: m.cdkeys,
                        family_active: m.fam.map(|f| f.0).unwrap_or(false),
                        in_block_list: m.fam.map(|f| f.1).unwrap_or(false),
                        mod_or_shortcut: g.kind == GameKind::Shortcut,
                        // private-apps data and CancelLicenseForApp live behind the CM transport (not reachable from page JS): see SPEC "Verify live"
                        private: None,
                        removable_license: false,
                    },
                    a,
                )
                    .into_iter()
                    .map(|e| match e {
                        E::RemoveFromHidden => it(t_or("#GameAction_RemoveFromHidden", "Убрать из скрытого"), MAct::Hide(false)),
                        E::AddToFamilyGames => it(t_or("#GameAction_AddToFamilyGames", "Добавить в игры для семьи"), MAct::Family(false)),
                        E::RemoveFromFamilyGames => it(t_or("#GameAction_RemoveFromFamilyGames", "Убрать из игр для семьи"), MAct::Family(true)),
                        E::MarkPrivate { unmark, disabled } => {
                            let mut i = it(t_or(if unmark { "#GameAction_UnmarkAsPrivate" } else { "#GameAction_MarkAsPrivate" }, if unmark { "Показывать всем" } else { "Сделать приватной" }), MAct::Private(!unmark));
                            i.dis = disabled;
                            i
                        }
                        E::RemoveLicense => it(t_or("#GameAction_RemoveGameLicense", "Удалить с аккаунта"), MAct::RemoveLicense),
                        E::CdKeys => it(t_or("#GameAction_ViewCDKeys", "Ключи активации"), MAct::CdKeys),
                        E::AddToHidden => it(t_or("#GameAction_AddToHidden", "Скрыть игру"), MAct::Hide(true)),
                        E::RemoveShortcut => it(t_or("#GameAction_RemoveShortcut", "Удалить стороннюю игру из библиотеки"), MAct::RemoveShortcut),
                        E::Uninstall => it(t_or("#GameAction_Uninstall", "Удалить с устройства"), MAct::Uninstall),
                    })
                    .collect();
                (t_or("#Button_Manage", "Управление"), v)
            }
        }
    }

    /// START (with the primary action) / gear (without) on the game page.
    pub fn gmenu_open_with(&mut self, with_primary: bool) {
        let Some((id, gi)) = self.mn_game() else { return };
        self.gm.mn = Menu { open: true, with_primary, ..Default::default() };
        let top = self.mn_top(gi);
        self.gm.mn.levels = vec![Level { title: self.games()[gi].name.clone(), items: top, idx: 0 }];
        let js = format!(
            "(()=>{{const id={id};const c=collectionStore;return {{colls:c.userCollections.filter(x=>x.bAllowsDragAndDrop&&!c.BIsSystemCollectionId(x.id)&&!c.BIsPartnerCollectionId(x.id)).map(x=>[x.id,x.displayName,x.apps.has(id)]),hidden:!!c.BIsHidden(id),fav:!!c.BIsFavorite(id),cdkeys:!!appDetailsStore.GetAppDetails(id)?.bShowCDKeyInMenus,fam:(()=>{{const p=window.parentalStore||window.g_ParentalStore;const o=appStore.GetAppOverviewByAppID(id);return p&&o?[!!p.isEnabled&&!p.isParentalLocked,!!o.BIsAppInBlockList()]:null}})()}}}})()"
        );
        setters::read(js, move |a, v: Value| {
            if !a.gm.mn.open || v.is_null() {
                return;
            }
            let m = &mut a.gm.mn;
            m.colls = v["colls"]
                .as_array()
                .map(|x| x.iter().map(|c| (c[0].as_str().unwrap_or("").to_string(), c[1].as_str().unwrap_or("").to_string(), c[2].as_bool().unwrap_or(false))).collect())
                .unwrap_or_default();
            m.loaded = true;
            m.hidden = v["hidden"].as_bool().unwrap_or(false);
            m.fav = v["fav"].as_bool();
            m.cdkeys = v["cdkeys"].as_bool().unwrap_or(false);
            m.fam = v["fam"].as_array().map(|f| (f[0].as_bool().unwrap_or(false), f[1].as_bool().unwrap_or(false)));
            a.gmenu_rebuild_levels();
            a.sync_game();
        });
        self.sync_game();
    }

    /// Library tile menu "Add to…": game page + gear menu, already inside the collection submenu.
    pub fn gmenu_open_add_to(&mut self, gi: usize) {
        self.open_game(gi);
        self.gmenu_open_with(true);
        let Some((_, gi)) = self.mn_game() else { return };
        let top = self.mn_top(gi);
        let Some(pos) = top.iter().position(|i| matches!(i.act, MAct::Sub(Sub::AddTo))) else { return };
        let (title, items) = self.mn_sub(gi, Sub::AddTo);
        self.gm.mn.levels[0].idx = pos as i32;
        self.gm.mn.levels.push(Level { title, items, idx: 0 });
        self.sync_game();
    }

    /// Re-derive the visible levels after the collection / hidden state changed (keeps the cursor).
    fn gmenu_rebuild_levels(&mut self) {
        let Some((_, gi)) = self.mn_game() else { return };
        let depth = self.gm.mn.levels.len();
        let idx: Vec<i32> = self.gm.mn.levels.iter().map(|l| l.idx).collect();
        let kinds: Vec<Option<Sub>> = (1..depth).map(|d| self.gm.mn.levels[d - 1].items.get(idx[d - 1] as usize).and_then(|i| if let MAct::Sub(s) = i.act { Some(s) } else { None })).collect();
        let mut levels = vec![Level { title: self.games()[gi].name.clone(), items: self.mn_top(gi), idx: idx[0] }];
        for (d, k) in kinds.into_iter().enumerate() {
            let Some(s) = k else { break };
            let (title, items) = self.mn_sub(gi, s);
            levels.push(Level { title, idx: idx[d + 1].min((items.len() as i32 - 1).max(0)), items });
        }
        self.gm.mn.levels = levels;
    }

    /// Control-socket `manage <appid>`: open the game page + gear menu and list every level (top, Add to, Remove from, Manage).
    pub fn manage_dump(&mut self, id: u32) -> String {
        let Some(&gi) = self.gidx.get(&id) else { return "err appid not in view".into() };
        self.open_game(gi);
        self.gmenu_open_with(true);
        let mut out = vec![];
        for (name, lvl) in [("top", None), ("add", Some(Sub::AddTo)), ("remove", Some(Sub::RemoveFrom)), ("manage", Some(Sub::Manage))] {
            let (title, items) = match lvl {
                None => (self.games()[gi].name.clone(), self.mn_top(gi)),
                Some(s) => self.mn_sub(gi, s),
            };
            let list: Vec<String> = items.iter().map(|i| format!("{}{}{}", i.label, if i.sub { " >" } else { "" }, if i.dis { " (disabled)" } else { "" })).collect();
            out.push(format!("{name} [{title}]: {}", list.join(" | ")));
        }
        self.gmenu_close();
        out.join("\n")
    }

    pub fn gmenu_close(&mut self) {
        self.gm.mn.open = false;
        self.sync_game();
    }

    /// Menu level / row data for Slint: (title, labels, has-submenu, disabled, idx).
    pub fn gmenu_view(&self) -> (String, Vec<String>, Vec<bool>, Vec<bool>, i32) {
        match self.gm.mn.levels.last() {
            Some(l) => (l.title.clone(), l.items.iter().map(|i| i.label.clone()).collect(), l.items.iter().map(|i| i.sub).collect(), l.items.iter().map(|i| i.dis).collect(), l.idx),
            None => Default::default(),
        }
    }

    pub fn gmenu_set_idx(&mut self, i: i32) {
        if let Some(l) = self.gm.mn.levels.last_mut() {
            l.idx = i.clamp(0, (l.items.len() as i32 - 1).max(0));
        }
    }

    /// B / back arrow: one level up, or close.
    pub fn gmenu_back(&mut self) {
        if self.gm.mn.levels.len() > 1 {
            self.gm.mn.levels.pop();
        } else {
            self.gm.mn.open = false;
        }
        self.sync_game();
    }

    pub fn gmenu_handle(&mut self, a: Action) {
        match a {
            Action::Up | Action::Down => {
                if let Some(l) = self.gm.mn.levels.last_mut() {
                    let n = l.items.len() as i32;
                    let d = if a == Action::Up { -1 } else { 1 };
                    let mut i = l.idx;
                    // skip disabled rows
                    loop {
                        let j = i + d;
                        if j < 0 || j >= n {
                            break;
                        }
                        i = j;
                        if !l.items[j as usize].dis {
                            l.idx = j;
                            break;
                        }
                    }
                }
            }
            Action::Left | Action::B => return self.gmenu_back_or_close(a),
            Action::Menu => self.gm.mn.open = false,
            Action::A | Action::Right => return self.gmenu_activate(),
            _ => return,
        }
        self.sync_game();
    }

    fn gmenu_back_or_close(&mut self, a: Action) {
        if a == Action::Left && self.gm.mn.levels.len() <= 1 {
            return;
        }
        self.gmenu_back();
    }

    pub fn gmenu_activate(&mut self) {
        let Some((id, gi)) = self.mn_game() else { return };
        let Some(item) = self.gm.mn.levels.last().and_then(|l| l.items.get(l.idx as usize)).cloned() else { return };
        if item.dis {
            return;
        }
        let close = |a: &mut App| {
            a.gm.mn.open = false;
            a.sync_game();
        };
        match item.act {
            MAct::Sub(s) => {
                let (title, items) = self.mn_sub(gi, s);
                self.gm.mn.levels.push(Level { title, items, idx: 0 });
                play(Snd::BasicNav);
                self.sync_game();
            }
            MAct::Primary => {
                close(self);
                self.primary_action(gi);
            }
            MAct::Store => {
                close(self);
                self.act(Act::OpenStore(id));
            }
            MAct::Achievements => {
                close(self);
                self.act(Act::Achievements(id));
            }
            MAct::ToggleFav => {
                close(self);
                let fav = self.gm.mn.fav.unwrap_or_else(|| Self::is_favorite(&self.games()[gi]));
                self.set_favorite_game(gi, !fav);
            }
            MAct::Artwork => {
                close(self);
                crate::sgdb::open(self, id);
            }
            MAct::Properties => {
                close(self);
                crate::screens::properties::open(self, id, None);
            }
            MAct::Cancel | MAct::Loading => close(self),
            MAct::AddTo(cid) => {
                self.collection_apply(id, &cid, true);
                close(self);
            }
            MAct::RemoveFrom(cid) => {
                self.collection_apply(id, &cid, false);
                close(self);
            }
            MAct::NewCollection => {
                close(self);
                let title = t_or("#GameAction_AddToCollectionOption_NewCollection", "Создать коллекцию...");
                crate::screens::properties::prompt(self, title, String::new(), Box::new(move |a, name| {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    setters::write("new-collection", format!("(async()=>{{const c=collectionStore.NewUnsavedCollection({},undefined,[{id}]);await c.Save()}})()", js_str(&name)));
                    a.toast(&name);
                }));
            }
            MAct::Hide(on) => {
                close(self);
                setters::write("hide", format!("collectionStore.SetAppsAsHidden([{id}],{on})"));
                let n = self.games()[gi].name.clone();
                self.toast(&format!("{}: {n}", if on { t_or("#GameAction_AddToHidden", "Скрыть игру") } else { t_or("#GameAction_RemoveFromHidden", "Убрать из скрытого") }));
            }
            MAct::CdKeys => {
                close(self);
                setters::write("cdkeys", format!("SteamClient.URL.ExecuteSteamURL('steam://cdkeys/{id}')"));
            }
            MAct::Family(block) => {
                close(self);
                // sp.js:498937: Q.jR.SetAppBlockedState(apps, blocked)
                setters::write("family", format!("(window.parentalStore||window.g_ParentalStore).SetAppBlockedState([{id}],{block})"));
            }
            MAct::Private(_) | MAct::RemoveLicense => {
                // unreachable until the live check finds a JS handle (SPEC "Verify live"); log-only
                close(self);
                crate::setters::write("manage-unsupported", "0".to_string());
            }
            MAct::RemoveShortcut => {
                close(self);
                let name = self.games()[gi].name.clone();
                crate::screens::properties::confirm(
                    self,
                    t_or("#GameAction_RemoveShortcut", "Удалить стороннюю игру из библиотеки"),
                    name,
                    t_or("#Button_OK", "OK"),
                    Box::new(move |a| {
                        // sp.js:498507 Et(): RemoveShortcut after 500 ms, then drop it from the user collections
                        setters::write(
                            "remove-shortcut",
                            format!("setTimeout(()=>{{SteamClient.Apps.RemoveShortcut({id});for(const c of collectionStore.userCollections)c.bAllowsDragAndDrop&&c.apps.has({id})&&c.AsDragDropCollection().RemoveApps([appStore.GetAppOverviewByAppID({id})])}},500)"),
                        );
                        a.go_back();
                    }),
                );
            }
            MAct::Uninstall => {
                close(self);
                self.confirm_uninstall(id);
            }
        }
    }

    /// Both entry points capture appid, never a mutable library index.
    pub(crate) fn confirm_uninstall(&mut self, id: u32) {
        let Some(&gi) = self.gidx.get(&id) else { return };
        let g = &self.games()[gi];
        if g.kind != GameKind::Steam || !g.installed || self.is_running(id) {
            self.toast("Удаление недоступно. Закройте игру и проверьте состояние установки.");
            return;
        }
        let text = t_or("#UninstallDialog_Confirmation_Single", "Вы точно хотите удалить %1$s с этого устройства?").replace("%1$s", &g.name);
        crate::screens::properties::confirm(self,
            t_or("#UninstallDialog_Title", "Удаление"), text,
            t_or("#UninstallDialog_Uninstall", "Удалить"),
            Box::new(move |a| a.act(Act::Uninstall(id))));
    }

    pub(crate) fn can_uninstall(&self, gi: usize) -> bool {
        use super::manage_rules::{allowed, AT_GAME};
        let g = &self.games()[gi];
        let ex = self.extras.get(&g.app_id);
        g.kind == GameKind::Steam && g.installed && allowed(
            ex.map(|e| e.app_type).filter(|t| *t != 0).unwrap_or(AT_GAME),
            ex.map(|e| e.display_status).unwrap_or(11), g.installed, false).uninstall
    }

    fn collection_apply(&mut self, id: u32, cid: &str, add: bool) {
        let op = if add { "AddApps" } else { "RemoveApps" };
        setters::write(
            "collection",
            format!("collectionStore.userCollections.find(c=>c.id=={}).AsDragDropCollection().{op}([appStore.GetAppOverviewByAppID({id})])", js_str(cid)),
        );
        let name = self.gm.mn.colls.iter().find(|c| c.0 == cid).map(|c| c.1.clone()).unwrap_or_default();
        if let Some(gi) = self.gidx.get(&id).copied() {
            let g = &mut self.lib_games_mut()[gi];
            if add {
                if !g.collections.contains(&name) {
                    g.collections.push(name.clone());
                }
            } else {
                g.collections.retain(|c| *c != name);
            }
        }
        self.toast(&format!("{}: {}", if add { t_or("#GameAction_AddToCollection", "Добавить в...") } else { t_or("#GameAction_RemoveFromCollection", "Убрать из...") }, name));
    }
}

impl App {
    /// Favorite toggle: collectionStore write + local view patched at once (same as the library tile menu).
    fn set_favorite_game(&mut self, gi: usize, on: bool) {
        let id = self.games()[gi].app_id;
        setters::set_favorite(id, on);
        let lib = Arc::make_mut(&mut self.lib);
        let g = &mut lib.games[gi];
        let is_fav = |c: &String| c.to_lowercase().contains("favorite") || c.to_lowercase().contains("избран");
        g.collections.retain(|c| !is_fav(c));
        if on {
            g.collections.push(t_or("#GameList_Category_Favorites", "Избранное"));
        }
        self.gm.mn.fav = Some(on);
        self.rebuild_view(Some(id));
        self.sync_lib();
        let n = self.games()[gi].name.clone();
        self.toast(&format!("{}: {n}", if on { t_or("#GameAction_AddToFavorites", "Добавить в избранное") } else { t_or("#GameAction_RemoveFromFavorites", "Убрать из избранного") }));
    }

    fn lib_games_mut(&mut self) -> &mut Vec<steam_data::Game> {
        &mut Arc::make_mut(&mut self.lib).games
    }

    fn menu_primary_label(&self, gi: usize) -> String {
        self.primary_label_pub(gi)
    }
}
