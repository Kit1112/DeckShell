//! Headless-тесты механизма «поле → клавиатура»: фокус/активация → клавиатура открыта → ввод попал в поле →
//! закрытие возвращает фокус полю. По одному тесту на тип поля; PNG ключевых состояний — при `DECKSHELL_PNG_DIR`.
use super::*;
use crate::input::Action;
use slint::ComponentHandle;

pub(crate) fn app() -> (crate::AppWindow, App) {
    app_with_library(steam_data::Library::default())
}

pub(crate) fn app_with_library(lib: steam_data::Library) -> (crate::AppWindow, App) {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = crate::AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let ds = std::sync::Arc::new(crate::data::DataSource::new());
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let a = App::new(ui.as_weak(), ds, std::sync::Arc::new(lib), loader);
    // чистое состояние клавиатуры потока
    with(|o| o.open = false);
    SINK.with(|c| c.set(None));
    (ui, a)
}

/// PNG состояния (если задан DECKSHELL_PNG_DIR).
pub(crate) fn shot(ui: &crate::AppWindow, name: &str) {
    let Some(dir) = std::env::var_os("DECKSHELL_PNG_DIR") else { return };
    // анимации (выезд клавиатуры, смена страницы) в тестовом бэкенде идут по мок-времени
    for _ in 0..4 {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
        let _ = ui.window().take_snapshot();
    }
    let Ok(buf) = ui.window().take_snapshot() else { return };
    let dir = std::path::PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    image::save_buffer(dir.join(format!("{name}.png")), buf.as_bytes(), buf.width(), buf.height(), image::ColorType::Rgba8).unwrap();
}

fn type_str(a: &mut App, s: &str) {
    for c in s.chars() {
        a.handle(Action::Char(c));
    }
}

#[test]
fn top_search_a_on_header_opens_keyboard_types_and_returns_focus() {
    let (ui, mut a) = app();
    assert!(a.header_try_enter());
    assert_eq!(super::super::chrome::hf(), 1);
    assert!(!a.lb.search && !a.osk_is_open(), "фокус только раскрывает поле");
    shot(&ui, "top-search-focus");
    let bar = ui.global::<crate::TopBarState>();
    bar.set_dl_active(true);
    a.handle(Action::Right);
    assert_eq!(bar.get_hfocus(), 6);
    assert!(!a.lb.search && !a.osk_is_open());
    a.handle(Action::Left);
    assert_eq!(bar.get_hfocus(), 1);
    a.handle(Action::A);
    assert!(a.osk_is_open(), "A на строке поиска открывает клавиатуру");
    assert!(a.lb.search);
    type_str(&mut a, "ab");
    assert_eq!(a.lb.query, "ab", "ввод попал в поле");
    shot(&ui, "top-search-osk");
    a.handle(Action::Backspace);
    assert_eq!(a.lb.query, "a");
    // B/«Свернуть» закрывает клавиатуру, фокус остаётся в поле; повторное A открывает снова
    a.handle(Action::B);
    assert!(!a.osk_is_open());
    assert!(a.lb.search, "поле поиска осталось активным");
    a.handle(Action::A);
    assert!(a.osk_is_open());
    a.osk_emit(Out::Enter);
    assert!(!a.osk_is_open(), "Поиск/Enter закрывает клавиатуру и выполняет поиск");
    assert_eq!(a.lb.query, "a");
}

#[test]
fn top_search_chord_with_header_focus_routes_to_search() {
    let (_ui, mut a) = app();
    assert!(a.header_try_enter());
    a.osk_show(); // Steam+X/B
    assert!(a.osk_is_open());
    assert!(a.lb.search);
    a.osk_text("q");
    assert_eq!(a.lb.query, "q");
}

#[test]
fn top_search_physical_typing_on_header_focus_opens_search() {
    let (_ui, mut a) = app();
    assert!(a.header_try_enter());
    assert!(a.on_key("z", false, false));
    assert!(a.lb.search, "символ физической клавиатуры на строке поиска открывает поиск");
    assert_eq!(a.lb.query, "z");
}

#[test]
fn typing_into_osk_opened_by_chord_on_header_focus_is_not_lost() {
    let (_ui, mut a) = app();
    assert!(a.header_try_enter());
    // клавиатура открыта «в обход» (старый путь): первый символ сам находит поле
    a.osk_set_open(true);
    a.osk_text("x");
    assert_eq!(a.lb.query, "x");
}

