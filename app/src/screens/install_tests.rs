use crate::screens::expected_text;
use super::*;
use crate::install_flow::{Config, Folder, Eula};
#[test]
fn install_flow_ui_modal_renders_and_level_popup_does_not_change_it() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).unwrap();
    let ui = crate::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let global = ui.global::<InstallState>();
    global.set_open(true); global.set_title("Установка · fixture".into());
    let mut w = Wizard::default();
    let variants = [View::Error("Steam недоступен".into()),
        View::Config(Config { folders: vec![Folder { index: 0, label: "Native storage".into(), free: None, mounted: Some(true), default: Some(true) }], required: None, selected: None }),
        View::Eula(Eula { id: "fixture".into(), version: "1".into(), text: "Long agreement.\n".repeat(200) }),
        View::Unknown("Результат неизвестен".into())];
    let dir = std::env::var_os("DECKSHELL_INSTALL_GALLERY").map(std::path::PathBuf::from);
    if let Some(d) = &dir { std::fs::create_dir_all(d).unwrap(); }
    for (i, view) in variants.into_iter().enumerate() {
        w.view = view; global.set_body(body(&w).into());
        global.set_rows(ModelRc::from(Rc::new(VecModel::from(choices(&w).into_iter().map(|(s,_)| s.into()).collect::<Vec<_>>()))));
        let snapshot = ui.window().take_snapshot().unwrap();
        assert_eq!((snapshot.width(), snapshot.height()), (1280, 800));
        let pixel = |x: usize, y: usize| &snapshot.as_bytes()[(y * 1280 + x) * 4..(y * 1280 + x) * 4 + 4];
        assert_eq!(pixel(110, 100), [14, 20, 27, 255], "side-menu panel background");
        assert_eq!(pixel(110, 76), [35, 38, 46, 255], "shared menu panel border");
        if let Some(d) = &dir { image::save_buffer(d.join(format!("install-{i}.png")), snapshot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap(); }
    }
    // Moving focus between actions must redraw the shared menu row highlight.
    global.set_rows(ModelRc::from(Rc::new(VecModel::from(vec!["Retry".into(), "Cancel".into()]))));
    global.set_cursor(0);
    let _ = ui.window().take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
    slint::platform::update_timers_and_animations();
    let first = ui.window().take_snapshot().unwrap();
    global.set_cursor(1);
    let _ = ui.window().take_snapshot().unwrap();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
    slint::platform::update_timers_and_animations();
    let second = ui.window().take_snapshot().unwrap();
    assert_ne!(first.as_bytes(), second.as_bytes(), "action focus must be visible");
    let before = global.get_body();
    let popup = crate::LevelPopupWindow::new().unwrap();
    popup.window().set_size(slint::PhysicalSize::new(420, 80));
    popup.global::<crate::Shell>().set_level_shown(true);
    popup.global::<crate::Shell>().set_level_value(0.6);
    let _ = popup.window().take_snapshot().unwrap();
    assert_eq!(global.get_body(), before); assert!(global.get_open());
}

