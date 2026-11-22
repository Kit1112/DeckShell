//! Вход в Steam через сервис IAuthenticationService — тот же протокол, что у стокового экрана входа
//! (sp.js: BeginAuthSessionViaQR 247544, PollAuthSessionStatus 245778, BeginAuthSessionViaCredentials 247063).
//! Сток ходит через CM-транспорт клиента; здесь тот же protobuf идёт через WebAPI (`input_protobuf_encoded`),
//! ответ — protobuf, EResult — заголовок `x-eresult`.
//!
//! Результат входа — [`Tokens`] (refresh-токен + имя аккаунта + новые данные Steam Guard). Стоковый клиент
//! передаёт их в `SteamClient.Auth.SetLoginToken(refresh, account)` и `SetSteamGuardData` (sp.js 414798, 535708);
//! это делает поток смены аккаунта. Токены не пишутся на диск и не печатаются (`Debug` скрывает их).

pub mod proto;
pub mod qr;
pub mod credentials;
mod rsa;

use proto::{Msg, Writer};

/// EResult (library.js модуль 1345).
pub mod eresult {
    pub const OK: i32 = 1;
    pub const FAIL: i32 = 2;
    pub const NO_CONNECTION: i32 = 3;
    pub const INVALID_PASSWORD: i32 = 5;
    pub const FILE_NOT_FOUND: i32 = 9;
    pub const TIMEOUT: i32 = 16;
    pub const SERVICE_UNAVAILABLE: i32 = 20;
    pub const EXPIRED: i32 = 27;
    pub const INVALID_LOGIN_AUTH_CODE: i32 = 65;
    pub const RATE_LIMIT: i32 = 84;
    pub const ACCOUNT_LOGIN_DENIED_THROTTLE: i32 = 87;
    pub const TWO_FACTOR_MISMATCH: i32 = 88;
    pub const AGREEMENT_REQUIRED: i32 = 118;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Нет связи / транспорт (повторяемо).
    Network(String),
    /// Ответ Steam с EResult != OK.
    Steam(i32),
    /// Неразборчивый ответ.
    Protocol(&'static str),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(e) => write!(f, "сеть: {e}"),
            Error::Steam(r) => write!(f, "Steam EResult {r}"),
            Error::Protocol(e) => write!(f, "протокол: {e}"),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Вызов метода сервиса: `("Authentication", "BeginAuthSessionViaQR")` + тело запроса → (EResult, тело ответа).
pub trait Transport: Send + Sync {
    fn call(&self, service: &str, method: &str, body: &[u8], post: bool) -> Result<(i32, Vec<u8>)>;
}

/// api.steampowered.com, как WebApiTransport стока; таймаут короткий — опрос всё равно повторяется.
pub struct WebApi { agent: ureq::Agent, base: String }
impl Default for WebApi {
    fn default() -> Self {
        WebApi {
            agent: ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(15)).build(),
            base: "https://api.steampowered.com".into(),
        }
    }
}
pub(crate) fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}
fn url_escape(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
impl Transport for WebApi {
    fn call(&self, service: &str, method: &str, body: &[u8], post: bool) -> Result<(i32, Vec<u8>)> {
        let url = format!("{}/I{service}Service/{method}/v1/", self.base);
        let enc = b64(body);
        let resp = if post {
            self.agent.post(&url).set("Content-Type", "application/x-www-form-urlencoded")
                .send_string(&format!("input_protobuf_encoded={}", url_escape(&enc)))
        } else {
            self.agent.get(&format!("{url}?input_protobuf_encoded={}", url_escape(&enc))).call()
        };
        let resp = match resp {
            Ok(r) => r,
            // 4xx/5xx тоже несут x-eresult (например 429 → 84).
            Err(ureq::Error::Status(code, r)) => {
                let er = r.header("x-eresult").and_then(|v| v.parse().ok());
                return match er { Some(e) => Ok((e, Vec::new())), None => Err(Error::Network(format!("HTTP {code}"))) };
            }
            Err(e) => return Err(Error::Network(e.kind().to_string())),
        };
        let er = resp.header("x-eresult").and_then(|v| v.parse().ok()).unwrap_or(eresult::OK);
        let mut out = Vec::new();
        use std::io::Read;
        resp.into_reader().take(1 << 20).read_to_end(&mut out).map_err(|e| Error::Network(e.to_string()))?;
        Ok((er, out))
    }
}

/// Строка-секрет: затирается при Drop, в `Debug` не выводится.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);
impl Secret {
    pub fn new(s: String) -> Self { Secret(s) }
    pub fn expose(&self) -> &str { &self.0 }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}
impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: обнуляем байты строки (нули — валидный UTF-8) перед освобождением.
        unsafe { for b in self.0.as_bytes_mut() { std::ptr::write_volatile(b, 0); } }
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Secret(<{} байт>)", self.0.len()) }
}

/// Итог успешного входа. Передаётся потоку смены аккаунта: `SetLoginToken(refresh_token, account_name)`,
/// при непустом `guard_data` — `SetSteamGuardData(account_name, guard_data)`.
#[derive(Clone, Debug)]
pub struct Tokens {
    pub account_name: String,
    pub refresh_token: Secret,
    pub guard_data: Secret,
}