#[test]
fn open_search_route_opens_keyboard() {
    let (_ui, mut a) = app();
    a.open_search();
    assert!(a.osk_is_open(), "вход в поиск открывает клавиатуру");
}

// Measure the animation of the actual layer in AppWindow. The leftmost column
// is clear of the keys, so the first changed pixel is the top of the panel.
fn animation_frame(ui: &crate::AppWindow, elapsed_ms: u64) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(elapsed_ms));
    ui.window().take_snapshot().unwrap()
}

fn panel_top(ui: &crate::AppWindow, background: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, elapsed_ms: u64) -> Option<u32> {
    let frame = animation_frame(ui, elapsed_ms);
    let width = frame.width() as usize;
    (443..frame.height()).find(|&y| {
        let pixel = y as usize * width;
        frame.as_slice()[pixel] != background.as_slice()[pixel]
    })
}

fn assert_panel_top(actual: Option<u32>, expected: u32) {
    // The Bézier samples are rounded to whole pixels by the software renderer.
    assert!(actual.is_some_and(|y| y.abs_diff(expected) <= 2), "panel top {actual:?}, expected {expected}");
}

#[test]
fn osk_slide_follows_easing_and_releases_input_when_closing() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use std::{cell::Cell, rc::Rc};

    let (ui, mut a) = app();
    ui.global::<crate::Shell>().set_game_overlay(true);
    let background = animation_frame(&ui, 0);
    let state = ui.global::<OskState>();
    let presses = Rc::new(Cell::new(0));
    state.on_touch_down({
        let presses = presses.clone();
        move |_, _| presses.set(presses.get() + 1)
    });
    let tap_panel = || {
        let position = slint::LogicalPosition::new(100.0, 650.0);
        let button = PointerEventButton::Left;
        ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
        ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
    };

    reset();
    attach(&mut a, sink_of(10, InputMode::Normal));
    assert!(a.osk_is_open() && state.get_open());
    assert_eq!(panel_top(&ui, &background, 0), None);
    // Opening progress at 25% and 50% of 300 ms is about 0.722 and 0.899.
    assert_panel_top(panel_top(&ui, &background, 75), 542);
    assert_panel_top(panel_top(&ui, &background, 75), 479);
    assert_eq!(panel_top(&ui, &background, 150), Some(443));
    tap_panel();
    assert_eq!(presses.get(), 1);

    a.handle(Action::B);
    assert!(!a.osk_is_open() && !state.get_open());
    assert!(sink().is_none());
    assert_eq!(log().2, 1);
    assert_eq!(panel_top(&ui, &background, 0), Some(443));
    // The closing curve has travelled about 0.058 and 0.234 at 55 and 110 ms.
    assert_panel_top(panel_top(&ui, &background, 55), 464);
    assert_panel_top(panel_top(&ui, &background, 55), 527);
    tap_panel();
    assert_eq!(presses.get(), 1, "the exiting panel still paints but no longer takes touch input");
    assert_eq!(panel_top(&ui, &background, 110), None);
}

#[test]
fn osk_slide_reverses_from_its_current_position() {
    let (ui, mut a) = app();
    ui.global::<crate::Shell>().set_game_overlay(true);
    let background = animation_frame(&ui, 0);

    a.osk_set_open(true);
    assert_eq!(panel_top(&ui, &background, 0), None);
    let opening = panel_top(&ui, &background, 75).unwrap();
    a.osk_set_open(false);
    assert_eq!(panel_top(&ui, &background, 0), Some(opening));
    let closing = panel_top(&ui, &background, 110).unwrap();
    assert!(closing > opening);

    a.osk_set_open(true);
    assert_eq!(panel_top(&ui, &background, 0), Some(closing));
    assert!(panel_top(&ui, &background, 75).unwrap() < closing);
    assert_eq!(panel_top(&ui, &background, 225), Some(443));
}

// ---- приёмники (Properties, SGDB, Wi-Fi, вход...) -------------------------------------------------------------

