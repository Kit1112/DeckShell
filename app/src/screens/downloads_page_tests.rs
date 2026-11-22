use crate::screens::expected_text;
use super::*;
use serde_json::Value;
use steam_bridge::{parse_download_settings, parse_downloads};

const FIXTURE: &str = include_str!("downloads_menu_fixture.json");

fn fixture() -> (Downloads, DownloadSettings) {
    let v: Value = serde_json::from_str(FIXTURE).unwrap();
    (parse_downloads(&v["downloads"]).unwrap(), parse_download_settings(&v["settings"]).unwrap())
}
fn txt() -> Txt<'static> {
    Txt { tr: &|_, fallback| fallback.to_string(), time: &|ts| format!("T{ts}"), size: &|b| format!("{b}B") }
}

#[test]
fn bytes_are_the_download_stage_only() {
    // js::DOWNLOADS присылает progress[2]: сумма стадий 0..6 (в 2–4 раза больше) больше не попадает в bytes/total.
    let v: Value = serde_json::from_str(include_str!("downloads_page_native_fixture.json")).unwrap();
    let d = parse_downloads(&v["downloads"]).unwrap();
    let i = &d.items[0];
    assert_eq!((i.dl_bytes, i.dl_total), (Some(2_000), Some(8_000)));
    assert_eq!((i.bytes, i.total), (2_000, 8_000), "bytes/total — тоже стадия 2, не сумма всех стадий");
    assert_eq!(item_bytes(i, item_ov(&d, i)), Some((2_000, 8_000)));
}
fn item_ov<'a>(d: &'a Downloads, i: &DownloadItem) -> Option<&'a DownloadOverview> {
    super::super::downloads_menu::item_overview(i, &d.overview)
}

#[test]
fn counts_are_games_not_bytes() {
    let (d, _) = fixture();
    // Очередь без расписания: 100 (активная), 200, 300, 350 и «Не запланировано» (queue_index -1) не входят; завершено: 2.
    let c = Counts::of(&d);
    assert_eq!(c, Counts { queued: 4, completed: 2 });
    assert_eq!((c.index(), c.total()), (3, 6));
    let s = summary(Some(&d), None, &txt());
    assert_eq!(s.headline, expected_text("Загрузка 3 из 6", "Downloading 3 of 6"));
}

#[test]
fn queue_size_uses_stage_two_and_known_sizes_only() {
    let (d, _) = fixture();
    // Alpha: числа обзора 4000/10000, Beta 0/6000, Gamma 500/2000, Broken 0/100 → всего 18100, скачано 4500.
    let q = queue_size(&d);
    assert_eq!(q, QueueSize { total: 18_100, done: 4_500, unknown: 0 });
    assert_eq!(q.remaining(), 13_600);
    let s = summary(Some(&d), None, &txt());
    assert!(s.detail.starts_with(expected_text("Осталось 13600B из 18100B · 4 игры", "13600B of 18100B remaining · 4 games")), "{}", s.detail);
    assert!(s.detail.ends_with(expected_text("Текущая игра: осталось примерно 5 мин.", "Current Game: about 5 min remaining")), "{}", s.detail);
}

#[test]
fn unknown_sizes_are_counted_not_guessed() {
    let (mut d, _) = fixture();
    d.items.iter_mut().find(|i| i.appid == 200).map(|i| { i.dl_bytes = None; i.dl_total = None; i.bytes = 0; i.total = 0; });
    let q = queue_size(&d);
    assert_eq!((q.total, q.unknown), (12_100, 1));
    assert!(summary(Some(&d), None, &txt()).detail.contains(expected_text("размер неизвестен у 1 игре", "size unknown for 1 games")) == false);
    assert!(summary(Some(&d), None, &txt()).detail.contains(expected_text("размер неизвестен у 1 игра", "size unknown for 1 game")));
}

