//! Read side of Steam's per-game controller configuration ("Настройки управления" of a game).
//!
//! Shapes come from two sources: the stock UI bundle (call sites in the configurator modules)
//! and one read-only probe of the Deck's `SharedJSContext` (2026-11-13, Balatro 2379780). The
//! full layout (action sets, bindings, source modes) arrives from Steam only as protobuf bytes
//! and is not decoded yet; see docs/controller-config.md, section «API и данные».
//!
//! Nothing here writes to Steam. Writes are a later step, gated on a readback of the same call.
use serde_json::{json, Value};

use crate::{Bridge, Error, Result};

/// Result of `SteamClient.Input.GetConfigForAppAndController(appid, controllerIndex)`.
/// The stock client returns only these three fields, not the layout itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerConfigInfo {
    /// Controller slot the lookup was made for.
    pub controller_index: u32,
    /// Steam Input is enabled for this game on this controller (the stock toggle).
    pub configuration_enabled: bool,
    /// The layout needs a per-client install step (stock uses it to show a notice).
    pub needs_game_install: bool,
}

/// JS expression for `GetConfigForAppAndController`. Both arguments are integers, so nothing
/// from outside can change the script text. Booleans are coerced so the reply is always typed.
pub fn config_info_expr(appid: u32, controller_index: u32) -> String {
    format!(
        "SteamClient.Input.GetConfigForAppAndController({appid},{controller_index}).then(r=>({{\
unControllerIndex:r.unControllerIndex,\
bConfigurationEnabled:!!r.bConfigurationEnabled,\
bNeedsGameInstall:!!r.bNeedsGameInstall}}))"
    )
}

/// JS expression for `GetControllerPreviouslySeen()`: the controller type ids Steam has seen.
pub const PREVIOUSLY_SEEN_EXPR: &str = "SteamClient.Input.GetControllerPreviouslySeen().then(a=>Array.from(a||[]))";

/// Parse the reply of [`config_info_expr`]. Missing or mistyped fields are an error, not a
/// default: a silent `false` would show the toggle in the wrong state.
pub fn parse_config_info(v: &Value) -> Result<ControllerConfigInfo> {
    let obj = v
        .as_object()
        .ok_or_else(|| Error::Protocol(format!("config info is not an object: {v}")))?;
    let index = obj
        .get("unControllerIndex")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Protocol("config info: unControllerIndex missing".into()))?;
    let enabled = obj
        .get("bConfigurationEnabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::Protocol("config info: bConfigurationEnabled missing".into()))?;
    let needs_install = obj
        .get("bNeedsGameInstall")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::Protocol("config info: bNeedsGameInstall missing".into()))?;
    Ok(ControllerConfigInfo {
        controller_index: u32::try_from(index).map_err(|_| Error::Protocol("unControllerIndex out of range".into()))?,
        configuration_enabled: enabled,
        needs_game_install: needs_install,
    })
}

/// Parse the reply of [`PREVIOUSLY_SEEN_EXPR`] into controller type ids.
pub fn parse_previously_seen(v: &Value) -> Result<Vec<u32>> {
    let arr = v
        .as_array()
        .ok_or_else(|| Error::Protocol(format!("previously-seen is not an array: {v}")))?;
    arr.iter()
        .map(|x| {
            x.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Error::Protocol(format!("controller type not a u32: {x}")))
        })
        .collect()
}

impl Bridge {
    /// Read the per-game controller configuration state (read-only).
    pub fn controller_config_info(&self, appid: u32, controller_index: u32) -> Result<ControllerConfigInfo> {
        let v = self.eval_json(&config_info_expr(appid, controller_index))?;
        parse_config_info(&v)
    }

    /// Controller type ids Steam has seen on this device (read-only).
    pub fn controller_types_previously_seen(&self) -> Result<Vec<u32>> {
        let v = self.eval_json(PREVIOUSLY_SEEN_EXPR)?;
        parse_previously_seen(&v)
    }
}

/// Reply shape used by fixtures: the object exactly as the Deck returned it for Balatro,
/// with values chosen by the test, so the parser is checked against the real field names.
pub fn fixture_config_info(enabled: bool, needs_install: bool) -> Value {
    json!({
        "unControllerIndex": 0,
        "bConfigurationEnabled": enabled,
        "bNeedsGameInstall": needs_install,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_field_names() {
        let info = parse_config_info(&fixture_config_info(true, false)).unwrap();
        assert_eq!(
            info,
            ControllerConfigInfo { controller_index: 0, configuration_enabled: true, needs_game_install: false }
        );
    }

    #[test]
    fn missing_field_is_an_error_not_a_default() {
        let v = json!({"unControllerIndex": 0, "bConfigurationEnabled": true});
        assert!(matches!(parse_config_info(&v), Err(Error::Protocol(_))));
        let v = json!({"unControllerIndex": 0, "bConfigurationEnabled": "yes", "bNeedsGameInstall": false});
        assert!(matches!(parse_config_info(&v), Err(Error::Protocol(_))));
    }

    #[test]
    fn rejects_non_object_and_out_of_range_index() {
        assert!(matches!(parse_config_info(&json!(null)), Err(Error::Protocol(_))));
        let v = json!({"unControllerIndex": u64::MAX, "bConfigurationEnabled": true, "bNeedsGameInstall": false});
        assert!(matches!(parse_config_info(&v), Err(Error::Protocol(_))));
    }

    #[test]
    fn expression_uses_only_integer_arguments() {
        let e = config_info_expr(2379780, 3);
        assert!(e.starts_with("SteamClient.Input.GetConfigForAppAndController(2379780,3)"));
        assert!(e.contains("bConfigurationEnabled:!!r.bConfigurationEnabled"));
    }

    #[test]
    fn previously_seen_parses_ids_and_rejects_strings() {
        assert_eq!(parse_previously_seen(&json!([4, 34, 33, 31, 32])).unwrap(), vec![4, 34, 33, 31, 32]);
        assert!(matches!(parse_previously_seen(&json!([4, "x"])), Err(Error::Protocol(_))));
        assert!(matches!(parse_previously_seen(&json!({})), Err(Error::Protocol(_))));
    }
}
