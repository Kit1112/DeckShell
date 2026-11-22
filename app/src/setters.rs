//! Writes into Steam state done by the library / home / media / settings / downloads screens. Same calls the native
//! handlers make (JS evaluated in SharedJSContext through `Bridge::eval_json`, source line cited per setter).
//! `DECKSHELL_DRYRUN=1` logs the JS instead of running it. Reads (`read`) are harmless and always run.
use serde_json::Value;

use crate::actions;
use crate::app::App;

/// Fire-and-forget write. `what` is a log tag.
pub fn write(what: &str, js: String) {
    if actions::dryrun() {
        eprintln!("[deckshell] DRYRUN setter {what}: {js}");
        return;
    }
    let what = what.to_string();
    std::thread::spawn(move || match actions::bridge() {
        Some(b) => match b.eval_json(&js) {
            Ok(_) => eprintln!("[deckshell] setter {what} ok"),
            Err(e) => eprintln!("[deckshell] setter {what} failed: {e}"),
        },
        None => eprintln!("[deckshell] setter {what}: needs the Steam bridge (unavailable)"),
    });
}

/// Write followed by a read of the stored value, on one worker thread at a time (writes from fast presses keep their order).
/// `cb` runs on the UI thread with the read value (Null when the read or the bridge failed). DRYRUN only logs and never calls `cb`.
pub fn write_then_read(what: &str, js: String, read_js: String, cb: impl FnOnce(&mut App, Value) + Send + 'static) {
    if actions::dryrun() {
        eprintln!("[deckshell] DRYRUN setter {what}: {js}");
        return;
    }
    static ORDER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let what = what.to_string();
    std::thread::spawn(move || {
        let _turn = ORDER.lock().unwrap_or_else(|e| e.into_inner());
        let read = match actions::bridge() {
            Some(b) => {
                match b.eval_json(&js) {
                    Ok(_) => eprintln!("[deckshell] setter {what} ok"),
                    Err(e) => eprintln!("[deckshell] setter {what} failed: {e}"),
                }
                b.eval_json(&read_js).unwrap_or(Value::Null)
            }
            None => { eprintln!("[deckshell] setter {what}: needs the Steam bridge (unavailable)"); Value::Null }
        };
        crate::screens::defer(move |a| cb(a, read));
    });
}

/// Read-only evaluation off the UI thread; `cb` runs on the UI thread with the value (Null on failure / no bridge).
pub fn read(js: String, cb: impl FnOnce(&mut App, Value) + Send + 'static) {
    std::thread::spawn(move || {
        let v = actions::bridge().and_then(|b| b.eval_json(&js).ok()).unwrap_or(Value::Null);
        crate::screens::defer(move |a| cb(a, v));
    });
}

pub fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Library tile menu: collectionStore.SetAppsAsFavorite(appids, on) (sp.js:437831 -> AddOrRemoveApp(.., "favorite")).
pub fn set_favorite(appid: u32, on: bool) {
    write("favorite", format!("collectionStore.SetAppsAsFavorite([{appid}],{on})"));
}

/// Library X dialog: collectionStore.SetGamepadCollectionFilter(option) (sp.js:437724, dialog 534912: options YA=25 verified,
/// eb=15 +playable, gg=24 all). `level` 0/1/2 = dialog row.
pub fn set_gamepad_filter(level: usize) {
    let opt = [25, 15, 24][level.min(2)];
    write("gamepad-filter", format!("collectionStore.SetGamepadCollectionFilter({opt})"));
}
pub const READ_GAMEPAD_FILTER: &str = "collectionStore.GetCurrentGamepadFilter()";

const CLIENT_FIELDS: &str = include_str!("client_settings_fields.json");

fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// CMsgClientSettings with one field set, base64 (sp.js:455259 K(): serializeBinaryToWriter(fromObject({[key]: v})) -> getResultBase64String).
/// Field numbers come from the bundle's message table (client_settings_fields.json). Bool/int/enum = varint, string = length-delimited.
pub fn encode_client_setting(key: &str, v: &Value) -> Option<String> {
    let tbl: Value = serde_json::from_str(CLIENT_FIELDS).ok()?;
    let f = tbl.get(key)?;
    let (n, ty) = (f[0].as_u64()?, f[1].as_str()?);
    let mut out = Vec::new();
    match ty {
        "Bool" | "Int32" | "Uint32" | "Int64" | "Uint64" | "Enum" => {
            varint(n << 3, &mut out);
            let x = match v {
                Value::Bool(b) => *b as u64,
                other => other.as_i64()? as u64,
            };
            varint(x, &mut out);
        }
        "String" => {
            let st = v.as_str()?;
            varint(n << 3 | 2, &mut out);
            varint(st.len() as u64, &mut out);
            out.extend_from_slice(st.as_bytes());
        }
        "Float" => {
            varint(n << 3 | 5, &mut out);
            out.extend_from_slice(&(v.as_f64()? as f32).to_le_bytes());
        }
        _ => return None,
    }
    Some(b64(&out))
}

/// settingsStore client setting (steamos_cec_control, enable_overlay, ...): SteamClient.Settings.SetSetting(base64 message) - sp.js:455264.
pub fn client_setting(key: &str, v: &Value) -> bool {
    match encode_client_setting(key, v) {
        Some(b) => {
            write("client-setting", format!("SteamClient.Settings.SetSetting({})", js_str(&b)));
            true
        }
        None => {
            eprintln!("[deckshell] client setting {key}: no protobuf field known");
            false
        }
    }
}

/// Stock `SystemStoragStore.SetIdleBacklightDimSeconds(bOnAC, sec)` (sp.js:427407): a CMsgSystemSettings with
/// idle_backlight_dim_battery_seconds (field 1) or idle_backlight_dim_ac_seconds (field 2), both float, sent through
/// `SteamClient.System.UpdateSettings(base64)`.
pub fn encode_idle_dim(ac: bool, sec: i64) -> String {
    let mut out = Vec::new();
    varint((if ac { 2 } else { 1 }) << 3 | 5, &mut out);
    out.extend_from_slice(&(sec as f32).to_le_bytes());
    b64(&out)
}
pub fn idle_dim(ac: bool, sec: i64) {
    write("idle-dim", format!("SteamClient.System.UpdateSettings({})", js_str(&encode_idle_dim(ac, sec))));
}

/// Friends settings (bCompactFriendsList, ...): settingsStore.UpdateFriendSetting(name, value) - sp.js:455263.
pub fn friend_setting(key: &str, v: &Value) {
    write("friend-setting", format!("settingsStore.UpdateFriendSetting({},{})", js_str(key), v));
}

