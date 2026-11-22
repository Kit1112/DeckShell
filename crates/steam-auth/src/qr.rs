//! QR-вход: BeginAuthSessionViaQR → challenge_url (рисуется QR) → PollAuthSessionStatus с интервалом сервера.
//! Повтор логики стока (sp.js 247516–247590, базовый класс 245740–245850):
//! * ответ без refresh_token: ждать дальше; `new_challenge_url` / `new_client_id` заменяют QR (сервер сам его обновляет);
//! * `had_remote_interaction`: телефон отсканировал код — QR размывается, крутится загрузка;
//! * EResult 9/27 → «истёк» (Expired), 84 → лимит запросов, прочее → общая ошибка; кнопка «обновить» начинает новую сессию.

use crate::{eresult, poll, Device, Error, Result, Tokens, Transport, Writer};
use crate::proto::Msg;
use std::time::Duration;

/// Причина неудачи QR-сессии (sp.js 245725: None/Generic/Expired/Network/MoveAuthenticator/RateLimitExceeded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure { Generic, Expired, Network, RateLimit }

pub fn classify(e: &Error) -> Failure {
    match e {
        Error::Steam(eresult::FILE_NOT_FOUND | eresult::EXPIRED) => Failure::Expired,
        Error::Steam(eresult::RATE_LIMIT) => Failure::RateLimit,
        Error::Network(_) | Error::Steam(eresult::NO_CONNECTION | eresult::TIMEOUT | eresult::SERVICE_UNAVAILABLE) => Failure::Network,
        _ => Failure::Generic,
    }
}

#[derive(Debug)]
pub struct Session {
    pub client_id: u64,
    request_id: Vec<u8>,
    pub challenge_url: String,
    pub interval: Duration,
    pub remote_interaction: bool,
    network_errors: u32,
}

#[derive(Debug)]
pub enum Event {
    /// Ждём телефона. `url_changed`: QR нужно перерисовать.
    Waiting { url_changed: bool },
    Done(Tokens),
    /// Steam требует принять соглашение (EResult 118) — в стоке открывается страница соглашения; здесь — сообщение.
    Agreement,
}

pub fn begin(t: &dyn Transport, device: &Device) -> Result<Session> {
    let body = Writer::default()
        .string(1, &device.friendly_name)
        .uint(2, crate::PLATFORM_STEAM_CLIENT)
        .message(3, device.encode())
        .string(4, crate::WEBSITE_ID);
    let (er, data) = t.call("Authentication", "BeginAuthSessionViaQR", &body.0, true)?;
    if er != eresult::OK { return Err(Error::Steam(er)); }
    let m = Msg::new(&data).ok_or(Error::Protocol("BeginAuthSessionViaQR"))?;
    let (Some(client_id), Some(url), Some(req)) = (m.uint(1), m.string(2), m.bytes(3)) else {
        return Err(Error::Protocol("BeginAuthSessionViaQR: нет client_id/challenge_url/request_id"));
    };
    if url.is_empty() || req.is_empty() { return Err(Error::Protocol("BeginAuthSessionViaQR: пустой ответ")); }
    // interval в секундах (float); сток: m_msPollInterval = interval*1000. Ограничим разумными рамками.
    let secs = m.float(4).filter(|v| v.is_finite()).unwrap_or(5.0).clamp(1.0, 30.0);
    Ok(Session { client_id, request_id: req.to_vec(), challenge_url: url, interval: Duration::from_secs_f32(secs), remote_interaction: false, network_errors: 0 })
}

