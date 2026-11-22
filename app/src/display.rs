//! Brightness through Steam's own display API (task 270): the one writer and the one reader.
//! The stock Quick Access slider makes the same calls, so the 0..1 position scale and the backlight
//! curve behind it are Steam's. Raw sysfs values are only logged for diagnosis (`actions`).
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static MOCK_NIGHT_MODE: AtomicBool = AtomicBool::new(false);

pub fn night_mode(mock: bool) -> Option<bool> {
    if mock || cfg!(test) || crate::actions::dryrun() {
        return Some(MOCK_NIGHT_MODE.load(Ordering::Relaxed));
    }
    let bridge = crate::actions::bridge()?;
    bridge.night_mode_settings().ok()?["enabled"].as_bool()
}

/// Persist Steam's setting and wait for its settings callback and compositor update.
pub fn set_night_mode(on: bool, mock: bool) -> Result<bool, String> {
    if mock || cfg!(test) || crate::actions::dryrun() {
        MOCK_NIGHT_MODE.store(on, Ordering::Relaxed);
        return Ok(on);
    }
    let bridge = crate::actions::bridge().ok_or("Steam bridge unavailable")?;
    if !bridge.set_night_mode(on).map_err(|e| e.to_string())? {
        return Err("Steam did not accept the night mode request".into());
    }
    let mut confirmed = None;
    for _ in 0..10 {
        let settings = bridge.night_mode_settings().map_err(|e| e.to_string())?;
        if settings["enabled"].as_bool() == Some(on) {
            if crate::gamescope_atoms::night_mode_active() != Some(!on) { return Ok(on); }
            confirmed = Some(settings);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let settings = confirmed.ok_or("Steam did not report the requested night mode state")?;
    let field = |name: &str| settings[name].as_f64().map(|v| v as f32)
        .filter(|v| v.is_finite()).ok_or_else(|| format!("Steam night mode setting {name} unavailable"));
    let amount = if on { field("strength")? } else { 0.0 };
    crate::gamescope_atoms::write_night_mode(amount, field("hue")?, field("saturation")?)?;
    Ok(on)
}

/// Lowest scale position the stock slider reaches: backlight 1209 of 600000, the value Steam keeps as
/// `DisplayBrightness` at its minimum (config.vdf). Below it Steam drives the backlight to 0, a black
/// screen (task 284, QA 2026-11-13: hotkey down went 1209 → 0 in 0.1 s). Measured on the Deck: see handoff.
pub const FLOOR: f32 = 0.0016666665;

/// Keeps a scale position in [`FLOOR`, 1]. Every writer and reader goes through it: the chord, the QAM
/// slider, the popup and the stored value must never show or write a position below the stock minimum.
pub fn clamp(v: f32) -> f32 {
    v.clamp(FLOOR, 1.0)
}

/// Scale position 0..1 from Steam's display callback; `None` without a bridge or before Steam reports one.
pub fn read() -> Option<f32> {
    crate::actions::bridge()?.brightness().ok().flatten().map(clamp)
}

/// Writes a scale position in [`FLOOR`, 1]. Steam refuses positions above its top step and then keeps the
/// previous backlight level; the scale still shows the requested position, as the stock slider does.
pub fn write(v: f32) -> io::Result<()> {
    let bridge = crate::actions::bridge().ok_or_else(|| io::Error::other("Steam bridge unavailable"))?;
    bridge.set_brightness(clamp(v)).map_err(|e| io::Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{clamp, FLOOR};

    #[test]
    fn positions_below_the_stock_minimum_are_raised_to_it() {
        assert_eq!(clamp(0.0), FLOOR);
        assert_eq!(clamp(-1.0), FLOOR);
        assert_eq!(clamp(FLOOR / 2.0), FLOOR);
    }

    #[test]
    fn positions_inside_the_range_and_the_top_are_kept() {
        assert_eq!(clamp(0.3), 0.3);
        assert_eq!(clamp(1.0), 1.0);
        assert_eq!(clamp(1.5), 1.0);
    }
}
