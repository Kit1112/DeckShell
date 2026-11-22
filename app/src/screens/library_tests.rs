use super::{collate_key, default_tabs, TabKind};
#[test]
fn native_default_tabs_keep_source_order() {
    assert_eq!(default_tabs(false), vec![TabKind::All, TabKind::Installed, TabKind::NonSteam]);
    assert_eq!(default_tabs(true), vec![TabKind::All, TabKind::Installed, TabKind::Collections, TabKind::NonSteam]);
}
#[test]
fn bumper_moves_the_pill_indicator_at_once() {
    use crate::input::Action;
    use crate::router::Route;
    use slint::ComponentHandle;
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = crate::AppWindow::new().unwrap();
    let ds = std::sync::Arc::new(crate::data::DataSource::new());
    let lib = std::sync::Arc::new(steam_data::Library::default());
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let mut app = crate::app::App::new(ui.as_weak(), ds, lib, loader);
    app.lb.tabs = vec![TabKind::All, TabKind::Installed, TabKind::NonSteam];
    app.lb.tab = 0;
    app.router.push(Route::Library("AllGames".into()));
    app.sync_lib();
    assert_eq!(ui.global::<crate::LibraryState>().get_tab(), 0);
    // R1 returns early in handle_library; the indicator must follow without a D-pad press.
    app.handle_library(Action::RB);
    assert_eq!(app.lb.tab, 1);
    assert_eq!(ui.global::<crate::LibraryState>().get_tab(), 1);
    app.handle_library(Action::LB);
    assert_eq!(ui.global::<crate::LibraryState>().get_tab(), 0);
}
#[test]
fn both_uninstall_paths_confirm_and_library_manage_stays_on_library() {
    // Modal scope has process-global flags, while Slint test windows are per-thread.
    // Exercise the real confirmation in a separate test process so its open flag
    // cannot steal input from concurrent navigation fixtures.
    const CHILD: &str = "DECKSHELL_UNINSTALL_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "screens::library::tests::both_uninstall_paths_confirm_and_library_manage_stays_on_library", "--nocapture"])
            .env(CHILD, "1").env("DECKSHELL_DRYRUN", "1").output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    use super::{MenuAct, MenuKind};
    use crate::input::Action;
    use crate::router::Route;
    use slint::ComponentHandle;
    use slint::platform::{Platform, PlatformError, WindowAdapter, software_renderer::{MinimalSoftwareWindow, RepaintBufferType}};
    struct RenderPlatform(std::rc::Rc<MinimalSoftwareWindow>);
    impl Platform for RenderPlatform {
        fn create_window_adapter(&self) -> Result<std::rc::Rc<dyn WindowAdapter>, PlatformError> { Ok(self.0.clone()) }
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(RenderPlatform(window.clone()))).unwrap();
    window.set_size(slint::PhysicalSize::new(1280, 800));
    let ui = crate::AppWindow::new().unwrap();
    ui.show().unwrap();
    let render = || {
        let mut pixels = vec![slint::Rgb8Pixel::default(); 1280 * 800];
        window.request_redraw();
        window.draw_if_needed(|r| { r.render(&mut pixels, 1280); });
        pixels
    };
    let assert_visible_confirmation = || {
        assert!(!ui.global::<crate::PropsState>().get_open(), "standalone dialog outside Properties");
        let pixels = render();
        // Actual composited pixels, not just dlg-open: the parent used to hide it.
        assert_eq!(pixels[260 * 1280 + 325], slint::Rgb8Pixel::new(35, 38, 46), "dialog panel must be rendered");
        assert_eq!(pixels[445 * 1280 + 540], slint::Rgb8Pixel::new(255, 255, 255), "Cancel must be visibly selected");
        assert_eq!(pixels[445 * 1280 + 740], slint::Rgb8Pixel::new(61, 68, 80), "Delete must be visible and unselected");
    };
    let ds = std::sync::Arc::new(crate::data::DataSource::new());
    let game = steam_data::Game { app_id: 42, kind: steam_data::GameKind::Steam, name: "Test installation".into(),
        sort_name: "test".into(), installed: true, install_dir: None, size_on_disk: 0, playtime_minutes: 0,
        last_played: None, update_pending: false, collections: vec![], art: Default::default() };
    let lib = std::sync::Arc::new(steam_data::Library { games: vec![game], ..Default::default() });
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let mut app = crate::app::App::new(ui.as_weak(), ds, lib, loader);
    app.router.push(Route::Library("Installed".into()));
    app.open_menu(MenuKind::Tile(0));
    app.activate_menu(MenuAct::Manage(0));
    assert_eq!(app.router.current(), &Route::Library("Installed".into()));
    assert_eq!(app.lb.menu.as_ref().unwrap().kind, MenuKind::Manage(42));
    assert!(app.lb.menu.as_ref().unwrap().items.iter().any(|i| i.act == MenuAct::Uninstall(42)));
    app.handle_sort(Action::B);
    assert_eq!(app.lb.menu.as_ref().unwrap().kind, MenuKind::Tile(0));
    app.activate_menu(MenuAct::Manage(0));
    app.activate_menu(MenuAct::Uninstall(42));
    let props = ui.global::<crate::PropsState>();
    assert!(props.get_dlg_open());
    assert_visible_confirmation();
    assert!(props.get_dlg_text().contains("Test installation"));
    assert_eq!(props.get_dlg_idx(), 0); // A defaults to Cancel, never deletes.
    super::super::properties::handle(&mut app, Action::A);
    assert!(!props.get_dlg_open());
    assert!(app.games()[0].installed); // no optimistic removal
    assert_eq!(app.router.current(), &Route::Library("Installed".into()));
    // The page's real gear action reaches exactly the same dialog.
    app.open_game(0);
    app.gmenu_open_with(false);
    app.gmenu_set_idx(2); // Favorite, Add to, Manage
    app.gmenu_activate();
    let (_, labels, _, _, _) = app.gmenu_view();
    app.gmenu_set_idx(labels.len() as i32 - 1); // shared rules put Uninstall last
    app.gmenu_activate();
    assert!(props.get_dlg_open());
    assert_visible_confirmation();
    assert!(props.get_dlg_text().contains("Test installation"));
    assert_eq!(props.get_dlg_idx(), 0);
    super::super::properties::handle(&mut app, Action::B);
    assert!(!props.get_dlg_open());
    // Only an explicit second-button activation runs the callback, exactly once.
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = calls.clone();
    super::super::properties::confirm(&mut app, "Consent".into(), "Test installation".into(), "Delete".into(),
        Box::new(move |_| count.set(count.get() + 1)));
    assert_eq!(calls.get(), 0);
    super::super::properties::handle(&mut app, Action::Right);
    assert_eq!(calls.get(), 0);
    super::super::properties::handle(&mut app, Action::A);
    assert_eq!(calls.get(), 1);
    assert!(!props.get_dlg_open());
    super::super::properties::handle(&mut app, Action::A);
    assert_eq!(calls.get(), 1);
    // Running, uninstalled and shortcuts cannot reach a destructive confirm.
    app.live.running = vec![42];
    app.confirm_uninstall(42);
    assert!(!props.get_dlg_open());
    app.live.running.clear();
    std::sync::Arc::make_mut(&mut app.lib).games[0].installed = false;
    app.open_menu(MenuKind::Manage(42));
    assert!(!app.lb.menu.as_ref().unwrap().items.iter().any(|i| i.act == MenuAct::Uninstall(42)));
    app.close_menu();
    app.confirm_uninstall(42);
    assert!(!props.get_dlg_open());
    std::sync::Arc::make_mut(&mut app.lib).games[0].installed = true;
    std::sync::Arc::make_mut(&mut app.lib).games[0].kind = steam_data::GameKind::Shortcut;
    app.confirm_uninstall(42);
    assert!(!props.get_dlg_open());
    // Real backend snapshot path updates both Installed membership and the page.
    app.lb.tabs = vec![TabKind::All, TabKind::Installed];
    app.lb.tab = 1;
    app.open_menu(MenuKind::Tile(0));
    app.on_backend(crate::backend::Update::Library(Box::new(crate::backend::LibData {
        apps: vec![steam_bridge::App { appid: 42, display_name: "Test installation".into(),
            app_type: 1, installed: false, visible: true, ..Default::default() }],
        ..Default::default()
    })));
    assert!(app.lb.view.is_empty());
    assert!(app.lb.menu.is_none());
    assert_eq!(app.primary_kind(0), super::super::game::Primary::Install);
    assert_eq!(app.router.current(), &Route::App(42));
}
#[test]
fn ru_collation_puts_cyrillic_before_latin_after_digits() {
    let mut v = vec!["a way out", "клеть", "7 days to die", "ведьмак 3", "battlefield\u{2122} 1"];
    v.sort_by_key(|s| collate_key(s));
    assert_eq!(v, vec!["7 days to die", "ведьмак 3", "клеть", "a way out", "battlefield\u{2122} 1"]);
}
