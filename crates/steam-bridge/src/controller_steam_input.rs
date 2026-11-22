//! The per-game Steam Input switch: the stock «Конфигурация включена» / «Отключить Steam Input»
//! row in the controller configurator (task 209).
//!
//! Stock calls, from the UI bundle (`sp.js`, the component that shows the row):
//! - `SteamClient.Apps.SetThirdPartyControllerConfiguration(appid, value)` — the write.
//!   Its values come from the stock enum `EThirdPartyControllerConfiguration`; the numbers were
//!   read on the Deck through the stock module that exports them (2026-11-13, see below).
//! - the store's `ClearSelectedConfigCache(appid)` — the stock drops its cached selection after
//!   the write, so the next read is fresh.
//! - `SteamClient.Apps.RegisterForAppDetails(appid, cb)` — the stock reads the stored value from
//!   the app details (`eEnableThirdPartyControllerConfiguration`). Read here with the same call.
//! - `SteamClient.Input.GetConfigForAppAndController(appid, idx)` — `bConfigurationEnabled`,
//!   the state the row shows as enabled/disabled.
//!
//! Enum on the Deck (read from the stock module exports, not guessed): `pg` (Off) = 0,
//! `gG` (Default) = 1, `Dt` (On) = 2. Balatro (2379780) stored value on 2026-11-13: Default (1).
//!
//! The write is one call with no retry; read the result back with
//! [`Bridge::steam_input_state`]. Nothing here changes the controller layout.
use serde_json::Value;

use crate::{Bridge, Error, Result};

/// Stock `EThirdPartyControllerConfiguration` values that the Steam Input row can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamInputSetting {
    /// Steam Input off for this game («Отключить Steam Input»). Stock `pg`.
    Off = 0,
    /// Steam's default for this game. Stock `gG`.
    Default = 1,
    /// Steam Input on for this game. Stock `Dt`.
    On = 2,
}

impl SteamInputSetting {
    pub fn from_value(v: u64) -> Result<Self> {
        match v {
            0 => Ok(Self::Off),
            1 => Ok(Self::Default),
            2 => Ok(Self::On),
            other => Err(Error::Protocol(format!("unknown steam input setting {other}"))),
        }
    }

    pub fn value(self) -> u32 {
        self as u32
    }
}

/// The stored setting and the per-controller switch as Steam reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamInputState {
    pub setting: SteamInputSetting,
    /// `bConfigurationEnabled` for the controller slot asked for. Stock shows the switch as
    /// «Отключить» when this is true.
    pub configuration_enabled: bool,
}

/// Write expression. The ids and the value are integers, so no outside text reaches the script.
/// Same order as the stock `la(appid, controllerIndex, enabled)`: the native write, the store's
/// save of the editing state, the cache clear, then the store's ensure for the controller slot.
/// Without the save and the ensure the switch read back unchanged (checked on the Deck).
pub fn set_setting_expr(appid: u32, controller_index: u32, setting: SteamInputSetting) -> String {
    format!(
        "(async()=>{{await SteamClient.Apps.SetThirdPartyControllerConfiguration({appid},{value});\
const s=window.controllerConfiguratorStore;s.SaveEditingConfiguration({appid});\
s.ClearSelectedConfigCache({appid});s.EnsureEditingConfiguration({appid},{controller_index});return true}})()",
        value = setting.value()
    )
}

/// Read expression for the stored setting: subscribe to the app details, take the first reply,
/// drop the subscription. Times out after 8 s instead of hanging.
pub fn read_setting_expr(appid: u32) -> String {
    format!(
        "(async()=>{{let got=null;const h=SteamClient.Apps.RegisterForAppDetails({appid},(d)=>{{if(!got)got=d;}});\
const t0=Date.now();while(!got&&Date.now()-t0<8000)await new Promise(r=>setTimeout(r,100));\
try{{h&&h.unregister&&h.unregister();}}catch(e){{}}\
if(!got)throw new Error('app details timeout');\
return {{eEnableThirdPartyControllerConfiguration:got.eEnableThirdPartyControllerConfiguration}};}})()"
    )
}

/// Parse the reply of [`read_setting_expr`]. A missing or non-integer field is an error.
pub fn parse_setting(v: &Value) -> Result<SteamInputSetting> {
    let n = v
        .get("eEnableThirdPartyControllerConfiguration")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Protocol(format!("app details without a steam input setting: {v}")))?;
    SteamInputSetting::from_value(n)
}

