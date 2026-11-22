//! Вход нового аккаунта на странице «Войти»: QR (основной путь) и имя/пароль + Steam Guard.
//! Сетевой протокол — `steam_auth` (как стоковый экран, sp.js 247516 / 245740 / 247030); всё сетевое — в фоновом
//! потоке, UI обновляется через `defer` с проверкой поколения (уход со страницы останавливает поток).
//! Итог — `LoginRequest::Token` в общую модель (`account_request::submit`), дальше — поток смены аккаунта.
use steam_assets::i18n;
use super::account_request::{self, LoginRequest};
use crate::{app::App, AccountState};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc};
use std::time::{Duration, Instant};
use steam_auth::{credentials, qr, Device, Guard, Secret, Transport};

/// Состояния QR (как `AccountState.qr-state`).
pub const QR_LOADING: i32 = 0;
pub const QR_READY: i32 = 1;
pub const QR_SCANNED: i32 = 2;
pub const QR_FAILED: i32 = 3;
pub const QR_DONE: i32 = 4;

#[derive(Default)]
pub struct AuthState {
    generation: u64,
    stop: Option<Arc<AtomicBool>>,
    pw_stop: Option<Arc<AtomicBool>>,
    code_tx: Option<mpsc::Sender<(String, Guard)>>,
    pub username: String,
    pub guard_code: String,
    /// 1 — письмо, 2 — код приложения, 3 — подтверждение в приложении (`AccountState.guard-kind`).
    pub guard_kind: i32,
    /// Код из приложения доступен как запасной путь при подтверждении в приложении.
    guard_alt_code: bool,
    pub password_busy: bool,
    /// Вход подтверждён и передан потоку смены: повторный запуск QR/пароля не нужен.
    pub done: bool,
}

/// Без сети: мок-данные (`DECKSHELL_MOCK`), тесты, явное `DECKSHELL_AUTH_OFFLINE`.
fn offline(a: &App) -> bool {
    cfg!(test) || a.ds.is_mock() || std::env::var_os("DECKSHELL_AUTH_OFFLINE").is_some()
}

// ---------- QR-картинка ----------

/// Белая карточка 248×248 (снимок стока: 196..444), код ~200px по центру; модули rgb(21,23,28) (sp.js 249284).
pub const QR_CARD: u32 = 248;
pub fn render_qr(url: &str) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    use qrcodegen::{QrCode, QrCodeEcc};
    // Сток: quality Q при длине ≤ 90, иначе по умолчанию библиотеки (M).
    let ecc = if url.len() <= 90 { QrCodeEcc::Quartile } else { QrCodeEcc::Medium };
    let code = QrCode::encode_text(url, ecc).ok()?;
    let n = code.size() as u32;
    let scale = (200 / n).max(1);
    let off = (QR_CARD - n * scale) / 2;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(QR_CARD, QR_CARD);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() { *p = Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 }; }
    for y in 0..n {
        for x in 0..n {
            if code.get_module(x as i32, y as i32) {
                for dy in 0..scale { for dx in 0..scale {
                    let (px_x, px_y) = (off + x * scale + dx, off + y * scale + dy);
                    px[(px_y * QR_CARD + px_x) as usize] = Rgba8Pixel { r: 21, g: 23, b: 28, a: 255 };
                }}
            }
        }
    }
    Some(buf)
}
/// Размытая копия (CSS Blur над QR, пока идёт загрузка/подтверждение/ошибка): три прохода box-blur r=5.
pub fn blur(src: &SharedPixelBuffer<Rgba8Pixel>) -> SharedPixelBuffer<Rgba8Pixel> {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let mut a: Vec<[f32; 3]> = src.as_slice().iter().map(|p| [p.r as f32, p.g as f32, p.b as f32]).collect();
    let r = 5isize;
    for _ in 0..3 {
        for horizontal in [true, false] {
            let mut out = a.clone();
            for y in 0..h as isize { for x in 0..w as isize {
                let mut s = [0f32; 3]; let mut c = 0f32;
                for k in -r..=r {
                    let (xx, yy) = if horizontal { (x + k, y) } else { (x, y + k) };
                    if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize { continue; }
                    let p = a[yy as usize * w + xx as usize];
                    s[0] += p[0]; s[1] += p[1]; s[2] += p[2]; c += 1.0;
                }
                out[y as usize * w + x as usize] = [s[0] / c, s[1] / c, s[2] / c];
            }}
            a = out;
        }
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
    for (d, s) in buf.make_mut_slice().iter_mut().zip(a) {
        *d = Rgba8Pixel { r: s[0] as u8, g: s[1] as u8, b: s[2] as u8, a: 255 };
    }
    buf
}

fn set_qr(a: &App, sharp: Option<SharedPixelBuffer<Rgba8Pixel>>, blurred: Option<SharedPixelBuffer<Rgba8Pixel>>) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if let Some(s) = sharp { st.set_qr(slint::Image::from_rgba8(s)); }
    if let Some(b) = blurred { st.set_qr_blur(slint::Image::from_rgba8(b)); }
}
fn set_qr_state(a: &App, state: i32, error: &str) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_qr_state(state);
    st.set_qr_error(error.into());
}

