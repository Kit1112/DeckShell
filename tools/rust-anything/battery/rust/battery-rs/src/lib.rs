//! JS bindings for `battery.mjs`: generated from the signatures in `port.rs` by rust-anything.
//! Regenerate with `rust-anything refresh` after changing them.

use wasm_bindgen::prelude::*;

pub mod js;
pub mod port;
pub mod wire;

#[allow(unused_imports)]
use serde_json::Value;

/// `confidenceOnBatteryUpdate`.
fn confidence_on_battery_update_inner(prev_ac: String, new_ac: String, confidence: String) -> Result<String, String> {
    let prev_ac = crate::js::from_json(&prev_ac)?;
    let new_ac = crate::js::from_json(&new_ac)?;
    let confidence = crate::js::from_json(&confidence)?;
    let out = port::confidence_on_battery_update(prev_ac, new_ac, confidence)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "confidenceOnBatteryUpdate")]
pub fn confidence_on_battery_update(prev_ac: String, new_ac: String, confidence: String) -> Result<String, JsError> {
    confidence_on_battery_update_inner(prev_ac, new_ac, confidence).map_err(|e| JsError::new(&e))
}

/// `confidenceTimerDelayMs`.
fn confidence_timer_delay_ms_inner(confidence: String) -> Result<String, String> {
    let confidence = crate::js::from_json(&confidence)?;
    let out = port::confidence_timer_delay_ms(confidence)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "confidenceTimerDelayMs")]
pub fn confidence_timer_delay_ms(confidence: String) -> Result<String, JsError> {
    confidence_timer_delay_ms_inner(confidence).map_err(|e| JsError::new(&e))
}

/// `confidenceAfterTimer`.
fn confidence_after_timer_inner(confidence: String) -> Result<String, String> {
    let confidence = crate::js::from_json(&confidence)?;
    let out = port::confidence_after_timer(confidence)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "confidenceAfterTimer")]
pub fn confidence_after_timer(confidence: String) -> Result<String, JsError> {
    confidence_after_timer_inner(confidence).map_err(|e| JsError::new(&e))
}

/// `sayFullAction`.
fn say_full_action_inner(prev_battery_state: String, new_battery_state: String) -> Result<String, String> {
    let prev_battery_state = crate::js::from_json(&prev_battery_state)?;
    let new_battery_state = crate::js::from_json(&new_battery_state)?;
    let out = port::say_full_action(prev_battery_state, new_battery_state)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "sayFullAction")]
pub fn say_full_action(prev_battery_state: String, new_battery_state: String) -> Result<String, JsError> {
    say_full_action_inner(prev_battery_state, new_battery_state).map_err(|e| JsError::new(&e))
}