/// CAuthentication_DeviceDetails (sp.js 99640). Сток берёт имя хоста, ОС, тип устройства у SteamClient (247869).
#[derive(Clone, Debug)]
pub struct Device {
    pub friendly_name: String,
    pub os_type: Option<i32>,
    pub gaming_device_type: Option<u32>,
    pub machine_id: Option<Vec<u8>>,
}
impl Default for Device {
    fn default() -> Self { Device { friendly_name: "Steam Deck".into(), os_type: None, gaming_device_type: None, machine_id: None } }
}
/// k_EAuthTokenPlatformType_SteamClient — токен, который принимает клиент (SetLoginToken).
pub const PLATFORM_STEAM_CLIENT: u64 = 1;
/// website_id стокового клиента (library.js TS.WEBSITE_ID по умолчанию «Unknown»; клиент подставляет «Client»).
pub const WEBSITE_ID: &str = "Client";
impl Device {
    fn encode(&self) -> Writer {
        let mut w = Writer::default().string(1, &self.friendly_name).uint(2, PLATFORM_STEAM_CLIENT);
        if let Some(v) = self.os_type { w = w.int32(3, v); }
        if let Some(v) = self.gaming_device_type { w = w.uint(4, v as u64); }
        if let Some(v) = &self.machine_id { w = w.bytes(6, v); }
        w
    }
}

/// Способ подтверждения (EAuthSessionGuardType).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard { None, EmailCode, DeviceCode, DeviceConfirmation, EmailConfirmation, MachineToken, Unknown(u64) }
impl Guard {
    pub fn from_raw(v: u64) -> Self {
        match v { 1 => Guard::None, 2 => Guard::EmailCode, 3 => Guard::DeviceCode, 4 => Guard::DeviceConfirmation,
            5 => Guard::EmailConfirmation, 6 => Guard::MachineToken, o => Guard::Unknown(o) }
    }
    pub fn raw(self) -> u64 {
        match self { Guard::None => 1, Guard::EmailCode => 2, Guard::DeviceCode => 3, Guard::DeviceConfirmation => 4,
            Guard::EmailConfirmation => 5, Guard::MachineToken => 6, Guard::Unknown(v) => v }
    }
}

/// Ответ PollAuthSessionStatus (sp.js 100110).
#[derive(Debug, Default)]
pub struct PollReply {
    pub new_client_id: Option<u64>,
    pub new_challenge_url: Option<String>,
    pub had_remote_interaction: bool,
    pub tokens: Option<Tokens>,
    pub agreement_url: Option<String>,
}

pub(crate) fn poll(t: &dyn Transport, client_id: u64, request_id: &[u8]) -> Result<PollReply> {
    let body = Writer::default().uint(1, client_id).bytes(2, request_id);
    let (er, data) = t.call("Authentication", "PollAuthSessionStatus", &body.0, true)?;
    let m = Msg::new(&data).ok_or(Error::Protocol("PollAuthSessionStatus"))?;
    if er == eresult::AGREEMENT_REQUIRED {
        return Ok(PollReply { agreement_url: m.string(8), ..Default::default() });
    }
    if er != eresult::OK { return Err(Error::Steam(er)); }
    let refresh = m.string(3).filter(|s| !s.is_empty());
    Ok(PollReply {
        new_client_id: m.uint(1).filter(|v| *v != 0),
        new_challenge_url: m.string(2).filter(|s| !s.is_empty()),
        had_remote_interaction: m.bool(5),
        tokens: refresh.map(|r| Tokens {
            account_name: m.string(6).unwrap_or_default(),
            refresh_token: Secret(r),
            guard_data: Secret(m.string(7).unwrap_or_default()),
        }),
        agreement_url: None,
    })
}

#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use std::sync::Mutex;
    /// Сценарий ответов по методам; записывает вызовы.
    #[derive(Default)]
    pub struct Mock { pub replies: Mutex<Vec<(&'static str, Result<(i32, Vec<u8>)>)>>, pub calls: Mutex<Vec<(String, Vec<u8>)>> }
    impl Mock {
        pub fn push(&self, method: &'static str, r: Result<(i32, Vec<u8>)>) { self.replies.lock().unwrap().push((method, r)); }
    }
    impl Transport for Mock {
        fn call(&self, _service: &str, method: &str, body: &[u8], _post: bool) -> Result<(i32, Vec<u8>)> {
            self.calls.lock().unwrap().push((method.into(), body.to_vec()));
            let mut r = self.replies.lock().unwrap();
            let i = r.iter().position(|(m, _)| *m == method).unwrap_or_else(|| panic!("нет ответа для {method}"));
            r.remove(i).1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_is_hidden_in_debug_and_tokens_debug_has_no_token() {
        let t = Tokens { account_name: "fixture".into(), refresh_token: Secret::new("eyJhbGciOi.secret".into()), guard_data: Secret::default() };
        let s = format!("{t:?}");
        assert!(!s.contains("secret"), "{s}");
        assert!(s.contains("fixture"));
    }
    #[test]
    fn base64_and_escape_match_reference() {
        assert_eq!(b64(b"\x0a\x0aSteam"), "CgpTdGVhbQ==");
        assert_eq!(url_escape("a+b/c="), "a%2Bb%2Fc%3D");
    }
}