impl Session {
    /// Один опрос. Сетевой сбой повторяется молча до трёх раз подряд (сток в этом случае делает MakeReady и повтор).
    pub fn poll(&mut self, t: &dyn Transport) -> std::result::Result<Event, Failure> {
        match poll(t, self.client_id, &self.request_id) {
            Ok(r) => {
                self.network_errors = 0;
                if r.agreement_url.is_some() { return Ok(Event::Agreement); }
                self.remote_interaction |= r.had_remote_interaction;
                if let Some(tokens) = r.tokens {
                    if tokens.account_name.is_empty() { return Err(Failure::Generic); }
                    return Ok(Event::Done(tokens));
                }
                if let Some(id) = r.new_client_id { self.client_id = id; }
                let url_changed = match r.new_challenge_url { Some(u) if u != self.challenge_url => { self.challenge_url = u; true } _ => false };
                Ok(Event::Waiting { url_changed })
            }
            Err(e) => {
                let f = classify(&e);
                if f == Failure::Network && self.network_errors < 3 {
                    self.network_errors += 1;
                    return Ok(Event::Waiting { url_changed: false });
                }
                Err(f)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::Mock;
    use crate::proto::Writer;

    fn begin_reply(url: &str) -> (i32, Vec<u8>) {
        (1, Writer::default().uint(1, 777).string(2, url).bytes(3, b"\x01\x02\x03").fixed32_float(4, 5.0).0)
    }
    impl Writer {
        fn fixed32_float(mut self, field: u32, v: f32) -> Self {
            self.0.push(((field << 3) | 5) as u8);
            self.0.extend_from_slice(&v.to_bits().to_le_bytes());
            self
        }
    }

    #[test]
    fn begin_sends_client_platform_and_device_and_reads_reply() {
        let m = Mock::default();
        m.push("BeginAuthSessionViaQR", Ok(begin_reply("https://s.team/q/1/1")));
        let s = begin(&m, &Device { friendly_name: "steamdeck".into(), os_type: Some(-203), gaming_device_type: Some(544), machine_id: None }).unwrap();
        assert_eq!(s.client_id, 777);
        assert_eq!(s.challenge_url, "https://s.team/q/1/1");
        assert_eq!(s.interval, Duration::from_secs(5));
        let calls = m.calls.lock().unwrap();
        let req = Msg::new(&calls[0].1).unwrap();
        assert_eq!(req.uint(2), Some(1), "platform_type SteamClient");
        assert_eq!(req.string(4).as_deref(), Some("Client"));
        let dd = &req.all(3)[0];
        assert_eq!(dd.string(1).as_deref(), Some("steamdeck"));
        assert_eq!(dd.uint(3).map(|v| v as i64 as i32), Some(-203));
        assert_eq!(dd.uint(4), Some(544));
    }

    #[test]
    fn poll_waits_rotates_url_marks_remote_and_finishes_with_tokens() {
        let m = Mock::default();
        m.push("BeginAuthSessionViaQR", Ok(begin_reply("https://s.team/q/1/1")));
        let mut s = begin(&m, &Device::default()).unwrap();
        m.push("PollAuthSessionStatus", Ok((1, Vec::new())));
        assert!(matches!(s.poll(&m), Ok(Event::Waiting { url_changed: false })));
        m.push("PollAuthSessionStatus", Ok((1, Writer::default().uint(1, 778).string(2, "https://s.team/q/1/2").0)));
        assert!(matches!(s.poll(&m), Ok(Event::Waiting { url_changed: true })));
        assert_eq!((s.client_id, s.challenge_url.as_str()), (778, "https://s.team/q/1/2"));
        m.push("PollAuthSessionStatus", Ok((1, Writer::default().uint(5, 1).0)));
        assert!(matches!(s.poll(&m), Ok(Event::Waiting { .. })));
        assert!(s.remote_interaction);
        m.push("PollAuthSessionStatus", Ok((1, Writer::default().string(3, "REFRESH").string(4, "ACCESS").string(6, "fixture").string(7, "GUARD").0)));
        match s.poll(&m) {
            Ok(Event::Done(t)) => { assert_eq!(t.account_name, "fixture"); assert_eq!(t.refresh_token.expose(), "REFRESH"); assert_eq!(t.guard_data.expose(), "GUARD"); }
            other => panic!("{other:?}"),
        }
        // Повторный опрос шёл уже с новым client_id.
        let calls = m.calls.lock().unwrap();
        assert_eq!(Msg::new(&calls.last().unwrap().1).unwrap().uint(1), Some(778));
    }

    #[test]
    fn failures_are_classified_like_stock() {
        let m = Mock::default();
        m.push("BeginAuthSessionViaQR", Ok(begin_reply("u")));
        let mut s = begin(&m, &Device::default()).unwrap();
        m.push("PollAuthSessionStatus", Ok((27, Vec::new())));
        assert_eq!(s.poll(&m).unwrap_err(), Failure::Expired);
        m.push("PollAuthSessionStatus", Ok((9, Vec::new())));
        assert_eq!(s.poll(&m).unwrap_err(), Failure::Expired);
        m.push("PollAuthSessionStatus", Ok((84, Vec::new())));
        assert_eq!(s.poll(&m).unwrap_err(), Failure::RateLimit);
        m.push("PollAuthSessionStatus", Ok((2, Vec::new())));
        assert_eq!(s.poll(&m).unwrap_err(), Failure::Generic);
        m.push("PollAuthSessionStatus", Ok((118, Writer::default().string(8, "https://agreement").0)));
        assert!(matches!(s.poll(&m), Ok(Event::Agreement)));
    }

    #[test]
    fn transient_network_errors_retry_three_times_then_fail() {
        let m = Mock::default();
        m.push("BeginAuthSessionViaQR", Ok(begin_reply("u")));
        let mut s = begin(&m, &Device::default()).unwrap();
        for _ in 0..3 {
            m.push("PollAuthSessionStatus", Err(Error::Network("reset".into())));
            assert!(matches!(s.poll(&m), Ok(Event::Waiting { url_changed: false })));
        }
        m.push("PollAuthSessionStatus", Err(Error::Network("reset".into())));
        assert_eq!(s.poll(&m).unwrap_err(), Failure::Network);
    }

    #[test]
    fn begin_errors_and_empty_replies() {
        let m = Mock::default();
        m.push("BeginAuthSessionViaQR", Ok((84, Vec::new())));
        assert_eq!(begin(&m, &Device::default()).unwrap_err(), Error::Steam(84));
        m.push("BeginAuthSessionViaQR", Ok((1, Vec::new())));
        assert!(matches!(begin(&m, &Device::default()), Err(Error::Protocol(_))));
    }
}
