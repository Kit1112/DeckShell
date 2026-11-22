use crate::screens::expected_text;
use super::*;
use crate::screens::downloads_menu::WRITES;
use serde_json::Value;
use steam_bridge::{parse_download_settings, parse_downloads, DownloadSettings, Downloads};

#[test]
fn download_portrait_prefers_steam_icon_then_legacy_icon_then_capsule() {
    let root = std::env::temp_dir().join(format!("deckshell-download-icon-{}", std::process::id()));
    let resolver = ArtResolver::new(&root, 0);
    let cache = resolver.cache_dir();
    let app = cache.join("42");
    std::fs::create_dir_all(&app).unwrap();
    let hash = "0123456789012345678901234567890123456789";
    let hashed = app.join(format!("{hash}.jpg"));
    let flat = cache.join("42_icon.jpg");
    let capsule = app.join("library_600x900.png");
    image::RgbImage::new(64, 64).save(&hashed).unwrap();
    image::RgbImage::new(32, 32).save(&flat).unwrap();
    image::RgbImage::new(48, 72).save(&capsule).unwrap();

    let artwork = DownloadArtwork { appid: 42, steam_root: root.clone(), account: None, icon: None, capsule: None };
    let urls = [format!("/assets/42/{hash}.jpg?c=7"), "/assets/42_icon.jpg?c=7".into()];
    let read = || {
        let cached = steam_data::art::librarycache(&cache, "42");
        let (pixels, is_capsule) = artwork.decode(&resolver, &cached, &urls).unwrap();
        (pixels.width(), pixels.height(), is_capsule)
    };
    assert_eq!(read(), (64, 64, false));
    // Keep the hash URL as a candidate even if the cache file is corrupt.
    std::fs::write(&hashed, b"invalid image").unwrap();
    assert_eq!(read(), (32, 32, false));
    std::fs::remove_file(&flat).unwrap();
    assert_eq!(read(), (48, 72, true));
    std::fs::remove_dir_all(&root).unwrap();
}

fn fixture() -> (Downloads, DownloadSettings) {
    let v: Value = serde_json::from_str(include_str!("downloads_menu_fixture.json")).unwrap();
    (parse_downloads(&v["downloads"]).unwrap(), parse_download_settings(&v["settings"]).unwrap())
}
fn txt() -> Txt<'static> {
    Txt { tr: &|_, f| f.to_string(), time: &|ts| format!("T{ts}"), size: &|b| format!("{b}B") }
}
fn lines() -> Vec<Line> {
    let (d, s) = fixture();
    layout(&downloads_page::page(Some(&d), Some(&s), &txt()))
}
fn writes() -> Vec<String> {
    WRITES.with(|w| std::mem::take(&mut *w.borrow_mut()))
}
fn open_page(d: Option<Downloads>, s: Option<DownloadSettings>) -> (crate::AppWindow, App) {
    let (ui, mut a) = crate::screens::osk::field_tests::app();
    wire_models(&a, &ui);
    a.live.downloads = d;
    a.dm.settings = Some(s);
    a.router.push(Route::Downloads);
    a.ui().global::<crate::Shell>().set_page(3);
    a.on_downloads_data();
    writes();
    (ui, a)
}

#[test]
fn layout_follows_the_stock_order_and_heights() {
    let l = lines();
    let kinds: Vec<i32> = l.iter().map(|x| x.kind).collect();
    // герой (сводки сверху нет), «Далее (3)» + 3 карточки, «Запланировано» + 1, «Не запланировано» + 3, «Завершено» + 2, примечания нет
    assert_eq!(kinds, vec![1, 2, 3, 3, 3, 2, 3, 2, 3, 3, 3, 2, 3, 3]);
    assert_eq!(l[0].y, 0.0);
    assert!(l.windows(2).all(|w| (w[0].y + w[0].h - w[1].y).abs() < 0.01), "строки идут встык, без наложения");
    assert_eq!(l[0].name, "Alpha");
    assert_eq!((l[0].b1.as_str(), l[0].b1_icon), (expected_text("Пауза", "Pause"), 1));
    assert_eq!(l[0].remove, None, "у активной загрузки нет кнопки «убрать»");
    assert_eq!(l[0].speed, expected_text("2,0 МБ/с · Осталось примерно 5 мин.", "2.0 MB/s · About 5 min remaining"));
    assert_eq!(l[1].name, expected_text("Далее (3)", "Up Next (3)"));
    assert_eq!(l[1].hb, expected_text("АВТООБНОВЛЕНИЯ ЗАПЛАНИРОВАНЫ С 01:00 ПО 05:00", "AUTO UPDATES SCHEDULED FROM 01:00 TO 05:00"));
    assert_eq!((l[2].order, l[2].b1.as_str(), l[2].b2.as_str()), (1, expected_text("Загрузить первым", "Download First"), "×"));
    assert_eq!(l[4].b1, expected_text("Повторить", "Retry"), "карточка с ошибкой");
    assert_eq!(l[11].hb, expected_text("СКРЫТЬ ВСЕ", "CLEAR ALL"));
    assert_eq!(l[11].header, Some(HeaderAct::ClearCompleted(vec![900, 800])));
    assert_eq!(l[12].b1, expected_text("К игре", "Play"));
    assert!(l.iter().all(|x| x.kind != 5 && !x.name.contains(expected_text("Не реализовано", "Not implemented"))), "заметки «не реализовано» больше нет");
}