#[test]
fn install_compatibility_gallery_draws_original_glyphs_and_accepts_pointer_input() {
    use slint::Model;
    use slint::platform::{PointerEventButton, WindowEvent};
    use steam_bridge::install::DeckCompatItem;
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    ))).unwrap();
    let ui = crate::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let global = ui.global::<InstallState>();
    global.set_title("Игра для проверки совместимости".into());
    global.set_open(true);
    global.set_compatibility(true);
    let set_choices = |report| {
        let wizard = Wizard { appid: 42, view: View::Compatibility(report) };
        let labels = choices(&wizard).into_iter().map(|(label, _)| label.into()).collect::<Vec<_>>();
        global.set_rows(ModelRc::from(Rc::new(VecModel::from(labels))));
    };
    let dir = std::env::var_os("DECKSHELL_INSTALL_GALLERY").map(std::path::PathBuf::from);
    if let Some(d) = &dir { std::fs::create_dir_all(d).unwrap(); }
    for (category, filename) in [
        (Some(DeckCategory::Playable), "playable"),
        (Some(DeckCategory::Unsupported), "unsupported"),
        (Some(DeckCategory::Unknown), "unknown"),
        (None, "unavailable"),
    ] {
        let items = match category {
            Some(DeckCategory::Playable) => Some(vec![
                DeckCompatItem { result: DeckTestResult::Note, token: Some("cloud".into()) },
                DeckCompatItem { result: DeckTestResult::Pass, token: Some("controller".into()) },
                DeckCompatItem { result: DeckTestResult::Warning, token: Some("text".into()) },
                DeckCompatItem { result: DeckTestResult::Unknown, token: None },
            ]),
            Some(DeckCategory::Unsupported) => Some(vec![
                DeckCompatItem { result: DeckTestResult::Fail, token: Some("anticheat".into()) },
            ]),
            Some(DeckCategory::Unknown) => Some(vec![]),
            _ => None,
        };
        let report = DeckCompatibility {
            category,
            items,
            blog_url: (category == Some(DeckCategory::Playable)).then(|| "https://steamcommunity.com/ogg/42/announcements/detail/123".into()),
        };
        present_compatibility(&global, &report, |token| Some(match token {
            "controller" => "Весь функционал доступен при использовании стандартных настроек управления контроллером",
            "text" => "Текст в некоторых частях внутриигрового интерфейса слишком маленький и может быть неудобен для чтения",
            "cloud" => "Для этой игры требуется вручную включить в настройках синхронизацию сохранений со Steam Cloud",
            "anticheat" => "Античит этой игры не настроен для поддержки Steam Deck",
            _ => return None,
        }.into()));
        if category == Some(DeckCategory::Playable) {
            let rows = global.get_checks();
            assert_eq!(rows.row_count(), 4);
            let size = rows.row_data(0).unwrap().icon.size();
            assert_eq!((size.width, size.height), (20, 20));
            assert_eq!(rows.row_data(2).unwrap().text, expected_text("Описание результата проверки неизвестно.", "No description is available for this compatibility result."));
            assert!(rows.row_data(3).unwrap().note);
        }
        set_choices(report);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
        slint::platform::update_timers_and_animations();
        let snapshot = ui.window().take_snapshot().unwrap();
        let pixels = snapshot.as_bytes();
        let has_color = |x: &[u8]| pixels.chunks_exact(4).any(|pixel| pixel == x);
        assert_eq!(&pixels[(120 * 1280 + 280) * 4..][..4], [14, 20, 27, 255], "compatibility panel must render");
        if category == Some(DeckCategory::Playable) {
            assert!(has_color(&[255, 200, 44, 255]), "playable glyph must render");
            assert!(has_color(&[89, 191, 64, 255]), "pass glyph must render");
        }
        if let Some(d) = &dir {
            image::save_buffer(d.join(format!("install-compat-{filename}.png")), pixels, 1280, 800, image::ColorType::Rgba8).unwrap();
        }
    }
    let pressed = Rc::new(std::cell::Cell::new(-1));
    let picked = Rc::clone(&pressed);
    global.on_pick_compatibility(move |i| picked.set(i));
    for (x, expected) in [(435.0, 0), (840.0, 1)] {
        let position = slint::LogicalPosition::new(x, 630.0);
        ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        assert_eq!(pressed.get(), expected, "compatibility button must accept touch/pointer input");
    }

    let long_report = DeckCompatibility {
        category: Some(DeckCategory::Playable),
        items: Some(vec![DeckCompatItem {
            result: DeckTestResult::Warning,
            token: Some("long".into()),
        }; 18]),
        blog_url: None,
    };
    present_compatibility(&global, &long_report, |_| Some(
        "Для этой игры требуется вручную изменить настройки управления и проверить, что подсказки на экране соответствуют кнопкам контроллера.".into()
    ));
    set_choices(long_report);
    let top = ui.window().take_snapshot().unwrap();
    let bottom = global.get_compatibility_view_height() - global.get_compatibility_scroll_height();
    assert!(bottom < -180.0, "long reports must be scrollable");
    global.set_compatibility_scroll_y(bottom);
    let scrolled = ui.window().take_snapshot().unwrap();
    assert_ne!(top.as_bytes(), scrolled.as_bytes(), "scrolling must reveal the rest of the report");
    if let Some(d) = dir {
        image::save_buffer(d.join("install-compat-scrolled.png"), scrolled.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
    }
}