#[test]
fn headline_follows_the_stock_status_string() {
    let (mut d, _) = fixture();
    d.overview.paused = Some(true);
    assert_eq!(summary(Some(&d), None, &txt()).headline, expected_text("Загрузка приостановлена — 4 в очереди", "Download paused — 4 queued"));
    d.overview.paused = Some(false);
    d.overview.state = "None".into();
    d.overview.appid = 0;
    assert_eq!(summary(Some(&d), None, &txt()).headline, expected_text("В очереди: 4", "In queue: 4"));
    d.items.retain(|i| i.completed);
    assert_eq!(summary(Some(&d), None, &txt()).headline, expected_text("Загрузки — завершено 2 из 2", "Downloads — 2 of 2 complete"));
    d.items.clear();
    assert_eq!(summary(Some(&d), None, &txt()).headline, expected_text("Нет активных загрузок", "No active downloads"));
    assert_eq!(summary(None, None, &txt()).headline, expected_text("Нет данных от Steam", "No data from Steam"));
    let (mut d, _) = fixture();
    d.overview.state = "Unpacking".into();
    assert_eq!(summary(Some(&d), None, &txt()).headline, expected_text("Распаковка", "Unpacking"));
}

#[test]
fn hero_card_shows_stage_speed_and_progress() {
    let (d, _) = fixture();
    let p = page(Some(&d), None, &txt());
    let h = p.hero.as_ref().unwrap();
    assert_eq!((h.name.as_str(), h.state.as_str(), h.size.as_str(), h.percent), ("Alpha", expected_text("Загрузка данных", "Downloading data"), "4000B / 10000B", Some(41)));
    assert_eq!((h.speed.as_str(), h.eta.as_str()), (expected_text("2,0 МБ/с", "2.0 MB/s"), expected_text("Осталось примерно 5 мин.", "About 5 min remaining")));
    assert_eq!(h.progress, 0.41);
    assert!(h.primary.as_ref().is_some_and(|a| a.op == Op::EnableAll(false)));
    assert!(h.remove.is_none(), "стоковая кнопка удаления не показывается у активной загрузки");
}

#[test]
fn stage_text_waiting_and_complete() {
    let (mut d, _) = fixture();
    d.overview.dl_bytes = Some(10_000);
    assert_eq!(page(Some(&d), None, &txt()).hero.unwrap().state, expected_text("Загрузка завершена", "Download complete"));
    let (mut d, _) = fixture();
    d.overview.state = "Verifying".into();
    assert_eq!(page(Some(&d), None, &txt()).hero.unwrap().state, expected_text("Проверка", "Validating"));
    let (mut d, _) = fixture();
    d.overview.appid = 999;
    assert_eq!(page(Some(&d), None, &txt()).hero.unwrap().state, expected_text("Остановка предыдущей загрузки", "Stopping Previous Download"));
}

#[test]
fn sections_and_cards_of_the_queue() {
    let (d, s) = fixture();
    let p = page(Some(&d), Some(&s), &txt());
    let titles: Vec<&str> = p.blocks.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, vec![expected_text("Далее (3)", "Up Next (3)"), expected_text("Запланировано (1)", "Scheduled (1)"), expected_text("Не запланировано (3)", "Unscheduled (3)"), expected_text("Завершено (2)", "Completed (2)")]);
    let q = &p.blocks[0];
    assert_eq!(q.cards.iter().map(|c| c.order).collect::<Vec<_>>(), vec![Some(1), Some(2), Some(3)]);
    let beta = &q.cards[0];
    assert_eq!((beta.state.as_str(), beta.size.as_str(), beta.progress), (expected_text("Далее", "Next"), "6000B", -1.0));
    assert!(beta.primary.as_ref().is_some_and(|a| a.token == "#Downloads_MoveToTop_Tooltip"));
    assert!(beta.remove.is_some());
    let broken = &q.cards[2];
    assert!(broken.error && !broken.no_space);
    assert_eq!(broken.state, expected_text("Ошибка обновления (код 6)", "Update error (code 6)"));
    assert!(broken.primary.as_ref().is_some_and(|a| a.token == "#Downloads_Retry_Tooltip"));
    let done = &p.blocks[3].cards[0];
    assert_eq!((done.name.as_str(), done.state.as_str(), done.size.as_str()), ("Newest", expected_text("Завершено: T1780000500", "Completed: T1780000500"), "50B"));
    assert!(done.primary.as_ref().is_some_and(|a| a.op == Op::ViewInLibrary(900)));
    let unq = &p.blocks[2].cards;
    assert_eq!(unq[1].state, expected_text("Приостановлено", "Paused"));
    assert_eq!(unq[2].state, expected_text("В приложении временно отключены обновления", "The application has temporarily disabled updates"));
    assert!(unq[2].primary.is_none(), "обновления отключены приложением — кнопки нет");
    assert_eq!(order(&p), vec![100, 200, 300, 350, 400, 600, 500, 700, 900, 800]);
}

