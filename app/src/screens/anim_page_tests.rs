use crate::screens::expected_text;
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
    for (id, len) in [("BootB", 3_877_957), ("BootC", 838_027), ("Sleep", 25_859), ("Extra", 1_000)] {
        std::fs::write(d.plugin_data.join("downloads").join(format!("{id}.webm")), webm(len)).unwrap();
    }
    std::fs::write(
        &d.plugin_config,
        serde_json::to_vec(&json!({
            "boot": "BootB", "suspend": "Sleep", "throbber": "Sleep", "randomize": "all", "force_ipv4": true,
            "downloads": [
                {"id": "BootB", "name": "Retro Console", "author": "Fixture Creator B", "target": "boot", "likes": 262, "downloads": 60373, "last_changed": "2022-10-11T21:25:37.000000Z", "description": "A synthetic console startup sequence"},
                {"id": "BootC", "name": "90s Terminal Intro", "author": "Fixture Creator C", "target": "boot", "likes": 198, "downloads": 46690, "last_changed": "2022-10-12T20:18:21.000000Z"},
                {"id": "Sleep", "name": "Minimal Sleep", "author": "x", "target": "suspend", "likes": 5, "downloads": 9, "last_changed": "2023-01-01T00:00:00.000000Z"}
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
    st.set_rail(ModelRc::new(VecModel::from(vec![crate::ListEntry { label: i18n::tr_ref(LABEL).into(), ..Default::default() }])));
    let ds = Arc::new(crate::data::DataSource::new());
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let mut a = App::new(ui.as_weak(), ds, Arc::new(steam_data::Library::default()), loader);
    let dirs = fixture_dirs(name);
    let root = dirs.own.parent().unwrap().to_path_buf();
    anim::install_page_config(Config::load_from(dirs));
    ST.with(|s| *s.borrow_mut() = St { lib_kind: repo::Kind::All, cat_q: repo::Query { deck_only: true, ..Default::default() }, ..Default::default() });
    a.se.pages = vec![Page { id: PAGE.into(), label: i18n::tr_ref(LABEL).into() }];
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
                s.downloads.insert("BootA".into(), Dl { done: 50, total: Some(100), cancel: Arc::new(AtomicBool::new(false)) });
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
    assert_eq!(first, "BootB", "opens on the current choice");
    assert_eq!(n, 1 + 3, "default + three boot animations (suspend ones are not offered)");
    // options: default, then by name: 90s Terminal Intro, Extra, Retro Console (current)
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
    assert_eq!(first, "BootC", "sorted by name: 90s Terminal Intro first");
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
    with(|s| s.focus = Some(Key::Card("Sleep".into())));
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
    assert!(cfg(|c| c.is_excluded("Sleep")));
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
    assert!(cfg(|c| c.entry("Sleep").is_none()));
    assert_eq!(cfg(|c| (c.get("suspend"), c.get("throbber"))), (String::new(), String::new()), "slots reset");
    assert!(cfg(|c| c.plugin_download("Sleep").is_file()), "the plugin's file is not deleted");
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
        "/post/download/BootA" => (302, vec![("Location".into(), format!("{base}/videos/BootA.webm?sig=1"))], vec![], 0),
        "/videos/BootA.webm?sig=1" => (200, vec![], v2.clone(), 0),
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
    assert!(status.starts_with(expected_text("5 анимаций", "5 animations")), "{status}");
    assert!(cfg(|c| c.dirs.repo.join("posts.json").is_file()), "saved for offline use");
    // search through the shared keyboard target
    with(|s| s.focus = Some(Key::Search));
    a.handle_pad(Action::A);
    assert!(keyboard_target());
    for c in "star".chars() {
        type_char(a, Some(c));
    }
    assert_eq!(with(|s| s.cat_view.iter().map(|i| s.cat.as_ref().unwrap()[*i].id.clone()).collect::<Vec<_>>()), vec!["BootA"]);
    assert!(a.settings_back(), "B closes the keyboard first");
    assert!(!keyboard_target());
    for _ in 0..4 {
        type_char(a, None);
    }
    // an animation the plugin already downloaded is not fetched again
    with(|s| s.focus = Some(Key::Card("BootB".into())));
    a.handle_pad(Action::X);
    assert!(!with(|s| s.downloads.contains_key("BootB")));
    // download with progress → appears in «Мои анимации»
    with(|s| s.focus = Some(Key::Card("BootA".into())));
    a.handle_pad(Action::X);
    assert!(with(|s| s.downloads.contains_key("BootA")));
    drain_until(a, "download", |_| with(|s| !s.downloads.contains_key("BootA")));
    assert_eq!(std::fs::read(cfg(|c| c.own_download("BootA"))).unwrap(), video);
    assert_eq!(cfg(|c| c.origin("BootA")), Origin::Own);
    assert_eq!(cfg(|c| c.entry("BootA").unwrap()["download_url"].clone()), json!("https://steamdeckrepo.com/post/download/BootA"));
    let card = with(|s| s.items.iter().find(|i| i.key == Key::Card("BootA".into())).cloned().unwrap());
    assert_eq!(card.badge2, expected_text("Скачано", "Downloaded."));
    assert!(cfg(|c| locals(c).iter().any(|l| l.id == "BootA" && l.author == "Fixture Creator A")));
    // 404 → a readable error on the card and in details
    with(|s| s.focus = Some(Key::Card("RuZ01".into())));
    a.handle_pad(Action::X);
    drain_until(a, "error", |_| with(|s| s.errors.contains_key("RuZ01")));
    assert!(with(|s| s.errors["RuZ01"].contains("404")));
    assert!(!cfg(|c| c.own_download("RuZ01")).exists());
    assert_eq!(with(|s| s.items.iter().find(|i| i.key == Key::Card("RuZ01".into())).map(|i| i.badge2.clone())), Some(expected_text("Ошибка", "Error").into()));
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
    assert_eq!(labels.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), vec![expected_text("Скачать", "Download"), expected_text("Предпросмотр", "Preview")]);
    a.handle_pad(Action::B);
    // offline: the saved catalog stays usable with a warning
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap().to_string();
    drop(dead);
    *TEST_NET.lock().unwrap() = Some((format!("http://{addr}"), addr));
    ensure_catalog(a, true);
    drain_until(a, "offline", |_| with(|s| matches!(s.cat_state, Some(CatState::Ready { offline: Some(_), .. }))));
    let status = with(|s| s.items.iter().find(|i| i.kind == 10).map(|i| (i.label.clone(), i.on)).unwrap());
    assert!(status.0.contains(expected_text("Нет подключения", "Could not connect")) && status.0.contains(expected_text("сохранённый каталог", "cached catalog")), "{}", status.0);
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
        s.focus = Some(Key::Card("BootB".into()));
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
    assert_eq!(focus(), Some(Key::Card("BootB".into())));

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
    let ids = ["BootB", "BootC", "Sleep", "Extra", "BootA", "RuZ01", "Mach1"];
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
    cfg(|c| c.toggle_favorite("BootB"));
    rebuild(a);
    a.handle_pad(Action::RB);
    with(|s| {
        s.cat = Some(catalog());
        s.cat_state = Some(CatState::Ready { fetched: SystemTime::now() - Duration::from_secs(300), offline: None });
        s.downloads.insert("BootA".into(), Dl { done: 45, total: Some(100), cancel: Arc::new(AtomicBool::new(false)) });
        s.errors.insert("RuZ01".into(), repo::NetErr::Status(404).message());
        s.focus = Some(Key::Card("BootA".into()));
    });
    rebuild(a);
    save("catalog");
    with(|s| s.focus = Some(Key::Card("BootB".into())));
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
        s.cat_state = Some(CatState::Ready { fetched: SystemTime::now() - Duration::from_secs(3 * 86_400), offline: Some(repo::NetErr::Offline(String::new()).message()) });
        s.focus = Some(Key::Search);
        s.cat_q.text = "star".into();
    });
    rebuild(a);
    save("catalog-offline-search");
    with(|s| {
        s.cat = None;
        s.cat_state = Some(CatState::Failed(repo::NetErr::RateLimited.message()));
        s.cat_q.text.clear();
    });
    rebuild(a);
    save("catalog-error");
    a.handle_pad(Action::RB);
    with(|s| s.storage = Some(Storage { own: (3, 5 << 20), plugin: (31, 107 << 20), frames: 640 << 20, frames_budget: 3 << 30, repo: 45 << 20, free: Some(23 << 30) }));
    rebuild(a);
    save("storage");
}
