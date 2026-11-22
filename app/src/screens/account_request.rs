//! Общая модель «что выбрал пользователь» между экраном выбора/входа (задача 431) и потоком смены аккаунта
//! (перезапуск Steam в фоне + экран загрузки, задача 430).
//!
//! Интерфейс: экран вызывает [`submit`] ровно один раз на выбор. Поток смены получает [`LoginRequest`]:
//! * `Saved` — плитка сохранённого аккаунта (сток: `SteamClient.User.RestartAsUser(account)`, sp.js 35200);
//! * `Token` — вход подтверждён по QR или паролю/Steam Guard: после перезапуска Steam в экран входа
//!   (сток: `RestartToLoginScreen`) нужно отдать `SteamClient.Auth.SetLoginToken(refresh, account)` и
//!   `SetSteamGuardData(account, guard)` — `steam_bridge::Bridge::login_with_token` (только состояния 0/1).
//!
//! Токены — `steam_auth::Secret`: только в памяти, затираются при Drop, не попадают в логи/Debug.

#[derive(Debug)]
pub enum LoginRequest {
    Saved { account: String },
    Token(steam_auth::Tokens),
}

/// Передать выбор потоку смены аккаунта.
pub fn submit(a: &mut crate::app::App, req: LoginRequest) {
    super::account_switch::run_request(a, req);
}