#[test]
fn no_space_error_has_its_own_text() {
    let (mut d, _) = fixture();
    d.items.iter_mut().find(|i| i.appid == 350).unwrap().update_result = Some(12);
    let p = page(Some(&d), None, &txt());
    let c = &p.blocks[0].cards[2];
    assert!(c.no_space && c.error);
    assert_eq!(c.state, expected_text("Недостаточно свободного места на диске", "Not enough free disk space"));
    // 14 в локализации Steam — «Ожидание установочного диска», а не нехватка места (контракт Gemini ошибался)
    d.items.iter_mut().find(|i| i.appid == 350).unwrap().update_result = Some(14);
    assert!(!page(Some(&d), None, &txt()).blocks[0].cards[2].no_space);
}

#[test]
fn everything_paused_shows_paused_not_speed() {
    let (mut d, _) = fixture();
    d.overview.paused = Some(true);
    let p = page(Some(&d), None, &txt());
    let h = p.hero.unwrap();
    assert_eq!(h.state, expected_text("Приостановлено", "Paused"));
    assert!((h.speed.as_str(), h.eta.as_str()) == ("", "") && h.dim, "на паузе скорость и оставшееся время не показываются");
    assert!(p.summary.net.is_empty() && p.summary.disc.is_empty());
    assert!(!p.summary.detail.contains(expected_text("Текущая игра", "Current Game")));
}

#[test]
fn auto_updates_button_follows_the_schedule() {
    let (_, mut s) = fixture();
    assert_eq!(auto_label(Some(&s), &txt()), expected_text("Автообновления запланированы с 01:00 по 05:00", "Auto Updates scheduled from 01:00 to 05:00"));
    s.restrict_auto_updates = Some(false);
    assert_eq!(auto_label(Some(&s), &txt()), expected_text("Автообновления включены", "Auto-updates enabled"));
    assert_eq!(auto_label(None, &txt()), expected_text("Настройки загрузок", "Download Settings"));
}

#[test]
fn eta_words_pick_the_unit() {
    let t = txt();
    assert_eq!(eta_words(45, &t), expected_text("45 сек.", "45 sec"));
    assert_eq!(eta_words(61, &t), expected_text("2 мин.", "2 min"));
    assert_eq!(eta_words(3700, &t), expected_text("1 ч 2 мин.", "1 hr 2 min"));
    assert_eq!(eta_words(-1, &t), expected_text("Подсчёт...", "Calculating..."));
}

#[test]
fn empty_page_has_the_stock_placeholder_state() {
    let (mut d, _) = fixture();
    d.items.clear();
    d.overview = DownloadOverview { state: "None".into(), paused: Some(false), ..Default::default() };
    let p = page(Some(&d), None, &txt());
    assert!(p.empty && p.hero.is_none());
    assert_eq!(p.blocks.len(), 1, "«Далее (0)» с кнопкой автообновлений остаётся");
    assert_eq!(p.blocks[0].title, expected_text("Далее (0)", "Up Next (0)"));
}
