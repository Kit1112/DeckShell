//! Applying art exactly as the Decky plugin does (hooks/useSGDB.tsx changeAsset / clearAsset):
//!   ClearCustomArtworkForApp(appid, eAssetType) -> wait 500 ms (it resolves before Steam finished) ->
//!   SetCustomArtworkForApp(appid, base64, "png", eAssetType)      (the format argument is always "png", Steam sniffs the data)
//! Logo position: SteamClient.Apps.SetCustomLogoPositionForApp(appid, JSON {nVersion:1, logoPosition}) (= appDetailsStore.SaveCustomLogoPosition).
//! Icons of Steam apps: the plugin "poisons" appcache/librarycache/<appid>/<icon_hash>.jpg through a temp file; same here.
//! `DECKSHELL_DRYRUN=1` logs every call and changes nothing (undo snapshots included).
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::path::{Path, PathBuf};

use steam_bridge::Bridge;

use super::api::AType;
use super::cache;

pub fn dry() -> bool {
    crate::actions::dryrun()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub fn base64(d: &[u8]) -> String {
    let mut o = String::with_capacity(d.len().div_ceil(3) * 4);
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(B64[(n >> 18) as usize & 63] as char);
        o.push(B64[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    o
}

pub fn grid_dir(root: &Path, account: u32) -> PathBuf {
    root.join("userdata").join(account.to_string()).join("config/grid")
}

/// The custom image file Steam stores for (appid, type): grid/<appid><suffix>.<ext>.
pub fn current_custom(grid: &Path, appid: u32, t: AType) -> Option<PathBuf> {
    ["png", "jpg", "jpeg", "webp", "ico", "gif"].iter().map(|e| grid.join(format!("{appid}{}.{e}", t.suffix()))).find(|p| fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false))
}

// ------------------------------------------------------------------ undo (one level per app+type, survives restarts, swaps on undo)
fn undo_names(appid: u32, t: AType) -> (PathBuf, PathBuf) {
    let d = cache::undo_dir();
    (d.join(format!("{appid}_{}.bak", t.key())), d.join(format!("{appid}_{}.none", t.key())))
}

#[derive(Debug, PartialEq)]
pub enum Undo {
    /// previous custom file saved
    Restore(PathBuf),
    /// there was no custom art before: undo = clear
    Clear,
    Nothing,
}

pub fn undo_state(appid: u32, t: AType) -> Undo {
    let (bak, none) = undo_names(appid, t);
    if bak.is_file() {
        Undo::Restore(bak)
    } else if none.is_file() {
        Undo::Clear
    } else {
        Undo::Nothing
    }
}

/// Remember `prev` (the file about to be replaced, None = no custom art) as the undo target.
pub fn save_undo(appid: u32, t: AType, prev: Option<&Path>) {
    let (bak, none) = undo_names(appid, t);
    let _ = fs::create_dir_all(cache::undo_dir());
    let _ = fs::remove_file(&bak);
    let _ = fs::remove_file(&none);
    match prev {
        Some(p) => {
            let _ = fs::copy(p, &bak);
        }
        None => {
            let _ = fs::write(&none, b"");
        }
    }
}

// ------------------------------------------------------------------ Steam calls
fn js_set(appid: u32, t: AType, b64: &str) -> String {
    let e = t.e_asset();
    format!("(async()=>{{await SteamClient.Apps.ClearCustomArtworkForApp({appid},{e});await new Promise(r=>setTimeout(r,500));await SteamClient.Apps.SetCustomArtworkForApp({appid},\"{b64}\",\"png\",{e});return true}})()")
}

/// Set custom art from a local image file (downloaded cover or an undo snapshot).
pub fn set_art(b: Option<&Bridge>, appid: u32, t: AType, file: &Path) -> Result<(), String> {
    let data = fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if dry() {
        eprintln!("[deckshell] DRYRUN sgdb SteamClient.Apps.ClearCustomArtworkForApp({appid}, {})", t.e_asset());
        eprintln!("[deckshell] DRYRUN sgdb SteamClient.Apps.SetCustomArtworkForApp({appid}, <base64 {} bytes>, \"png\", {})", data.len(), t.e_asset());
        return Ok(());
    }
    let b = b.ok_or("Steam недоступен (нет моста)")?;
    b.eval_json(&js_set(appid, t, &base64(&data))).map(|_| ()).map_err(|e| e.to_string())
}

pub fn clear_art(b: Option<&Bridge>, appid: u32, t: AType) -> Result<(), String> {
    if dry() {
        eprintln!("[deckshell] DRYRUN sgdb SteamClient.Apps.ClearCustomArtworkForApp({appid}, {})", t.e_asset());
        return Ok(());
    }
    let b = b.ok_or("Steam недоступен (нет моста)")?;
    b.eval_json(&format!("(async()=>{{await SteamClient.Apps.ClearCustomArtworkForApp({appid},{});await new Promise(r=>setTimeout(r,500));return true}})()", t.e_asset())).map(|_| ()).map_err(|e| e.to_string())
}

pub const PINS: [&str; 5] = ["BottomLeft", "UpperLeft", "UpperCenter", "CenterCenter", "BottomCenter"];

pub fn set_logo_position(b: Option<&Bridge>, appid: u32, pin: usize, w: f32, h: f32) -> Result<(), String> {
    let json = format!("{{\"nVersion\":1,\"logoPosition\":{{\"pinnedPosition\":\"{}\",\"nWidthPct\":{w},\"nHeightPct\":{h}}}}}", PINS[pin % 5]);
    if dry() {
        eprintln!("[deckshell] DRYRUN sgdb SteamClient.Apps.SetCustomLogoPositionForApp({appid}, {json})");
        return Ok(());
    }
    let b = b.ok_or("Steam недоступен (нет моста)")?;
    b.eval_json(&format!("SteamClient.Apps.SetCustomLogoPositionForApp({appid},{}).then(()=>true)", serde_json::Value::String(json))).map(|_| ()).map_err(|e| e.to_string())
}

pub fn clear_logo_position(b: Option<&Bridge>, appid: u32) -> Result<(), String> {
    if dry() {
        eprintln!("[deckshell] DRYRUN sgdb SteamClient.Apps.ClearCustomLogoPositionForApp({appid})");
        return Ok(());
    }
    let b = b.ok_or("Steam недоступен (нет моста)")?;
    b.eval_json(&format!("SteamClient.Apps.ClearCustomLogoPositionForApp({appid}).then(()=>true)")).map(|_| ()).map_err(|e| e.to_string())
}

/// grid/<appid>.json -> (pin index, width %, height %)
pub fn read_logo_position(grid: &Path, appid: u32) -> Option<(usize, f32, f32)> {
    let v = super::api::read_json(&grid.join(format!("{appid}.json")));
    let lp = v.get("logoPosition")?;
    let pin = PINS.iter().position(|p| Some(*p) == lp.get("pinnedPosition").and_then(|x| x.as_str()))?;
    Some((pin, lp.get("nWidthPct")?.as_f64()? as f32, lp.get("nHeightPct")?.as_f64()? as f32))
}

/// Icon of a Steam app: librarycache/<appid>/<icon_hash>.jpg replaced through a temp file (Steam restores the original if it
/// sees its cache file written in place). Returns the replaced path and the previous content snapshot for undo.
pub fn set_steam_icon(b: Option<&Bridge>, root: &Path, appid: u32, file: &Path) -> Result<PathBuf, String> {
    let hash = if dry() && b.is_none() {
        "<icon_hash>".to_string()
    } else {
        b.ok_or("Steam недоступен (нет моста)")?
            .eval_json(&format!("appStore.GetAppOverviewByAppID({appid}).icon_hash"))
            .map_err(|e| e.to_string())?
            .as_str()
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .ok_or("у приложения нет icon_hash")?
            .to_string()
    };
    let dir = root.join("appcache/librarycache").join(appid.to_string());
    let dest = dir.join(format!("{hash}.jpg"));
    if dry() {
        eprintln!("[deckshell] DRYRUN sgdb icon cache write {} <- {}", dest.display(), file.display());
        return Ok(dest);
    }
    let (bak, none) = undo_names(appid, AType::Icon);
    replace_icon(&dest, file, &bak, &none)?;
    Ok(dest)
}

// Stage both copies before replacing either file. In particular, `file` may be `bak`.
fn stage_icon(path: &Path, data: &[u8]) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or("некорректный путь иконки")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    loop {
        let tmp = parent.join(format!(".deckshell-icon-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(data) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e.to_string());
                }
                return Ok(tmp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub(super) fn replace_icon(dest: &Path, file: &Path, bak: &Path, none: &Path) -> Result<(), String> {
    // An absent cache icon cannot be restored by the existing icon Reset operation.
    // Refuse the change instead of recording an undo that cannot be performed.
    let previous = fs::read(dest).map_err(|e| format!("не удалось сохранить прежнюю иконку {}: {e}", dest.display()))?;
    let incoming = fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if incoming.is_empty() || incoming == previous {
        return Err("иконка не изменилась".into());
    }
    let snapshot = stage_icon(bak, &previous)?;
    let staged = match stage_icon(dest, &incoming) {
        Ok(p) => p,
        Err(e) => { let _ = fs::remove_file(&snapshot); return Err(e); }
    };
    if let Err(e) = fs::rename(&staged, dest) {
        let _ = fs::remove_file(&staged);
        let _ = fs::remove_file(&snapshot);
        return Err(e.to_string());
    }
    // If this fails, keep the staged snapshot: it is the only copy of the previous icon.
    fs::rename(&snapshot, bak).map_err(|e| format!("не удалось сохранить Undo; копия сохранена в {}: {e}", snapshot.display()))?;
    let _ = fs::remove_file(none);
    let actual = fs::read(dest).map_err(|e| format!("не удалось проверить иконку: {e}"))?;
    if actual != incoming { return Err("запись иконки не подтверждена".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!("deckshell-icon-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TestDir { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

    #[test]
    fn icon_undo_roundtrip_preserves_bytes() {
        let dir = TestDir::new();
        let dest = dir.0.join("librarycache/42/hash.jpg");
        let bak = dir.0.join("undo/42_icon.bak");
        let none = dir.0.join("undo/42_icon.none");
        let new = dir.0.join("new");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, b"old\0bytes").unwrap();
        fs::write(&new, b"new\xffbytes").unwrap();
        replace_icon(&dest, &new, &bak, &none).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new\xffbytes");
        replace_icon(&dest, &bak, &bak, &none).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"old\0bytes");
        replace_icon(&dest, &bak, &bak, &none).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new\xffbytes");
        assert_eq!(fs::read(&bak).unwrap(), b"old\0bytes");
    }

    #[test]
    fn icon_missing_original_is_rejected_without_changes() {
        let d = TestDir::new();
        let dest = d.0.join("missing"); let input = d.0.join("input");
        let bak = d.0.join("backup"); let none = d.0.join("none");
        fs::write(&input, b"new").unwrap(); fs::write(&bak, b"previous undo").unwrap();
        assert!(replace_icon(&dest, &input, &bak, &none).is_err());
        assert!(!dest.exists()); assert_eq!(fs::read(&bak).unwrap(), b"previous undo");
    }

    #[test]
    fn icon_copy_and_snapshot_errors_preserve_existing_files() {
        let d = TestDir::new();
        let dest = d.0.join("icon"); let bak = d.0.join("backup"); let none = d.0.join("none");
        fs::write(&dest, b"old").unwrap(); fs::write(&bak, b"undo").unwrap();
        assert!(replace_icon(&dest, &d.0.join("missing"), &bak, &none).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"old"); assert_eq!(fs::read(&bak).unwrap(), b"undo");
        let input = d.0.join("input"); fs::write(&input, b"new").unwrap();
        let blocked = d.0.join("blocked"); fs::write(&blocked, b"file").unwrap();
        assert!(replace_icon(&dest, &input, &blocked.join("backup"), &none).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"old"); assert_eq!(fs::read(&bak).unwrap(), b"undo");
    }

    #[test]
    fn icon_failed_backup_commit_keeps_snapshot() {
        let d = TestDir::new();
        let dest = d.0.join("icon"); let input = d.0.join("input");
        let bak = d.0.join("backup"); let none = d.0.join("none");
        fs::write(&dest, b"old").unwrap(); fs::write(&input, b"new").unwrap();
        fs::create_dir(&bak).unwrap();
        assert!(replace_icon(&dest, &input, &bak, &none).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        let snapshots: Vec<_> = fs::read_dir(&d.0).unwrap().map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(".deckshell-icon-")).collect();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(fs::read(&snapshots[0]).unwrap(), b"old");
    }

    #[test]
    fn b64_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn js_matches_plugin_sequence() {
        let s = js_set(7, AType::Wide, "QQ==");
        assert!(s.contains("ClearCustomArtworkForApp(7,3)") && s.contains("SetCustomArtworkForApp(7,\"QQ==\",\"png\",3)"));
        assert!(s.find("Clear").unwrap() < s.find("SetCustom").unwrap());
    }
}
