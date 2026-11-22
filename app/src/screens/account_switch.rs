//! «Сменить аккаунт»: страница 0 — выбор (плитки сохранённых аккаунтов + «Добавить аккаунт», как сток
//! sp.js 72654/35200), страница 1 — «Войти» (QR + имя/пароль, `account_auth`), страница 2 — Steam Guard.
//! Выбор передаётся потоку смены через общую модель `account_request::LoginRequest`.
//! UI stays mapped; native writes run on one bounded worker per attempt.
use steam_assets::i18n;
use super::account_auth;
use super::account_request::LoginRequest;
use crate::{app::App, input::Action, AccountRow, AccountState, AccountTile, ChooserState};
use slint::{ComponentHandle, ModelRc, VecModel};
use steam_bridge::account_switch::{Account, Progress};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

/// Страницы (`AccountState.page`).
pub const PAGE_CHOOSER: i32 = 0;
pub const PAGE_LOGIN: i32 = 1;
pub const PAGE_GUARD: i32 = 2;
/// Сток показывает не больше 8 сохранённых аккаунтов (`.slice(0, 8)`, sp.js 535421) и без текущего (`bShowCurrentUser: !1`).
pub const MAX_TILES: usize = 8;

#[derive(Default)]
pub struct State {
    pub open: bool, pub busy: bool, generation: u64, users: Vec<Account>,
    selected: Option<usize>, uncertain: bool, last_id: Option<String>, cancel: Option<Arc<AtomicBool>>,
    /// Индексы `users`, показанные плитками (без текущего, ≤ 8).
    tiles: Vec<usize>,
    pub page: i32,
    /// «Запомнить меня» (сток: включено по умолчанию).
    remember: bool,
    /// Client setting always_show_user_chooser.
    ask_on_start: bool,
    pub auth: account_auth::AuthState,
}
thread_local! { static PASSWORD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) }; }

/// Поле «Пароль»: ввод через общую клавиатуру в режиме Password (вывод — точки); текст живёт только здесь
/// и стирается при закрытии окна или отправке.
const PASSWORD_SINK: super::osk::ExtSink = super::osk::ExtSink { id: 4, mode: super::osk::InputMode::Password, typed: password_typed, enter: password_done, cancel: password_cancel };
/// Поле имени аккаунта.
const USERNAME_SINK: super::osk::ExtSink = super::osk::ExtSink { id: 7, mode: super::osk::InputMode::Normal, typed: username_typed, enter: username_done, cancel: password_cancel };
/// Код Steam Guard (5 знаков).
const GUARD_SINK: super::osk::ExtSink = super::osk::ExtSink { id: 8, mode: super::osk::InputMode::Normal, typed: account_auth::guard_typed, enter: guard_done, cancel: password_cancel };

fn password_sync(a: &App) {
    let mask = PASSWORD.with(|p| super::osk::mask(super::osk::InputMode::Password, &p.borrow()));
    a.ui().global::<AccountState>().set_password_mask(mask.into());
    sync_login(a);
}
fn password_typed(a: &mut App, c: Option<char>) {
    PASSWORD.with(|p| match c { Some(c) => p.borrow_mut().push(c), None => { p.borrow_mut().pop(); } });
    password_sync(a);
}
fn password_done(a: &mut App) {
    // Enter на клавиатуре в поле пароля = «Войти» (как форма стока).
    a.osk_set_open(false);
    if can_login(a) { a.ui().global::<AccountState>().set_focus(3); login(a); }
}
fn password_cancel(_: &mut App) {}
fn username_typed(a: &mut App, c: Option<char>) {
    let name = &mut a.account.auth.username;
    match c { Some(c) => { if name.chars().count() < 64 { name.push(c) } } None => { name.pop(); } }
    a.ui().global::<AccountState>().set_username(a.account.auth.username.clone().into());
    sync_login(a);
}
fn username_done(a: &mut App) {
    // Enter в поле имени — к паролю (сток: Tab/Enter переводит фокус).
    a.ui().global::<AccountState>().set_focus(1);
    super::osk::attach(a, PASSWORD_SINK);
}
fn guard_done(a: &mut App) { a.osk_set_open(false); account_auth::guard_primary(a); }
#[cfg(test)]
pub(crate) fn password_text() -> String { PASSWORD.with(|p| p.borrow().clone()) }
fn take_password() -> steam_auth::Secret {
    steam_auth::Secret::new(PASSWORD.with(|p| std::mem::take(&mut *p.borrow_mut())))
}

