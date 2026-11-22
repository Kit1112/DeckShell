//! Вход по имени и паролю (sp.js 247030–247120): GetPasswordRSAPublicKey → пароль RSA → BeginAuthSessionViaCredentials
//! → подтверждение (код из письма / мобильный код / подтверждение в приложении) → PollAuthSessionStatus.

use steam_assets::i18n::tr_ref;
use crate::proto::Msg;
use crate::{eresult, poll, Device, Error, Guard, Result, Secret, Tokens, Transport, Writer};
use std::time::Duration;

/// ESessionPersistence: «Запомнить меня» → Persistent (1), иначе Ephemeral (0) — sp.js 247063.
fn persistence(remember: bool) -> u64 { if remember { 1 } else { 0 } }

/// Сессия после принятия пароля: какие подтверждения допускает Steam.
#[derive(Debug)]
pub struct Session {
    pub client_id: u64,
    request_id: Vec<u8>,
    pub steamid: u64,
    pub interval: Duration,
    /// Порядок как у сервера (первый — предпочтительный).
    pub confirmations: Vec<(Guard, String)>,
}

impl Session {
    /// Нужно ли вводить код (письмо/приложение), и какой.
    pub fn code_kind(&self) -> Option<Guard> {
        self.confirmations.iter().map(|(g, _)| *g).find(|g| matches!(g, Guard::DeviceCode | Guard::EmailCode))
    }
    pub fn needs_nothing(&self) -> bool { self.confirmations.iter().any(|(g, _)| matches!(g, Guard::None | Guard::MachineToken)) }
    pub fn device_confirmation(&self) -> bool { self.confirmations.iter().any(|(g, _)| *g == Guard::DeviceConfirmation) }
    /// Подсказка сервера для письма (домен почты) — `associated_message`.
    pub fn email_hint(&self) -> Option<&str> {
        self.confirmations.iter().find(|(g, _)| *g == Guard::EmailCode).map(|(_, m)| m.as_str()).filter(|m| !m.is_empty())
    }

    /// UpdateAuthSessionWithSteamGuardCode. Ошибки: 65 (неверный код письма), 88 (неверный код приложения), 27/9 (истёк).
    pub fn submit_code(&self, t: &dyn Transport, code: &str, kind: Guard) -> Result<()> {
        let body = Writer::default().uint(1, self.client_id).fixed64(2, self.steamid).string(3, code.trim()).uint(4, kind.raw());
        let (er, _) = t.call("Authentication", "UpdateAuthSessionWithSteamGuardCode", &body.0, true)?;
        // Сток (sp.js): k_EResultDuplicateRequest (29) после уже принятого кода — тоже успех.
        if er == eresult::OK || er == 29 { Ok(()) } else { Err(Error::Steam(er)) }
    }

    /// Опрос: `Ok(None)` — ещё ждём (например подтверждения в приложении).
    pub fn poll(&self, t: &dyn Transport) -> Result<Option<Tokens>> {
        let r = poll(t, self.client_id, &self.request_id)?;
        if r.agreement_url.is_some() { return Err(Error::Steam(eresult::AGREEMENT_REQUIRED)); }
        Ok(r.tokens)
    }
}

