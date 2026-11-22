//! The Rust port of `battery.mjs` (stock Steam UI battery decisions, sp.js).
//!
//! Arguments stay `serde_json::Value` because the recorded behaviour includes JavaScript's loose
//! coercions (`null` → 0, `[]` → 0, objects → NaN, truthiness); the helpers below reproduce them.

#![forbid(unsafe_code)]

use serde_json::{Value, json};

type R = Result<Value, String>;

// ---------------------------------------------------------------- JS semantics

/// JS truthiness.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// JS `ToNumber` for the JSON value space (an array goes through its string form: `[]` → 0, `[x]` → x).
fn to_num(v: &Value) -> f64 {
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match s.trim() {
            "" => 0.0,
            "Infinity" | "+Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            t if t.bytes().all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b)) => t.parse().unwrap_or(f64::NAN),
            _ => f64::NAN,
        },
        Value::Array(a) if a.is_empty() => 0.0,
        Value::Array(a) if a.len() == 1 && !a[0].is_array() && !a[0].is_object() => to_num(&a[0]),
        _ => f64::NAN,
    }
}

/// JS `v === n` for a number literal.
fn is_num(v: &Value, n: f64) -> bool {
    v.as_f64() == Some(n)
}

/// JS `Math.round`.
fn js_round(x: f64) -> f64 {
    if x - x.floor() >= 0.5 { x.floor() + 1.0 } else { x.floor() }
}

/// JS `String(n)` for the integral values this module produces.
fn int_str(x: f64) -> String {
    if x.abs() < 1e21 { format!("{:.0}", x + 0.0) } else { format!("{x:e}").replace('e', "e+") }
}

// ---------------------------------------------------------------- port

/// Port of `confidenceOnBatteryUpdate` (`battery.mjs:6`). Distinct objects/arrays are never `===`.
pub fn confidence_on_battery_update(prev_ac: Value, new_ac: Value, confidence: Value) -> R {
    let same = !new_ac.is_array() && !new_ac.is_object() && (new_ac == prev_ac || new_ac.as_f64().is_some_and(|n| is_num(&prev_ac, n)));
    Ok(if same { confidence } else { json!(0) })
}

/// Port of `confidenceTimerDelayMs` (`battery.mjs:11`).
pub fn confidence_timer_delay_ms(confidence: Value) -> R {
    Ok(json!(if is_num(&confidence, 0.0) {
        10000
    } else if is_num(&confidence, 1.0) {
        60000
    } else {
        -1
    }))
}

/// Port of `confidenceAfterTimer` (`battery.mjs:18`).
pub fn confidence_after_timer(confidence: Value) -> R {
    Ok(json!(if is_num(&confidence, 0.0) { 1 } else { 2 }))
}

/// Port of `sayFullAction` (`battery.mjs:24`).
pub fn say_full_action(prev_battery_state: Value, new_battery_state: Value) -> R {
    Ok(json!(if is_num(&new_battery_state, 3.0) {
        "set"
    } else if is_num(&prev_battery_state, 3.0) {
        "clear-after-5000"
    } else {
        "keep"
    }))
}

/// Port of `batteryTimeParts` (`battery.mjs:31`).
pub fn battery_time_parts(seconds_remaining: Value) -> R {
    let s = to_num(&seconds_remaining);
    if !(s.is_finite() && s >= 0.0) {
        return Ok(json!("?|?"));
    }
    Ok(json!(format!("{}|{}", int_str((s / 60.0 / 60.0).floor()), int_str(((s / 60.0) % 60.0).floor()))))
}

/// Port of `batteryTimeShown` (`battery.mjs:41`).
pub fn battery_time_shown(confidence: Value) -> R {
    Ok(json!(to_num(&confidence) > 0.0))
}

/// Port of `batteryTimeCaptionToken` (`battery.mjs:46`).
pub fn battery_time_caption_token(battery_state: Value, confidence: Value) -> R {
    let calculating = to_num(&confidence) < 2.0;
    Ok(json!(match (battery_state.as_f64(), calculating) {
        (Some(1.0), true) => "#QuickAccess_Tab_Perf_CalculatingBatteryLife",
        (Some(1.0), false) => "#QuickAccess_Tab_Perf_ProjectedBatteryLife",
        (Some(2.0), true) => "#QuickAccess_Tab_Perf_CalculatingChargingTime",
        (Some(2.0), false) => "#QuickAccess_Tab_Perf_ProjectedChargingTime",
        _ => "",
    }))
}

/// Port of `batteryPercentText` (`battery.mjs:58`).
pub fn battery_percent_text(level: Value) -> R {
    let l = to_num(&level);
    Ok(json!(if l.is_finite() { format!("{}%", int_str(js_round(l * 100.0).min(99.0))) } else { String::new() }))
}

/// Port of `showHeaderPercent` (`battery.mjs:64`). Like the JS `&&` chain it returns the first
/// falsy operand itself, so a falsy `prefShow` comes back unchanged.
pub fn show_header_percent(pref_show: Value, never_show: Value, region: Value, is_full: Value, hint_visible: Value) -> R {
    if !region.is_null() && to_num(&region) == 2.0 {
        return Ok(json!(false));
    }
    if !truthy(&pref_show) {
        return Ok(pref_show);
    }
    Ok(json!(!truthy(&never_show) && (!truthy(&is_full) || hint_visible == json!(true))))
}

/// Port of `batteryPrefShow` (`battery.mjs:70`). Only a string (or a one-string array) can parse to an object.
pub fn battery_pref_show(stored_json: Value) -> R {
    let text = match &stored_json {
        Value::String(s) => s.as_str(),
        Value::Array(a) if a.len() == 1 => a[0].as_str().unwrap_or(""),
        _ => "",
    };
    let flag = matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(o)) if o.get("bShowBatteryPercentage").is_some_and(truthy));
    Ok(json!(flag))
}

/// Port of `batteryPrefStored` (`battery.mjs:78`).
pub fn battery_pref_stored(v: Value) -> R {
    Ok(json!(format!("{{\"bShowBatteryPercentage\":{}}}", truthy(&v))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_decisions() {
        assert_eq!(confidence_on_battery_update(json!(1), json!(3), json!(2)).unwrap(), json!(0));
        assert_eq!(confidence_timer_delay_ms(json!(1)).unwrap(), json!(60000));
        assert_eq!(battery_time_parts(json!(7384)).unwrap(), json!("2|3"));
        assert_eq!(battery_percent_text(json!(1.0)).unwrap(), json!("99%"));
        assert_eq!(battery_time_caption_token(json!(2), json!(1)).unwrap(), json!("#QuickAccess_Tab_Perf_CalculatingChargingTime"));
        assert_eq!(battery_pref_show(json!("{\"bShowBatteryPercentage\":true}")).unwrap(), json!(true));
        assert_eq!(show_header_percent(json!(true), json!(false), json!(1), json!(true), json!(false)).unwrap(), json!(false));
    }
}