fn failure_text(f: qr::Failure) -> &'static str {
    // Стоковые строки shared_russian: Login_Error_*_Title.
    match f {
        qr::Failure::Expired => i18n::tr_ref("account_auth.qr_code_expires_click_update"),
        qr::Failure::Network => i18n::tr_ref("account_auth.connection_problem"),
        qr::Failure::RateLimit => i18n::tr_ref("account_auth.too_many_retries"),
        qr::Failure::Generic => i18n::tr_ref("account_auth.error_of_qr_code_entry"),
    }
}

/// Сведения об устройстве, как у стока (имя хоста, ОС, тип устройства) — read-only через мост; без моста — «Steam Deck».
fn device() -> Device {
    if cfg!(test) { return Device::default(); }
    let info = crate::actions::bridge().and_then(|b| b.login_device_info().ok()).unwrap_or_default();
    Device {
        friendly_name: if info.hostname.is_empty() { "Steam Deck".into() } else { info.hostname },
        os_type: info.os_type,
        gaming_device_type: info.gaming_device_type,
        machine_id: None,
    }
}

fn sleep_or_stop(stop: &AtomicBool, d: Duration) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if stop.load(Ordering::SeqCst) { return true; }
        std::thread::sleep(Duration::from_millis(100));
    }
    stop.load(Ordering::SeqCst)
}

