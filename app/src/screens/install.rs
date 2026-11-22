//! Native install modal; all Steam IO runs outside the event loop.
use crate::{app::App, input::Action, InstallCompatRow, InstallState};
use crate::install_flow::{View, Wizard, Config, Folder, Eula};
use slint::{ComponentHandle, ModelRc, VecModel};
use steam_bridge::install::{DeckCategory, DeckCompatibility, DeckTestResult};
use std::rc::Rc;

#[derive(Default)]
pub struct State { pub wizard: Wizard, cursor: usize, generation: u64, pending: bool, data: Option<steam_bridge::install::PreflightData>, eulas: Vec<Eula>, eula_index: usize, writing: bool }
#[derive(Clone, Copy)]
enum Pick { Folder(u32), Continue, ContinueAfterCompatibility, Accept, Cancel, Retry, Close }
fn choices(w: &Wizard) -> Vec<(String, Pick)> {
    let mut rows = vec![];
    match &w.view {
        View::Compatibility(report) => {
            let label = match report.category {
                Some(DeckCategory::Unsupported | DeckCategory::Playable) => "Всё равно установить",
                _ => "Установить",
            };
            return vec![(label.into(), Pick::ContinueAfterCompatibility), ("Отмена".into(), Pick::Cancel)];
        }
        View::Config(c) => {
            for f in &c.folders {
                if f.usable(c.required) && c.folders.iter().filter(|other| other.index == f.index).count() == 1 {
                    rows.push((format!("{}{} · свободно {}", if Some(f.index) == c.selected { "● " } else { "○ " }, f.label, bytes(f.free)), Pick::Folder(f.index)));
                }
            }
            if w.can_continue() { rows.push(("Установить".into(), Pick::Continue)); }
        }
        View::Eula(_) => rows.push(("Принять соглашение".into(), Pick::Accept)),
        View::Error(_) => rows.push(("Повторить проверку".into(), Pick::Retry)),
        View::Unknown(_) | View::Waiting => {},
        _ => {}
    }

    rows.push((if matches!(w.view, View::Unknown(_)) { "Закрыть" } else { "Отмена / отклонить" }.into(),
        if matches!(w.view, View::Unknown(_)) { Pick::Close } else { Pick::Cancel }));
    rows
}
fn bytes(v: Option<u64>) -> String { v.map(|n| format!("{:.2} ГиБ", n as f64 / 1073741824.0)).unwrap_or_else(|| "неизвестно (недоступно)".into()) }
fn body(w: &Wizard) -> String {
    match &w.view {
        View::Config(c) => {
            let mut s = format!("Требуется: {}\n", bytes(c.required));
            for f in &c.folders {
                s.push_str(&format!("\n{}{} — свободно: {}{}", f.label, match f.default { Some(true) => " (по умолчанию)", Some(false) => "", None => " (статус по умолчанию неизвестен)" }, bytes(f.free),
                    if f.mounted.is_none() { " · подключение неизвестно" } else if f.mounted == Some(false) { " · не подключено" } else if !f.usable(c.required) { " · недостаточно места или размер неизвестен" } else { "" }));
            }
            if c.folders.is_empty() { s.push_str("\nПодтверждённых хранилищ нет."); }
            s
        }
        View::Eula(e) => format!("Соглашение {} · версия {}\n\n{}", e.id, e.version, e.text),
        View::Error(s) | View::Unknown(s) => s.clone(),
        View::Waiting => "Ожидание подтверждённого состояния Steam…".into(),
        View::Closed | View::Compatibility(_) => String::new(),
    }
}
thread_local! {
    // Keep the four original SVG masks and their tinted pixels stable across report rows and wizard sessions.
    static COMPAT_BADGES: [(slint::Image, slint::Color); 4] = {
        let sources: [(&[u8], _); 4] = [
            (include_bytes!("../../ui/assets/steam/deck-compat-unknown.svg"), (220, 222, 223)),
            (include_bytes!("../../ui/assets/steam/deck-compat-unsupported.svg"), (220, 222, 223)),
            (include_bytes!("../../ui/assets/steam/deck-compat-playable.svg"), (255, 200, 44)),
            (include_bytes!("../../ui/assets/steam/deck-compat-verified.svg"), (89, 191, 64)),
        ];
        sources.map(|(svg, (r, g, b))| {
            let color = slint::Color::from_rgb_u8(r, g, b);
            let image = slint::Image::load_from_svg_data(svg).ok()
                .and_then(|image| image.to_rgba8()).map(slint::Image::from_rgba8).unwrap_or_default();
            (crate::assets::tint_image(image, color), color)
        })
    };
}