/// `batteryTimeParts`.
fn battery_time_parts_inner(seconds_remaining: String) -> Result<String, String> {
    let seconds_remaining = crate::js::from_json(&seconds_remaining)?;
    let out = port::battery_time_parts(seconds_remaining)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryTimeParts")]
pub fn battery_time_parts(seconds_remaining: String) -> Result<String, JsError> {
    battery_time_parts_inner(seconds_remaining).map_err(|e| JsError::new(&e))
}

/// `batteryTimeShown`.
fn battery_time_shown_inner(confidence: String) -> Result<String, String> {
    let confidence = crate::js::from_json(&confidence)?;
    let out = port::battery_time_shown(confidence)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryTimeShown")]
pub fn battery_time_shown(confidence: String) -> Result<String, JsError> {
    battery_time_shown_inner(confidence).map_err(|e| JsError::new(&e))
}

/// `batteryTimeCaptionToken`.
fn battery_time_caption_token_inner(battery_state: String, confidence: String) -> Result<String, String> {
    let battery_state = crate::js::from_json(&battery_state)?;
    let confidence = crate::js::from_json(&confidence)?;
    let out = port::battery_time_caption_token(battery_state, confidence)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryTimeCaptionToken")]
pub fn battery_time_caption_token(battery_state: String, confidence: String) -> Result<String, JsError> {
    battery_time_caption_token_inner(battery_state, confidence).map_err(|e| JsError::new(&e))
}

/// `batteryPercentText`.
fn battery_percent_text_inner(level: String) -> Result<String, String> {
    let level = crate::js::from_json(&level)?;
    let out = port::battery_percent_text(level)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryPercentText")]
pub fn battery_percent_text(level: String) -> Result<String, JsError> {
    battery_percent_text_inner(level).map_err(|e| JsError::new(&e))
}

/// `showHeaderPercent`.
fn show_header_percent_inner(
    pref_show: String,
    never_show: String,
    region: String,
    is_full: String,
    hint_visible: String,
) -> Result<String, String> {
    let pref_show = crate::js::from_json(&pref_show)?;
    let never_show = crate::js::from_json(&never_show)?;
    let region = crate::js::from_json(&region)?;
    let is_full = crate::js::from_json(&is_full)?;
    let hint_visible = crate::js::from_json(&hint_visible)?;
    let out = port::show_header_percent(pref_show, never_show, region, is_full, hint_visible)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "showHeaderPercent")]
pub fn show_header_percent(
    pref_show: String,
    never_show: String,
    region: String,
    is_full: String,
    hint_visible: String,
) -> Result<String, JsError> {
    show_header_percent_inner(pref_show, never_show, region, is_full, hint_visible).map_err(|e| JsError::new(&e))
}

/// `batteryPrefShow`.
fn battery_pref_show_inner(stored_json: String) -> Result<String, String> {
    let stored_json = crate::js::from_json(&stored_json)?;
    let out = port::battery_pref_show(stored_json)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryPrefShow")]
pub fn battery_pref_show(stored_json: String) -> Result<String, JsError> {
    battery_pref_show_inner(stored_json).map_err(|e| JsError::new(&e))
}

/// `batteryPrefStored`.
fn battery_pref_stored_inner(v: String) -> Result<String, String> {
    let v = crate::js::from_json(&v)?;
    let out = port::battery_pref_stored(v)?;
    crate::js::to_json(&out)
}

#[wasm_bindgen(js_name = "batteryPrefStored")]
pub fn battery_pref_stored(v: String) -> Result<String, JsError> {
    battery_pref_stored_inner(v).map_err(|e| JsError::new(&e))
}

// ---- the binary wire ------------------------------------------------------------------------

thread_local! {
    static WIRE_IN: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    static WIRE_OUT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// This module's memory, where `index.cjs` reads and writes the wire buffers.
#[wasm_bindgen(js_name = "rustAnythingWireMemory")]
pub fn rust_anything_wire_memory() -> JsValue {
    wasm_bindgen::memory()
}

/// The address of an argument buffer of `len` bytes.
#[wasm_bindgen(js_name = "rustAnythingWireIn")]
pub fn rust_anything_wire_in(len: u32) -> u32 {
    WIRE_IN.with(|b| {
        let mut b = b.borrow_mut();
        b.resize(len as usize, 0);
        b.as_mut_ptr() as usize as u32
    })
}

/// The address of the last result.
#[wasm_bindgen(js_name = "rustAnythingWireOut")]
pub fn rust_anything_wire_out() -> u32 {
    WIRE_OUT.with(|b| b.borrow().as_ptr() as usize as u32)
}

/// Reads `len` argument bytes, runs `f`, and keeps its result for `rustAnythingWireOut`.
fn wire_io(len: u32, f: impl FnOnce(&mut wire::Reader, &mut wire::Writer) -> Result<(), String>) -> Result<u32, JsError> {
    let input = WIRE_IN.with(|b| std::mem::take(&mut *b.borrow_mut()));
    let mut w = wire::Writer::default();
    let result = f(&mut wire::Reader::new(&input[..(len as usize).min(input.len())]), &mut w);
    WIRE_IN.with(|b| *b.borrow_mut() = input);
    match result {
        // A result of another shape than the TS type: JS runs the call through JSON.
        Err(e) if e.starts_with(wire::MISMATCH) => return Ok(u32::MAX),
        other => other.map_err(|e| JsError::new(&e))?,
    }
    let n = w.0.len();
    WIRE_OUT.with(|b| *b.borrow_mut() = w.0);
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

static S_CONFIDENCE_ON_BATTERY_UPDATE_0: wire::Schema = wire::Schema::Dyn;
static S_CONFIDENCE_ON_BATTERY_UPDATE_1: wire::Schema = wire::Schema::Dyn;
static S_CONFIDENCE_ON_BATTERY_UPDATE_2: wire::Schema = wire::Schema::Dyn;
static S_CONFIDENCE_ON_BATTERY_UPDATE_OUT: wire::Schema = wire::Schema::Dyn;

/// `confidenceOnBatteryUpdate` over the binary wire.
#[wasm_bindgen(js_name = "confidenceOnBatteryUpdateWire")]
pub fn confidence_on_battery_update_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let prev_ac: serde_json::Value = wire::read_value(r, &S_CONFIDENCE_ON_BATTERY_UPDATE_0)?;
        let new_ac: serde_json::Value = wire::read_value(r, &S_CONFIDENCE_ON_BATTERY_UPDATE_1)?;
        let confidence: serde_json::Value = wire::read_value(r, &S_CONFIDENCE_ON_BATTERY_UPDATE_2)?;
        let out = &port::confidence_on_battery_update(prev_ac, new_ac, confidence)?;
        wire::write_value(w, out, &S_CONFIDENCE_ON_BATTERY_UPDATE_OUT)?;
        Ok(())
    })
}

static S_CONFIDENCE_TIMER_DELAY_MS_0: wire::Schema = wire::Schema::Dyn;
static S_CONFIDENCE_TIMER_DELAY_MS_OUT: wire::Schema = wire::Schema::Dyn;

/// `confidenceTimerDelayMs` over the binary wire.
#[wasm_bindgen(js_name = "confidenceTimerDelayMsWire")]
pub fn confidence_timer_delay_ms_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let confidence: serde_json::Value = wire::read_value(r, &S_CONFIDENCE_TIMER_DELAY_MS_0)?;
        let out = &port::confidence_timer_delay_ms(confidence)?;
        wire::write_value(w, out, &S_CONFIDENCE_TIMER_DELAY_MS_OUT)?;
        Ok(())
    })
}

