use crate::screens::expected_text;
use super::*;
use steam_bridge::{parse_download_settings, parse_downloads};

const FIXTURE: &str = include_str!("downloads_menu_fixture.json");

fn fixture() -> (Downloads, DownloadSettings) {
    let v: Value = serde_json::from_str(FIXTURE).unwrap();
    (parse_downloads(&v["downloads"]).unwrap(), parse_download_settings(&v["settings"]).unwrap())
}

fn txt() -> Txt<'static> {
    Txt { tr: &|_, fallback| fallback.to_string(), time: &|ts| format!("T{ts}"), size: &|b| format!("{b}B") }
}

fn ids(v: &[&DownloadItem]) -> Vec<u32> {
    v.iter().map(|i| i.appid).collect()
}

#[test]
fn sections_follow_the_stock_store_getters() {
    let (d, _) = fixture();
    let s = sections(&d);
    assert_eq!(s.iter().map(|x| x.0).collect::<Vec<_>>(), vec![Section::Active, Section::Queued, Section::Scheduled, Section::NotQueued, Section::Completed]);
    assert_eq!(ids(&s[0].1), vec![100], "head of the queue is the active section");
    assert_eq!(ids(&s[1].1), vec![200, 300, 350], "rest of the queue in queue order");
    assert_eq!(ids(&s[2].1), vec![400], "scheduled: no queue index, deferred time set");
    assert_eq!(ids(&s[3].1), vec![600, 500, 700], "not queued, by name");
    assert_eq!(ids(&s[4].1), vec![900, 800], "completed, newest first");
}

#[test]
fn item_states_are_the_stock_enum() {
    let (d, _) = fixture();
    let st = |id: u32| { let (_, i) = find(&d, id).unwrap(); status(i, item_overview(i, &d.overview)) };
    assert_eq!(st(100), Status::ActiveDownloading);
    assert_eq!(st(200), Status::QueuedNext);
    assert_eq!(st(300), Status::Queued);
    assert_eq!(st(350), Status::Error, "an update error wins over the queue position");
    assert_eq!(st(400), Status::Scheduled);
    assert_eq!(st(500), Status::RemovedFromQueue);
    assert_eq!(st(600), Status::DisabledByUser);
    assert_eq!(st(700), Status::DisabledByApp);
    assert_eq!(st(800), Status::Completed);
    let mut paused = d.clone();
    paused.overview.paused = Some(true);
    paused.overview.state = "None".into();
    paused.items[0].active = false;
    let (_, head) = find(&paused, 100).unwrap();
    assert_eq!(status(head, item_overview(head, &paused.overview)), Status::ActivePaused);
    // The overview never applies to an item that is neither active nor at the head.
    let (_, second) = find(&paused, 200).unwrap();
    assert_eq!(item_overview(second, &paused.overview), None);
}

