//! Headless smoke gallery. The tree dump is deliberately the same collector used by live parity diagnostics.
use crate::*;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::rc::Rc;

fn model<T: Clone + 'static>(values: Vec<T>) -> ModelRc<T> { Rc::new(VecModel::from(values)).into() }

// Keyboard picker geometry is the product table (controller_editor::keyboard_keys),
// which is measured from Gaming Mode capture 14d; labels are fixture values only.
fn controller_keyboard_fixture() -> Vec<ControllerPickerKey> {
    crate::screens::controller_editor::keyboard_keys().into_iter().map(|k| ControllerPickerKey { label: k.label.into(), x: k.x, y: k.y, w: k.w, h: k.h, center: k.center, bottom: k.bottom, ..Default::default() }).collect()
}

#[test]
fn render_gallery() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).unwrap();
    let ui = AppWindow::new().unwrap();
    ui.global::<crate::Icons>().on_tinted(crate::assets::tint_image);
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    ui.global::<HomeState>().set_hero_title("An exceptionally long game title with missing artwork".into());
    let cards: Vec<CardData> = (0..12).map(|i| CardData {
        id: i, name: format!("Fixture game {i}: an unusually long title").into(),
        caption: format!("Fixture game {i}").into(), x: i as f32 * 155., w: 145.,
        ..Default::default()
    }).collect();
    ui.global::<HomeState>().set_recent(model(cards.clone()));
    ui.global::<LibraryState>().set_rows(model(vec![LibRow { cards: model(cards), y: 0., h: 240., ..Default::default() }]));
    ui.global::<HomeState>().set_empty_text("No recent items — offline".into());
    ui.global::<LibraryState>().set_tabs(model(vec!["All games".into(), "Installed".into(), "Collections".into()]));
    ui.global::<LibraryState>().set_empty_text("No games available offline".into());
    ui.global::<MediaState>().set_title("Media".into());
    ui.global::<MediaState>().set_empty_text("No screenshots available".into());
    ui.global::<SettingsState>().set_title("Settings".into());
    ui.global::<SettingsState>().set_page_title("Storage".into());
    ui.global::<SettingsState>().set_empty_text("No storage devices available".into());
    ui.global::<SettingsState>().set_rail(model(["General", "Internet", "Storage", "A very long settings category"].into_iter().map(|s| ListEntry { label: s.into(), ..Default::default() }).collect()));
    ui.global::<SteamMenuState>().set_items(model(["Home", "Library", "Downloads", "Settings", "Power"].into_iter().map(|s| MenuItem { label: s.into(), ..Default::default() }).collect()));
    ui.global::<SteamMenuState>().set_row_y(model(vec![52.5, 124.5, 196.5, 268.5, 340.5]));
    ui.global::<SteamMenuState>().set_content_h(681.);
    ui.global::<SteamMenuState>().set_game_focus(-1);
    ui.global::<SteamMenuState>().set_game_title("A very long running game title".into());
    ui.global::<SteamMenuState>().set_game_items(model(vec!["Resume".into(), "Exit game".into(), "Game info".into()]));
    ui.global::<QamState>().set_tab_labels(model(vec!["Notifications".into(), "Friends".into(), "Quick settings".into(), "Performance".into()]));
    ui.global::<HotkeysState>().set_power_title("Power".into());
    ui.global::<HotkeysState>().set_power_items(model(["Controller layout", "Sleep", "Restart", "Shut down"].into_iter().enumerate().map(|(i,s)| PowerRow { label: s.into(), enabled: true, y: i as f32 * 55., ..Default::default() }).collect()));
    ui.global::<HotkeysState>().set_power_content_height(220.);
    let output = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &output { std::fs::create_dir_all(dir).unwrap(); }
    for (name, page, menu, game, qam, tab, power) in [
        ("home",0,false,false,false,0,false), ("library",1,false,false,false,0,false),
        ("media",5,false,false,false,0,false), ("settings",4,false,false,false,0,false),
        ("storage",4,false,false,false,0,false), ("side-menu",0,true,false,false,0,false),
        ("game-menu",0,true,true,false,0,false), ("qam",0,false,false,true,2,false),
        ("friends",0,false,false,true,1,false), ("power",0,false,false,false,0,true),
    ] {
        ui.global::<Shell>().set_page(page);
        ui.global::<SteamMenuState>().set_open(menu);
        ui.global::<SteamMenuState>().set_game_visible(game);
        ui.global::<SteamMenuState>().set_game_focus(if game { 0 } else { -1 });
        ui.global::<QamState>().set_open(qam);
        ui.global::<QamState>().set_tab(tab);
        ui.global::<QamState>().set_title(if tab == 1 { "Friends" } else { "Quick settings" }.into());
        ui.global::<QamState>().set_rows(model(if tab == 1 {
            vec![QamRow { kind: 7, fid: 0, label: "Offline friend with a long display name".into(), sub: "Offline".into(), y: 55., h: 64., ..Default::default() }]
        } else {
            vec![QamRow { kind: 1, fid: 0, label: "Brightness".into(), value: 0.5, y: 55., h: 72., ..Default::default() },
                 QamRow { kind: 1, fid: 1, label: "Volume".into(), value: -1., y: 130., h: 72., ..Default::default() }]
        }));
        ui.global::<SettingsState>().set_page_title(if name == "storage" { "Storage" } else { "General" }.into());
        ui.global::<SettingsState>().set_content(model(if name == "storage" {
            vec![GPageRow { kind: 1, label: "Internal drive — space unavailable".into(), x: 330., y: 160., w: 850., h: 60., ..Default::default() }]
        } else { Vec::new() }));
        ui.global::<HotkeysState>().set_power_open(power);
        // Start bindings/animations, then render after all transitions settle.
        let _ = ui.window().take_snapshot().expect("initial snapshot");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
        slint::platform::update_timers_and_animations();
        let snapshot = ui.window().take_snapshot().expect("software snapshot");
        assert_eq!((snapshot.width(), snapshot.height()), (1280, 800), "{name}");
        let nodes = crate::dumptree::collect(&ui, name, "fixture");
        assert!(!nodes.is_empty(), "{name}: empty tree");
        if let Some(dir) = &output {
            image::save_buffer(dir.join(format!("{name}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            std::fs::write(dir.join(format!("{name}.json")), serde_json::to_vec_pretty(&nodes).unwrap()).unwrap();
        }
    }
    // Controller-card fixture. This tests rendering and focus styling, not
    // parity with a Gaming Mode reference, which has not yet been captured.
    ui.global::<HotkeysState>().set_power_open(false);
    ui.global::<ControllerState>().set_open(true);
    ui.global::<ControllerState>().set_browser_open(true);
    let snapshot = ui.window().take_snapshot().expect("controller unavailable browser snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-browser-unavailable.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_browser_open(false);
    ui.global::<ControllerState>().set_card_fixture_open(true);
    ui.global::<ControllerState>().set_card_fixture(ControllerConfigCard {
        title: "An unusually long controller configuration title".into(),
        author: "Community author".into(), description: "Fixture description".into(), status: "Downloaded".into(),
        selected: true, recommended: false, ..Default::default()
    });
    let mut card_images = Vec::new();
    for focused in [false, true] {
        ui.global::<ControllerState>().set_card_fixture_focused(focused);
        let snapshot = ui.window().take_snapshot().expect("controller card snapshot");
        assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
        if let Some(dir) = &output {
            image::save_buffer(dir.join(format!("controller-card-{focused}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        card_images.push(snapshot.as_bytes().to_vec());
    }
    assert_ne!(card_images[0], card_images[1], "focus must change the card rendering");
    // The selected stripe belongs at the left edge; a centered stripe or an
    // unrelated overlay obscuring the card must fail this fixture.
    fn pixel(image: &[u8], x: usize, y: usize) -> &[u8] { &image[(y * 1280 + x) * 4..(y * 1280 + x) * 4 + 4] }
    assert_eq!(pixel(&card_images[0], 218, 240), &[26, 159, 255, 255]);
    assert_eq!(pixel(&card_images[1], 230, 240), &[255, 255, 255, 255]);
    ui.global::<ControllerState>().set_card_fixture_open(false);
    let footer = ui.global::<FooterState>();
    let pad_hint = |key: &str, label: &str| {
        let icon = crate::assets::glyph_image(key);
        Hint {
            glyph: key.to_uppercase().into(), label: label.into(), kind: 1,
            has_icon: icon.is_some(), icon: icon.unwrap_or_default(),
        }
    };
    footer.set_left_hint(Hint { glyph: "STEAM".into(), label: "МЕНЮ".into(), kind: 3, ..Default::default() });
    let selection_hints = || model(vec![pad_hint("a", "ВЫБРАТЬ"), pad_hint("b", "НАЗАД")]);
    footer.set_hints(selection_hints());
    ui.global::<ControllerState>().set_browser_fixture_open(true);
    ui.global::<ControllerState>().set_browser_tabs(model(vec![
        ControllerBrowserTab { label: "РЕКОМЕНДАЦИИ".into(), x: 123.0, w: 282.0, ..Default::default() },
        ControllerBrowserTab { label: "РАСКЛАДКИ СООБЩЕСТВА".into(), x: 627.0, w: 392.0, ..Default::default() },
        ControllerBrowserTab { label: "ПОИСК".into(), x: 1044.0, w: 130.0, ..Default::default() },
    ]));
    ui.global::<ControllerState>().set_browser_cards(model(vec![
        ControllerConfigCard { y: 51.0, title: "Community controller layout".into(), author: "Fixture author".into(), description: "A long description for the controller layout fixture that intentionally wraps onto another line, so the list can exercise variable card height and text clipping.".into(), description_height: 48.0, status: "НЕ ЗАГРУЖЕНО".into(), votes: "92".into(), playtime: "52065 Ч. СЫГРАНО".into(), ..Default::default() },
        ControllerConfigCard { y: 200.0, title: "Second layout".into(), author: "Another author".into(), description: "Another description".into(), status: "НЕ ЗАГРУЖЕНО".into(), ..Default::default() },
    ]));
    let snapshot = ui.window().take_snapshot().expect("controller chooser snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-chooser.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    // Task 505: own-layouts tab (Мои раскладки) with the user's copies; fixture data, not a Gaming Mode capture.
    ui.global::<ControllerState>().set_browser_tabs(model(vec![
        ControllerBrowserTab { label: "МОИ РАСКЛАДКИ".into(), x: 123.0, w: 282.0, ..Default::default() },
        ControllerBrowserTab { label: "ШАБЛОНЫ".into(), x: 430.0, w: 200.0, ..Default::default() },
        ControllerBrowserTab { label: "РАСКЛАДКИ СООБЩЕСТВА".into(), x: 655.0, w: 392.0, ..Default::default() },
        ControllerBrowserTab { label: "ПОИСК".into(), x: 1070.0, w: 130.0, ..Default::default() },
    ]));
    ui.global::<ControllerState>().set_browser_cards(model(vec![
        ControllerConfigCard { y: 51.0, title: "Геймпад (копия)".into(), description: "Своя копия шаблона «Геймпад»".into(), selected: true, ..Default::default() },
        ControllerConfigCard { y: 200.0, title: "Для карточных игр".into(), description: "Своя раскладка".into(), ..Default::default() },
    ]));
    let snapshot = ui.window().take_snapshot().expect("controller own layouts snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-chooser-mine.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_browser_fixture_search(true);
    ui.global::<ControllerState>().set_browser_cards(model(vec![ControllerConfigCard { y: 101.0, title: "Fixture: Gamepad".into(), description: "Fixture search result".into(), ..Default::default() }]));
    let snapshot = ui.window().take_snapshot().expect("controller search snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-chooser-search.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_browser_fixture_search(false);
    ui.global::<ControllerState>().set_browser_fixture_open(false);
    ui.global::<ControllerState>().set_editor_fixture_open(true);
    ui.global::<ControllerState>().set_editor_title("Кнопки".into());
    ui.global::<ControllerState>().set_editor_group_title("Лицевые кнопки".into());
    ui.global::<ControllerState>().set_editor_sections(model(["Кнопки", "Крестовина", "Триггеры", "Джойстики", "Трекпады", "Гироскопы", "Виртуальное меню", "Наборы действий"].into_iter().map(|label| ControllerEditorSection { label: label.into(), ..Default::default() }).collect()));
    ui.global::<ControllerState>().set_editor_rows(model([("Функции", "Блок кнопок"), ("A", "Кнопка A"), ("B", "Кнопка B"), ("X", "Кнопка X"), ("Y", "Кнопка Y")].into_iter().enumerate().map(|(i, (label, value))| ControllerEditorRow { label: label.into(), value: value.into(), gear: true, y: i as f32 * 105.0, h: 96.0, ..Default::default() }).collect()));
    ui.global::<ControllerState>().set_editor_content_height(525.0);
    let snapshot = ui.window().take_snapshot().expect("controller editor snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-editor.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_editor_title("Джойстики".into());
    ui.global::<ControllerState>().set_editor_group_title("Правый джойстик".into());
    ui.global::<ControllerState>().set_editor_active_section(3);
    ui.global::<ControllerState>().set_editor_rows(model(vec![
        ControllerEditorRow { label: "Функции".into(), value: "Джойстик".into(), gear: true, y: 0.0, h: 96.0, ..Default::default() },
        ControllerEditorRow { label: "Нажатие".into(), value: "Нажатие на правый стик".into(), gear: true, y: 105.0, h: 126.0, ..Default::default() },
        ControllerEditorRow { label: "Касание".into(), value: "Добавить команду".into(), gear: true, y: 240.0, h: 126.0, ..Default::default() },
    ]));
    ui.global::<ControllerState>().set_editor_content_height(366.0);
    let snapshot = ui.window().take_snapshot().expect("controller stick editor snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-editor-stick.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_editor_title("Виртуальное меню".into());
    ui.global::<ControllerState>().set_editor_group_title("".into());
    ui.global::<ControllerState>().set_editor_active_section(6);
    ui.global::<ControllerState>().set_editor_rows_y(93.0);
    ui.global::<ControllerState>().set_editor_content_height(126.0);
    ui.global::<ControllerState>().set_editor_rows(model(vec![ControllerEditorRow { label: "".into(), value: "Добавить виртуальное меню".into(), value_width: 406.0, y: 0.0, h: 126.0, ..Default::default() }]));
    let snapshot = ui.window().take_snapshot().expect("controller virtual menu snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-virtual-menu.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_editor_title("Наборы действий".into());
    ui.global::<ControllerState>().set_editor_active_section(7);
    ui.global::<ControllerState>().set_editor_rows_y(96.0);
    ui.global::<ControllerState>().set_editor_content_height(217.0);
    ui.global::<ControllerState>().set_editor_rows(model(vec![ControllerEditorRow { label: "По умолчанию".into(), gear: true, gear_only: true, y: 0.0, h: 96.0, ..Default::default() }]));
    ui.global::<ControllerState>().set_editor_actions(model(vec![
        ControllerEditorAction { label: "Добавить набор действий".into(), x: 0.0, y: 126.0, w: 410.0, h: 90.0 },
        ControllerEditorAction { label: "Глобальные настройки набора".into(), x: 440.0, y: 126.0, w: 409.0, h: 90.0 },
    ]));
    let snapshot = ui.window().take_snapshot().expect("controller action sets snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-action-sets.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    // Editor pages built from a real native layout (task 508); skipped when the owner's dump is not on this machine.
    if let Some(raw) = std::env::var_os("HOME").and_then(|h| std::fs::read(std::path::Path::new(&h).join(".cache/deckshell/task-264/20-start-raw.bin")).ok()) {
        for (section, name) in [(0, "buttons"), (1, "dpad"), (2, "triggers"), (3, "joysticks"), (4, "trackpads"), (5, "gyro"), (6, "virtual-menu"), (7, "action-sets")] {
            let (rows, spans, content) = crate::screens::controller::page_model_raw(&raw, section);
            assert!(!rows.is_empty(), "section {section} has rows");
            assert_eq!(rows.len(), spans.len());
            let c = ui.global::<ControllerState>();
            c.set_editor_title(crate::screens::controller::section_title(section).into());
            c.set_editor_group_title("".into());
            c.set_editor_active_section(section as i32);
            c.set_editor_actions(model(Vec::<ControllerEditorAction>::new()));
            c.set_editor_rows_y(if section == 7 { 96.0 } else { 93.0 });
            c.set_editor_help(section == 7);
            c.set_editor_content_height(content);
            c.set_editor_focus_row(rows.iter().position(|r| !r.header).map_or(-1, |i| i as i32));
            c.set_editor_rows(model(rows));
            let snapshot = ui.window().take_snapshot().expect("controller editor page snapshot");
            assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
            if let Some(dir) = &output {
                image::save_buffer(dir.join(format!("controller-page-{section}-{name}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        }
    }
    // "Посмотреть раскладку" (view-layout/07): stock Balatro fixture rows, focus on the first row.
    {
        let c = ui.global::<ControllerState>();
        c.set_open(true);
        c.set_browser_open(false);
        c.set_card_fixture_open(false);
        c.set_editor_fixture_open(false);
        c.set_picker_fixture_open(false);
        c.set_settings_fixture_open(false);
        c.set_browser_fixture_open(false);
        c.set_title("Настройки контроллера для Balatro".into());
        c.set_tab(0);
        c.set_summary_rows(model(crate::screens::controller::summary_rows()));
        c.set_summary_device(crate::screens::controller::summary_device_art());
        c.set_summary_open(true);
        c.set_summary_focus(0);
        footer.set_hints(model(vec![pad_hint("b", "НАЗАД")]));
        let snapshot = ui.window().take_snapshot().expect("controller summary snapshot");
        assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
        assert_eq!(pixel(snapshot.as_bytes(), 640, 300), &[13, 19, 27, 255]);
        assert!((315..335).any(|x| (228..247).any(|y| pixel(snapshot.as_bytes(), x, y)[0] > 200)),
            "original L4 glyph should render in the grip binding row");
        if let Some(dir) = &output {
            image::save_buffer(dir.join("controller-summary.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        c.set_summary_open(false);
        footer.set_hints(selection_hints());
    }
    // Root page with the Balatro X360 values of Gaming Mode capture root/03 (tab X360 focused), then root/05 (card), root/06 (view), root/40m (Steam Deck tab).
    {
        let c = ui.global::<ControllerState>();
        c.set_open(true);
        c.set_browser_open(false);
        c.set_summary_open(false);
        c.set_card_fixture_open(false);
        c.set_editor_fixture_open(false);
        c.set_picker_fixture_open(false);
        c.set_settings_fixture_open(false);
        c.set_browser_fixture_open(false);
        c.set_title("Настройки контроллера для Balatro".into());
        c.set_author("LocalThunk".into());
        c.set_layout_name("Официальная раскладка Balatro - Геймпад".into());
        c.set_tab(1);
        c.set_rows(model(vec![CRow { kind: 0, gear: false, label: "Инвертировать ось Y правого джойстика".into(), value: "".into(), on: false, frac: 0.0, y: 0.0, h: 69.0 }]));
        c.set_content_h(78.0);
        for (name, foc, tab) in [("controller-root-03", 1, 1), ("controller-root-05-card", 3, 1), ("controller-root-06-view", 4, 1), ("controller-root-40m-deck-tab", 0, 0)] {
            c.set_foc(foc);
            c.set_tab(tab);
            slint::platform::update_timers_and_animations();
            let snapshot = ui.window().take_snapshot().expect("controller root snapshot");
            assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
            if let Some(dir) = &output {
                image::save_buffer(dir.join(format!("{name}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        }
    }
    {
        let c = ui.global::<ControllerState>();
        c.set_tab(0); c.set_foc(6); c.set_menu_page(1); c.set_menu_focus(0);
        let snapshot = ui.window().take_snapshot().expect("layout menu snapshot");
        if let Some(dir) = &output { image::save_buffer(dir.join("controller-layout-menu.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap(); }
        c.set_menu_page(2); c.set_menu_info_name("Геймпад".into()); c.set_menu_info_description("Образец описания".into()); c.set_menu_info_url("template://controller_neptune_gamepad_joystick.vdf".into());
        let snapshot = ui.window().take_snapshot().expect("layout information snapshot");
        if let Some(dir) = &output { image::save_buffer(dir.join("controller-layout-info.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap(); }
        c.set_menu_page(0);
    }
    ui.global::<ControllerState>().set_editor_fixture_open(false);
    ui.global::<ControllerState>().set_picker_fixture_open(true);
    let keys = controller_keyboard_fixture();
    for key in &keys {
        assert!(key.x >= 0.0 && key.x + key.w <= 1280.5 && key.y >= 0.0 && key.y + key.h <= 680.0);
    }
    for pair in keys.windows(2).filter(|pair| pair[0].y == pair[1].y) {
        assert!(pair[0].x + pair[0].w <= pair[1].x, "keyboard fixture keys overlap");
    }
    ui.global::<ControllerState>().set_picker_keys(model(keys));
    ui.global::<ControllerState>().set_picker_tabs(model([
        "ГЕЙМПАД", "МЫШЬ", "КЛАВИАТУРА", "ЦИФРОВАЯ КЛАВИАТУРА", "НАБОРЫ ДЕЙСТВИЙ", "СИСТЕМА", "КАМЕРА",
    ].into_iter().map(Into::into).collect()));
    let snapshot = ui.window().take_snapshot().expect("controller command picker snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-command-picker.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_picker_fixture_tab(1);
    ui.global::<ControllerState>().set_picker_fixture_title("Выберите команду мыши: Лицевые кнопки → A".into());
    let mouse = [
        ("Нажатие левой кнопки мыши", 211., 185.), ("Нажатие средней кнопки", 211., 260.),
        ("Нажатие правой кнопки мыши", 211., 335.), ("Нажатие 4-й кнопки мыши", 211., 434.),
        ("Нажатие 5-й кнопки мыши", 211., 509.), ("Колесо мыши вверх", 813., 185.),
        ("Колесо мыши вниз", 813., 260.), ("Перемещение в позицию", 813., 434.),
        ("Перемещение на значение", 813., 509.),
    ].into_iter().map(|(label, x, y)| ControllerPickerKey { label: label.into(), x, y, w: 252., h: 57., center: true, ..Default::default() }).collect();
    ui.global::<ControllerState>().set_picker_keys(model(mouse));
    let snapshot = ui.window().take_snapshot().expect("controller mouse picker snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-mouse-picker.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    {
        use crate::screens::controller_editor as editor;
        let c = ui.global::<ControllerState>();
        let gamepad_keys = editor::picker_keys(0).into_iter().enumerate().map(|(index, key)| {
            let (_, _, value) = editor::picker_command(0, index).expect("gamepad command");
            let binding = steam_bridge::controller_edit::gamepad_binding(value);
            ControllerPickerKey {
                label: key.label.into(), x: key.x, y: key.y, w: key.w, h: key.h,
                center: key.center, bottom: key.bottom, glyph_only: index >= 6,
                glyph: editor::command_glyph(&binding, false),
                focused_glyph: editor::command_glyph(&binding, true),
            }
        }).collect();
        c.set_picker_fixture_tab(0);
        c.set_picker_fixture_title("Выберите команду геймпада: Лицевые кнопки → A".into());
        c.set_picker_keys(model(gamepad_keys));
        let frame = ui.window().take_snapshot().expect("gamepad command picker snapshot");
        assert!((460..820).any(|x| (350..550).any(|y| pixel(frame.as_bytes(), x, y)[0] > 65)),
            "the gamepad artwork should render in the center of the gamepad tab");
        if let Some(dir) = &output {
            image::save_buffer(dir.join("controller-gamepad-picker.png"), frame.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
    }
    ui.global::<ControllerState>().set_picker_fixture_open(false);
    ui.global::<ControllerState>().set_settings_fixture_open(true);
    ui.global::<ControllerState>().set_setting_title("Чувствительность".into());
    ui.global::<ControllerState>().set_setting_rows(model(vec![
        ControllerSettingRow { kind: 0, label: "Горизонтальная чувствительность".into(), value: "100%".into(), fraction: 1.0, y: 0.0, h: 138.0, ..Default::default() },
        ControllerSettingRow { kind: 0, label: "Вертикальная чувствительность".into(), value: "100%".into(), fraction: 1.0, y: 147.0, h: 138.0, ..Default::default() },
        ControllerSettingRow { kind: 1, label: "Инверсия горизонтальной оси".into(), value: "".into(), fraction: 0.0, y: 294.0, h: 69.0, ..Default::default() },
        ControllerSettingRow { kind: 1, label: "Инверсия вертикальной оси".into(), value: "".into(), fraction: 0.0, y: 372.0, h: 69.0, ..Default::default() },
        ControllerSettingRow { kind: 2, label: "Кривая чувствительности стика".into(), value: "Линейная".into(), fraction: 0.0, y: 450.0, h: 96.0, ..Default::default() },
    ]));
    let snapshot = ui.window().take_snapshot().expect("controller settings snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    assert_eq!(pixel(snapshot.as_bytes(), 500, 234), &[26, 159, 255, 255]);
    assert_eq!(pixel(snapshot.as_bytes(), 1210, 234), &[255, 255, 255, 255]);
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-stick-settings.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_setting_title("Тактильная отдача".into());
    ui.global::<ControllerState>().set_setting_section(4);
    ui.global::<ControllerState>().set_setting_rows(model(vec![
        ControllerSettingRow { kind: 2, label: "Уровень тактильной отдачи".into(), value: "Выкл.".into(), y: 0.0, h: 96.0, ..Default::default() },
        ControllerSettingRow { kind: 2, label: "Триггеры тактильной отдачи при активации гироскопа".into(), description: "Выберите, когда должна срабатывать тактильная отдача.".into(), value: "При включении/выключении".into(), y: 105.0, h: 250.0, ..Default::default() },
    ]));
    let snapshot = ui.window().take_snapshot().expect("controller haptics snapshot");
    assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
    if let Some(dir) = &output {
        image::save_buffer(dir.join("controller-haptics.png"), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    ui.global::<ControllerState>().set_settings_fixture_open(false);
    ui.global::<ControllerState>().set_open(false);
    // Header geometry lint for the generated compact and expanded layouts:
    // the text and icon must remain inside the viewport and not overlap.
    for (text_right, icon_left, icon_right) in [(1280.0_f32 - 51.60, 1280.0 - 45.59, 1280.0 - 45.59 + 37.38),
                                                 (1280.0 - 63.60, 1280.0 - 57.59, 1280.0 - 57.59 + 37.38)] {
        assert!(text_right <= icon_left && icon_right <= 1280.0);
    }
    // Battery gallery uses the same top bar and stock SVG renderer as the live reader.
    ui.global::<Shell>().set_page(0);
    ui.global::<HotkeysState>().set_power_open(false);
    for (name, pct, online, full) in [
        ("full", 100, true, true), ("80", 80, false, false),
        ("20", 20, false, false), ("10", 10, false, false),
        ("charging", 50, true, false), ("not-charging", 50, false, false),
        ("unknown", -1, false, false),
    ] {
        let bar = ui.global::<TopBarState>();
        bar.set_bat_pct(if pct < 0 { -1 } else { pct.min(99) });
        bar.set_bat_show_pct(true);
        bar.set_bat_text(format!("{}%", bar.get_bat_pct()).into());
        bar.set_bat_full(full);
        bar.set_battery_icon(crate::assets::battery_icon(pct.max(0) as f64 / 100.0, online, full));
        bar.set_battery_icon_dark(crate::assets::battery_icon_dark(pct.max(0) as f64 / 100.0, online, full));
        let snapshot = ui.window().take_snapshot().expect("battery snapshot");
        assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
        if let Some(dir) = &output {
            image::save_buffer(dir.join(format!("battery-{name}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
    }
    game_bar_icons_survive_focus_move(&ui);
}

#[test]
fn top_search_background_stops_before_status_slots() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use std::cell::Cell;

    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            ..Default::default()
        },
    )))
    .unwrap();
    let ui = AppWindow::new().unwrap();
    let bar = ui.global::<TopBarState>();
    bar.set_search_query("Long search query ".repeat(20).into());
    ui.global::<SearchCaret>().set_before(bar.get_search_query());
    bar.set_battery_icon(crate::assets::battery_icon(0.38, false, false));
    bar.set_battery_icon_dark(crate::assets::battery_icon_dark(0.38, false, false));
    let clicked = Rc::new(Cell::new(-1));
    let target = Rc::clone(&clicked);
    bar.on_header_clicked(move |index| target.set(index));

    // Each row starts at the first visible status hit slot, rounded up to a whole pixel.
    for (width, page, sdcard, unread, wifi, status_left, status_target) in [
        (1280, 6, true, 1, 1, 973, 2),
        (1280, 6, true, 0, 1, 1033, 2),
        (1280, 6, false, 1, 1, 1033, 3),
        (640, 1, false, 0, 0, 453, 4),
        (640, 1, false, 0, -1, 510, 5),
    ] {
        ui.window().set_size(slint::PhysicalSize::new(width, 800));
        ui.global::<Shell>().set_page(page);
        bar.set_has_sdcard(sdcard);
        bar.set_unread(unread);
        bar.set_wifi(wifi);
        bar.set_bat_pct(38);
        bar.set_bat_show_pct(true);
        bar.set_bat_text(format!("{}%", bar.get_bat_pct()).into());
        ui.global::<OskState>().set_open(page == 6);
        bar.set_search_active(false);
        let _ = ui.window().take_snapshot().expect("initial search header");
        // Route entry includes a 200 ms delay and a 600 ms fade. Compare
        // only after the page behind the bar has settled.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(850));
        slint::platform::update_timers_and_animations();
        let inactive = ui.window().take_snapshot().expect("inactive search header");
        bar.set_search_active(true);
        let _ = ui.window().take_snapshot().expect("start search animation");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(250));
        slint::platform::update_timers_and_animations();
        let active = ui.window().take_snapshot().expect("active search header");
        assert_eq!((active.width(), active.height()), (width, 800));

        let stride = width as usize * 4;
        let status_start = status_left * 4;
        for (y, (before, after)) in inactive
            .as_bytes()
            .chunks_exact(stride)
            .zip(active.as_bytes().chunks_exact(stride))
            .take(60)
            .enumerate()
        {
            for (dx, (before, after)) in before[status_start..].chunks_exact(4)
                .zip(after[status_start..].chunks_exact(4)).enumerate()
            {
                assert_eq!(before, after, "search changed the status area at {width}px, page {page}, x={}, y={y}", status_left + dx);
            }
        }
        let field_pixel = 4 * stride + (status_left - 2) * 4;
        assert_eq!(
            &active.as_bytes()[field_pixel..field_pixel + 4],
            &[255; 4],
            "the active field should fill the space before the status area"
        );

        for (x, expected) in [(status_left - 3, 1), (status_left + 3, status_target)] {
            clicked.set(-1);
            let position = slint::LogicalPosition::new(x as f32, 30.0);
            ui.window().dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            ui.window().dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
            assert_eq!(clicked.get(), expected, "incorrect header target at x={x}");
        }
    }
}

/// Lint the painted bar at both supported layout extremes using distinct
/// solid fixtures. Slot edges also exercise the actual touch targets.
#[test]
fn top_bar_overlap_lint() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
    use std::{cell::Cell, time::Duration};

    let (ui, _) = crate::screens::osk::field_tests::app();
    let bar = ui.global::<TopBarState>();
    let icons = ui.global::<crate::Icons>();
    let colors = [
        [17, 203, 119, 255], // search
        [237, 31, 101, 255], // download
        [219, 181, 19, 255], // SD
        [193, 53, 241, 255], // bell
        [69, 153, 233, 255], // Wi-Fi
        [199, 83, 17, 255],  // battery
    ];
    let solid = |rgba: [u8; 4]| {
        Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, 1, 1))
    };
    icons.on_tinted(|icon, _| icon);
    icons.set_search(solid(colors[0]));
    bar.set_dl_icon(solid(colors[1]));
    bar.set_dl_has_icon(true);
    bar.set_dl_progress(0.5);
    icons.set_sdcard(solid(colors[2]));
    icons.set_bell(solid(colors[3]));
    icons.set_wifi_levels(model(vec![solid(colors[4]); 5]));
    icons.set_wifi_off(solid(colors[4]));
    bar.set_battery_icon(solid(colors[5]));
    bar.set_search_placeholder("Поиск по играм или профилям…".into());
    let clicked = Rc::new(Cell::new(-1));
    let target = clicked.clone();
    bar.on_header_clicked(move |id| target.set(id));
    let click = |x, y| {
        clicked.set(-1);
        let position = slint::LogicalPosition::new(x, y);
        let button = PointerEventButton::Left;
        ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
        ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
        clicked.get()
    };
    let frame_after = |ms| {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ms));
        slint::platform::update_timers_and_animations();
        ui.window().take_snapshot().expect("header frame")
    };

    let at = |width: u32, x: usize, y: usize| (y * width as usize + x) * 4;
    for width in [640, 1280] {
        ui.window().set_size(slint::PhysicalSize::new(width, 400));
        for (page, pad) in [(0, 15), (1, 0)] {
            ui.global::<Shell>().set_page(page);
            frame_after(0);
            frame_after(420);
            for mask in 0..16 {
                let has = |bit| mask & (1 << bit) != 0;
                bar.set_dl_active(has(0));
                bar.set_has_sdcard(has(1));
                bar.set_unread(i32::from(has(2)));
                bar.set_bat_pct(if has(3) { 77 } else { -1 });
                bar.set_bat_show_pct(true);
                bar.set_bat_text(format!("{}%", bar.get_bat_pct()).into());
                for wifi in [-1, 0, 1] {
                    bar.set_wifi(wifi);
                    let mut right = (width - pad) as f32;
                    let ordered = [
                        (has(3), 130.375, 5, colors[5]),
                        (wifi >= 0, 56.875, 4, colors[4]),
                        (has(2), 60., 3, colors[3]),
                        (has(1), 60., 2, colors[2]),
                        (has(0), 60., 6, colors[1]),
                    ];
                    let slots: Vec<_> = ordered.into_iter().map(|(visible, w, id, color)| {
                        let slot_right = right;
                        if visible { right -= w; }
                        (visible, right, slot_right, id, color)
                    }).collect();
                    let search_right = right;
                    for (phase, (focused, active, delay)) in [
                        (false, false, 250), (true, false, 100), (true, false, 250),
                        (false, true, 0), (false, false, 100), (false, false, 250),
                    ].into_iter().enumerate() {
                        bar.set_bat_full(phase >= 3);
                        bar.set_dl_muted(phase == 4);
                        bar.set_dl_progress(if phase == 5 { -1. } else { 0.5 });
                        bar.set_hfocus(if focused { 1 } else { -1 });
                        bar.set_search_active(active);
                        frame_after(0);
                        let frame = frame_after(delay);
                        let rgba = frame.as_bytes();
                        let y = pad as usize + 30;
                        let line = &rgba[y * width as usize * 4..(y + 1) * width as usize * 4];
                        let span = |color| {
                            let mut xs = line.chunks_exact(4).enumerate()
                                .filter_map(|(x, px)| (px == color).then_some(x));
                            let first = xs.next()?;
                            Some((first as f32, xs.last().unwrap_or(first) as f32 + 1.))
                        };
                        for &(visible, left, right, id, color) in &slots {
                            let span = span(color);
                            assert_eq!(span.is_some(), visible, "width={width} page={page} mask={mask} wifi={wifi} item={id}");
                            if let Some((first, end)) = span {
                                assert!(first >= left && end <= right, "item {id} crosses its slot");
                                assert_eq!(click(left + 1., y as f32), id, "left edge of item {id}");
                                assert_eq!(click(right - 1., y as f32), id, "right edge of item {id}");
                            }
                        }
                        let (first, end) = span(colors[0]).expect("visible magnifier");
                        assert!(first >= 72. + pad as f32 && end <= search_right);
                        assert_eq!(click(first + 1., y as f32), 1);
                        assert_eq!(click(search_right - 1., y as f32), 1);
                        let x = at(width, search_right as usize - 2, pad as usize + 4);
                        let plate = &rgba[x..x + 4];
                        if active || focused && delay == 250 {
                            assert_eq!(plate, &[255; 4]);
                        } else if delay == 250 {
                            assert_ne!(plate, &[255; 4]);
                        }
                        if has(0) {
                            let x = at(width, search_right as usize + 16, pad as usize + 48);
                            let progress = &rgba[x..x + 4];
                            match phase {
                                4 => assert_eq!(progress, &[139, 146, 154, 255]),
                                5 => assert_ne!(progress, &[26, 159, 255, 255]),
                                _ => assert_eq!(progress, &[26, 159, 255, 255]),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Renders Home with the focused recents cover at `idx` and every card's `glow` set to `glow`.
/// Returns the software-rendered frame bytes.
fn render_home_frame(idx: i32, glow: slint::Color) -> Vec<u8> {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let cards: Vec<CardData> = (0..5).map(|i| CardData {
        id: i, name: format!("Fixture game {i}").into(), caption: format!("Fixture game {i}").into(),
        x: i as f32 * 155., w: 145., glow,
        ..Default::default()
    }).collect();
    ui.global::<HomeState>().set_recent(model(cards));
    ui.global::<HomeState>().set_idx(idx);
    ui.global::<Shell>().set_page(0);
    ui.global::<HomeState>().set_zone(0);
    let _ = ui.window().take_snapshot().expect("initial snapshot");
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
    slint::platform::update_timers_and_animations();
    ui.window().take_snapshot().expect("software snapshot").as_bytes().to_vec()
}

/// Regression: a focused Home cover must not paint a backing plate. The per-game `glow` colour is
/// the only input that could add one, so changing it must leave every pixel of the frame unchanged,
/// for both the wide first tile and a portrait tile.
#[test]
fn home_focus_plate_ignores_glow() {
    let vivid = slint::Color::from_rgb_u8(255, 0, 0);
    for idx in [0, 1] {
        let without = render_home_frame(idx, slint::Color::from_argb_u8(0, 0, 0, 0));
        let with = render_home_frame(idx, vivid);
        let differing = without.iter().zip(&with).filter(|(a, b)| a != b).count();
        assert_eq!(without.len(), with.len(), "idx {idx}: frame size");
        assert_eq!(differing, 0, "idx {idx}: glow colour changed {differing} bytes of the Home frame (backing plate)");
    }
}

/// Owner, 2026-11-12: with focus on the game bar's gear, the controller glyph broke. The glyph is
/// unfocused in both frames below, so its pixels must not change when focus moves to the gear.
/// Geometry from ui/generated/game/game.slint: the controller cell is at `width - 163.05px`, glyph at +14.39px, 28.8px.
fn game_bar_icons_survive_focus_move(ui: &AppWindow) {
    ui.global::<HotkeysState>().set_power_open(false);
    ui.global::<QamState>().set_open(false);
    ui.global::<SteamMenuState>().set_open(false);
    ui.global::<SteamMenuState>().set_game_visible(false);
    // Icons are white alpha masks; synthetic ones keep this independent of the extracted Steam assets.
    let mask = |w: u32, h: u32| {
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
            let on = ((i as u32 / w) + (i as u32 % w)) % 3 != 0;
            *p = slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: if on { 255 } else { 0 } };
        }
        slint::Image::from_rgba8(buf)
    };
    ui.global::<Icons>().on_tinted(crate::assets::tint_image);
    ui.global::<Icons>().set_controller(mask(28, 28));
    ui.global::<Icons>().set_settings(mask(28, 28));
    ui.global::<Shell>().set_page(2);
    ui.global::<GameState>().set_zone(0);
    ui.global::<GameState>().set_menu_open(false);
    // Cell backgrounds animate (120 ms); settle them fully before each snapshot.
    let frame = |focus: i32| {
        ui.global::<GameState>().set_focus(focus);
        for _ in 0..4 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
            slint::platform::update_timers_and_animations();
        }
        ui.window().take_snapshot().expect("game bar snapshot")
    };
    let none = frame(-1);
    let on_gear = frame(2);
    // Pixel rows and columns of the glyph box (a few px of margin) and of the gear glyph box.
    let region = |img: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x0: u32, x1: u32| -> Vec<u8> {
        let stride = img.width() as usize * 4;
        (684..726).flat_map(|y| {
            let row = y as usize * stride;
            img.as_bytes()[row + x0 as usize * 4..row + x1 as usize * 4].to_vec()
        }).collect()
    };
    let (a, b) = (region(&none, 1128, 1164), region(&on_gear, 1128, 1164));
    let changed: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let bbox = changed.iter().fold(None, |acc: Option<(usize, usize, usize, usize)>, &i| {
        let (x, y) = (1128 + (i / 4) % 36, 684 + (i / 4) / 36);
        Some(acc.map_or((x, y, x, y), |(x0, y0, x1, y1)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y))))
    });
    assert!(changed.is_empty(), "controller glyph changed when focus moved to the gear: {} bytes differ in box {bbox:?}", changed.len());
    assert_ne!(region(&none, 1194, 1226), region(&on_gear, 1194, 1226), "gear focus must be visible, otherwise the test sees nothing");
}

/// Owner, 2026-11-12 (build 6c5c27f): opening and closing the side menu with the Steam button many times
/// broke the Settings icon; moving focus Home -> Library -> Home also changes the icons' colour. This runs
/// the real side menu with the production `Icons.tinted` wiring for N open/close cycles and requires:
/// every opening (focus on Home) renders the same frame as the first one, and the frame after the
/// focus round-trip matches it again. Focus on Library must visibly change the frame, so the check is real.
#[test]
fn side_menu_open_close_cycles_render_identically() {
    const CYCLES: usize = 60;
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).ok();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    // Synthetic white alpha masks with a non-uniform pattern, so every icon is distinguishable.
    let mask = |seed: u32| {
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(24, 24);
        for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
            let on = ((i as u32 / 24) * 7 + (i as u32 % 24) * 3 + seed) % 4 != 0;
            *p = slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: if on { 255 } else { 0 } };
        }
        slint::Image::from_rgba8(buf)
    };
    let icons = ui.global::<Icons>();
    icons.on_tinted(crate::assets::tint_image);
    icons.set_home(mask(1));
    icons.set_library(mask(2));
    icons.set_settings(mask(3));
    ui.global::<Shell>().set_page(0);
    let names = ["Home", "Library", "Downloads", "Settings", "Power"];
    ui.global::<SteamMenuState>().set_items(model(names.iter().enumerate().map(|(i, s)| MenuItem {
        label: (*s).into(), icon: mask(i as u32 + 1), color_icon: false, indent: false, current: i == 0, ..Default::default()
    }).collect()));
    ui.global::<SteamMenuState>().set_row_y(model(vec![52.5, 124.5, 196.5, 268.5, 340.5]));
    ui.global::<SteamMenuState>().set_content_h(681.);
    ui.global::<SteamMenuState>().set_game_visible(false);
    ui.global::<SteamMenuState>().set_game_focus(-1);
    ui.global::<SteamMenuState>().set_focus(0);
    let settle = |ui: &AppWindow| -> Vec<u8> {
        for _ in 0..3 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(700));
            slint::platform::update_timers_and_animations();
        }
        ui.window().take_snapshot().expect("side menu snapshot").as_bytes().to_vec()
    };
    let set_open = |ui: &AppWindow, open: bool| ui.global::<SteamMenuState>().set_open(open);

    set_open(&ui, true);
    let baseline = settle(&ui);
    let baseline_library = {
        ui.global::<SteamMenuState>().set_focus(1);
        let f = settle(&ui);
        ui.global::<SteamMenuState>().set_focus(0);
        f
    };
    assert_ne!(baseline, baseline_library, "focus on Library must change the side menu, otherwise the check sees nothing");
    set_open(&ui, false);
    settle(&ui);

    for cycle in 1..=CYCLES {
        set_open(&ui, true);
        let opened = settle(&ui);
        let diff = opened.iter().zip(&baseline).filter(|(a, b)| a != b).count();
        assert_eq!(diff, 0, "cycle {cycle}: opened side menu differs from the first opening in {diff} bytes");
        ui.global::<SteamMenuState>().set_focus(1);
        settle(&ui);
        ui.global::<SteamMenuState>().set_focus(0);
        let back = settle(&ui);
        let diff = back.iter().zip(&baseline).filter(|(a, b)| a != b).count();
        assert_eq!(diff, 0, "cycle {cycle}: focus Library->Home changed {diff} bytes of the side menu");
        set_open(&ui, false);
        settle(&ui);
    }
    set_open(&ui, true);
    let final_frame = settle(&ui);
    assert!(final_frame == baseline, "after {CYCLES} cycles the side menu no longer renders as at first opening");
}

/// Hero crop editor (sgdb): the pan must move the picture even at zoom 1 (the cover overflows the frame), and zoom and the button
/// focus must redraw too. With DECKSHELL_GALLERY_DIR the editor frames are written as PNGs for a layout check at 1280x800.
#[test]
fn hero_editor_pan_and_focus_redraw() {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    // 2:1 striped banner: bars every 40 px, so any horizontal shift is visible
    let (w, h) = (960u32, 310u32);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let bar = (x / 40) % 2 == 0;
        *p = slint::Rgba8Pixel { r: if bar { 200 } else { 60 }, g: (y * 255 / h) as u8, b: if bar { 60 } else { 200 }, a: 255 };
    }
    let s = ui.global::<SgdbState>();
    s.set_open(true);
    s.set_hero(slint::Image::from_rgba8(buf));
    s.set_hero_has(true);
    s.set_hero_edit_open(true);
    s.set_hero_edit_info("Масштаб 2.0×\nСдвиг по X +100%\nСдвиг по Y +0%".into());
    let output = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &output { std::fs::create_dir_all(dir).unwrap(); }
    let mut shot = |name: &str, zoom: f32, pan_x: f32, pan_y: f32, focus: i32| -> Vec<u8> {
        s.set_edit_crop(HeroFrame { zoom, pan_x, pan_y });
        s.set_hero_focus(focus);
        let snap = ui.window().take_snapshot().expect("hero editor snapshot");
        if let Some(dir) = &output {
            image::save_buffer(dir.join(format!("{name}.png")), snap.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        snap.as_bytes().to_vec()
    };
    let differing = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    let centre = shot("hero-zoom1-centre", 1.0, 0.0, 0.0, 0);
    let left = shot("hero-zoom1-pan-left", 1.0, -1.0, 0.0, 0);
    let right = shot("hero-zoom1-pan-right", 1.0, 1.0, 0.0, 0);
    let zoomed = shot("hero-zoom2-pan-right", 2.0, 1.0, 0.0, 0);
    let focused = shot("hero-zoom2-focus-plus", 2.0, 1.0, 0.0, 5);
    assert!(differing(&centre, &left) > 1000, "pan -1 at zoom 1 must move the picture");
    assert!(differing(&left, &right) > 1000, "pan +1 must differ from pan -1 at zoom 1");
    assert!(differing(&zoomed, &right) > 1000, "zoom 2 must redraw the picture");
    assert!(differing(&zoomed, &focused) > 0, "moving the focus ring must redraw the editor buttons");
}

/// Phone send panel in the media viewer («Отправить на телефон»): busy stages, sent and failed. Each state is
/// rendered once; the panel must change the frame against the hidden state, and each stage must change the bar.
/// With DECKSHELL_GALLERY_DIR set, full frames and bottom crops are written there (PC render, not a Deck test).
#[test]
fn media_phone_panel_renders() {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    ui.global::<Shell>().set_page(5);
    let m = ui.global::<MediaState>();
    m.set_title("Media".into());
    m.set_viewer_open(true);
    m.set_viewer_has(false);
    m.set_viewer_title("Fixture game: Balatro".into());
    m.set_viewer_sub("Скриншот  -  сегодня".into());
    m.set_viewer_actions(model(vec!["Поделиться".into(), "Отправить на телефон…".into(), "Удалить".into()]));
    m.set_viewer_focus(1);
    let output = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &output { std::fs::create_dir_all(dir).unwrap(); }
    // Bottom 240 rows: the panel sits 180 px above the bottom edge, so this crop holds it and the action row.
    let crop = |bytes: &[u8]| -> Vec<u8> { bytes[(560 * 1280 * 4)..(800 * 1280 * 4)].to_vec() };
    let mut shot = |name: &str, tone: i32, stage: i32, text: &str| -> Vec<u8> {
        m.set_phone_tone(tone);
        m.set_phone_stage(stage);
        m.set_phone_text(text.into());
        let _ = ui.window().take_snapshot().expect("phone panel initial snapshot");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        let snap = ui.window().take_snapshot().expect("phone panel snapshot");
        assert_eq!((snap.width(), snap.height()), (1280, 800), "{name}");
        if let Some(dir) = &output {
            image::save_buffer(dir.join(format!("{name}.png")), snap.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            image::save_buffer(dir.join(format!("{name}-crop.png")), &crop(snap.as_bytes()), 1280, 240, image::ColorType::Rgba8).unwrap();
        }
        snap.as_bytes().to_vec()
    };
    let hidden = shot("media-phone-hidden", 0, 0, "");
    let check = shot("media-phone-busy-check", 1, 0, "Проверка мобильного устройства…");
    let export = shot("media-phone-busy-export", 1, 1, "Экспорт...");
    let upload = shot("media-phone-busy-upload", 1, 2, "Загрузка…");
    let sent = shot("media-phone-sent", 2, 3, "Отправлено. Ролик придёт уведомлением в мобильном приложении Steam.");
    let failed = shot("media-phone-failed", 3, 0, "Отправка не удалась: upload failed. Ничего не отправлено.");
    let differs = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    let panel = |a: &[u8], b: &[u8], what: &str| assert!(differs(&a[560 * 1280 * 4..700 * 1280 * 4], &b[560 * 1280 * 4..700 * 1280 * 4]) > 0, "{what}: panel did not render");
    panel(&hidden, &check, "busy panel vs hidden");
    panel(&hidden, &failed, "failed panel vs hidden");
    assert!(differs(&check, &export) > 0 && differs(&export, &upload) > 0, "stage bar does not move");
    assert!(differs(&upload, &sent) > 0, "sent state does not change the panel");
}

/// Media grid with real-size thumbnails (same row geometry as `screens/media.rs`, page.json geo). The grid must reach the
/// footer hint row: no reserved plate band above the footer. Thumbnails are solid fixtures; the bottom band is sampled.
/// With DECKSHELL_GALLERY_DIR set, the full frame is written there (PC render, not a Deck test).
#[test]
fn media_grid_fills_to_footer_with_thumbnails() {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    ui.global::<Shell>().set_page(5);
    let (x0, y0, px, py, w, h, iw, ih, cols) = (24.0_f32, 56.0_f32, 342.5_f32, 196.0_f32, 335.0_f32, 188.0_f32, 334.0_f32, 187.0_f32, 3usize);
    const SHOT: (u8, u8, u8) = (40, 120, 200);
    let mut thumb = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(334, 187);
    thumb.make_mut_slice().iter_mut().for_each(|p| *p = slint::Rgba8Pixel { r: SHOT.0, g: SHOT.1, b: SHOT.2, a: 255 });
    let thumb = slint::Image::from_rgba8(thumb);
    let count = 12usize;
    let mut rows: Vec<GPageRow> = vec![];
    for n in 0..count {
        let (c, r) = ((n % cols) as f32, (n / cols) as f32);
        let (x, y) = (x0 + c * px, y0 + r * py);
        let label = format!("Fixture shot {n}");
        let base = crate::screens::settings::PRow { role: "button".into(), x, y, w, h, lab: label.clone(), ..Default::default() };
        let mut outer = crate::screens::settings::to_grow(&crate::screens::settings::PRow { k: 0, pad: "touch,mouse".into(), ..base.clone() }, 0);
        outer.label = label.as_str().into();
        rows.push(outer);
        let mut inner = crate::screens::settings::to_grow(&crate::screens::settings::PRow { k: 8, pad: "A,START,FOCUS,touch,mouse,long_press".into(), w: iw, h: ih, ..base }, 0);
        inner.label = label.as_str().into();
        inner.has_img = true;
        inner.img = thumb.clone();
        rows.push(inner);
    }
    let rowsn = (count + cols - 1) / cols;
    let m = ui.global::<MediaState>();
    m.set_title("Media".into());
    m.set_cards(model(rows));
    m.set_focus(-1);
    m.set_content_h(((y0 + rowsn as f32 * py) * crate::screens::settings::SC).max(588.0 * crate::screens::settings::SC).into());
    m.set_wrap_pad("B,X,Y,FOCUS,keyboard".into());
    let snap = ui.window().take_snapshot().expect("media grid snapshot");
    assert_eq!((snap.width(), snap.height()), (1280, 800));
    if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(dir.join("media-grid-thumbs.png"), snap.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
    // Row 3 thumbnail body spans ~537..737 px; the old 100 px plate reserve ended the grid at 650 px, so y=700/730 must be
    // thumbnail pixels. Row 4 starts below the footer (~775 px), so the label strip at 737..750 is the last visible band.
    let px_at = |x: usize, y: usize| -> (u8, u8, u8) { let i = (y * 1280 + x) * 4; (snap.as_bytes()[i], snap.as_bytes()[i + 1], snap.as_bytes()[i + 2]) };
    for y in [700usize, 730] {
        assert_eq!(px_at(400, y), SHOT, "thumbnail must reach the footer at y={y}; reserved band still visible");
    }
}

/// Виртуальная клавиатура: PNG ключевых состояний (QWERTY, Shift, русская, эмодзи, наведение тачпадом) 1280x800 для сравнения
/// со скриншотами Деки. С DECKSHELL_GALLERY_DIR кадры сохраняются; проверяются цвета фона, клавиш и фокуса.
#[test]
fn osk_states_render() {
    use crate::input::Action;
    use crate::screens::osk::{fixture_focus_input, fixture_state};
    use crate::screens::osk_model::{Act, Mode, CAT_STEAM};
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).unwrap();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    ui.global::<Icons>().on_tinted(crate::assets::tint_image);
    let output = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    let snap = |ui: &AppWindow, name: &str| -> Vec<u8> {
        let s = ui.window().take_snapshot().expect("osk snapshot");
        assert_eq!((s.width(), s.height()), (1280, 800));
        if let Some(dir) = &output {
            image::save_buffer(dir.join(name), s.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        s.as_bytes().to_vec()
    };
    let px = |b: &[u8], x: usize, y: usize| [b[(y * 1280 + x) * 4], b[(y * 1280 + x) * 4 + 1], b[(y * 1280 + x) * 4 + 2]];
    fixture_state(&ui, |_| {}, false, None, false);
    let b = snap(&ui, "osk-qwerty.png");
    assert_eq!(px(&b, 640, 446), [35, 38, 46], "фон панели #23262E");
    assert_eq!(px(&b, 60, 470), [14, 20, 27], "клавиша #0E141B");
    assert_eq!(px(&b, 50, 560), [0, 0, 0], "Tab чёрная");
    fixture_state(&ui, |_| {}, true, None, false);
    let b = snap(&ui, "osk-qwerty-focus-g.png");
    assert_eq!(px(&b, 520, 600), [139, 146, 154], "приглушённый фокус на g");
    fixture_state(&ui, |_| {}, true, None, true);
    let b = snap(&ui, "osk-qwerty-press-g.png");
    assert_eq!(px(&b, 520, 600), [255, 255, 255], "нажатие на g белое");
    fixture_state(&ui, |k| k.shift = 2, false, None, false);
    let b = snap(&ui, "osk-qwerty-shift.png");
    assert_eq!(px(&b, 20, 706), [26, 159, 255], "Shift закреплён синий");
    fixture_state(&ui, |k| k.shift = 1, false, None, false);
    let b = snap(&ui, "osk-qwerty-shift-oneshot.png");
    assert_eq!(px(&b, 60, 680), [35, 38, 46], "OneShot: фон клавиши снят (011624)");
    fixture_state(&ui, |k| k.layout = 1, false, None, false);
    snap(&ui, "osk-russian.png");
    fixture_state(&ui, |k| { k.layout = 1; k.shift = 1; }, true, None, false);
    snap(&ui, "osk-russian-shift.png");
    fixture_state(&ui, |k| { k.mode = Mode::Emoji; k.cat = CAT_STEAM + 1; k.focus_initial(); }, true, None, false);
    let b = snap(&ui, "osk-emoji.png");
    assert_eq!(px(&b, 575, 470), [61, 68, 80], "выбранная категория #3D4450");
    fixture_state(&ui, |k| { k.mode = Mode::Emoji; k.cat = 0; k.focus_initial(); }, false, None, false);
    snap(&ui, "osk-emoji-steam-items.png");
    fixture_state(&ui, |k| {
        let i = k.keys().iter().position(|x| x.def.act == Act::Char("e".into())).unwrap();
        k.focus = i;
        k.open_ext(i, None);
    }, true, None, false);
    let b = snap(&ui, "osk-extended-e.png");
    assert_eq!(px(&b, 400, 458), [26, 159, 255], "ряд вариантов синий над клавишей");
    fixture_state(&ui, |_| {}, true, Some((false, -0.2, 0.1)), false);
    let hover = snap(&ui, "osk-pad-left-hover.png");
    assert_eq!(px(&hover, 520, 600), [14, 20, 27], "левый тачпад скрывает фокус g");
    fixture_state(&ui, |_| {}, true, Some((true, 0.0, 0.1)), false);
    let hover = snap(&ui, "osk-pad-right-hover.png");
    assert_eq!(px(&hover, 520, 600), [14, 20, 27], "правый тачпад скрывает фокус g");
    assert_eq!(px(&hover, 970, 636), [184, 188, 191]);
    fixture_focus_input(&ui, Action::Y);
    let restored = snap(&ui, "osk-focus-restored.png");
    assert_eq!(px(&restored, 520, 600), [139, 146, 154], "кнопка возвращает фокус g");
    fixture_focus_input(&ui, Action::KbPad { right: true, x: 100, y: 3200, on: true });
    let moved = snap(&ui, "osk-focus-hidden-on-motion.png");
    assert_eq!(px(&moved, 520, 600), [14, 20, 27], "движение в пределах той же клавиши скрывает фокус");
    assert_eq!(px(&moved, 970, 636), [184, 188, 191]);
    fixture_state(&ui, |_| {}, true, Some((true, 0.0, 0.1)), true);
    let press = snap(&ui, "osk-pad-right-press.png");
    assert_eq!(px(&press, 970, 636), [255, 255, 255]);
}

/// Focus zone render test (owner reference: stock screenshots 2, 7-10 of next-wave2). Three states of Home at 1280x800:
/// 1. default - focus on a tile, bar transparent (no strip, tile ring, row bright);
/// 2. bar focus on each element - dark strip over the full width, solid white block under the focused item, row dimmed, no tile ring;
/// 3. search focus - the expanded white field, strip and dimmed row stay.
/// PNGs go to DECKSHELL_GALLERY_DIR when it is set.
#[test]
fn top_bar_focus_zone_states() {
    use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
    let (ui, _) = crate::screens::osk::field_tests::app();
    let bar = ui.global::<TopBarState>();
    let icons = ui.global::<crate::Icons>();
    icons.on_tinted(|icon, _| icon);
    let solid = |rgba: [u8; 4], w: u32, h: u32| {
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        buf.make_mut_slice().fill(Rgba8Pixel { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] });
        Image::from_rgba8(buf)
    };
    let icon = solid([40, 40, 40, 255], 8, 8);
    icons.set_search(icon.clone());
    icons.set_bell(icon.clone());
    icons.set_wifi_levels(model(vec![icon.clone(); 5]));
    icons.set_sdcard(icon);
    bar.set_unread(1);
    bar.set_wifi(1);
    bar.set_bat_pct(90);
    bar.set_bat_show_pct(true);
    bar.set_bat_text(format!("{}%", bar.get_bat_pct()).into());
    bar.set_battery_icon(crate::assets::battery_icon(0.9, false, false));
    bar.set_battery_icon_dark(crate::assets::battery_icon_dark(0.9, false, false));
    bar.set_clock("3:17 PM".into());
    bar.set_clock_12h(true);
    bar.set_search_placeholder("Поиск по играм или профилям…".into());
    let art = solid([200, 150, 90, 255], 64, 96);
    let cards: Vec<CardData> = (0..5).map(|i| CardData {
        id: i, name: format!("Fixture game {i}").into(), img: art.clone(), has_img: true, wide: art.clone(), has_wide: true, ..Default::default()
    }).collect();
    ui.global::<HomeState>().set_recent(model(cards));
    ui.global::<HomeState>().set_idx(1);
    ui.global::<HomeState>().set_zone(0);
    ui.global::<Shell>().set_page(0);
    let output = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &output { std::fs::create_dir_all(dir).unwrap(); }
    let frame = |hf: i32| {
        bar.set_hfocus(hf);
        for _ in 0..4 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
            slint::platform::update_timers_and_animations();
        }
        ui.window().take_snapshot().expect("focus zone frame")
    };
    let px = |f: &slint::SharedPixelBuffer<Rgba8Pixel>, x: u32, y: u32| { let p = f.as_slice()[(y * 1280 + x) as usize]; [p.r, p.g, p.b] };
    let save = |f: &slint::SharedPixelBuffer<Rgba8Pixel>, name: &str| {
        if let Some(dir) = &output { image::save_buffer(dir.join(format!("topbar-zone-{name}.png")), f.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap(); }
    };
    let lum = |f: &slint::SharedPixelBuffer<Rgba8Pixel>, x: u32, y: u32| { let p = px(f, x, y); p[0] as u32 + p[1] as u32 + p[2] as u32 };

    let base = frame(-1);
    save(&base, "default");
    assert!(lum(&base, 695, 550) > 300, "default: the focused tile has its ring");
    // the strip is absent: a pixel inside the would-be strip equals the one at the same x just below it, up to art
    let strip_probe = lum(&base, 640, 70);

    for (hf, name) in [(0, "avatar"), (2, "sdcard"), (3, "bell"), (4, "wifi"), (5, "battery")] {
        bar.set_has_sdcard(true);
        let f = frame(hf);
        save(&f, name);
        assert!(lum(&f, 640, 70) < strip_probe || strip_probe == 0, "{name}: dark strip over the art");
        assert!(lum(&f, 695, 550) < lum(&base, 695, 550) * 7 / 10, "{name}: no tile keeps its ring");
        assert!(lum(&f, 800, 500) < lum(&base, 800, 500) * 8 / 10, "{name}: the row is dimmed");
        // task 462: the dim is one uniform layer - no horizontal edge where it used to start (y 274.5)
        for x in [100u32, 400, 640, 900, 1200] {
            for y in 270..280u32 {
                let (u, d) = (lum(&f, x, y) as i32, lum(&f, x, y + 1) as i32);
                assert!((u - d).abs() <= 6, "{name}: dim edge at x {x}, y {y}->{}: {u} vs {d}", y + 1);
            }
            let (above, below) = (lum(&f, x, 270) as f32, lum(&f, x, 280) as f32);
            let (b_above, b_below) = (lum(&base, x, 270) as f32, lum(&base, x, 280) as f32);
            if b_above > 30. && b_below > 30. { assert!((above / b_above - below / b_below).abs() < 0.05, "{name}: unequal dimming above/below y 275 at x {x}"); }
        }
        // the white block spans the row height from the top of the row (y 15..75) and is solid white
        let top = (15..75u32).filter(|&y| (0..1280u32).any(|x| px(&f, x, y) == [255, 255, 255])).count();
        assert_eq!(top, 60, "{name}: the white block fills the full row height");
    }
    bar.set_has_sdcard(false);
    let search = frame(1);
    save(&search, "search");
    assert_eq!(px(&search, 600, 20), [255, 255, 255], "search: expanded white field");
    assert!(lum(&search, 800, 500) < lum(&base, 800, 500) * 8 / 10, "search: row dimmed");
    let back = frame(-1);
    save(&back, "returned");
    assert_eq!(back.as_bytes(), base.as_bytes(), "back to the default frame once the focus returns");
}