fn compatibility_badge(category: DeckCategory) -> (slint::Image, slint::Color) {
    let index = match category {
        DeckCategory::Unknown => 0,
        DeckCategory::Unsupported => 1,
        DeckCategory::Playable => 2,
        DeckCategory::Verified => 3,
    };
    COMPAT_BADGES.with(|badges| badges[index].clone())
}

fn present_compatibility(s: &InstallState, report: &DeckCompatibility, localize: impl Fn(&str) -> Option<String>) {
    let category = report.category.unwrap_or(DeckCategory::Unknown);
    let (label, summary) = match report.category {
        Some(DeckCategory::Verified) => ("Полностью совместима", "Игра полностью функциональна на Steam Deck и отлично работает со встроенным управлением и настройками графики."),
        Some(DeckCategory::Playable) => ("Можно играть", "Игра полностью функциональна на Steam Deck, но для работы может потребоваться дополнительная настройка."),
        Some(DeckCategory::Unsupported) => ("Не поддерживается", "Вся игра или её части на данный момент не функционируют на Steam Deck."),
        Some(DeckCategory::Unknown) => ("Неизвестно", "Valve ещё изучает эту игру. На данный момент у нас нет дополнительной информации о совместимости игры со Steam Deck."),
        None => ("Неизвестно", "Данные о совместимости игры со Steam Deck недоступны."),
    };
    let (icon, tint) = compatibility_badge(category);
    let label_color = if category == DeckCategory::Unsupported {
        slint::Color::from_rgb_u8(124, 142, 163)
    } else {
        tint
    };
    s.set_rating(InstallCompatRow { text: label.into(), icon, note: false });
    s.set_rating_label_color(label_color);
    s.set_summary(summary.into());
    s.set_developer_post(report.blog_url.is_some());
    s.set_compatibility_scroll_y(0.0);
    let rows = match &report.items {
        Some(items) if !items.is_empty() => {
            let mut rows: Vec<_> = items.iter().map(|item| {
                let icon = match item.result {
                    DeckTestResult::Pass => compatibility_badge(DeckCategory::Verified).0,
                    DeckTestResult::Warning => compatibility_badge(DeckCategory::Playable).0,
                    DeckTestResult::Fail => compatibility_badge(DeckCategory::Unsupported).0,
                    DeckTestResult::Unknown => compatibility_badge(DeckCategory::Unknown).0,
                    DeckTestResult::Note => slint::Image::default(),
                };
                let text = item.token.as_deref().and_then(&localize)
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "Описание результата проверки неизвестно.".into());
                InstallCompatRow { text: text.into(), icon, note: item.result == DeckTestResult::Note }
            }).collect();
            rows.sort_by_key(|row| row.note);
            rows
        }
        Some(_) if report.category == Some(DeckCategory::Unknown) => vec![],
        _ => vec![InstallCompatRow {
            text: "Результаты отдельных проверок недоступны.".into(),
            note: true,
            ..Default::default()
        }],
    };
    s.set_checks(ModelRc::from(Rc::new(VecModel::from(rows))));
}

