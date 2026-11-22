//! Saved-account login, without starting or navigating Steam UI. No credential material crosses CDP.
use crate::{Bridge, Error, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const SCRIPT: &str = include_str!("../js/account_switch.js");
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub account_name: String, pub persona_name: String, pub avatar_url: String,
    pub steamid: String, pub remembered: bool, pub has_pin: bool, pub current: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Snapshot {
    pub users: Vec<Account>,
    pub account_name: Option<String>,
    pub state: Option<u32>,
    pub steamid: Option<String>,
}

impl Snapshot {
    /// A profile is usable once Steam confirms the login and both stores agree on its identity.
    pub fn ready_profile(&self, target: Option<&str>) -> Option<(u64, &str)> {
        let account = self.account_name.as_deref().filter(|name| !name.is_empty())?;
        if self.state != Some(5) || target.is_some_and(|name| !account.eq_ignore_ascii_case(name.trim())) {
            return None;
        }
        let user = self.users.iter().find(|u| u.current && u.account_name.eq_ignore_ascii_case(account));
        let cached_id = user.and_then(|u| u.steamid.parse::<u64>().ok()).filter(|&id| id != 0);
        let live_id = self.steamid.as_deref().and_then(|id| id.parse::<u64>().ok()).filter(|&id| id != 0);
        if cached_id.zip(live_id).is_some_and(|(cached, live)| cached != live) {
            return None;
        }
        let name = user.map(|u| u.persona_name.as_str()).filter(|name| !name.is_empty()).unwrap_or(account);
        Some((cached_id.or(live_id)?, name))
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct Progress {
    pub phase: String, pub state: Option<u32>, pub result: Option<i32>,
    pub percentage: Option<f64>, pub auth_result: Option<i32>,
    /// Аккаунт, под которым Steam сейчас (из `current()` скрипта).
    #[serde(default)] pub account_name: Option<String>,
}
pub fn expression(command: Value) -> String { SCRIPT.replace("__COMMAND__", &command.to_string()) }
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| Error::Protocol("Invalid account API reply".into()))
}
impl Bridge {
    pub fn login_accounts(&self) -> Result<Snapshot> { decode(self.eval_json(&expression(json!({"kind":"read"})))?) }
    pub fn login_cached_account(&self, id: &str, account: &str) -> Result<Progress> {
        deny_dryrun()?;
        decode(self.eval_json_once(&expression(json!({"kind":"start","id":id,"account":account})))?)
    }
    pub fn login_progress(&self, id: &str) -> Result<Progress> {
        decode(self.eval_json(&expression(json!({"kind":"poll","id":id})))?)
    }
    pub fn cancel_account_login(&self, id: &str) -> Result<Progress> {
        deny_dryrun()?;
        decode(self.eval_json_once(&expression(json!({"kind":"cancel","id":id})))?)
    }
}
fn deny_dryrun() -> Result<()> {
    if std::env::var_os("DECKSHELL_DRYRUN").is_some() {
        Err(Error::Protocol("DRYRUN: account changes disabled".into()))
    } else { Ok(()) }
}
impl Progress {
    pub fn message(&self) -> String {
        match self.phase.as_str() {
            "complete" => "Вход выполнен. Загрузка главного экрана…".into(),
            "restart_required" => "Смена активного аккаунта без перезапуска Steam пока не поддерживается.".into(),
            "unavailable" => "Steam уже выполняет вход или завершает работу. Дождитесь окончания операции.".into(),
            "password" => "Нужен пароль. Вход с паролем пока недоступен.".into(),
            "pin" => "Нужен PIN сохранённого аккаунта. Ввод PIN пока недоступен.".into(),
            "missing" => "Аккаунта больше нет в сохранённых данных Steam.".into(),
            "cancelled" => "Запрос входа отменён. Предыдущий вход не восстанавливается автоматически.".into(),
            "unknown" => "Steam не подтвердил результат. Повторный вход не отправлен.".into(),
            "rejected" => match self.auth_result {
                Some(63 | 65 | 85 | 88) => "Нужен Steam Guard. Подтверждение пока недоступно.".into(),
                Some(3 | 16 | 20 | 48) => "Нет соединения со Steam. Проверьте сеть.".into(),
                Some(5 | 15 | 18 | 86) => "Сохранённые данные не подходят: нужен пароль.".into(),
                Some(84) => "Слишком много попыток входа. Попробуйте позже.".into(),
                code => format!("Steam отклонил вход (код {}).", code.map(|v| v.to_string()).unwrap_or_else(|| "неизвестен".into())),
            },
            _ => match self.state {
                Some(2) => "Ожидание сети…".into(), Some(3) => "Ожидание сервера Steam…".into(),
                Some(4) => format!("Загрузка библиотеки… {}%", (self.percentage.unwrap_or(0.0).clamp(0.0, 1.0) * 100.0).round()),
                Some(6) => "Steam завершает работу…".into(),
                _ => format!("Вход в Steam… (состояние {}, код {})", self.state.unwrap_or(0), self.result.unwrap_or(0)),
            }
        }
    }
    pub fn terminal(&self) -> bool { !matches!(self.phase.as_str(), "starting" | "waiting" | "cancelling") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_profile_rejects_the_previous_steamid_until_stores_agree() {
        let mut snapshot: Snapshot = decode(json!({
            "state": 4, "account_name": "second", "steamid": "76561197960265729",
            "users": [
                {"account_name":"first", "persona_name":"First",
                 "avatar_url":"/avatarcache/76561197960265729.png", "steamid":"76561197960265729",
                 "remembered":true, "has_pin":false, "current":false},
                {"account_name":"second", "persona_name":"Second",
                 "avatar_url":"/avatarcache/76561197960265730.png", "steamid":"76561197960265730",
                 "remembered":true, "has_pin":false, "current":true}
            ]
        })).unwrap();
        assert_eq!(snapshot.ready_profile(Some("second")), None);
        snapshot.state = Some(5);
        assert_eq!(snapshot.ready_profile(Some("second")), None);
        snapshot.steamid = Some("76561197960265730".into());
        assert_eq!(snapshot.ready_profile(Some("first")), None);
        assert_eq!(snapshot.ready_profile(Some("Second")), Some((76561197960265730, "Second")));
    }

    #[test]
    fn identity_change_reloads_even_when_both_accounts_have_equal_libraries() {
        let before = json!({"account":"first","lib":"same","dl":"same","n":"same","run":""});
        let after = json!({"account":"second","lib":"same","dl":"same","n":"same","run":""});
        assert_eq!(crate::diff_fingerprint(&before, &after), vec![crate::BridgeEvent::Connected]);
    }
    #[test]
    fn guard_network_password_and_unknown_are_distinct() {
        let progress = |code| Progress { phase:"rejected".into(), auth_result:code, state:Some(1), result:Some(1), percentage:None, account_name:None };
        assert!(progress(Some(63)).message().contains("Steam Guard"));
        assert!(progress(Some(16)).message().contains("соединения"));
        assert!(progress(Some(5)).message().contains("пароль"));
        assert!(progress(None).message().contains("неизвестен"));
    }
}
