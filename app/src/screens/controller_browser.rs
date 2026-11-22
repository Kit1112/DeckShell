//! Source-derived state for Steam's controller-layout chooser.
//!
//! `sp.js`, chooser components `ki` and `Ot`: tabs depend on available
//! configurations, and Search filters the loaded store, not a remote query.
//! This model deliberately performs no Steam writes.

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Tab { Recommended, User, Templates, Community, Search }

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Availability {
    pub official: usize,
    pub recommended: usize,
    pub personal: usize,
    pub community_allowed: bool,
}

impl Availability {
    pub fn tabs(&self) -> Vec<Tab> {
        let mut tabs = Vec::new();
        if self.official > 0 || self.recommended > 0 { tabs.push(Tab::Recommended); }
        if self.personal > 0 { tabs.push(Tab::User); }
        tabs.push(Tab::Templates);
        if self.community_allowed { tabs.push(Tab::Community); }
        tabs.push(Tab::Search);
        tabs
    }

    pub fn initial_tab(&self) -> Tab {
        self.tabs()[0]
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchEntry {
    pub title: String,
    pub description: String,
    pub game_name: String,
    pub app_id: u32,
    pub uses_siapi: bool,
}

/// Steam excludes SIAPI entries for a different nonzero app id, even when
/// their text matches. Missing text is represented by an empty string.
pub fn matches_search(entry: &SearchEntry, app_id: u32, query: &str) -> bool {
    if entry.uses_siapi && entry.app_id != 0 && entry.app_id != app_id { return false; }
    let query = query.to_lowercase();
    [&entry.title, &entry.description, &entry.game_name]
        .into_iter().any(|value| value.to_lowercase().contains(&query))
}

/// Workshop entries are ordered by descending playtime in the stock chooser.
/// Stable sorting preserves the order returned by Steam for equal values.
pub fn sort_community_by_playtime<T>(entries: &mut [T], playtime: impl Fn(&T) -> u64) {
    entries.sort_by_key(|entry| std::cmp::Reverse(playtime(entry)));
}

/// Save focus and scroll independently per tab, clamping stale focus after a
/// refresh. Empty lists cannot produce a focus index.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position { pub focused: Option<usize>, pub scroll: f32 }
impl Position {
    pub fn after_refresh(self, count: usize, max_scroll: f32) -> Self {
        Self {
            focused: if count == 0 { None } else { Some(self.focused.unwrap_or(0).min(count - 1)) },
            scroll: if self.scroll.is_finite() && max_scroll.is_finite() { self.scroll.clamp(0.0, max_scroll.max(0.0)) } else { 0.0 },
        }
    }
}

/// Query generations prevent a slow earlier request from replacing newer
/// results. An empty successful response is distinct from a failed request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoadState<T> { Loading, Ready(Vec<T>), Error(String) }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Query<T> { pub generation: u64, pub state: LoadState<T> }
impl<T> Query<T> {
    pub fn new() -> Self { Self { generation: 0, state: LoadState::Loading } }
    pub fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.state = LoadState::Loading;
        self.generation
    }
    pub fn finish(&mut self, generation: u64, result: Result<Vec<T>, String>) -> bool {
        if generation != self.generation { return false; }
        self.state = match result { Ok(entries) => LoadState::Ready(entries), Err(error) => LoadState::Error(error) };
        true
    }
}

/// Focus transitions observed in the Gaming Mode chooser capture 37a–37p.
/// `Back` from the search field returns to its tab after the keyboard closes;
/// it must not close the whole chooser in that state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus { Tab, SearchField, Card(usize) }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input { Left, Right, Up, Down, Back }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome { Stayed, CloseKeyboard, CloseChooser }

/// Navigation state survives moving between tabs and temporarily empty data.
/// A refresh may remove a tab; in that case Steam's initial-tab rule provides
/// a valid fallback rather than leaving focus on a missing page.
#[derive(Debug)]
pub struct Browser {
    pub availability: Availability,
    pub active: Tab,
    pub focus: Focus,
    pub keyboard_open: bool,
    /// The compatible-only filter is toggled by X and requires a fresh query.
    pub show_all: bool,
    positions: std::collections::HashMap<Tab, Position>,
}
impl Browser {
    pub fn new(availability: Availability) -> Self {
        let active = availability.initial_tab();
        Self { availability, active, focus: Focus::Tab, keyboard_open: false, show_all: false, positions: Default::default() }
    }
    /// First list load of a chooser opening: Steam's initial-tab rule applies once the tabs are known.
    /// The user cannot move between tabs while the list loads, so this overrides the placeholder tab.
    pub fn finish_load(&mut self, availability: Availability) {
        self.active = availability.initial_tab();
        self.availability = availability;
        self.focus = Focus::Tab;
        self.keyboard_open = false;
    }
    pub fn refresh_availability(&mut self, availability: Availability) {
        if !availability.tabs().contains(&self.active) { self.active = availability.initial_tab(); self.focus = Focus::Tab; self.keyboard_open = false; }
        self.availability = availability;
    }
    pub fn select_tab(&mut self, tab: Tab) -> bool {
        if !self.availability.tabs().contains(&tab) { return false; }
        self.active = tab;
        self.focus = Focus::Tab;
        self.keyboard_open = false;
        true
    }
    /// L1/R1: the adjacent tab from any element (stock TabBar wraps); false when there is no other tab.
    pub fn shift_tab(&mut self, delta: i32) -> bool {
        let tabs = self.availability.tabs();
        let index = tabs.iter().position(|&tab| tab == self.active).unwrap_or(0);
        match crate::nav::tab_step(index, delta, tabs.len(), None) {
            Some(next) if tabs[next] != self.active => { let tab = tabs[next]; self.select_tab(tab) }
            _ => false,
        }
    }
    pub fn input(&mut self, input: Input, count: usize) -> Outcome {
        if let Focus::Card(index) = self.focus {
            self.focus = if count == 0 { Focus::Tab } else { Focus::Card(index.min(count - 1)) };
        }
        if input == Input::Back && self.keyboard_open {
            self.keyboard_open = false;
            return Outcome::CloseKeyboard;
        }
        match (self.focus, input) {
            (Focus::Tab, Input::Left | Input::Right) => {
                let tabs = self.availability.tabs();
                let index = tabs.iter().position(|&tab| tab == self.active).unwrap_or(0);
                let delta = if input == Input::Left { -1 } else { 1 };
                if let Some(next) = crate::nav::tab_step(index, delta, tabs.len(), None) { self.active = tabs[next]; }
            }
            (Focus::Tab, Input::Down) if self.active == Tab::Search => self.focus = Focus::SearchField,
            (Focus::Tab, Input::Down) if count > 0 => self.focus = Focus::Card(self.position(count, f32::MAX).focused.unwrap_or(0)),
            (Focus::SearchField, Input::Down) if count > 0 => self.focus = Focus::Card(self.position(count, f32::MAX).focused.unwrap_or(0)),
            (Focus::SearchField, Input::Back | Input::Up) => self.focus = Focus::Tab,
            (Focus::Card(_), Input::Back) => self.focus = if self.active == Tab::Search { Focus::SearchField } else { Focus::Tab },
            (Focus::Card(0), Input::Up) if self.active == Tab::Search => self.focus = Focus::SearchField,
            (Focus::Card(0), Input::Up) => self.focus = Focus::Tab,
            (Focus::Card(index), Input::Up) => self.focus = Focus::Card(index.saturating_sub(1)),
            (Focus::Card(index), Input::Down) if count > 0 => self.focus = Focus::Card((index + 1).min(count - 1)),
            (Focus::Tab, Input::Back) => return Outcome::CloseChooser,
            _ => {}
        }
        if let Focus::Card(index) = self.focus {
            self.positions.entry(self.active).or_default().focused = Some(index);
        }
        Outcome::Stayed
    }
    /// Toggle the Steam query filter without discarding the current tab or its position.
    pub fn toggle_show_all(&mut self) -> bool {
        self.show_all = !self.show_all;
        self.show_all
    }
    pub fn set_position(&mut self, position: Position) { self.positions.insert(self.active, position); }
    pub fn position(&self, count: usize, max_scroll: f32) -> Position {
        self.positions.get(&self.active).copied().unwrap_or_default().after_refresh(count, max_scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn available_tabs_and_initial_selection_follow_steam() {
        let base = Availability::default();
        assert_eq!(base.tabs(), vec![Tab::Templates, Tab::Search]);
        assert_eq!(Availability { personal: 2, community_allowed: true, ..base.clone() }.tabs(), vec![Tab::User, Tab::Templates, Tab::Community, Tab::Search]);
        assert_eq!(Availability { official: 1, personal: 1, ..base.clone() }.initial_tab(), Tab::Recommended);
        assert_eq!(Availability { recommended: 1, ..base }.initial_tab(), Tab::Recommended);
    }
    #[test]
    fn search_is_case_insensitive_and_rejects_other_siapi_games() {
        let ru = steam_assets::i18n::by_code("ru").unwrap();
        let mut e = SearchEntry { title: ru.tr("controller.gamepad_3").into(), description: "Desktop".into(), game_name: "Balatro".into(), app_id: 42, uses_siapi: false };
        let query = ru.tr("controller.game_2");
        for q in [query.as_str(), "desk", "LAT", ""] { assert!(matches_search(&e, 2379780, q)); }
        assert!(!matches_search(&e, 2379780, "missing"));
        e.uses_siapi = true;
        assert!(!matches_search(&e, 2379780, &ru.tr("controller.game_3")));
        e.app_id = 0;
        assert!(matches_search(&e, 2379780, &ru.tr("controller.game_3")));
        e.app_id = 2379780;
        assert!(matches_search(&e, 2379780, &ru.tr("controller.game_3")));
    }
    #[test]
    fn community_sort_is_descending_and_stable_on_ties() {
        let mut entries = [("a", 5), ("b", 9), ("c", 5), ("d", u64::MAX)];
        sort_community_by_playtime(&mut entries, |e| e.1);
        assert_eq!(entries.map(|e| e.0), ["d", "b", "a", "c"]);
    }
    #[test]
    fn stock_tab_card_and_search_back_transitions() {
        let mut browser = Browser::new(Availability { official: 1, community_allowed: true, ..Default::default() });
        assert_eq!(browser.input(Input::Left, 0), Outcome::Stayed);
        assert_eq!(browser.active, Tab::Search); // tab strip wraps
        browser.input(Input::Down, 0);
        assert_eq!(browser.focus, Focus::SearchField);
        browser.keyboard_open = true;
        assert_eq!(browser.input(Input::Back, 0), Outcome::CloseKeyboard);
        assert_eq!(browser.focus, Focus::SearchField);
        assert_eq!(browser.input(Input::Back, 0), Outcome::Stayed);
        assert_eq!(browser.focus, Focus::Tab);
        browser.input(Input::Left, 3);
        assert_eq!(browser.active, Tab::Community);
        browser.input(Input::Down, 3);
        assert_eq!(browser.focus, Focus::Card(0));
        browser.input(Input::Down, 3);
        browser.input(Input::Up, 3);
        browser.input(Input::Up, 3);
        assert_eq!(browser.focus, Focus::Tab);
        assert_eq!(browser.input(Input::Back, 3), Outcome::CloseChooser);
    }
    #[test]
    fn unavailable_community_has_a_back_path_and_preserves_card_position() {
        let mut browser = Browser::new(Availability { official: 1, community_allowed: true, ..Default::default() });
        assert!(browser.select_tab(Tab::Community));
        browser.input(Input::Down, 3);
        browser.input(Input::Down, 3);
        assert_eq!(browser.focus, Focus::Card(1));
        assert_eq!(browser.input(Input::Back, 3), Outcome::Stayed);
        browser.input(Input::Down, 3);
        assert_eq!(browser.focus, Focus::Card(1));
        assert_eq!(browser.input(Input::Back, 0), Outcome::CloseChooser);
    }
    #[test]
    fn stale_queries_cannot_replace_current_result_and_empty_is_not_error() {
        let mut query = Query::<u32>::new();
        let old = query.begin();
        let new = query.begin();
        assert!(!query.finish(old, Ok(vec![1])));
        assert_eq!(query.state, LoadState::Loading);
        assert!(query.finish(new, Ok(vec![])));
        assert_eq!(query.state, LoadState::Ready(vec![]));
        let retry = query.begin();
        assert!(query.finish(retry, Err("offline".into())));
        assert_eq!(query.state, LoadState::Error("offline".into()));
    }
    #[test]
    fn tab_switch_restores_position_and_removal_has_valid_fallback() {
        let mut browser = Browser::new(Availability { personal: 1, community_allowed: true, ..Default::default() });
        browser.set_position(Position { focused: Some(3), scroll: 70.0 });
        assert!(browser.select_tab(Tab::Community));
        browser.set_position(Position { focused: Some(1), scroll: 20.0 });
        assert!(browser.select_tab(Tab::User));
        assert_eq!(browser.position(4, 100.0), Position { focused: Some(3), scroll: 70.0 });
        browser.refresh_availability(Availability::default());
        assert_eq!(browser.active, Tab::Templates);
        assert!(!browser.select_tab(Tab::Community));
    }
    #[test]
    fn first_load_selects_stock_initial_tab_and_refresh_keeps_it() {
        // Before the list arrives only the placeholder tabs exist; no Official tab is assumed.
        let mut browser = Browser::new(Availability { community_allowed: true, ..Default::default() });
        assert_eq!(browser.active, Tab::Templates);
        browser.finish_load(Availability { official: 2, community_allowed: true, ..Default::default() });
        assert_eq!(browser.active, Tab::Recommended);
        assert!(browser.select_tab(Tab::Search));
        // A later refresh (show-all toggle) must not reset the tab the user chose.
        browser.refresh_availability(Availability { official: 3, community_allowed: true, ..Default::default() });
        assert_eq!(browser.active, Tab::Search);
        // Without Official entries the first load falls back to Templates.
        let mut none = Browser::new(Availability::default());
        none.finish_load(Availability { community_allowed: true, ..Default::default() });
        assert_eq!(none.active, Tab::Templates);
    }
    #[test]
    fn show_all_preserves_tab_and_focus_across_refresh() {
        let mut browser = Browser::new(Availability { official: 1, ..Default::default() });
        browser.input(Input::Down, 3);
        browser.input(Input::Down, 3);
        browser.set_position(Position { focused: Some(1), scroll: 42.0 });
        assert!(browser.toggle_show_all());
        assert_eq!(browser.active, Tab::Recommended);
        assert_eq!(browser.focus, Focus::Card(1));
        assert_eq!(browser.position(3, 100.0).scroll, 42.0);
        assert!(!browser.toggle_show_all());
        assert_eq!(browser.input(Input::Back, 3), Outcome::Stayed);
        assert_eq!(browser.input(Input::Back, 3), Outcome::CloseChooser);
    }
    #[test]
    fn search_results_have_a_path_back_to_the_field_and_tab() {
        let mut browser = Browser::new(Availability::default());
        assert!(browser.select_tab(Tab::Search));
        browser.input(Input::Down, 2);
        assert_eq!(browser.focus, Focus::SearchField);
        browser.input(Input::Down, 2);
        assert_eq!(browser.focus, Focus::Card(0));
        browser.input(Input::Down, 2);
        browser.input(Input::Back, 2);
        assert_eq!(browser.focus, Focus::SearchField);
        browser.input(Input::Down, 2);
        assert_eq!(browser.focus, Focus::Card(1));
        browser.input(Input::Up, 2);
        browser.input(Input::Up, 2);
        assert_eq!(browser.focus, Focus::SearchField);
        browser.input(Input::Back, 2);
        assert_eq!(browser.focus, Focus::Tab);
        assert_eq!(browser.input(Input::Back, 2), Outcome::CloseChooser);
    }
    #[test]
    fn refresh_clamps_focus_and_scroll_and_recovers_from_empty() {
        let pos = Position { focused: Some(7), scroll: 800.0 };
        assert_eq!(pos.after_refresh(3, 100.0), Position { focused: Some(2), scroll: 100.0 });
        let empty = pos.after_refresh(0, 0.0);
        assert_eq!(empty, Position { focused: None, scroll: 0.0 });
        assert_eq!(empty.after_refresh(1, 0.0).focused, Some(0));
        assert_eq!(Position { focused: None, scroll: f32::NAN }.after_refresh(1, 1.0).scroll, 0.0);
    }
}
