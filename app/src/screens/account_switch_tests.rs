use super::*;
use crate::screens::osk::field_tests::{app, shot};

fn users(n: usize) -> Vec<Account> {
    (0..n).map(|i| Account { account_name: format!("fixture{i}"), persona_name: format!("Игрок {i}"), avatar_url: String::new(),
        steamid: String::new(), remembered: true, has_pin: false, current: i == 0 }).collect()
}
fn snap(ui: &crate::AppWindow) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    for _ in 0..3 { i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(300)); let _ = ui.window().take_snapshot(); }
    ui.window().take_snapshot().unwrap()
}
fn px(b: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x: u32, y: u32) -> (u8, u8, u8) {
    let p = b.as_slice()[(y * b.width() + x) as usize]; (p.r, p.g, p.b)
}
/// Цветные «аватары» для PNG-сравнения со стоком (у фикстур нет avatarcache).
fn paint_avatars(ui: &crate::AppWindow) {
    use slint::Model;
    let ch = ui.global::<ChooserState>();
    let tiles: Vec<AccountTile> = (0..ch.get_tiles().row_count()).map(|i| {
        use slint::Model;
        let mut t = ch.get_tiles().row_data(i).unwrap();
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(64, 64);
        for (k, p) in buf.make_mut_slice().iter_mut().enumerate() {
            *p = slint::Rgba8Pixel { r: (40 * i as u8).wrapping_add((k % 64) as u8), g: 90 + (k / 64) as u8, b: 160 - 15 * i as u8, a: 255 };
        }
        t.avatar = slint::Image::from_rgba8(buf); t.has_avatar = true; t
    }).collect();
    ch.set_tiles(ModelRc::new(VecModel::from(tiles)));
}

#[test]
fn chooser_shows_saved_accounts_without_current_and_plus_last() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(7), false);
    assert_eq!(a.account.page, PAGE_CHOOSER);
    assert_eq!(a.account.tiles, vec![1, 2, 3, 4, 5, 6], "текущий аккаунт не показывается (сток bShowCurrentUser: !1)");
    sync(&a);
    let st = ui.global::<AccountState>(); st.set_open(true);
    paint_avatars(&ui);
    let ch = ui.global::<ChooserState>();
    let first = snap(&ui);
    shot(&ui, "account-chooser-first");
    // Панель как у стока: фон #0d141c, рамка #23262e (снимок 147..1135 × 171..626).
    assert_eq!(px(&first, 640, 200), (0x0d, 0x14, 0x1c));
    assert_eq!(px(&first, 148, 300), (0x23, 0x26, 0x2e));
    // Плитка в фокусе: белая рамка, поднята; «+» без рамки.
    assert_eq!(px(&first, 185, 380), (255, 255, 255), "рамка фокуса первой плитки");
    // Вправо до «+»
    for _ in 0..6 { a.handle(Action::Right); }
    assert_eq!(ch.get_focus(), 6);
    a.handle(Action::Right);
    assert_eq!(ch.get_focus(), 6, "«+» — последний элемент ряда");
    let plus = snap(&ui);
    shot(&ui, "account-chooser-plus");
    assert_eq!(px(&plus, 1031, 390), (255, 255, 255), "белая рамка «+» в фокусе (стоковый снимок x=1030..1032)");
    assert_eq!(px(&plus, 1063, 380), (255, 255, 255), "крест «+»");
    assert_ne!(px(&plus, 185, 380), (255, 255, 255), "фокус ушёл с первой плитки");
    a.handle(Action::Down);
    assert_eq!(ch.get_focus(), 7, "переключатель «спрашивать при запуске»");
    shot(&ui, "account-chooser-toggle");
    a.handle(Action::A);
    assert!(a.account.ask_on_start, "переключатель меняет client setting (в DRYRUN без записи)");
    a.handle(Action::Up);
    assert_eq!(ch.get_focus(), 6);
    a.handle(Action::A);
    assert_eq!(a.account.page, PAGE_LOGIN, "«+» открывает страницу «Войти»");
    assert_eq!(st.get_focus(), 0, "фокус в поле имени (autoFocus стока)");
    assert_eq!(st.get_qr_state(), account_auth::QR_READY, "QR нарисован (офлайн-образец в тесте)");
    // Для сравнения со снимком стока: родной фон Steam (steamui/images/client_login_bg_grid.jpg).
    if let Some(path) = std::env::var_os("DECKSHELL_ACCOUNT_BACKGROUND") {
        st.set_backdrop(slint::Image::load_from_path(std::path::Path::new(&path)).unwrap());
    }
    let login = snap(&ui);
    shot(&ui, "account-login-username-focus");
    assert_eq!(px(&login, 700, 290), (255, 255, 255), "поле имени в фокусе белое");
    // QR: белая карточка 196..444 и тёмные модули внутри.
    assert_eq!(px(&login, 200, 304), (255, 255, 255));
    let dark = (220..420).flat_map(|y| (220..420).map(move |x| (x, y))).filter(|&(x, y)| px(&login, x, y) == (21, 23, 28)).count();
    assert!(dark > 5000, "QR-модули rgb(21,23,28): {dark}");
    a.handle(Action::Right);
    assert_eq!(st.get_focus(), 0);
    a.handle(Action::Down);
    let unfocused = snap(&ui);
    shot(&ui, "account-login-password-focus");
    assert_ne!(px(&unfocused, 700, 290), (255, 255, 255), "поле имени без фокуса тёмное");
    a.handle(Action::B);
    assert_eq!(a.account.page, PAGE_CHOOSER, "B со страницы входа возвращает к выбору");
    assert_eq!(ch.get_focus(), 6, "фокус на «+»");
    a.handle(Action::B);
    assert!(!a.account.open, "B с выбора закрывает окно");
}