static S_CONFIDENCE_AFTER_TIMER_0: wire::Schema = wire::Schema::Dyn;
static S_CONFIDENCE_AFTER_TIMER_OUT: wire::Schema = wire::Schema::Dyn;

/// `confidenceAfterTimer` over the binary wire.
#[wasm_bindgen(js_name = "confidenceAfterTimerWire")]
pub fn confidence_after_timer_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let confidence: serde_json::Value = wire::read_value(r, &S_CONFIDENCE_AFTER_TIMER_0)?;
        let out = &port::confidence_after_timer(confidence)?;
        wire::write_value(w, out, &S_CONFIDENCE_AFTER_TIMER_OUT)?;
        Ok(())
    })
}

static S_SAY_FULL_ACTION_0: wire::Schema = wire::Schema::Dyn;
static S_SAY_FULL_ACTION_1: wire::Schema = wire::Schema::Dyn;
static S_SAY_FULL_ACTION_OUT: wire::Schema = wire::Schema::Dyn;

/// `sayFullAction` over the binary wire.
#[wasm_bindgen(js_name = "sayFullActionWire")]
pub fn say_full_action_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let prev_battery_state: serde_json::Value = wire::read_value(r, &S_SAY_FULL_ACTION_0)?;
        let new_battery_state: serde_json::Value = wire::read_value(r, &S_SAY_FULL_ACTION_1)?;
        let out = &port::say_full_action(prev_battery_state, new_battery_state)?;
        wire::write_value(w, out, &S_SAY_FULL_ACTION_OUT)?;
        Ok(())
    })
}

static S_BATTERY_TIME_PARTS_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_TIME_PARTS_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryTimeParts` over the binary wire.
#[wasm_bindgen(js_name = "batteryTimePartsWire")]
pub fn battery_time_parts_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let seconds_remaining: serde_json::Value = wire::read_value(r, &S_BATTERY_TIME_PARTS_0)?;
        let out = &port::battery_time_parts(seconds_remaining)?;
        wire::write_value(w, out, &S_BATTERY_TIME_PARTS_OUT)?;
        Ok(())
    })
}

static S_BATTERY_TIME_SHOWN_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_TIME_SHOWN_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryTimeShown` over the binary wire.
#[wasm_bindgen(js_name = "batteryTimeShownWire")]
pub fn battery_time_shown_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let confidence: serde_json::Value = wire::read_value(r, &S_BATTERY_TIME_SHOWN_0)?;
        let out = &port::battery_time_shown(confidence)?;
        wire::write_value(w, out, &S_BATTERY_TIME_SHOWN_OUT)?;
        Ok(())
    })
}