pub fn wire(ui: &crate::AppWindow) {
    let account = ui.global::<AccountState>();
    account.set_label_back(i18n::tr("account_switch.back").into());
    account.set_label_cancel(crate::screens::stock_label("#Button_Cancel", "account_switch.cancel").into());
    account.set_label_confirm(crate::screens::stock_label("#Button_Confirm", "account_switch.confirm").into());
    account.set_label_create_a_new_account(i18n::tr("account_switch.create_a_new_account").into());
    account.set_label_enter_the_code(i18n::tr("account_switch.enter_the_code").into());
    account.set_label_enter_the_code_from_your_steam_mobile(crate::screens::stock_label("#Login_EnterMobileCode", "account_switch.enter_the_code_from_your_steam_mobile").into());
    account.set_label_enter_the_code_received_to_the_email(i18n::tr("account_switch.enter_the_code_received_to_the_email").into());
    account.set_label_help_i_can_t_sign_in(crate::screens::stock_label("#Login_Help_SignIn", "account_switch.help_i_can_t_sign_in").into());
    account.set_label_log_in(i18n::tr("account_switch.log_in").into());
    account.set_label_menue(i18n::tr("account_switch.menue").into());
    account.set_label_mobile_app(i18n::tr("account_switch.mobile_app").into());
    account.set_label_password(i18n::tr("account_switch.password").into());
    account.set_label_remember_me(crate::screens::stock_label("#Login_RememberMe_Short", "account_switch.remember_me").into());
    account.set_label_select(i18n::tr("account_switch.select").into());
    account.set_label_sign_in(crate::screens::stock_label("#Login_Login", "account_switch.sign_in").into());
    account.set_label_sign_in_using_the_account_name(i18n::tr("account_switch.sign_in_using_the_account_name").into());
    account.set_label_so_that(i18n::tr("account_switch.so_that").into());
    account.set_label_use_it(i18n::tr("account_switch.use_it").into());
    account.set_label_use_the_steam_mobile_app_to_confirm(i18n::tr("account_switch.use_the_steam_mobile_app_to_confirm").into());
    account.set_label_you_have_a_mobile_authenticator_protecting_this(crate::screens::stock_label("#Login_MobileProtectingAccount", "account_switch.you_have_a_mobile_authenticator_protecting_this").into());
    account.set_label_you_have_an_email_authenticator_protecting_this(crate::screens::stock_label("#Login_EmailProtectingAccount", "account_switch.you_have_an_email_authenticator_protecting_this").into());
    let chooser = ui.global::<ChooserState>();
    chooser.set_label_add_account(crate::screens::stock_label("#Login_AddAccount", "account_chooser.add_account").into());
    chooser.set_label_ask_which_account_to_use_each_time(i18n::tr("account_chooser.ask_which_account_to_use_each_time").into());
    chooser.set_label_change_account(i18n::tr("account_chooser.change_account").into());
    chooser.set_label_select_a_user_or_sign_in_with(crate::screens::stock_label("#Login_CachedUsers", "account_chooser.select_a_user_or_sign_in_with").into());
    ui.global::<AccountState>().on_pick(|i| super::defer(move |a| pick(a, i)));
    ui.global::<ChooserState>().on_activate(|i| super::defer(move |a| chooser_activate(a, i)));
}
fn can_login(a: &App) -> bool {
    !a.account.auth.username.trim().is_empty() && PASSWORD.with(|p| !p.borrow().is_empty())
        && !a.account.busy && !a.account.uncertain && !a.account.auth.password_busy && !a.account.auth.done
}
fn sync_login(a: &App) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_can_login(can_login(a));
    st.set_remembered(a.account.remember);
}
fn avatar(u: &Account) -> Option<slint::Image> {
    // Avatarcache is under Steam/config; never open arbitrary native URLs as paths.
    if u.steamid.is_empty() || u.avatar_url != format!("/avatarcache/{}.png", u.steamid) { return None; }
    slint::Image::load_from_path(&steam_assets::steam_root().join("config").join(u.avatar_url.trim_start_matches('/'))).ok()
}
fn sync(a: &App) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_open(a.account.open); st.set_busy(a.account.busy || a.account.auth.password_busy);
    st.set_page(a.account.page);
    let rows = a.account.users.iter().map(|u| AccountRow { name: u.account_name.clone().into(), persona: u.persona_name.clone().into(),
        steamid: u.steamid.clone().into(), avatar: avatar(u).unwrap_or_default(), current: u.current }).collect::<Vec<_>>();
    st.set_users(ModelRc::new(VecModel::from(rows)));
    let tiles = a.account.tiles.iter().filter_map(|&i| a.account.users.get(i)).map(|u| {
        let img = avatar(u);
        AccountTile { persona: (if u.persona_name.is_empty() { &u.account_name } else { &u.persona_name }).clone().into(),
            has_avatar: img.is_some(), avatar: img.unwrap_or_default() }
    }).collect::<Vec<_>>();
    let ch = ui.global::<ChooserState>();
    ch.set_tiles(ModelRc::new(VecModel::from(tiles)));
    ch.set_ask_on_start(a.account.ask_on_start);
    ch.set_busy(a.account.busy);
    sync_login(a);
}
/// Плитки: все сохранённые, кроме текущего, не больше 8 (сток).
fn tiles_for(users: &[Account]) -> Vec<usize> {
    users.iter().enumerate().filter(|(_, u)| !u.current).map(|(i, _)| i).take(MAX_TILES).collect()
}
pub fn open(a: &mut App) {
    if a.account.open { return; }
    a.account.open = true; a.account.busy = true; a.account.page = PAGE_CHOOSER; a.account.remember = true;
    a.account.generation = a.account.generation.wrapping_add(1);
    let generation = a.account.generation;
    a.header_clear();
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_focus(0); st.set_message("".into()); st.set_username("".into());
    ui.global::<ChooserState>().set_focus(0);
    st.set_backdrop(slint::Image::load_from_path(&steam_assets::steam_root().join("steamui/images/client_login_bg_grid.jpg")).unwrap_or_default());
    sync(a); a.sync_overlays();
    if a.ds.is_mock() { load_users(a, mock_users(), false); return; }
    std::thread::spawn(move || {
        let bridge = crate::actions::bridge();
        let result = bridge.as_ref().ok_or_else(|| i18n::tr_ref("account_switch.steam_is_not_available").to_string())
            .and_then(|b| b.login_accounts().map_err(|_| i18n::tr_ref("account_switch.i_couldn_t_read_the_steam_accounts").to_string()));
        let ask = bridge.and_then(|b| b.login_device_info().ok()).is_some_and(|d| d.always_show_user_chooser);
        super::defer(move |a| {
            if generation != a.account.generation || !a.account.open { return; }
            match result {
                Ok(data) => load_users(a, data.users, ask),
                Err(e) => { a.account.busy = false; a.ui().global::<AccountState>().set_message(e.into()); sync(a); }
            }
        });
    });
}
/// Мок (`DECKSHELL_MOCK`): три сохранённых аккаунта, первый — текущий.
fn mock_users() -> Vec<Account> {
    (0..3).map(|i| Account { account_name: format!("mock{i}"), persona_name: i18n::tr_args("account_switch.player", &[("index", &(i).to_string())]), avatar_url: String::new(),
        steamid: String::new(), remembered: true, has_pin: false, current: i == 0 }).collect()
}
pub(crate) fn load_users(a: &mut App, users: Vec<Account>, ask: bool) {
    a.account.busy = false;
    a.account.users = users;
    a.account.ask_on_start = ask;
    a.account.tiles = tiles_for(&a.account.users);
    a.account.selected = a.account.users.iter().position(|u| u.current);
    if a.account.uncertain {
        a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.the_result_of_the_previous_login_is").into());
    }
    // Сток (35200): при единственном сохранённом аккаунте выбора нет — сразу экран входа.
    if a.account.tiles.is_empty() { go_login(a); } else { sync(a); }
}
fn go_login(a: &mut App) {
    a.account.page = PAGE_LOGIN;
    // Форма стока монтируется заново: «Запомнить меня» включено по умолчанию.
    a.account.remember = true;
    // Сток: autoFocus у поля имени.
    a.ui().global::<AccountState>().set_focus(0);
    sync(a);
    account_auth::qr_start(a);
}
fn close(a: &mut App) {
    a.osk_set_open(false);
    account_auth::reset(a);
    PASSWORD.with(|p| p.borrow_mut().clear());
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_username("".into()); st.set_guard_code("".into());
    password_sync(a);
    a.account.open = false; a.account.generation = a.account.generation.wrapping_add(1);
    a.account.users.clear(); a.account.tiles.clear(); a.account.selected = None; a.account.page = PAGE_CHOOSER;
    sync(a); a.sync_overlays(); a.update_hint();
}
/// B / «Отмена»: Steam Guard → вход → выбор → закрыть (без выбора — сразу закрыть, как сток).
fn back(a: &mut App) {
    match a.account.page {
        PAGE_GUARD => {
            account_auth::pw_stop(a);
            a.account.page = PAGE_LOGIN;
            let ui = a.ui(); let st = ui.global::<AccountState>(); st.set_focus(4); st.set_message("".into());
            sync(a);
        }
        PAGE_LOGIN if !a.account.tiles.is_empty() && !a.account.auth.done => {
            account_auth::qr_stop(a); account_auth::pw_stop(a);
            a.osk_set_open(false);
            a.account.page = PAGE_CHOOSER;
            a.ui().global::<AccountState>().set_message("".into());
            let plus = a.account.tiles.len() as i32;
            a.ui().global::<ChooserState>().set_focus(plus);
            sync(a);
        }
        _ => close(a),
    }
}
fn chooser_activate(a: &mut App, i: i32) {
    if !a.account.open || a.account.page != PAGE_CHOOSER || a.account.busy { return; }
    let n = a.account.tiles.len() as i32;
    if (0..n).contains(&i) {
        let account = a.account.users[a.account.tiles[i as usize]].account_name.clone();
        run_request(a, LoginRequest::Saved { account });
    } else if i == n {
        go_login(a);
    } else if i == n + 1 {
        // Стоковый переключатель: client setting always_show_user_chooser (запись в DRYRUN не уходит).
        a.account.ask_on_start = !a.account.ask_on_start;
        crate::setters::client_setting("always_show_user_chooser", &serde_json::Value::Bool(a.account.ask_on_start));
        sync(a);
    }
}
/// Общая модель → поток смены. Saved: сохранённый аккаунт; Token: вход подтверждён по QR/паролю.
pub fn run_request(a: &mut App, req: LoginRequest) {
    match req {
        LoginRequest::Saved { account } => {
            let Some(index) = a.account.users.iter().position(|u| u.account_name == account) else { return };
            a.account.selected = Some(index);
            start(a);
        }
        LoginRequest::Token(tokens) => token_login(a, tokens),
    }
}
/// Вход по токену. До входа Steam (0/1) — SetLoginToken сразу; в Ready (5) нужен перезапуск Steam в экран входа —
/// это поток смены (задача 430), он получает тот же `LoginRequest::Token`.
fn token_login(a: &mut App, tokens: steam_auth::Tokens) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active() || a.ds.is_mock() {
        st.set_message(i18n::tr_args("account_switch.dryrun_sign_in_confirmed_for_account_change", &[("account_name", &(tokens.account_name).to_string())]).into());
        return;
    }
    if !a.live.running.is_empty() {
        st.set_message(i18n::tr_ref("account_switch.close_the_running_games_before_changing_your").into()); return;
    }
    a.account.busy = true; sync(a);
    let generation = a.account.generation;
    std::thread::spawn(move || {
        let reply = crate::actions::bridge().ok_or(()).and_then(|b| {
            b.login_with_token(&tokens.account_name, tokens.refresh_token.expose(), tokens.guard_data.expose()).map(|r| (b, r)).map_err(|_| ())
        });
        let name = tokens.account_name.clone();
        drop(tokens);
        let completed = matches!(&reply, Ok((b, r)) if r.phase == "accepted" && {
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let ready = b.login_accounts().ok().is_some_and(|s| s.users.iter().any(|u| u.current && u.account_name.eq_ignore_ascii_case(&name)));
                if ready || Instant::now() > deadline { break ready; }
                std::thread::sleep(Duration::from_millis(500));
            }
        } && crate::backend::reload_account(b));
        let phase = reply.as_ref().map(|(_, r)| r.phase.clone()).unwrap_or_default();
        super::defer(move |a| {
            if a.account.generation != generation { return; }
            a.account.busy = false;
            if completed { close(a); a.navigate(crate::router::Route::Home); return; }
            let msg = match phase.as_str() {
                "restart_required" => i18n::tr_ref("account_switch.sign_in_confirmed_the_active_account_is"),
                "rejected" => i18n::tr_ref("account_switch.steam_rejected_the_token_entry_try_again"),
                "busy" => i18n::tr_ref("account_switch.steam_s_already_logging_in_wait_till"),
                _ => i18n::tr_ref("account_switch.steam_hasn_t_confirmed_entry_result_unknown"),
            };
            a.ui().global::<AccountState>().set_message(msg.into()); sync(a);
        });
    });
}
fn login(a: &mut App) {
    if !can_login(a) { return; }
    let remember = a.account.remember;
    let pw = take_password();
    password_sync(a);
    account_auth::password_start(a, pw, remember);
    sync(a);
}
fn pick(a: &mut App, i: i32) {
    if !a.account.open { return; }
    if i == 4 {
        if a.account.busy && a.account.uncertain && a.account.cancel.is_none() { return; }
        if a.account.uncertain && !a.account.busy {
            cancel_uncertain(a); return;
        }
        if let Some(cancel) = &a.account.cancel {
            cancel.store(true, Ordering::SeqCst);
            a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.cancellation_of_entry_request").into());
        } else { back(a); }
        return;
    }
    if a.account.busy { return; }
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if a.account.page == PAGE_GUARD {
        match i { 0 if a.account.auth.guard_kind != 3 => super::osk::attach(a, GUARD_SINK), 3 => account_auth::guard_primary(a), _ => {} }
        return;
    }
    if a.account.page != PAGE_LOGIN { return; }
    match i {
        0 => super::osk::attach(a, USERNAME_SINK),
        1 => super::osk::attach(a, PASSWORD_SINK),
        2 => { a.account.remember = !a.account.remember; sync_login(a); }
        3 => login(a),
        // Сток открывает help.steampowered.com / store…/join / store…/mobile во встроенном браузере.
        5 => st.set_message(i18n::tr_ref("account_switch.help_with_login_help_steampowered_com_open").into()),
        6 => st.set_message(i18n::tr_ref("account_switch.you_can_create_an_account_at_store").into()),
        7 => st.set_message(i18n::tr_ref("account_switch.steam_mobile_app_search_the_app_store").into()),
        8 if st.get_qr_state() == account_auth::QR_FAILED => account_auth::qr_start(a),
        _ => {}
    }
}
fn progress_text(progress: &Progress) -> String {
    let key = match progress.phase.as_str() {
        "complete" => "account_progress.complete",
        "restart_required" => "account_progress.restart_required",
        "unavailable" => "account_progress.unavailable",
        "password" => "account_progress.password",
        "pin" => "account_progress.pin",
        "missing" => "account_progress.missing",
        "cancelled" => "account_progress.cancelled",
        "unknown" => "account_progress.unknown",
        "rejected" => match progress.auth_result {
            Some(63 | 65 | 85 | 88) => "account_progress.guard",
            Some(3 | 16 | 20 | 48) => "account_progress.connection",
            Some(5 | 15 | 18 | 86) => "account_progress.credentials",
            Some(84) => "account_progress.rate_limited",
            code => {
                let code = code.map_or_else(|| i18n::tr("account_progress.code_unknown"), |value| value.to_string());
                return i18n::tr_args("account_progress.rejected", &[("code", &code)]);
            }
        },
        _ => match progress.state {
            Some(2) => "account_progress.waiting_network",
            Some(3) => "account_progress.waiting_steam",
            Some(4) => {
                let percent = (progress.percentage.unwrap_or(0.0).clamp(0.0, 1.0) * 100.0).round();
                return i18n::tr_args("account_progress.loading_library", &[("percent", &percent.to_string())]);
            }
            Some(6) => "account_progress.shutting_down",
            _ => {
                return i18n::tr_args("account_progress.signing_in", &[
                    ("state", &progress.state.unwrap_or(0).to_string()),
                    ("result", &progress.result.unwrap_or(0).to_string()),
                ]);
            }
        },
    };
    i18n::tr(key)
}

