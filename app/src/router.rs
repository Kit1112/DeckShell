//! Route stack mirroring Steam's `/routes/...` (docs/engine/routes.json) so B = history.goBack() behaves like Steam.
//! Menu navigation = history.push, tab switch = history.replace, B pops; the root (Home) never pops.

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Route {
    /// /routes/library/home
    Home,
    /// /routes/library/tab/<id>  (AllGames, Installed, Favorites, Collections, DesktopApps, ...)
    Library(String),
    /// /routes/library/app/<appid>
    App(u32),
    /// /routes/library/downloads
    Downloads,
    /// /routes/settings/<page>
    Settings(String),
    /// /routes/media/grid
    Media,
    /// /routes/search
    Search,
}

impl Route {
    pub fn path(&self) -> String {
        match self {
            Route::Home => "/routes/library/home".into(),
            Route::Library(t) => format!("/routes/library/tab/{t}"),
            Route::App(id) => format!("/routes/library/app/{id}"),
            Route::Downloads => "/routes/library/downloads".into(),
            Route::Settings(p) => format!("/routes/settings/{p}"),
            Route::Media => "/routes/media/grid".into(),
            Route::Search => "/routes/search".into(),
        }
    }

    /// Page index understood by ui/shell.slint.
    pub fn page(&self) -> i32 {
        match self {
            Route::Home => 0,
            Route::Library(_) => 1,
            Route::App(_) => 2,
            Route::Downloads => 3,
            Route::Settings(_) => 4,
            Route::Media => 5,
            Route::Search => 6,
        }
    }

    /// Library grid is on screen (tab route or search mode).
    pub fn is_grid(&self) -> bool {
        matches!(self, Route::Library(_) | Route::Search)
    }
}

pub struct Router {
    stack: Vec<Route>,
}

impl Router {
    pub fn new() -> Router {
        Router { stack: vec![Route::Home] }
    }
    pub fn current(&self) -> &Route {
        self.stack.last().expect("router never empty")
    }
    pub fn page(&self) -> i32 {
        self.current().page()
    }
    /// history.push; pushing the current route again is a no-op (returns false).
    pub fn push(&mut self, r: Route) -> bool {
        if *self.current() == r {
            return false;
        }
        self.stack.push(r);
        true
    }
    /// history.replace
    pub fn replace(&mut self, r: Route) {
        *self.stack.last_mut().expect("router never empty") = r;
    }
    /// history.goBack; false at the root.
    pub fn back(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }
    /// The route below the current one (where B would go).
    pub fn previous(&self) -> Option<&Route> {
        self.stack.len().checked_sub(2).and_then(|i| self.stack.get(i))
    }
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
    /// Does any entry of the stack match (menu "current route" highlight uses the top only).
    pub fn is_library_family(&self) -> bool {
        matches!(self.current(), Route::Library(_) | Route::App(_) | Route::Search)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn back_works_like_history() {
        let mut r = Router::new();
        assert!(!r.back());
        r.push(Route::Library("AllGames".into()));
        r.push(Route::App(5));
        assert_eq!(r.page(), 2);
        assert!(r.back());
        assert_eq!(r.current(), &Route::Library("AllGames".into()));
        r.replace(Route::Library("Installed".into()));
        assert!(r.back());
        assert_eq!(r.current(), &Route::Home);
        assert!(!r.push(Route::Home));
    }
    #[test]
    fn paths() {
        assert_eq!(Route::App(7).path(), "/routes/library/app/7");
        assert_eq!(Route::Settings("system".into()).path(), "/routes/settings/system");
    }
}
