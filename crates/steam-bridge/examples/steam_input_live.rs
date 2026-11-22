//! Live check of the per-game Steam Input switch on a Steam Deck (STEAM_CDP_PORT points at it).
//! Read-only by default. `--write` switches Steam Input off for Balatro (2379780), reads back,
//! then restores the setting it found, and reads back again. The restore is attempted even when
//! a check fails, so the game is not left off. Other apps are refused.
//! Usage: STEAM_CDP_PORT=18080 cargo run -p steam-bridge --example steam_input_live -- <controller_index> [--write]
use std::time::Instant;

use steam_bridge::controller_steam_input::*;
use steam_bridge::Bridge;

const BALATRO: u32 = 2379780;

/// Read the raw state every 500 ms for up to 8 s until it matches `want` (Steam updates the
/// stored setting and the controller switch at different times). Returns the last reading.
fn poll(b: &Bridge, idx: u32, want: SteamInputSetting) -> Option<SteamInputState> {
    let mut last = None;
    let start = Instant::now();
    while start.elapsed().as_secs() < 8 {
        let setting = b.eval_json(&read_setting_expr(BALATRO)).ok().and_then(|v| parse_setting(&v).ok());
        let enabled = b.controller_config_info(BALATRO, idx).ok().map(|i| i.configuration_enabled);
        if let (Some(setting), Some(configuration_enabled)) = (setting, enabled) {
            let st = SteamInputState { setting, configuration_enabled };
            last = Some(st);
            if setting == want && settings_agree(&st) {
                println!("  settled after {} ms", start.elapsed().as_millis());
                return last;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    println!("  not settled after 8 s; last {:?}", last);
    last
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let idx: u32 = args.first().and_then(|s| s.parse().ok()).expect("controller index");
    let write = args.iter().any(|a| a == "--write");
    let b = Bridge::connect().expect("connect");

    let t = Instant::now();
    let before = b.steam_input_state(BALATRO, idx).expect("state before");
    println!("state.before {before:?} ms={}", t.elapsed().as_millis());
    if !write {
        return;
    }

    let t = Instant::now();
    let wrote = b.steam_input_set(BALATRO, idx, SteamInputSetting::Off);
    println!("write Off ms={} result={:?}", t.elapsed().as_millis(), wrote.as_ref().map(|_| ()).map_err(|e| e.to_string()));
    let off = poll(&b, idx, SteamInputSetting::Off);
    let off_ok = matches!(&off, Some(s) if s.setting == SteamInputSetting::Off && settings_agree(s));
    println!("off readback ok: {off_ok} (switch follows setting: {}; last raw {:?})", off.map(|s| settings_agree(&s)).unwrap_or(false), off);

    let restore_result = b.steam_input_set(BALATRO, idx, before.setting);
    println!("restore write {:?} result={:?}", before.setting, restore_result.as_ref().map(|_| ()).map_err(|e| e.to_string()));
    let restore = poll(&b, idx, before.setting);
    let restored = restore == Some(before);
    println!("restored to the starting state: {restored} (last raw {:?})", restore);
}