static S_BATTERY_TIME_CAPTION_TOKEN_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_TIME_CAPTION_TOKEN_1: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_TIME_CAPTION_TOKEN_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryTimeCaptionToken` over the binary wire.
#[wasm_bindgen(js_name = "batteryTimeCaptionTokenWire")]
pub fn battery_time_caption_token_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let battery_state: serde_json::Value = wire::read_value(r, &S_BATTERY_TIME_CAPTION_TOKEN_0)?;
        let confidence: serde_json::Value = wire::read_value(r, &S_BATTERY_TIME_CAPTION_TOKEN_1)?;
        let out = &port::battery_time_caption_token(battery_state, confidence)?;
        wire::write_value(w, out, &S_BATTERY_TIME_CAPTION_TOKEN_OUT)?;
        Ok(())
    })
}

static S_BATTERY_PERCENT_TEXT_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_PERCENT_TEXT_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryPercentText` over the binary wire.
#[wasm_bindgen(js_name = "batteryPercentTextWire")]
pub fn battery_percent_text_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let level: serde_json::Value = wire::read_value(r, &S_BATTERY_PERCENT_TEXT_0)?;
        let out = &port::battery_percent_text(level)?;
        wire::write_value(w, out, &S_BATTERY_PERCENT_TEXT_OUT)?;
        Ok(())
    })
}

static S_SHOW_HEADER_PERCENT_0: wire::Schema = wire::Schema::Dyn;
static S_SHOW_HEADER_PERCENT_1: wire::Schema = wire::Schema::Dyn;
static S_SHOW_HEADER_PERCENT_2: wire::Schema = wire::Schema::Dyn;
static S_SHOW_HEADER_PERCENT_3: wire::Schema = wire::Schema::Dyn;
static S_SHOW_HEADER_PERCENT_4: wire::Schema = wire::Schema::Dyn;
static S_SHOW_HEADER_PERCENT_OUT: wire::Schema = wire::Schema::Dyn;

/// `showHeaderPercent` over the binary wire.
#[wasm_bindgen(js_name = "showHeaderPercentWire")]
pub fn show_header_percent_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let pref_show: serde_json::Value = wire::read_value(r, &S_SHOW_HEADER_PERCENT_0)?;
        let never_show: serde_json::Value = wire::read_value(r, &S_SHOW_HEADER_PERCENT_1)?;
        let region: serde_json::Value = wire::read_value(r, &S_SHOW_HEADER_PERCENT_2)?;
        let is_full: serde_json::Value = wire::read_value(r, &S_SHOW_HEADER_PERCENT_3)?;
        let hint_visible: serde_json::Value = wire::read_value(r, &S_SHOW_HEADER_PERCENT_4)?;
        let out = &port::show_header_percent(pref_show, never_show, region, is_full, hint_visible)?;
        wire::write_value(w, out, &S_SHOW_HEADER_PERCENT_OUT)?;
        Ok(())
    })
}

static S_BATTERY_PREF_SHOW_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_PREF_SHOW_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryPrefShow` over the binary wire.
#[wasm_bindgen(js_name = "batteryPrefShowWire")]
pub fn battery_pref_show_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let stored_json: serde_json::Value = wire::read_value(r, &S_BATTERY_PREF_SHOW_0)?;
        let out = &port::battery_pref_show(stored_json)?;
        wire::write_value(w, out, &S_BATTERY_PREF_SHOW_OUT)?;
        Ok(())
    })
}

static S_BATTERY_PREF_STORED_0: wire::Schema = wire::Schema::Dyn;
static S_BATTERY_PREF_STORED_OUT: wire::Schema = wire::Schema::Dyn;

/// `batteryPrefStored` over the binary wire.
#[wasm_bindgen(js_name = "batteryPrefStoredWire")]
pub fn battery_pref_stored_wire(len: u32) -> Result<u32, JsError> {
    wire_io(len, |r, w| {
        let v: serde_json::Value = wire::read_value(r, &S_BATTERY_PREF_STORED_0)?;
        let out = &port::battery_pref_stored(v)?;
        wire::write_value(w, out, &S_BATTERY_PREF_STORED_OUT)?;
        Ok(())
    })
}