#[test]
fn hero_has_graph_and_badges_and_queue_cards_are_draggable() {
    let (mut d, s) = fixture();
    d.history = (0..130).map(|k| steam_bridge::HistorySample { time: 1000.0 + k as f64, net: if k % 10 == 0 { 0 } else { 1000 + k as u64 }, disc: 500, upload: false }).collect();
    d.items[0].types = vec![(0, false), (2, true)];
    let txt = txt();
    let l = layout(&downloads_page::page(Some(&d), Some(&s), &txt));
    let g = l[0].graph.as_ref().expect("у героя есть график");
    assert_eq!(g.net.len(), 120);
    assert!(g.net.iter().any(|&v| (v - 1.0).abs() < 1e-6), "самый высокий столбец = пик окна");
    assert!(disc_path(g).starts_with('M') && disc_path(g).contains('L'));
    let b = &l[0].badges;
    assert_eq!(b.iter().map(|x| (x.kind, x.state)).collect::<Vec<_>>(), vec![(2, 2), (0, 1)], "порядок стока: шейдеры, контент; завершён / качается");
    // перетаскивание: сдвиг на карточку вниз = следующий номер, за краем — зажимается, на месте — ничего
    let q: Vec<usize> = (0..l.len()).filter(|&i| l[i].drag).collect();
    assert!(q.len() >= 2);
    let first = &l[q[0]];
    assert_eq!(drag_target(&l, q[0], H_ITEM), Some((first.appid, (first.order + 1) as u32)));
    assert_eq!(drag_target(&l, q[0], 10.0), None);
    let last = &l[*q.last().unwrap()];
    assert_eq!(drag_target(&l, *q.last().unwrap(), 5.0 * H_ITEM), None, "ниже последнего некуда");
    assert_eq!(drag_target(&l, q[0], -5.0 * H_ITEM), None, "выше первого некуда");
    assert_eq!(drag_target(&l, *q.last().unwrap(), -H_ITEM).map(|x| x.1), Some((last.order - 1) as u32));
    assert_eq!(drag_target(&l, 0, H_ITEM), None, "герой не перетаскивается");
    // без истории графика нет
    d.history.clear();
    assert!(layout(&downloads_page::page(Some(&d), Some(&s), &txt))[0].graph.is_none());
}

#[test]
fn focus_moves_over_cards_and_buttons() {
    let l = lines();
    assert_eq!(first_focus(&l), 0);
    assert_eq!(step_row(&l, 0, 1), Some(1), "кнопка автообновлений в заголовке «Далее» достижима");
    assert_eq!(step_row(&l, 1, 1), Some(2));
    assert_eq!(step_row(&l, 0, -1), None, "выше героя нет строк");
    assert_eq!(step_row(&l, 13, 1), None, "примечание не фокусируется");
    assert_eq!(l[2].cols(), vec![0, 1, 2]);
    assert_eq!(l[0].cols(), vec![0, 1]);
    assert_eq!(l[1].cols(), vec![0]);
}