thread_local! {
    static LOG: std::cell::RefCell<(String, u8, u8)> = const { std::cell::RefCell::new((String::new(), 0, 0)) };
}
fn t_typed(_: &mut App, c: Option<char>) {
    LOG.with(|l| match c {
        Some(c) => l.borrow_mut().0.push(c),
        None => {
            l.borrow_mut().0.pop();
        }
    });
}
fn t_enter(_: &mut App) {
    LOG.with(|l| l.borrow_mut().1 += 1);
}
fn t_cancel(_: &mut App) {
    LOG.with(|l| l.borrow_mut().2 += 1);
}
fn sink_of(id: u8, mode: InputMode) -> ExtSink {
    ExtSink { id, mode, typed: t_typed, enter: t_enter, cancel: t_cancel }
}
fn reset() {
    LOG.with(|l| *l.borrow_mut() = Default::default());
}
fn log() -> (String, u8, u8) {
    LOG.with(|l| l.borrow().clone())
}

#[test]
fn sink_normal_text_enter_and_close_return_focus() {
    let (_ui, mut a) = app();
    reset();
    let s = sink_of(10, InputMode::Normal);
    attach(&mut a, s);
    assert!(a.osk_is_open());
    a.osk_text("hi Я");
    assert_eq!(log().0, "hi Я");
    a.osk_emit(Out::Backspace);
    assert_eq!(log().0, "hi ");
    a.osk_emit(Out::Enter);
    assert!(!a.osk_is_open());
    assert_eq!(log(), ("hi ".into(), 1, 0), "Enter = подтверждение, без отмены");
    // закрытие B → cancel ровно один раз
    attach(&mut a, s);
    a.handle(Action::B);
    assert!(!a.osk_is_open());
    assert_eq!(log().2, 1);
    assert!(sink().is_none(), "приёмник отвязан");
}

#[test]
fn sink_is_cancelled_when_keyboard_is_closed_by_other_means() {
    let (_ui, mut a) = app();
    reset();
    attach(&mut a, sink_of(11, InputMode::Normal));
    a.osk_set_open(false); // «Свернуть», свайп вверх, смена экрана
    assert_eq!(log().2, 1);
    a.osk_set_open(true);
    a.osk_text("x");
    assert_eq!(log().0, "", "после закрытия старый приёмник не получает текст");
}

#[test]
fn password_mode_masks_output_but_keeps_real_text() {
    assert_eq!(mask(InputMode::Password, "ab1"), "•••");
    assert_eq!(mask(InputMode::Normal, "ab1"), "ab1");
    let (_ui, mut a) = app();
    reset();
    attach(&mut a, sink_of(12, InputMode::Password));
    a.osk_text("p@ss");
    assert_eq!(log().0, "p@ss");
    assert_eq!(current_mode(&a), Some(InputMode::Password));
}

#[test]
fn numeric_mode_accepts_only_numbers() {
    let (_ui, mut a) = app();
    reset();
    attach(&mut a, sink_of(13, InputMode::Numeric));
    a.osk_text("1a2.5-b,x");
    assert_eq!(log().0, "12.5-,");
}

#[test]
fn search_mode_sink_reports_search_mode() {
    let (_ui, mut a) = app();
    reset();
    attach(&mut a, sink_of(14, InputMode::Search));
    assert_eq!(current_mode(&a), Some(InputMode::Search));
    a.osk_set_open(false);
    assert_eq!(current_mode(&a), None);
}

/// Focus zone «верхний бар»: Up с ряда плиток уводит курсор в бар, плитка/индекс не теряются, Down и B возвращают на ту же плитку.
#[test]
fn header_focus_zone_round_trip_keeps_the_tile() {
    let (ui, mut a) = app();
    let bar = ui.global::<crate::TopBarState>();
    bar.set_wifi(1);
    a.hm.recent = (0..5).collect();
    a.hm.idx = 2;
    a.hm.zone = 0;
    assert_eq!(bar.get_hfocus(), -1, "по умолчанию бар не в фокусе");
    assert!(a.header_try_enter());
    assert_eq!(bar.get_hfocus(), 1);
    assert_eq!((a.hm.idx, a.hm.zone), (2, 0), "плитка запомнена");
    a.handle(Action::Right);
    assert_ne!(bar.get_hfocus(), 1, "Right двигает по элементам бара");
    a.handle(Action::Down);
    assert_eq!(bar.get_hfocus(), -1);
    assert_eq!((a.hm.idx, a.hm.zone), (2, 0), "Down возвращает фокус на ту же плитку");
    assert!(a.header_try_enter());
    a.handle(Action::B);
    assert_eq!(bar.get_hfocus(), -1);
    assert_eq!(a.hm.idx, 2);
}