/// Начать вход. `guard_data` — сохранённые данные Steam Guard этого устройства (SteamClient.Auth.GetSteamGuardData):
/// с ними сервер может не спрашивать код из письма (как в стоке, `m_onGetMachineAuth`).
pub fn begin(t: &dyn Transport, device: &Device, account: &str, password: &Secret, remember: bool, guard_data: Option<&Secret>, language: u32) -> Result<Session> {
    // Сток: не-ASCII убирается, пароль обрезается до 64 символов (sp.js 247044).
    let account: String = account.chars().filter(char::is_ascii).collect();
    let pw = Secret::new(password.expose().chars().filter(char::is_ascii).take(64).collect());
    if account.is_empty() || pw.is_empty() { return Err(Error::Steam(eresult::INVALID_PASSWORD)); }

    let (er, data) = t.call("Authentication", "GetPasswordRSAPublicKey", &Writer::default().string(1, &account).0, false)?;
    if er != eresult::OK { return Err(Error::Steam(er)); }
    let key = Msg::new(&data).ok_or(Error::Protocol("GetPasswordRSAPublicKey"))?;
    let (Some(m), Some(e), Some(ts)) = (key.string(1), key.string(2), key.uint(3)) else { return Err(Error::Protocol(tr_ref("steam_auth.rsa_fields_missing"))) };
    let enc = crate::rsa::encrypt_pkcs1(&m, &e, pw.expose().as_bytes()).ok_or(Error::Protocol(tr_ref("steam_auth.invalid_rsa")))?;
    let enc = Secret::new(crate::b64(&enc));

    let mut body = Writer::default()
        .string(1, &device.friendly_name)
        .string(2, &account)
        .string(3, enc.expose())
        .uint(4, ts)
        .uint(5, remember as u64)
        .uint(6, crate::PLATFORM_STEAM_CLIENT)
        .uint(7, persistence(remember))
        .string(8, crate::WEBSITE_ID)
        .message(9, device.encode())
        .uint(11, language as u64);
    if let Some(g) = guard_data.filter(|g| !g.is_empty()) { body = body.string(10, g.expose()); }
    let (er, data) = t.call("Authentication", "BeginAuthSessionViaCredentials", &body.0, true)?;
    // Тело запроса содержит зашифрованный пароль: затираем.
    let mut raw = body.0; for b in raw.iter_mut() { unsafe { std::ptr::write_volatile(b, 0) } }
    if er != eresult::OK { return Err(Error::Steam(er)); }
    let r = Msg::new(&data).ok_or(Error::Protocol("BeginAuthSessionViaCredentials"))?;
    let (Some(client_id), Some(req), Some(steamid)) = (r.uint(1), r.bytes(2), r.uint(5)) else {
        return Err(Error::Protocol(tr_ref("steam_auth.credentials_fields_missing")));
    };
    let confirmations = r.all(4).iter().filter_map(|c| Some((Guard::from_raw(c.uint(1)?), c.string(2).unwrap_or_default()))).collect();
    let secs = r.float(3).filter(|v| v.is_finite()).unwrap_or(5.0).clamp(1.0, 30.0);
    Ok(Session { client_id, request_id: req.to_vec(), steamid, interval: Duration::from_secs_f32(secs), confirmations })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::Mock;
    use num_bigint::BigUint;

    fn key_reply() -> (i32, Vec<u8>) {
        let one = BigUint::from(1u32);
        let n = ((&one << 127usize) - &one) * ((&one << 521usize) - &one);
        (1, Writer::default().string(1, &n.to_str_radix(16)).string(2, "010001").uint(3, 123456).0)
    }
    fn begin_reply(conf: &[(u64, &str)]) -> (i32, Vec<u8>) {
        let mut w = Writer::default().uint(1, 99).bytes(2, b"rq").uint(5, 76561198000000001);
        for (t, m) in conf { w = w.message(4, Writer::default().uint(1, *t).string(2, m)); }
        (1, w.0)
    }

    #[test]
    fn password_is_encrypted_and_request_has_stock_fields() {
        let m = Mock::default();
        m.push("GetPasswordRSAPublicKey", Ok(key_reply()));
        m.push("BeginAuthSessionViaCredentials", Ok(begin_reply(&[(2, "gmail.com")])));
        let s = begin(&m, &Device::default(), "fixture", &Secret::new("пароль-s3cret".into()), true, Some(&Secret::new("GD".into())), 8).unwrap();
        assert_eq!(s.code_kind(), Some(Guard::EmailCode));
        assert_eq!(s.email_hint(), Some("gmail.com"));
        assert!(!s.needs_nothing());
        let calls = m.calls.lock().unwrap();
        let req = Msg::new(&calls[1].1).unwrap();
        assert_eq!(req.string(2).as_deref(), Some("fixture"));
        let enc = req.string(3).unwrap();
        assert!(!enc.contains("s3cret") && enc.len() > 100, "пароль только в зашифрованном виде");
        assert!(!calls[1].1.windows(6).any(|w| w == b"s3cret"));
        assert_eq!(req.uint(4), Some(123456), "encryption_timestamp из ключа");
        assert_eq!((req.uint(5), req.uint(6), req.uint(7)), (Some(1), Some(1), Some(1)));
        assert_eq!(req.string(10).as_deref(), Some("GD"));
        assert_eq!(req.uint(11), Some(8));
    }

    #[test]
    fn not_remembered_is_ephemeral_and_wrong_password_reports_eresult() {
        let m = Mock::default();
        m.push("GetPasswordRSAPublicKey", Ok(key_reply()));
        m.push("BeginAuthSessionViaCredentials", Ok((5, Vec::new())));
        assert_eq!(begin(&m, &Device::default(), "fixture", &Secret::new("x".into()), false, None, 0).unwrap_err(), Error::Steam(5));
        let calls = m.calls.lock().unwrap();
        let req = Msg::new(&calls[1].1).unwrap();
        assert_eq!((req.uint(5), req.uint(7)), (Some(0), Some(0)));
        assert!(req.string(10).is_none());
    }

    #[test]
    fn device_code_then_poll_gives_tokens() {
        let m = Mock::default();
        m.push("GetPasswordRSAPublicKey", Ok(key_reply()));
        m.push("BeginAuthSessionViaCredentials", Ok(begin_reply(&[(4, ""), (3, "")])));
        let s = begin(&m, &Device::default(), "fixture", &Secret::new("pw".into()), true, None, 0).unwrap();
        assert!(s.device_confirmation());
        assert_eq!(s.code_kind(), Some(Guard::DeviceCode));
        m.push("UpdateAuthSessionWithSteamGuardCode", Ok((88, Vec::new())));
        assert_eq!(s.submit_code(&m, "AAAAA", Guard::DeviceCode).unwrap_err(), Error::Steam(88));
        m.push("UpdateAuthSessionWithSteamGuardCode", Ok((1, Vec::new())));
        s.submit_code(&m, " BCD12 ", Guard::DeviceCode).unwrap();
        {
            let calls = m.calls.lock().unwrap();
            let req = Msg::new(&calls.last().unwrap().1).unwrap();
            assert_eq!((req.uint(1), req.uint(2), req.string(3).as_deref(), req.uint(4)), (Some(99), Some(76561198000000001), Some("BCD12"), Some(3)));
        }
        m.push("PollAuthSessionStatus", Ok((1, Vec::new())));
        assert!(s.poll(&m).unwrap().is_none());
        m.push("PollAuthSessionStatus", Ok((1, Writer::default().string(3, "R").string(6, "fixture").0)));
        assert_eq!(s.poll(&m).unwrap().unwrap().account_name, "fixture");
    }

    #[test]
    fn empty_input_is_rejected_without_network() {
        let m = Mock::default();
        assert_eq!(begin(&m, &Device::default(), "", &Secret::new("x".into()), true, None, 0).unwrap_err(), Error::Steam(5));
        assert_eq!(begin(&m, &Device::default(), "имя", &Secret::new("x".into()), true, None, 0).unwrap_err(), Error::Steam(5));
        assert!(m.calls.lock().unwrap().is_empty());
    }
}