pub fn wire(ui: &crate::AppWindow) {
    let s = ui.global::<InstallState>();
    s.on_pick(|i| super::defer(move |a| {
        if !matches!(a.install.wizard.view, View::Compatibility(_)) {
            pick(a, i.max(0) as usize);
        }
    }));
    s.on_pick_compatibility(|i| super::defer(move |a| {
        if matches!(a.install.wizard.view, View::Compatibility(_)) {
            if i == 2 { open_developer_post(a); } else { pick(a, i.max(0) as usize); }
        }
    }));
}
pub fn open(a: &mut App, id: u32) {
    if a.install.wizard.view != View::Closed || id == 0 { return; }
    a.install.wizard = Wizard { appid: id, view: View::Waiting };
    a.install.cursor = 0;
    a.header_clear();
    let generation = begin_read(&mut a.install);
    std::thread::spawn(move || {
        let report = if (std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active()) {
            DeckCompatibility::default()
        } else {
            crate::actions::bridge().and_then(|b| b.install_deck_compatibility(id).ok()).unwrap_or_default()
        };
        super::defer(move |a| finish_compatibility(a, generation, id, report));
    });
    sync(a);
}

fn begin_read(state: &mut State) -> u64 {
    state.generation = state.generation.wrapping_add(1);
    state.pending = true;
    state.generation
}

fn finish_read(state: &mut State, generation: u64, id: u32) -> bool {
    if !state.pending || state.generation != generation || state.wizard.appid != id {
        return false;
    }
    state.pending = false;
    true
}

fn finish_compatibility(a: &mut App, generation: u64, id: u32, report: DeckCompatibility) {
    if !finish_read(&mut a.install, generation, id) { return; }
    a.install.cursor = 0;
    if report.category == Some(DeckCategory::Verified) {
        preflight(a, id);
    } else {
        present_compatibility(&a.ui().global::<InstallState>(), &report, steam_assets::loc_russian);
        a.install.wizard.view = View::Compatibility(report);
    }
    sync(a);
}

fn open_developer_post(a: &mut App) {
    let View::Compatibility(report) = &a.install.wizard.view else { return; };
    let Some(url) = &report.blog_url else { return; };
    if (std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active()) { return; }
    match steam_control::open_url(&format!("steam://openurl/{url}")) {
        Ok(()) => { cancel(a); sync(a); }
        Err(_) => a.toast("Не удалось открыть пост разработчика."),
    }
}