#[test]
fn single_saved_account_goes_straight_to_login_like_stock() {
    let (_ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(1), false);
    assert_eq!(a.account.page, PAGE_LOGIN);
    a.handle(Action::B);
    assert!(!a.account.open, "без плиток B закрывает сразу");
}

#[test]
fn tile_activation_goes_to_the_switch_flow_with_that_account() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(4), false);
    ui.global::<ChooserState>().set_focus(1);
    a.handle(Action::A);
    assert_eq!(a.account.selected, Some(2), "вторая плитка = третий сохранённый аккаунт (текущий пропущен)");
    // В тесте (мок-данные) поток смены отказывает без записи.
    assert!(ui.global::<AccountState>().get_message().contains(crate::screens::expected_text("DRYRUN", "Dry run")));
    assert!(!a.account.busy);
}

#[test]
fn username_and_password_fields_use_the_keyboard_and_enable_sign_in() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(1), false);
    let st = ui.global::<AccountState>();
    assert!(!st.get_can_login(), "«Войти» недоступна, пока поля пусты (как сток)");
    a.handle(Action::A);
    assert!(a.osk_is_open(), "A на поле имени открывает клавиатуру");
    for c in "fixture".chars() { a.handle(Action::Char(c)); }
    assert_eq!(st.get_username().as_str(), "fixture");
    a.handle(Action::B);
    assert!(!a.osk_is_open());
    a.handle(Action::Down);
    a.handle(Action::A);
    for c in "s3cret".chars() { a.handle(Action::Char(c)); }
    assert_eq!(password_text(), "s3cret");
    assert_eq!(st.get_password_mask().as_str(), "••••••", "в интерфейс попадают только точки");
    assert!(st.get_can_login());
    a.handle(Action::B);
    shot(&ui, "account-login-filled");
    assert!(a.account.open);
    close(&mut a);
    assert_eq!(password_text(), "", "закрытие окна стирает пароль");
    assert_eq!(st.get_password_mask().as_str(), "");
    assert_eq!(a.account.auth.username, "");
}

#[test]
fn remember_toggle_and_navigation_map_cover_every_control() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(1), false);
    let st = ui.global::<AccountState>();
    let mut seen = std::collections::BTreeSet::new();
    let mut prev = snap(&ui).as_bytes().to_vec();
    for (act, want) in [(Action::Down, 1), (Action::Down, 2), (Action::Down, 3), (Action::Right, 4), (Action::Down, 5),
        (Action::Left, 6), (Action::Up, 7), (Action::Right, 1), (Action::Up, 0)] {
        a.handle(act);
        assert_eq!(st.get_focus(), want, "{act:?}");
        let now = snap(&ui).as_bytes().to_vec();
        assert_ne!(now, prev, "фокус {want} перерисован");
        prev = now; seen.insert(want);
        if let Some(dir) = std::env::var_os("DECKSHELL_PNG_DIR") { let _ = dir; shot(&ui, &format!("account-login-focus-{want}")); }
    }
    assert_eq!(seen.len(), 8);
    st.set_focus(2);
    assert!(a.account.remember, "«Запомнить меня» включено по умолчанию (сток)");
    a.handle(Action::A);
    assert!(!a.account.remember);
    assert!(!st.get_remembered());
}

#[test]
fn qr_failure_shows_refresh_button_reachable_by_pad() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(1), false);
    let st = ui.global::<AccountState>();
    st.set_qr_state(account_auth::QR_FAILED); st.set_qr_error("Истёк срок действия QR-кода — нажмите «обновить»".into());
    a.handle(Action::Left);
    assert_eq!(st.get_focus(), 8, "кнопка «обновить» QR в фокусе");
    let shot_failed = snap(&ui);
    shot(&ui, "account-login-qr-expired");
    assert_eq!(px(&shot_failed, 320, 400), (255, 255, 255), "кнопка обновления в фокусе белая");
    a.handle(Action::A);
    assert_eq!(st.get_qr_state(), account_auth::QR_READY, "новая QR-сессия");
    for state in [account_auth::QR_LOADING, account_auth::QR_SCANNED, account_auth::QR_DONE] {
        st.set_qr_state(state);
        shot(&ui, &format!("account-login-qr-{state}"));
    }
}

#[test]
fn guard_page_renders_email_and_app_variants() {
    let (ui, mut a) = app();
    a.account.open = true;
    load_users(&mut a, users(1), false);
    let st = ui.global::<AccountState>();
    a.account.page = PAGE_GUARD; a.account.auth.guard_kind = 1; st.set_page(PAGE_GUARD); st.set_guard_kind(1); st.set_guard_hint("g***@example.com".into());
    st.set_focus(0);
    a.handle(Action::A);
    assert!(a.osk_is_open());
    for c in "ab1c2x".chars() { a.handle(Action::Char(c)); }
    assert_eq!(a.account.auth.guard_code, "AB1C2", "5 знаков, верхний регистр");
    a.handle(Action::B);
    shot(&ui, "account-guard-email");
    st.set_guard_kind(3); a.account.auth.guard_kind = 3;
    shot(&ui, "account-guard-app-confirm");
    a.handle(Action::B);
    assert_eq!(a.account.page, PAGE_LOGIN, "B со Steam Guard — обратно ко входу");
}