#[test]
fn ok_action_is_the_stock_button_of_each_section() {
    let js = |a: Option<ItemAction>| a.and_then(|a| a.op.js());
    assert_eq!(js(primary(Section::Active, Status::ActiveDownloading, Some(false), 100)).as_deref(), Some(r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#));
    assert_eq!(js(primary(Section::Active, Status::ActivePaused, Some(true), 100)).as_deref(), Some(r#"(SteamClient.Downloads.EnableAllDownloads(true,"0"),true)"#));
    assert_eq!(primary(Section::Active, Status::Active, None, 100), None, "unknown pause state offers no pause/resume");
    for s in [Section::Queued, Section::Scheduled, Section::NotQueued] {
        assert_eq!(js(primary(s, Status::Queued, Some(false), 7)).as_deref(), Some(r#"(SteamClient.Downloads.ResumeAppUpdate(7,"0"),true)"#));
    }
    assert_eq!(primary(Section::Queued, Status::Error, Some(false), 7).unwrap().token, "#Downloads_Retry_Tooltip");
    assert_eq!(primary(Section::NotQueued, Status::DisabledByApp, Some(false), 7), None);
    assert_eq!(primary(Section::Completed, Status::Completed, Some(false), 7).unwrap().op, Op::ViewInLibrary(7));
}

#[test]
fn item_actions_match_the_stock_menu_and_drag_rules() {
    let (d, s) = fixture();
    let ops = |d: &Downloads, id: u32, rate: Option<i64>| item_actions(d, id, rate).into_iter().map(|a| a.op).collect::<Vec<_>>();
    // Active: pause, launch-when-complete (currently off), throttle override (limit set), view. No remove while active.
    assert_eq!(ops(&d, 100, s.throttle_rate), vec![Op::EnableAll(false), Op::LaunchOnComplete { appid: 100, on: true }, Op::SuspendThrottle(true), Op::ViewInLibrary(100)]);
    assert!(!ops(&d, 100, Some(0)).contains(&Op::SuspendThrottle(true)), "no override without a limit");
    assert!(!ops(&d, 100, None).iter().any(|o| matches!(o, Op::SuspendThrottle(_))), "unknown limit: not offered");
    // Second in the queue: cannot move up into the active slot by SetQueueIndex; can move down.
    assert_eq!(ops(&d, 200, None), vec![Op::DownloadNow(200), Op::QueueIndex(200, 2), Op::Unqueue(200), Op::Remove { appid: 200, reenable: false }, Op::ViewInLibrary(200)]);
    assert_eq!(ops(&d, 300, None), vec![Op::DownloadNow(300), Op::QueueIndex(300, 1), Op::QueueIndex(300, 3), Op::Unqueue(300), Op::Remove { appid: 300, reenable: false }, Op::ViewInLibrary(300)]);
    // Last in the queue: only up.
    assert!(ops(&d, 350, None).contains(&Op::QueueIndex(350, 2)) && !ops(&d, 350, None).contains(&Op::QueueIndex(350, 4)));
    assert_eq!(ops(&d, 400, None), vec![Op::DownloadNow(400), Op::Unqueue(400), Op::Remove { appid: 400, reenable: false }, Op::ViewInLibrary(400)]);
    // Not queued: download now only; stock shows no remove button there.
    assert_eq!(ops(&d, 500, None), vec![Op::DownloadNow(500), Op::ViewInLibrary(500)]);
    assert_eq!(ops(&d, 700, None), vec![Op::ViewInLibrary(700)], "disabled by the app: no button");
    assert_eq!(ops(&d, 800, None), vec![Op::ViewInLibrary(800), Op::Remove { appid: 800, reenable: false }]);
    assert_eq!(item_actions(&d, 800, None)[1].token, "#Downloads_RemoveFromList_Tooltip");
    // Paused head of the queue: remove also re-enables downloads, launch-when-complete is cleared with appid 0.
    let mut p = d.clone();
    p.overview.paused = Some(true);
    p.items[0].active = false;
    p.items[0].launch_on_completion = true;
    let a = item_actions(&p, 100, None);
    assert_eq!(a[0].op, Op::EnableAll(true));
    assert_eq!(a[1].op.js().as_deref(), Some("(SteamClient.Downloads.SetLaunchOnUpdateComplete(0),true)"));
    assert_eq!(a[1].check, Some(true));
    assert!(a.iter().any(|x| x.op == Op::Remove { appid: 100, reenable: true }));
    assert!(item_actions(&d, 12345, None).is_empty(), "an item Steam no longer lists has no actions");
}

#[test]
fn list_shows_sections_states_and_progress() {
    let (d, _) = fixture();
    let rows = list_rows(Some(&d), &txt());
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec![
        expected_text("СЕЙЧАС (1)", "NOW (1)"), "Alpha", expected_text("ДАЛЕЕ (3)", "UP NEXT (3)"), "Beta", "Gamma", "Broken", expected_text("ЗАПЛАНИРОВАНО (1)", "SCHEDULED (1)"), "Delta",
        expected_text("НЕ В ОЧЕРЕДИ (3)", "NOT QUEUED (3)"), "Epsilon", "Paused game", "Zeta", expected_text("ЗАВЕРШЕНО (2)", "COMPLETED (2)"), "Newest", "Done",
        expected_text("УПРАВЛЕНИЕ", "MANAGEMENT"), expected_text("Приостановить все загрузки", "Pause all downloads"), expected_text("Настройки загрузок", "Download Settings"), expected_text("Открыть страницу загрузок", "Open the download page"),
    ]);
    let row = |name: &str| rows.iter().find(|r| r.label == name).unwrap();
    assert_eq!(row("Alpha").value, expected_text("Загрузка", "Downloading"));
    assert_eq!(row("Alpha").sub, expected_text("4000B / 10000B · 41% · 2,0 МБ/с · осталось 5 мин", "4000B / 10000B · 41% · 2.0 MB/s · 5 min remaining"));
    assert_eq!(row("Alpha").progress, 0.41);
    assert!(!row("Alpha").dim_bar);
    assert_eq!((row("Beta").value.as_str(), row("Beta").sub.as_str(), row("Beta").progress), (expected_text("Далее", "Next"), "6000B", -1.0));
    assert_eq!((row("Gamma").sub.as_str(), row("Gamma").progress), ("500B / 2000B · 25%", 0.25));
    assert_eq!((row("Broken").value.as_str(), row("Broken").sub.as_str()), (expected_text("Ошибка", "Error"), expected_text("Ошибка обновления (код 6) · disk write failure · 100B", "Update error (code 6) · disk write failure · 100B")));
    assert_eq!((row("Delta").value.as_str(), row("Delta").sub.as_str()), (expected_text("Запланировано", "Scheduled"), expected_text("на T1790000000 · 300B", "at T1790000000 · 300B")));
    assert_eq!(row("Paused game").value, expected_text("Приостановлено", "Paused"));
    assert_eq!(row("Epsilon").sub, expected_text("Обновляется только при запуске", "Updated only at launch"));
    assert_eq!(row("Zeta").sub, expected_text("Обновления отключены приложением", "Updates disabled by app"));
    assert_eq!((row("Done").value.as_str(), row("Done").sub.as_str()), (expected_text("Завершено", "Completed"), "T1780000000"));
    assert_eq!(row("Alpha").target, Target::Item(100));
    assert_eq!(row(expected_text("Приостановить все загрузки", "Pause all downloads")).target, Target::Op(Op::EnableAll(false)));
    // Headers are never focusable; the cursor walks items only and stops at both ends.
    assert_eq!(step(&rows, 1, Action::Up), None);
    assert_eq!(step(&rows, 1, Action::Down), Some(3));
    assert_eq!(step(&rows, 5, Action::Down), Some(7));
    assert_eq!(step(&rows, rows.len() - 1, Action::Down), None);
    assert_eq!(step(&rows, 16, Action::Up), Some(14));
}

#[test]
fn unknown_data_is_shown_as_unavailable_not_guessed() {
    let rows = list_rows(None, &txt());
    assert_eq!(rows[0].kind, RowKind::Note);
    assert!(!rows.iter().any(|r| matches!(r.target, Target::Item(_) | Target::Op(_))), "no download actions without Steam data");
    let all = rows.iter().find(|r| r.label == expected_text("Приостановить все загрузки", "Pause all downloads")).unwrap();
    assert_eq!((all.enabled, all.value.as_str()), (false, i18n::tr_ref(UNAVAILABLE)));
    assert_eq!(status_line(None, None), expected_text("Нет данных от Steam", "No data from Steam"));
    // A snapshot without the pause flag: no pause/resume anywhere, the rest still listed.
    let (mut d, s) = fixture();
    d.overview.paused = None;
    d.overview.throttling_suspended = None;
    let rows = list_rows(Some(&d), &txt());
    assert!(!rows.iter().any(|r| matches!(r.target, Target::Op(Op::EnableAll(_)))));
    let actions = action_rows(Some(&d), 100, Some(&s), &txt());
    assert!(!actions.iter().any(|r| matches!(r.target, Target::Op(Op::EnableAll(_) | Op::SuspendThrottle(_)))));
    assert_eq!(actions.iter().filter(|r| !r.enabled && r.value == i18n::tr_ref(UNAVAILABLE)).count(), 2);
    assert!(status_line(Some(&d), Some(&s)).starts_with(expected_text("Пауза: неизвестно", "Pause status unknown")));
    // Empty Steam list is a real answer, not an error.
    let empty = Downloads { overview: DownloadOverview { state: "None".into(), paused: Some(false), ..Default::default() }, history: vec![], items: vec![] };
    assert_eq!(list_rows(Some(&empty), &txt())[0].label, expected_text("Нет загрузок", "No downloads"));
    assert_eq!(status_line(Some(&empty), None), expected_text("Нет активной загрузки", "No active downloads"));
    assert_eq!(action_rows(Some(&empty), 100, None, &txt()).len(), 1, "a vanished item leaves only the note");
    // An item without a state Steam recognises stays "unknown".
    let odd = DownloadItem { appid: 1, queue_index: -1, ..Default::default() };
    assert_eq!(status(&odd, None), Status::Unknown);
}

#[test]
fn settings_rows_follow_steam_values() {
    let (_, s) = fixture();
    let rows = settings_rows(Some(&s), &txt());
    let got: Vec<(&str, &str, i32, bool)> = rows.iter().map(|r| (r.label.as_str(), r.value.as_str(), r.check, r.enabled)).collect();
    assert_eq!(got, vec![
        (expected_text("Ограничить скорость скачивания", "Limit download speed"), expected_text("1000 КБ/с", "1000 KB/s"), -1, true),
        (expected_text("Скорость в битах в секунду", "Speed in bits per second"), "", 0, true),
        (expected_text("Обновление игр", "Update games"), expected_text("Разрешить Steam выбирать время обновления", "Let Steam decide when to update"), -1, true),
        (expected_text("Автообновление по расписанию", "Schedule auto-updates"), "", 1, true),
        (expected_text("Обновлять с", "Update from"), "01:00", -1, true),
        (expected_text("до", "to"), "05:00", -1, true),
        (expected_text("Разрешить загрузки во время игры", "Allow downloads during gameplay"), "", 0, true),
        (expected_text("Ограничить загрузки во время трансляции", "Throttle downloads while streaming"), "", 1, true),
        (expected_text("Передача файлов по локальной сети", "Transfer of files over a local network"), expected_text("Только мои устройства", "Only My Devices"), -1, true),
        (expected_text("Регион загрузки", "Download region"), "Poland - Katowice", -1, false),
    ]);
    assert_eq!(rows[1].target, Target::Toggle(Setting::RateBits, true));
    assert_eq!(rows[4].target, Target::Choose(Setting::RestrictStart));
    // "Update on launch" hides the schedule, as in Steam.
    let on_launch = DownloadSettings { update_behavior: Some(1), ..s.clone() };
    assert!(!settings_rows(Some(&on_launch), &txt()).iter().any(|r| r.label.contains(expected_text("расписанию", "schedule")) || r.label == expected_text("до", "to")));
    let no_schedule = DownloadSettings { restrict_auto_updates: Some(false), ..s.clone() };
    assert_eq!(settings_rows(Some(&no_schedule), &txt()).len(), 8);
    // Nothing read: every row unavailable, none writable.
    let none = settings_rows(None, &txt());
    assert_eq!(none[0].kind, RowKind::Note);
    assert!(none[1..].iter().all(|r| !r.enabled && r.value == i18n::tr_ref(UNAVAILABLE) && r.target == Target::None));
    // Partly reported: only the missing values are unavailable.
    let partial = DownloadSettings { rate_bits: None, peer_content: None, hours: vec![], ..s.clone() };
    let rows = settings_rows(Some(&partial), &txt());
    let row = |name: &str| rows.iter().find(|r| r.label == name).unwrap();
    assert!(!row(expected_text("Скорость в битах в секунду", "Speed in bits per second")).enabled);
    assert!(!row(expected_text("Передача файлов по локальной сети", "Transfer of files over a local network")).enabled);
    assert!(!row(expected_text("Обновлять с", "Update from")).enabled, "no hour list from Steam: nothing to choose from");
    assert_eq!(row(expected_text("Ограничить скорость скачивания", "Limit download speed")).value, expected_text("8000 кбит/с", "8000 kbit/s"), "unit setting unknown: the stored unit is shown");
    let unknown_enum = DownloadSettings { update_behavior: Some(9), ..s.clone() };
    assert_eq!(settings_rows(Some(&unknown_enum), &txt())[2].value, expected_text("неизвестно (9)", "unknown (9)"));
}

#[test]
fn setting_choices_and_writes() {
    let (_, s) = fixture();
    let limit = choices(Setting::Limit, &s);
    assert_eq!(limit[0], (0, expected_text("Без ограничения", "Unlimited").to_string()));
    assert!(limit.contains(&(8000, expected_text("1000 КБ/с", "1000 KB/s").to_string())), "current value is in the list");
    assert!(limit.windows(2).all(|w| w[0].0 < w[1].0));
    let bits = DownloadSettings { rate_bits: Some(true), throttle_rate: Some(3333), ..s.clone() };
    assert!(choices(Setting::Limit, &bits).contains(&(3333, expected_text("3333 кбит/с", "3333 kbit/s").to_string())));
    assert_eq!(limit_label(8004, Some(false)), expected_text("1000,5 КБ/с", "1000.5 KB/s"));
    assert_eq!(choices(Setting::RestrictStart, &s).len(), 3);
    assert_eq!(choices(Setting::PeerContent, &s).iter().map(|c| c.0).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    assert_eq!(choices(Setting::UpdateBehavior, &s).iter().map(|c| c.0).collect::<Vec<_>>(), vec![3, 1]);
    let picks = choice_rows(Setting::PeerContent, Some(&s));
    assert_eq!(picks.iter().map(|r| r.check).collect::<Vec<_>>(), vec![0, 1, 0, 0]);
    assert_eq!(picks[2].target, Target::Pick(Setting::PeerContent, 2));
    // download_throttle_rate is field 8012 (varint): tag e0 f4 03, value 8000 = c0 3e.
    assert_eq!(setting_write(Setting::Limit, &json!(8000)).as_deref(), Some(r#"SteamClient.Settings.SetSetting("4PQDwD4=")"#));
    for setting in [Setting::RateBits, Setting::Restrict, Setting::WhileRunning, Setting::ThrottleStreaming] {
        assert!(setting_write(setting, &json!(true)).is_some(), "{setting:?}");
    }
    for setting in [Setting::UpdateBehavior, Setting::RestrictStart, Setting::RestrictEnd, Setting::PeerContent] {
        assert!(setting_write(setting, &json!(1)).is_some(), "{setting:?}");
    }
    assert_eq!(setting_write(Setting::Region, &json!(1)), None, "region is read-only here");
}

#[test]
fn status_line_reports_speed_limit_and_pause() {
    let (d, s) = fixture();
    assert_eq!(status_line(Some(&d), Some(&s)), expected_text("Загрузка · 2,0 МБ/с · лимит 1000 КБ/с", "Downloading · 2.0 MB/s · limit 1000 KB/s"));
    let bits = DownloadSettings { rate_bits: Some(true), ..s.clone() };
    assert_eq!(status_line(Some(&d), Some(&bits)), expected_text("Загрузка · 16,0 Мбит/с · лимит 8000 кбит/с", "Downloading · 16.0 Mbit/s · limit 8000 kbit/s"));
    let mut p = d.clone();
    p.overview.paused = Some(true);
    p.overview.throttling_suspended = Some(true);
    assert_eq!(status_line(Some(&p), Some(&s)), expected_text("Загрузки приостановлены · лимит 1000 КБ/с (снят для текущей)", "Downloads paused · limit 1000 KB/s (lifted for current download)"));
}

/// Side menu glue: A on «Загрузки» opens the panel next to the rail, B walks back to the rail, closing the menu
/// drops the panel. No bridge in tests: settings come back as "not delivered" and no write is sent.
#[test]
fn side_menu_opens_and_closes_the_panel() {
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    let ds = std::sync::Arc::new(crate::data::DataSource::new());
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
    crate::screens::downloads_menu::wire_models(&a, &ui);
    a.live.downloads = Some(fixture().0);
    let route = a.router.current().clone();
    a.toggle_side();
    assert!(a.sm.open);
    let idx = a.sm.acts.iter().position(|x| *x == crate::screens::steam_menu::MenuAct::Downloads).unwrap();
    a.menu_activate(idx);
    assert_eq!(a.dm.view, View::List);
    assert!(a.sm.open, "the rail stays open next to the panel");
    assert_eq!(a.router.current(), &route, "no page navigation");
    let st = ui.global::<DownloadsMenuState>();
    assert!(st.get_open());
    assert_eq!(a.dm.rows[a.dm.focus].label, "Alpha");
    assert_eq!(a.dm.settings, Some(None), "no bridge: settings not delivered");
    // Down walks items only (the next header is skipped).
    a.handle(Action::Down);
    assert_eq!(a.dm.rows[a.dm.focus].label, "Beta");
    // A on an item opens its actions; B returns to the same row.
    a.handle(Action::A);
    assert_eq!(a.dm.view, View::Actions(200));
    a.handle(Action::B);
    assert_eq!((a.dm.view, a.dm.rows[a.dm.focus].label.as_str()), (View::List, "Beta"));
    // Settings without Steam data: every row unavailable, nothing focusable to write.
    a.dm.focus = a.dm.rows.iter().position(|r| r.target == Target::OpenSettings).unwrap();
    a.handle(Action::A);
    assert_eq!(a.dm.view, View::Settings);
    assert!(a.dm.rows.iter().all(|r| !r.enabled));
    a.handle(Action::A);
    assert_eq!(a.dm.view, View::Settings, "a disabled row does nothing");
    a.handle(Action::B);
    a.handle(Action::B);
    assert_eq!(a.dm.view, View::Closed);
    assert!(a.sm.open && !st.get_open(), "B from the list returns to the rail");
    // Closing the side menu (STEAM; called directly to skip the 300 ms guide debounce) drops the panel; reopening the menu does not bring it back.
    a.menu_activate(idx);
    assert!(a.dm.is_open());
    a.toggle_side();
    assert!(!a.sm.open && !a.dm.is_open() && !st.get_open());
    a.toggle_side();
    assert!(a.sm.open && !a.dm.is_open());
    // A write without a bridge (or in dry run) changes nothing locally: the panel waits for Steam.
    a.menu_activate(idx);
    let before = a.live.downloads.clone();
    a.dm.focus = a.dm.rows.iter().position(|r| r.target == Target::Op(Op::EnableAll(false))).unwrap();
    a.handle(Action::A);
    assert_eq!(a.live.downloads, before);
    assert_eq!(a.dm.view, View::List);
}

/// The panel drawn by the software renderer from the fixture: list, item actions, settings. PC render only.
/// With DECKSHELL_GALLERY_DIR set the frames are written there.
#[test]
fn panel_renders_from_fixture() {
    use crate::{MenuItem, SteamMenuState};
    let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
    )));
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::PhysicalSize::new(1280, 800));
    let menu = ui.global::<SteamMenuState>();
    let names = ["Главная", "Библиотека", "Магазин", "Друзья и чат", "Медиа", "Загрузки", "Настройки", "Выключение"];
    menu.set_items(ModelRc::new(VecModel::from(names.iter().enumerate().map(|(i, s)| MenuItem { label: (*s).into(), current: i == 5, ..Default::default() }).collect::<Vec<_>>())));
    menu.set_row_y(ModelRc::new(VecModel::from((0..names.len()).map(|i| 52.5 + 72.0 * i as f32).collect::<Vec<_>>())));
    menu.set_content_h(681.0);
    menu.set_focus(-1);
    menu.set_game_focus(-1);
    menu.set_open(true);
    let st = ui.global::<DownloadsMenuState>();
    let (d, s) = fixture();
    let out = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &out { std::fs::create_dir_all(dir).unwrap(); }
    let shot = |name: &str, open: bool, title: &str, rows: &[Row], focus: i32| -> Vec<u8> {
        let (ys, _) = layout(rows);
        st.set_rows(ModelRc::new(VecModel::from(rows.iter().zip(&ys).map(|(r, &y)| DlMenuRow {
            label: r.label.as_str().into(), value: r.value.as_str().into(), sub: r.sub.as_str().into(), progress: r.progress,
            kind: match r.kind { RowKind::Item => 0, RowKind::Header => 1, RowKind::Note => 2 },
            enabled: r.enabled, check: r.check, dim_bar: r.dim_bar, y, h: r.height(),
        }).collect::<Vec<_>>())));
        st.set_title(title.into());
        st.set_status(status_line(Some(&d), Some(&s)).into());
        st.set_focus(focus);
        st.set_open(open);
        let _ = ui.window().take_snapshot().expect("first snapshot");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
        slint::platform::update_timers_and_animations();
        let snap = ui.window().take_snapshot().expect("snapshot");
        assert_eq!((snap.width(), snap.height()), (1280, 800), "{name}");
        if let Some(dir) = &out {
            image::save_buffer(dir.join(format!("{name}.png")), snap.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        snap.as_bytes().to_vec()
    };
    let list = list_rows(Some(&d), &txt());
    let closed = shot("downloads-menu-closed", false, "", &list, -1);
    let open = shot("downloads-menu-list", true, "Загрузки", &list, 1);
    let moved = shot("downloads-menu-list-focus-next", true, "Загрузки", &list, 3);
    let actions = shot("downloads-menu-actions", true, "Alpha", &action_rows(Some(&d), 100, Some(&s), &txt()), 1);
    let settings = shot("downloads-menu-settings", true, "Настройки загрузок", &settings_rows(Some(&s), &txt()), 0);
    let unknown = shot("downloads-menu-unavailable", true, "Настройки загрузок", &settings_rows(None, &txt()), 1);
    let px = |b: &[u8], x: usize, y: usize| { let i = (y * 1280 + x) * 4; (b[i], b[i + 1], b[i + 2]) };
    let differs = |a: &[u8], b: &[u8]| a.iter().zip(b).filter(|(x, y)| x != y).count();
    // The panel sits right of the 360 px rail and covers the dimmed page with the side-menu surface colour.
    assert_ne!(px(&closed, 600, 400), px(&open, 600, 760 - 30), "panel surface is drawn");
    assert_eq!(px(&open, 900, 730), (0x0e, 0x14, 0x1b), "side-menu background (Tokens.menu-bg)");
    assert_eq!(px(&open, 1000, 400), px(&closed, 1000, 400), "page right of the panel is untouched");
    assert_eq!(px(&open, 100, 700), px(&closed, 100, 700), "the rail is untouched");
    assert!(differs(&open, &moved) > 0, "focus moves between rows");
    assert!(differs(&open, &actions) > 0 && differs(&actions, &settings) > 0 && differs(&settings, &unknown) > 0, "each view draws its own rows");
}

#[test]
fn custom_speed_limit_row_opens_numeric_keyboard_and_returns_to_choice() {
    let (ui, mut a) = crate::screens::osk::field_tests::app();
    crate::screens::downloads_menu::wire_models(&a, &ui);
    let (d, s) = fixture();
    a.live.downloads = Some(d);
    a.dm.settings = Some(Some(s));
    a.dm.view = View::Choice(Setting::Limit);
    a.dl_menu_rebuild(true);
    let last = a.dm.rows.len() - 1;
    assert_eq!(a.dm.rows[last].target, Target::Custom(Setting::Limit));
    a.dm.focus = last;
    a.handle_dl_menu(Action::A);
    assert!(a.osk_is_open(), "A на «Своё значение…» открывает клавиатуру");
    assert_eq!(crate::screens::osk::current_mode(&a), Some(crate::screens::osk::InputMode::Numeric));
    for c in "12x5".chars() { a.handle(Action::Char(c)); }
    assert_eq!(a.dm.rows[last].label, expected_text("Своё значение: 125 КБ/с", "Custom value: 125 KB/s"));
    crate::screens::osk::field_tests::shot(&ui, "downloads-custom-limit");
    a.handle(Action::B);
    assert!(!a.osk_is_open());
    assert_eq!(a.dm.view, View::Choice(Setting::Limit), "закрытие возвращает фокус списку");
    assert_eq!(a.dm.rows[a.dm.focus].target, Target::Custom(Setting::Limit));
    assert_eq!(a.dm.rows[last].label, expected_text("Своё значение…", "Custom value…"), "отмена очищает ввод");
    assert_eq!(custom_rate("125", Some(false)), Some(1000));
    assert_eq!(custom_rate("1.5", Some(false)), Some(12));
    assert_eq!(custom_rate("3000", Some(true)), Some(3000));
    assert_eq!(custom_rate("0", Some(true)), None);
}