fn preflight(a: &mut App, id: u32) {
    if a.install.pending { return; }
    let generation = begin_read(&mut a.install);
    a.install.wizard.appid = id;
    a.install.wizard.view = View::Waiting;
    std::thread::spawn(move || {
        let result = (|| {
            if (std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active()) { return Err("DRYRUN: установка отключена".to_string()); }
            let b = crate::actions::bridge().ok_or("Steam bridge недоступен")?;
            let data = b.install_preflight(id).map_err(|e| e.to_string())?;
            let eulas = data.eulas.iter().map(|e| b.load_eula_content(&e.url)
                .map(|text| Eula { id: e.id.clone(), version: e.version.to_string(), text }).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>()?;
            Ok((data, eulas))
        })();
        super::defer(move |a| finish_preflight(a, generation, id, result));
    });
}
fn finish_preflight(a: &mut App, generation: u64, id: u32, result: Result<(steam_bridge::install::PreflightData, Vec<Eula>), String>) {
    if !finish_read(&mut a.install, generation, id) { return; }
    match result {
        Ok((data, eulas)) => {
            a.install.data = Some(data); a.install.eulas = eulas; a.install.eula_index = 0;
            next_view(a);
        }
        Err(e) => a.install.wizard.view = View::Error(e),
    }
    a.install.cursor = 0;
    sync(a);
}
fn next_view(a: &mut App) {
    if let Some(e) = a.install.eulas.get(a.install.eula_index) {
        a.install.wizard.view = View::Eula(e.clone());
    } else if let Some(data) = &a.install.data {
        let folders: Vec<_> = data.folders.clone().into_iter().map(Folder::from_native).collect();
        let selected = folders.iter().filter(|f| f.usable(data.required)).min_by_key(|f| f.default != Some(true)).map(|f| f.index);
        a.install.wizard.view = View::Config(Config { folders, required: data.required, selected });
    }
    a.ui().global::<InstallState>().set_scroll_y(0.0);
}
fn cancel(a: &mut App) {
    // A dispatched write cannot be cancelled locally or safely replayed.
    if a.install.writing { return; }
    a.install.pending = false;
    a.install.generation = a.install.generation.wrapping_add(1);
    a.install.wizard.view = View::Closed;
    a.install.data = None; a.install.eulas.clear();
}
fn write(a: &mut App, accept: bool) {
    if a.install.pending || (std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active()) { return; }
    let id = a.install.wizard.appid;
    let eula = match &a.install.wizard.view {
        View::Eula(eula) if accept => Some(eula.clone()),
        _ => None,
    };
    let folder = match &a.install.wizard.view {
        View::Config(c) if !accept && a.install.wizard.can_continue() => c.selected,
        _ => None,
    };
    if eula.is_none() && folder.is_none() { return; }
    a.install.pending = true; a.install.writing = true;
    a.install.wizard.view = View::Waiting;
    let generation = a.install.generation;
    std::thread::spawn(move || {
        let result = (|| {
            let b = crate::actions::bridge().ok_or("Steam bridge недоступен")?;
            if let Some(e) = eula {
                b.accept_eula(id, &e.id, e.version.parse().map_err(|_| "Invalid EULA version")?).map_err(|e| e.to_string())
            } else {
                match b.install_start_headless(id, folder.unwrap()) {
                    Ok(true) => {
                        if let Ok(downloads) = b.downloads() {
                            super::defer(move |a| a.on_backend(crate::backend::Update::Downloads(Box::new(downloads))));
                        }
                        Ok(())
                    }, Ok(false) => Err("Запуск не подтверждён".into()), Err(e) => Err(e.to_string()),
                }
            }
        })();
        super::defer(move |a| {
            if a.install.generation != generation { return; }
            a.install.pending = false; a.install.writing = false;
            match result {
                Ok(()) if accept => { a.install.eula_index += 1; next_view(a); }
                Ok(()) => { a.install.wizard.view = View::Closed; a.toast("Запрос установки отправлен. Ход установки — в Загрузках."); }
                Err(e) => a.install.wizard.view = View::Unknown(format!("{e}\nПроверьте Загрузки перед повтором.")),
            }
            a.install.cursor = 0; sync(a);
        });
    });
}
fn sync(a: &App) {
    let ui = a.ui();
    let s = ui.global::<InstallState>();
    s.set_open(a.install.wizard.view != View::Closed);
    s.set_busy(a.install.writing);
    s.set_reading_eula(matches!(a.install.wizard.view, View::Eula(_)));
    s.set_compatibility(matches!(a.install.wizard.view, View::Compatibility(_)));
    let name = a.games().iter().find(|g| g.app_id == a.install.wizard.appid).map(|g| g.name.as_str()).unwrap_or("Игра");
    s.set_title(if s.get_compatibility() { name.into() } else { format!("Установка · {name}").into() });
    s.set_body(body(&a.install.wizard).into());
    s.set_rows(ModelRc::from(Rc::new(VecModel::from(choices(&a.install.wizard).into_iter().map(|(s,_)| s.into()).collect::<Vec<_>>()))));
    s.set_cursor(a.install.cursor as i32);
}
fn pick(a: &mut App, i: usize) {
    if a.install.writing { return; }
    let Some((_, p)) = choices(&a.install.wizard).get(i).cloned() else { return; };
    match p {
        Pick::Folder(i) => { if let View::Config(c) = &mut a.install.wizard.view { c.selected = Some(i); } }
        Pick::Continue => write(a, false),
        Pick::ContinueAfterCompatibility => preflight(a, a.install.wizard.appid),
        Pick::Accept => write(a, true),
        Pick::Cancel | Pick::Close => cancel(a),
        Pick::Retry => preflight(a, a.install.wizard.appid),
    }
    a.install.cursor = 0;
    a.ui().global::<InstallState>().set_scroll_y(0.0);
    sync(a);
}
pub fn handle(a: &mut App, action: Action, repeat: bool) -> bool {
    if a.install.wizard.view == View::Closed { return false; }
    if matches!(a.install.wizard.view, View::Compatibility(_)) {
        let ui = a.ui();
        let s = ui.global::<InstallState>();
        match action {
            Action::Left => a.install.cursor = 0,
            Action::Right => a.install.cursor = 1,
            Action::Up | Action::Down | Action::L2 | Action::R2 => {
                let delta = match action {
                    Action::Up => 56.0,
                    Action::Down => -56.0,
                    Action::L2 => 180.0,
                    _ => -180.0,
                };
                let min_y = -(s.get_compatibility_scroll_height() - s.get_compatibility_view_height()).max(0.0);
                s.set_compatibility_scroll_y((s.get_compatibility_scroll_y() + delta).clamp(min_y, 0.0));
            }
            Action::A if !repeat => pick(a, a.install.cursor),
            Action::B if !repeat => { cancel(a); sync(a); }
            Action::Y if !repeat => open_developer_post(a),
            _ => {}
        }
        s.set_cursor(a.install.cursor as i32);
        return true;
    }
    let n = choices(&a.install.wizard).len();
    match action {
        Action::Up => a.install.cursor = a.install.cursor.saturating_sub(1),
        Action::Down => a.install.cursor = (a.install.cursor + 1).min(n.saturating_sub(1)),
        Action::A if !repeat => { pick(a, a.install.cursor); return true; }
        Action::B if !repeat => { cancel(a); a.install.cursor = 0; }
        // Reading long EULAs works with pad as well as mouse/touch.
        Action::L2 | Action::R2 => {
            let ui = a.ui(); let s = ui.global::<InstallState>();
            let y = s.get_scroll_y() + if action == Action::L2 { 180.0 } else { -180.0 };
            s.set_scroll_y(y.clamp(-(s.get_scroll_height() - s.get_view_height()).max(0.0), 0.0));
        }
        _ => {}
    }
    sync(a);
    true
}

#[cfg(test)]
mod tests {
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
                assert_eq!(rows.row_data(2).unwrap().text, "Описание результата проверки неизвестно.");
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

}

#[cfg(test)]
mod style_tests {
    /// The wizard takes its colours from the side-menu tokens: no literal colours except the dimmer and the shadow.
    #[test]
    fn install_wizard_uses_side_menu_tokens() {
        let src = include_str!("../../ui/screens/install.slint");
        for token in ["Tokens.menu-bg", "Tokens.hover-bg", "Tokens.accent", "Tokens.menu-item-focus", "Tokens.menu-item", "Tokens.text-body", "Tokens.text-dim", "Tokens.menu-label-size", "Theme.panel", "GStyle.loc-font"] {
            assert!(src.contains(token), "{token}");
        }
        let literals: Vec<&str> = src.lines().filter(|l| !l.trim_start().starts_with("//")).flat_map(|l| l.split('#').skip(1)).map(|t| &t[..t.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(t.len())]).collect();
        assert_eq!(literals, ["000000cc", "00000080"], "only the dimmer and the drop shadow are literal");
    }
}


#[cfg(test)]
mod input_tests {
    use super::*;
    #[test]
    fn install_compatibility_skips_verified_and_requires_separate_eula_acceptance() {
        use steam_bridge::install::{NativeFolder, PreflightData};
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(Default::default()))).unwrap();
        let ui = crate::AppWindow::new().unwrap();
        let ds = std::sync::Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
        let eula = Eula { id: "fixture".into(), version: "1".into(), text: "Text".into() };
        let preflight_data = PreflightData {
            folders: vec![NativeFolder {
                index: 7, drive_name: "microSD".into(), user_label: None,
                mounted: Some(true), default: Some(true), free: Some(200),
            }],
            required: Some(100),
            eulas: vec![],
        };
        a.install.wizard.appid = 42;

        for category in [None, Some(DeckCategory::Unknown), Some(DeckCategory::Unsupported), Some(DeckCategory::Playable)] {
            a.install.wizard.view = View::Waiting;
            let generation = begin_read(&mut a.install);
            finish_compatibility(&mut a, generation, 42, DeckCompatibility { category, ..Default::default() });
            assert!(matches!(a.install.wizard.view, View::Compatibility(_)));
            assert!(!a.install.pending && !a.install.writing);
            assert!(!a.install.wizard.can_continue(), "compatibility is not install dispatch");
            let state = ui.global::<InstallState>();
            state.set_compatibility_scroll_height(800.0);
            state.set_compatibility_view_height(400.0);
            handle(&mut a, Action::Down, true);
            assert_eq!(state.get_compatibility_scroll_y(), -56.0);
            assert_eq!(a.install.cursor, 0);
            handle(&mut a, Action::Up, true);
            assert_eq!(state.get_compatibility_scroll_y(), 0.0);
            handle(&mut a, Action::Right, false);
            assert_eq!(a.install.cursor, 1);
            handle(&mut a, Action::A, true);
            assert!(matches!(a.install.wizard.view, View::Compatibility(_)), "repeat must not continue or cancel");
            let cancel_action = if category.is_none() { Action::B } else { Action::A };
            handle(&mut a, cancel_action, false);
            assert_eq!(a.install.wizard.view, View::Closed);
            finish_compatibility(&mut a, generation, 42, DeckCompatibility::default());
            assert_eq!(a.install.wizard.view, View::Closed, "cancel invalidates stale lookups");
        }

        for category in [DeckCategory::Playable, DeckCategory::Verified] {
            a.install.wizard.view = View::Waiting;
            let generation = begin_read(&mut a.install);
            finish_compatibility(&mut a, generation, 42, DeckCompatibility {
                category: Some(category), ..Default::default()
            });
            if category == DeckCategory::Playable {
                assert_eq!(choices(&a.install.wizard)[0].0, "Всё равно установить");
                handle(&mut a, Action::A, false);
            }
            assert_eq!(a.install.wizard.view, View::Waiting);
            assert!(a.install.pending && !a.install.writing);
            let generation = a.install.generation;
            finish_preflight(&mut a, generation, 42, Ok((preflight_data.clone(), vec![eula.clone()])));
            assert_eq!(a.install.wizard.view, View::Eula(eula.clone()));
            assert_eq!(a.install.eula_index, 0);
            assert!(!a.install.pending && !a.install.writing);
            handle(&mut a, Action::B, false);
            assert_eq!(a.install.wizard.view, View::Closed);
        }

        a.install.wizard.view = View::Waiting;
        let generation = begin_read(&mut a.install);
        finish_compatibility(&mut a, generation, 42, DeckCompatibility {
            category: Some(DeckCategory::Unsupported), ..Default::default()
        });
        handle(&mut a, Action::A, false);
        let generation = a.install.generation;
        finish_preflight(&mut a, generation, 42, Ok((preflight_data, vec![])));
        assert!(matches!(&a.install.wizard.view, View::Config(config) if config.selected == Some(7)));
        assert!(a.install.wizard.can_continue());
        assert!(!a.install.pending && !a.install.writing);
    }

    #[test]
    fn install_flow_cancel_invalidates_read_and_write_cannot_be_repeated() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(Default::default()))).unwrap();
        let ui = crate::AppWindow::new().unwrap();
        let ds = std::sync::Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
        a.install.pending = true; a.install.generation = 10; a.install.wizard.appid = 42;
        a.install.wizard.view = View::Waiting;
        cancel(&mut a);
        finish_preflight(&mut a, 10, 42, Err("stale".into()));
        assert_eq!(a.install.wizard.view, View::Closed);
        a.install.pending = true; a.install.writing = true; a.install.wizard.view = View::Waiting;
        handle(&mut a, Action::B, false); pick(&mut a, 0);
        assert_eq!(a.install.wizard.view, View::Waiting);
        a.install.pending = false; a.install.writing = false;
        a.install.wizard.view = View::Eula(Eula { id: "fixture".into(), version: "1".into(), text: "Text".into() });
        let before = a.install.wizard.view.clone();
        handle(&mut a, Action::A, true);
        assert_eq!(a.install.wizard.view, before);
        handle(&mut a, Action::B, false);
        assert_eq!(a.install.wizard.view, View::Closed);
    }
}