/// Whether the controller-level switch agrees with the stored setting: Off must mean disabled,
/// Default and On must mean enabled. On the Deck (2026-11-13) a write of Off changed the stored
/// setting but not the switch, so a `false` here is a real finding, not a bug to hide.
pub fn settings_agree(state: &SteamInputState) -> bool {
    (state.setting != SteamInputSetting::Off) == state.configuration_enabled
}

impl Bridge {
    /// The stored Steam Input setting for an app, and the switch for a controller slot (read-only).
    /// Both values are returned as Steam reports them; use [`settings_agree`] to compare them.
    pub fn steam_input_state(&self, appid: u32, controller_index: u32) -> Result<SteamInputState> {
        let setting = parse_setting(&self.eval_json(&read_setting_expr(appid))?)?;
        let info = self.controller_config_info(appid, controller_index)?;
        Ok(SteamInputState { setting, configuration_enabled: info.configuration_enabled })
    }

    /// Write the Steam Input setting for an app. One call, no retry; read it back with
    /// [`Bridge::steam_input_state`].
    pub fn steam_input_set(&self, appid: u32, controller_index: u32, setting: SteamInputSetting) -> Result<()> {
        if appid == 0 {
            return Err(Error::Protocol("steam input: app id 0 refused".into()));
        }
        self.eval_json_once(&set_setting_expr(appid, controller_index, setting))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enum_values_match_the_deck_probe() {
        assert_eq!(SteamInputSetting::Off.value(), 0);
        assert_eq!(SteamInputSetting::Default.value(), 1);
        assert_eq!(SteamInputSetting::On.value(), 2);
        assert_eq!(SteamInputSetting::from_value(1).unwrap(), SteamInputSetting::Default);
        assert!(SteamInputSetting::from_value(3).is_err());
    }

    #[test]
    fn parses_the_stored_value_from_deck_shape() {
        // Balatro on 2026-11-13: eEnableThirdPartyControllerConfiguration = 1 (Default).
        let v = json!({"eEnableThirdPartyControllerConfiguration": 1});
        assert_eq!(parse_setting(&v).unwrap(), SteamInputSetting::Default);
    }

    #[test]
    fn missing_or_mistyped_value_is_an_error_not_a_default() {
        assert!(parse_setting(&json!({})).is_err());
        assert!(parse_setting(&json!({"eEnableThirdPartyControllerConfiguration": "1"})).is_err());
        assert!(parse_setting(&json!({"eEnableThirdPartyControllerConfiguration": null})).is_err());
        assert!(parse_setting(&json!({"eEnableThirdPartyControllerConfiguration": 7})).is_err());
    }

    #[test]
    fn agreement_compares_setting_with_switch() {
        let st = |setting, configuration_enabled| SteamInputState { setting, configuration_enabled };
        assert!(settings_agree(&st(SteamInputSetting::Off, false)));
        assert!(settings_agree(&st(SteamInputSetting::Default, true)));
        assert!(settings_agree(&st(SteamInputSetting::On, true)));
        assert!(!settings_agree(&st(SteamInputSetting::Off, true)));
        assert!(!settings_agree(&st(SteamInputSetting::Default, false)));
    }

    #[test]
    fn write_expression_has_only_integers_and_the_stock_order() {
        let e = set_setting_expr(2379780, 15, SteamInputSetting::Off);
        assert!(e.contains("SetThirdPartyControllerConfiguration(2379780,0)"));
        let order = ["SetThirdPartyControllerConfiguration", "SaveEditingConfiguration", "ClearSelectedConfigCache", "EnsureEditingConfiguration(2379780,15)"];
        let pos: Vec<usize> = order.iter().map(|k| e.find(k).unwrap()).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]));
        let e = set_setting_expr(2379780, 15, SteamInputSetting::Default);
        assert!(e.contains("SetThirdPartyControllerConfiguration(2379780,1)"));
    }

    #[test]
    fn read_expression_names_the_app_and_drops_the_subscription() {
        let e = read_setting_expr(2379780);
        assert!(e.contains("RegisterForAppDetails(2379780"));
        assert!(e.contains("h.unregister"));
        assert!(e.contains("eEnableThirdPartyControllerConfiguration"));
    }
}