#[test]
fn page_actions_write_the_stock_calls_and_never_mirror_state() {
    let (d, s) = fixture();
    let (_ui, mut a) = open_page(Some(d.clone()), Some(s));
    assert_eq!(a.dl.focus, 0);
    // X на герое: пауза всех
    a.handle(Action::X);
    assert_eq!(writes(), vec![r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#]);
    // Y — «Сменить устройство»: другого устройства нет, в Steam ничего не пишется
    a.handle(Action::Y);
    assert!(writes().is_empty());
    // A на кнопке паузы героя (фокус там с самого начала) — пауза, как в стоке
    assert_eq!(a.dl.col, 1);
    a.handle(Action::A);
    assert_eq!(writes(), vec![r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#]);
    // вниз к «Далее», вниз к Beta, вправо к «Загрузить первым», A
    a.handle(Action::Down);
    a.handle(Action::Down);
    assert_eq!(a.dl.lines[a.dl.focus].name, "Beta");
    a.handle(Action::Right);
    assert_eq!(a.dl.col, 1);
    a.handle(Action::A);
    assert_eq!(writes(), vec![r#"(SteamClient.Downloads.ResumeAppUpdate(200,"0"),true)"#]);
    // ещё вправо: убрать из очереди
    a.handle(Action::Right);
    assert_eq!(a.dl.col, 2);
    a.handle(Action::A);
    assert_eq!(writes(), vec![r#"(SteamClient.Downloads.RemoveFromDownloadList(200,"0"),true)"#]);
    a.handle(Action::Right);
    assert_eq!(a.dl.col, 2, "правее убрать — края нет");
    assert_eq!(a.live.downloads.as_ref(), Some(&d), "страница не подменяет снимок Steam");
    assert!(a.dl.lines.iter().any(|l| l.name == "Beta"), "строка остаётся, пока Steam не пришлёт новый снимок");
    // «Скрыть все»
    a.dl.focus = a.dl.lines.iter().position(|l| l.hb == expected_text("СКРЫТЬ ВСЕ", "CLEAR ALL")).unwrap();
    a.dl.col = 0;
    a.handle(Action::A);
    let w = writes();
    assert_eq!(w.len(), 1);
    assert!(w[0].starts_with("[(SteamClient.Downloads.RemoveFromDownloadList(900,") && w[0].contains("RemoveFromDownloadList(800,"), "{}", w[0]);
}

#[test]
fn unknown_pause_state_sends_nothing() {
    let (mut d, s) = fixture();
    d.overview.paused = None;
    let (_ui, mut a) = open_page(Some(d), Some(s));
    a.handle(Action::Y);
    assert!(writes().is_empty());
}

#[test]
fn start_menu_offers_queue_order_actions() {
    let (d, s) = fixture();
    let (_ui, mut a) = open_page(Some(d), Some(s));
    a.dl.focus = a.dl.lines.iter().position(|l| l.name == "Gamma").unwrap();
    a.handle(Action::Menu);
    let labels: Vec<String> = a.lb.menu.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect();
    assert!(labels.contains(&expected_text("Выше в очереди", "Move up in queue").to_string()) && labels.contains(&expected_text("Ниже в очереди", "Move down in queue").to_string()), "{labels:?}");
    assert!(labels.contains(&expected_text("Убрать из очереди (не скачивать)", "Remove from queue (do not download)").to_string()));
    let up = a.lb.menu.as_ref().unwrap().items.iter().position(|i| i.label == expected_text("Выше в очереди", "Move up in queue")).unwrap();
    a.lb.menu.as_mut().unwrap().focus = up;
    a.handle(Action::A);
    assert_eq!(writes(), vec![r#"(SteamClient.Downloads.SetQueueIndex(300,1,"0"),true)"#]);
}

#[test]
fn bridge_down_shows_unknown_state_and_no_actions() {
    let (_ui, mut a) = open_page(None, None);
    assert_eq!(a.dl.page.as_ref().unwrap().summary.headline, expected_text("Нет данных от Steam", "No data from Steam"));
    assert!(a.dl.lines.iter().all(|l| !l.focusable() || l.primary.is_none()));
    a.handle(Action::Y);
    a.handle(Action::A);
    assert!(writes().is_empty());
}

/// Ключевые состояния страницы, отрисованные программным рендером (PC). PNG пишутся при DECKSHELL_PNG_DIR.
#[test]
fn renders_key_states() {
    let (mut d, s) = fixture();
    // размеры фикстуры в байтах слишком малы для подписей «Мбайт»: для картинок умножаем на миллион
    for i in d.items.iter_mut() {
        for v in [&mut i.bytes, &mut i.total] { *v *= 1_000_000; }
        for v in [&mut i.dl_bytes, &mut i.dl_total] { *v = v.map(|x| x * 1_000_000); }
    }
    for v in [&mut d.overview.dl_bytes, &mut d.overview.dl_total] { *v = v.map(|x| x * 1_000_000); }
    d.overview.peak_net_bytes_per_sec = 5_800_000;
    let (ui, mut a) = open_page(Some(d.clone()), Some(s.clone()));
    let snap = |ui: &crate::AppWindow| -> Vec<u8> {
        for _ in 0..4 {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(300));
            let _ = ui.window().take_snapshot();
        }
        ui.window().take_snapshot().unwrap().as_bytes().to_vec()
    };
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-hero");
    let hero = snap(&ui);
    // фокус на кнопке «Загрузить первым» второй карточки
    a.handle(Action::Down);
    a.handle(Action::Down);
    a.handle(Action::Right);
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-queue-button-focus");
    let queue = snap(&ui);
    assert_ne!(hero, queue, "фокус и прокрутка меняют кадр");
    // конец списка: «Завершено» и примечание
    for _ in 0..12 {
        a.handle(Action::Down);
    }
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-completed");
    // пауза всех, ошибка «нет места»
    let mut p = d.clone();
    p.overview.paused = Some(true);
    p.overview.state = "None".into();
    p.items.iter_mut().find(|i| i.appid == 350).unwrap().update_result = Some(12);
    a.live.downloads = Some(p);
    a.dl.vy = 0.0;
    a.dl.focus = 0;
    a.on_downloads_data();
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-paused-error");
    // пусто
    let mut e = d.clone();
    e.items.clear();
    e.overview = steam_bridge::DownloadOverview { state: "None".into(), paused: Some(false), ..Default::default() };
    a.live.downloads = Some(e);
    a.on_downloads_data();
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-empty");
    // нет данных
    a.live.downloads = None;
    a.on_downloads_data();
    crate::screens::osk::field_tests::shot(&ui, "downloads-page-no-data");
}