/// Запустить (или перезапустить по «обновить») QR-сессию. Старая останавливается.
pub fn qr_start(a: &mut App) {
    qr_stop(a);
    if a.account.auth.done { return; }
    a.account.auth.generation = a.account.auth.generation.wrapping_add(1);
    let generation = a.account.auth.generation;
    set_qr_state(a, QR_LOADING, "");
    if offline(a) {
        // Офлайн-образец (не сканируется в рабочий вход): адрес вида s.team/q/1/<id> как у сервера.
        let sharp = render_qr("https://s.team/q/1/0000000000000000000");
        let blurred = sharp.as_ref().map(blur);
        set_qr(a, sharp, blurred);
        set_qr_state(a, QR_READY, "");
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    a.account.auth.stop = Some(stop.clone());
    std::thread::spawn(move || qr_worker(generation, stop, &steam_auth::WebApi::default()));
}

fn post(generation: u64, f: impl FnOnce(&mut App) + Send + 'static) {
    super::defer(move |a| { if a.account.auth.generation == generation && a.account.open { f(a) } });
}

fn qr_worker(generation: u64, stop: Arc<AtomicBool>, t: &dyn Transport) {
    let dev = device();
    let mut s = match qr::begin(t, &dev) {
        Ok(s) => s,
        Err(e) => { let f = qr::classify(&e); post(generation, move |a| set_qr_state(a, QR_FAILED, failure_text(f))); return; }
    };
    let show = |url: &str| { let sharp = render_qr(url); let blurred = sharp.as_ref().map(blur); (sharp, blurred) };
    let (sharp, blurred) = show(&s.challenge_url);
    post(generation, move |a| { set_qr(a, sharp, blurred); set_qr_state(a, QR_READY, ""); });
    let mut scanned = false;
    loop {
        if sleep_or_stop(&stop, s.interval) { return; }
        match s.poll(t) {
            Ok(qr::Event::Waiting { url_changed }) => {
                if url_changed {
                    let (sharp, blurred) = show(&s.challenge_url);
                    post(generation, move |a| set_qr(a, sharp, blurred));
                }
                if s.remote_interaction && !scanned {
                    scanned = true;
                    post(generation, |a| set_qr_state(a, QR_SCANNED, ""));
                }
            }
            Ok(qr::Event::Done(tokens)) => {
                post(generation, move |a| { set_qr_state(a, QR_DONE, ""); finish(a, tokens); });
                return;
            }
            Ok(qr::Event::Agreement) => {
                post(generation, |a| set_qr_state(a, QR_FAILED, i18n::tr_ref("account_auth.steam_asks_to_accept_agreement_log_in")));
                return;
            }
            Err(f) => { post(generation, move |a| set_qr_state(a, QR_FAILED, failure_text(f))); return; }
        }
        if stop.load(Ordering::SeqCst) { return; }
    }
}

pub fn qr_stop(a: &mut App) {
    if let Some(s) = a.account.auth.stop.take() { s.store(true, Ordering::SeqCst); }
}

/// Вход подтверждён: дальше — поток смены аккаунта.
fn finish(a: &mut App, tokens: steam_auth::Tokens) {
    a.account.auth.done = true;
    qr_stop(a); pw_stop(a);
    let name = tokens.account_name.clone();
    a.ui().global::<AccountState>().set_message(i18n::tr_args("account_auth.signed_in_as_switching_accounts", &[("name", &(name).to_string())]).into());
    account_request::submit(a, LoginRequest::Token(tokens));
}

// ---------- имя + пароль + Steam Guard ----------

pub fn pw_stop(a: &mut App) {
    if let Some(s) = a.account.auth.pw_stop.take() { s.store(true, Ordering::SeqCst); }
    a.account.auth.code_tx = None;
    a.account.auth.password_busy = false;
}

/// Тексты ошибок — стоковые строки (shared/steamui_russian: Login_PasswordMismatch, Login_RateLimitExceeded…).
pub fn eresult_text(e: &steam_auth::Error) -> String {
    use steam_auth::{eresult as r, Error};
    match e {
        Error::Steam(r::INVALID_PASSWORD) => i18n::tr_ref("account_auth.incorrect_password").into(),
        Error::Steam(r::RATE_LIMIT | r::ACCOUNT_LOGIN_DENIED_THROTTLE) => i18n::tr_ref("account_auth.too_many_recent_sign_in_attempts_try").into(),
        Error::Steam(r::INVALID_LOGIN_AUTH_CODE | r::TWO_FACTOR_MISMATCH) => i18n::tr_ref("account_auth.incorrect_code_please_try_again").into(),
        Error::Steam(r::EXPIRED | r::FILE_NOT_FOUND) => i18n::tr_ref("account_auth.that_code_is_too_old_and_has").into(),
        Error::Steam(r::AGREEMENT_REQUIRED) => i18n::tr_ref("account_auth.steam_asks_you_to_accept_the_agreement").into(),
        Error::Network(_) | Error::Steam(r::NO_CONNECTION | r::TIMEOUT | r::SERVICE_UNAVAILABLE) => i18n::tr_ref("account_auth.there_was_a_problem_communicating_with_steam").into(),
        Error::Steam(c) => i18n::tr_args("account_auth.sign_in_failed_code_try_again_later", &[("code", &(c).to_string())]),
        Error::Protocol(_) => i18n::tr_ref("account_auth.something_went_wrong_while_attempting_to_sign").into(),
    }
}

/// «Войти» на странице входа: имя + пароль.
pub fn password_start(a: &mut App, password: Secret, remember: bool) {
    if a.account.auth.password_busy || a.account.auth.done { return; }
    let account = a.account.auth.username.trim().to_string();
    if account.is_empty() || password.is_empty() { return; }
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if offline(a) {
        st.set_message(i18n::tr_ref("account_auth.without_a_network_wet_test_no_password").into());
        return;
    }
    a.account.auth.password_busy = true;
    st.set_busy(true);
    st.set_message(i18n::tr_ref("account_auth.logging_in_to_steam").into());
    let generation = a.account.auth.generation;
    let stop = Arc::new(AtomicBool::new(false));
    a.account.auth.pw_stop = Some(stop.clone());
    let (tx, rx) = mpsc::channel();
    a.account.auth.code_tx = Some(tx);
    std::thread::spawn(move || password_worker(generation, stop, rx, account, password, remember, &steam_auth::WebApi::default()));
}

fn password_worker(generation: u64, stop: Arc<AtomicBool>, codes: mpsc::Receiver<(String, Guard)>, account: String, password: Secret,
    remember: bool, t: &dyn Transport) {
    let fail = move |msg: String| post(generation, move |a| {
        a.account.auth.password_busy = false; a.account.auth.code_tx = None;
        let ui = a.ui(); let st = ui.global::<AccountState>();
        st.set_busy(false); st.set_message(msg.into());
    });
    let dev = device();
    let guard = if cfg!(test) { None } else { crate::actions::bridge().and_then(|b| b.steam_guard_data(&account).ok()).map(Secret::new) };
    // Язык: k_Lang_Russian = 8 (как TS.LANGUAGE «russian» стока).
    let session = match credentials::begin(t, &dev, &account, &password, remember, guard.as_ref(), 8) {
        Ok(s) => s,
        Err(e) => { fail(eresult_text(&e)); return; }
    };
    drop(password);
    // Нужно ли подтверждение и какое: сток показывает подтверждение в приложении, если оно разрешено, иначе ввод кода.
    if !session.needs_nothing() {
        let kind = if session.device_confirmation() { 3 } else if session.code_kind() == Some(Guard::EmailCode) { 1 } else { 2 };
        let alt = session.code_kind().is_some();
        let hint = session.email_hint().unwrap_or("").to_string();
        post(generation, move |a| {
            let ui = a.ui(); let st = ui.global::<AccountState>();
            a.account.auth.guard_kind = kind; a.account.auth.guard_alt_code = alt; a.account.auth.guard_code.clear();
            st.set_busy(false); st.set_message("".into());
            st.set_guard_kind(kind); st.set_guard_hint(hint.into()); st.set_guard_code("".into());
            st.set_page(2); st.set_focus(if kind == 3 { 3 } else { 0 });
        });
    }
    let started = Instant::now();
    loop {
        if sleep_or_stop(&stop, session.interval.min(Duration::from_secs(3))) { return; }
        while let Ok((code, kind)) = codes.try_recv() {
            match session.submit_code(t, &code, kind) {
                Ok(()) => post(generation, |a| a.ui().global::<AccountState>().set_message(i18n::tr_ref("account_auth.code_accepted").into())),
                Err(e) => {
                    let msg = eresult_text(&e);
                    post(generation, move |a| { let ui = a.ui(); let st = ui.global::<AccountState>(); st.set_busy(false); st.set_message(msg.into()); });
                }
            }
        }
        match session.poll(t) {
            Ok(Some(tokens)) => { post(generation, move |a| { a.account.auth.password_busy = false; finish(a, tokens) }); return; }
            Ok(None) => {}
            Err(e) => { fail(eresult_text(&e)); post(generation, |a| a.ui().global::<AccountState>().set_page(1)); return; }
        }
        // Сервер сам завершает сессию по таймауту (Expired); подстраховка — 10 минут.
        if started.elapsed() > Duration::from_secs(600) { fail(i18n::tr_ref("account_auth.the_sign_in_request_has_expired").into()); return; }
    }
}

/// Страница Steam Guard: «Подтвердить и войти» (или «Ввести код» при подтверждении в приложении).
pub fn guard_primary(a: &mut App) {
    let ui = a.ui(); let st = ui.global::<AccountState>();
    if a.account.auth.guard_kind == 3 {
        // Переход к вводу кода из приложения; опрос подтверждения продолжается.
        if a.account.auth.guard_alt_code { a.account.auth.guard_kind = 2; st.set_guard_kind(2); st.set_focus(0); }
        return;
    }
    let code = a.account.auth.guard_code.trim().to_uppercase();
    if code.len() < 5 { st.set_message(i18n::tr_ref("account_auth.enter_the_entire_code_5_characters").into()); return; }
    let kind = if a.account.auth.guard_kind == 1 { Guard::EmailCode } else { Guard::DeviceCode };
    if let Some(tx) = &a.account.auth.code_tx {
        let _ = tx.send((code, kind));
        st.set_busy(true);
        st.set_message(i18n::tr_ref("account_auth.code_check").into());
    }
}

pub fn guard_typed(a: &mut App, c: Option<char>) {
    let code = &mut a.account.auth.guard_code;
    match c { Some(c) if c.is_ascii_alphanumeric() && code.len() < 5 => code.push(c.to_ascii_uppercase()), None => { code.pop(); } _ => {} }
    let ui = a.ui(); let st = ui.global::<AccountState>();
    st.set_guard_code(a.account.auth.guard_code.clone().into());
    st.set_busy(false);
}

/// Сбросить всё при закрытии экрана.
pub fn reset(a: &mut App) {
    qr_stop(a); pw_stop(a);
    a.account.auth.generation = a.account.auth.generation.wrapping_add(1);
    let st = &mut a.account.auth;
    st.username.clear(); st.guard_code.clear(); st.guard_kind = 0; st.done = false;
}

#[cfg(test)]
#[path = "account_auth_tests.rs"]
mod tests;