fn start(a: &mut App) {
    let Some(user) = a.account.selected.and_then(|i| a.account.users.get(i)).cloned() else { return; };
    if a.account.busy || a.account.uncertain { return; }
    if user.current { close(a); a.navigate(crate::router::Route::Home); return; }
    if !a.live.running.is_empty() {
        a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.close_the_running_games_before_changing_your").into()); return;
    }
    if std::env::var_os("DECKSHELL_DRYRUN").is_some() || deck_system::guard::active() || a.ds.is_mock() {
        a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.dryrun_change_of_account_disabled").into()); return;
    }
    a.account.busy = true;
    a.account.generation = a.account.generation.wrapping_add(1);
    let generation = a.account.generation;
    a.account.uncertain = true;
    let cancel = Arc::new(AtomicBool::new(false)); a.account.cancel = Some(cancel.clone());
    let id = format!("{}-{}-{}", std::process::id(), crate::app::now_secs(), generation);
    a.account.last_id = Some(id.clone());
    a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.logging_in_to_steam").into()); sync(a);
    std::thread::spawn(move || {
        let mut uncertain = true;
        let result = (|| {
            let b = crate::actions::bridge().ok_or_else(|| { uncertain = false; i18n::tr_ref("account_switch.steam_is_unavailable_no_login_request_has").to_string() })?;
            if cancel.load(Ordering::SeqCst) { uncertain = false; return Err(i18n::tr_ref("account_switch.entry_is_cancelled_before_the_request_is").into()); }
            let mut progress = b.login_cached_account(&id, &user.account_name)
                .map_err(|_| i18n::tr_ref("account_switch.steam_has_not_confirmed_the_shipment_the").to_string())?;
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let message = progress_text(&progress);
                super::defer(move |a| { if a.account.generation == generation { a.ui().global::<AccountState>().set_message(message.into()); } });
                if progress.terminal() {
                    uncertain = progress.phase == "unknown";
                    // Steam готов под другим аккаунтом: стоковый путь — перезапуск. Делаем его в фоне, под котиком (задача 430).
                    if progress.phase == "restart_required" {
                        return crate::account_flow::run(&cancel, &id, &user.account_name, progress.account_name.as_deref());
                    }
                    if progress.phase == "complete" {
                        crate::backend::refresh_profile(&b, Some(&user.account_name));
                        if !crate::backend::reload_account(&b) { return Err(i18n::tr_ref("account_switch.entrance_is_complete_but_the_new_library").into()); }
                        return Ok(());
                    }
                    return Err(progress_text(&progress));
                }
                if cancel.load(Ordering::SeqCst) {
                    // Ready can arrive between the poll above and cancellation. Feed
                    // its response through the same completion/library reload boundary.
                    progress = b.cancel_account_login(&id)
                        .map_err(|_| i18n::tr_ref("account_switch.the_cancellation_has_not_been_confirmed_by").to_string())?;
                    continue;
                }
                if Instant::now() >= deadline { return Err(i18n::tr_ref("account_switch.steam_didn_t_complete_the_login_in").into()); }
                std::thread::sleep(Duration::from_millis(300));
                progress = b.login_progress(&id).map_err(|_| i18n::tr_ref("account_switch.the_connection_to_steam_is_lost_the").to_string())?;
            }
        })();
        super::defer(move |a| {
            if a.account.generation != generation { return; }
            a.account.busy = false; a.account.cancel = None; a.account.uncertain = uncertain;
            if !uncertain { a.account.last_id = None; }
            match result {
                Ok(()) => { close(a); a.navigate(crate::router::Route::Home); }
                Err(message) => { a.ui().global::<AccountState>().set_message(message.into()); sync(a); }
            }
        });
    });
}
// Even after CDP observation fails, B attempts native cancellation on the original operation.
// Its outcome can remain unknown; closing the surface never re-enables a possibly live write.
fn cancel_uncertain(a: &mut App) {
    let Some(id) = a.account.last_id.clone() else { close(a); return; };
    a.account.busy = true;
    let generation = a.account.generation;
    a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_switch.cancellation_of_entry_request").into()); sync(a);
    std::thread::spawn(move || {
        let result = crate::actions::bridge().and_then(|b| b.cancel_account_login(&id).ok().map(|p| (b, p)));
        let completed = result.as_ref().is_some_and(|(b,p)| p.phase == "complete" && crate::backend::reload_account(b));
        super::defer(move |a| {
            if a.account.generation != generation { return; }
            a.account.busy = false;
            a.account.uncertain = !completed && !result.as_ref().is_some_and(|(_,p)| p.terminal() && p.phase != "unknown" && p.phase != "complete");
            if !a.account.uncertain { a.account.last_id = None; }
            close(a);
            if completed { a.navigate(crate::router::Route::Home); }
            else if a.account.uncertain { a.toast(i18n::tr_ref("account_switch.steam_has_not_confirmed_the_cancellation_re")); }
        });
    });
}
pub fn handle(a: &mut App, action: Action, repeat: bool) -> bool {
    if !a.account.open { return false; }
    // экранная клавиатура поверх окна (поля имени/пароля/кода): ввод и B принадлежат ей
    if a.osk_is_open() { return false; }
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if action == Action::B { if !repeat { pick(a, 4); } return true; }
    if a.account.busy { return true; }
    match a.account.page {
        PAGE_CHOOSER => {
            let ch = ui.global::<ChooserState>();
            let n = a.account.tiles.len() as i32;
            let f = ch.get_focus();
            match action {
                Action::Left if f <= n => ch.set_focus((f - 1).max(0)),
                Action::Right if f < n => ch.set_focus(f + 1),
                Action::Down if f <= n => ch.set_focus(n + 1),
                Action::Up if f == n + 1 => ch.set_focus(n),
                Action::A if !repeat => chooser_activate(a, f),
                _ => {}
            }
        }
        PAGE_GUARD => {
            let f = st.get_focus();
            let code = a.account.auth.guard_kind != 3;
            match action {
                Action::Up if code => st.set_focus(0),
                Action::Down if f == 0 => st.set_focus(3),
                Action::Left if f == 4 => st.set_focus(3),
                Action::Right if f == 3 => st.set_focus(4),
                Action::A if !repeat => pick(a, f),
                _ => {}
            }
        }
        _ => {
            let focus = st.get_focus();
            let qr_refresh = st.get_qr_state() == account_auth::QR_FAILED;
            // Левая колонка: (8 «обновить» QR) → 7 ссылка → 6 «Создать аккаунт»; правая: 0..5.
            match action {
                Action::Up => st.set_focus(match focus { 1 => 0, 2 => 1, 3 | 4 => 2, 5 => 4, 6 => 7, 7 if qr_refresh => 8, _ => focus }),
                Action::Down => st.set_focus(match focus { 0 => 1, 1 => 2, 2 => 3, 3 | 4 => 5, 8 => 7, 7 => 6, _ => focus }),
                Action::Left => st.set_focus(match focus { 4 => 3, 0 | 1 | 2 if qr_refresh => 8, 0 | 1 | 2 => 7, 3 | 5 => 6, _ => focus }),
                Action::Right => st.set_focus(match focus { 6 => 3, 7 => 1, 8 => 0, 3 => 4, _ => focus }),
                Action::A if !repeat => pick(a, focus), _ => {},
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "account_switch_tests.rs"]
mod tests;
