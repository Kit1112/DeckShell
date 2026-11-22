//! Вход по refresh-токену (после QR / пароля) и read-only сведения об устройстве для IAuthenticationService.
//! Токен передаётся в Steam только этим вызовом и только в состоянии до входа (0/1); не логируется.
use crate::{Bridge, Error, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const SCRIPT: &str = include_str!("../js/account_token.js");
pub fn expression(command: Value) -> String { SCRIPT.replace("__COMMAND__", &command.to_string()) }

#[derive(Clone, Debug, Default, Deserialize)]
pub struct DeviceInfo {
    pub os_type: Option<i32>,
    pub gaming_device_type: Option<u32>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub always_show_user_chooser: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct TokenReply { pub phase: String, pub result: Option<i32>, pub state: Option<u32> }

impl Bridge {
    pub fn login_device_info(&self) -> Result<DeviceInfo> {
        serde_json::from_value(self.eval_json(&expression(json!({"kind":"device"})))?).map_err(|_| Error::Protocol("Invalid device reply".into()))
    }
    /// Сохранённые данные Steam Guard устройства для аккаунта (пусто, если нет). Read-only.
    pub fn steam_guard_data(&self, account: &str) -> Result<String> {
        let v = self.eval_json(&expression(json!({"kind":"guard","account":account})))?;
        Ok(v.get("data").and_then(Value::as_str).unwrap_or("").to_string())
    }
    /// SetLoginToken (+ SetSteamGuardData). Запись: запрещена в DRYRUN, без повтора при обрыве.
    pub fn login_with_token(&self, account: &str, refresh: &str, guard: &str) -> Result<TokenReply> {
        if std::env::var_os("DECKSHELL_DRYRUN").is_some() {
            return Err(Error::Protocol("DRYRUN: account changes disabled".into()));
        }
        let v = self.eval_json_once(&expression(json!({"kind":"token","account":account,"refresh":refresh,"guard":guard})))?;
        serde_json::from_value(v).map_err(|_| Error::Protocol("Invalid token reply".into()))
    }
}
