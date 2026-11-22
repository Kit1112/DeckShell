//! Talks to the running Steam client via steam:// URLs and the filesystem.
//! CONTRACT shared with `app`.
pub use steam_data::Game;
pub mod steam_pipe;

use std::fs;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub enum SteamPage { Store, StoreApp(u32), Friends, Downloads, Settings, Achievements(u32), Properties(u32), Controller(u32), Screenshots, Keyboard }

/// Pure mapping page -> steam:// URL.
pub fn url_for(page: &SteamPage) -> String {
    match page {
        SteamPage::Store => "steam://store".into(),
        SteamPage::StoreApp(id) => format!("steam://store/{id}"),
        SteamPage::Friends => "steam://open/friends".into(),
        SteamPage::Downloads => "steam://open/downloads".into(),
        SteamPage::Settings => "steam://open/settings".into(),
        SteamPage::Achievements(id) => format!("steam://url/SteamIDAchievementsPage/{id}"),
        SteamPage::Properties(id) => format!("steam://gameproperties/{id}"),
        SteamPage::Controller(id) => format!("steam://controllerconfig/{id}"),
        SteamPage::Screenshots => "steam://open/screenshots".into(),
        SteamPage::Keyboard => "steam://open/keyboard".into(),
    }
}

fn home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default() }

fn steam_root() -> PathBuf {
    if let Some(r) = std::env::var_os("STEAM_ROOT") { return PathBuf::from(r); }
    let p = home().join(".local/share/Steam");
    if p.exists() { p } else { home().join(".steam/steam") }
}

/// True if the Steam client process from `~/.steam/steam.pid` is alive.
pub fn steam_running() -> bool {
    let pid = home().join(".steam/steam.pid");
    match fs::read_to_string(pid).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(p) => fs::read_to_string(format!("/proc/{p}/comm")).map(|c| c.trim().starts_with("steam")).unwrap_or(false),
        None => false,
    }
}

/// Hand a steam:// URL to the *running* Steam over `steam.pipe` (`steam_pipe`). Never starts Steam: with no
/// running client the request fails with `NotConnected`. `steam.sh <url>` / `xdg-open` were used here before;
/// both START a client when none is running, and in the gamescope session that client is the stock Gamepad UI
/// (docs/steam-ui-parallel-2026-11-20.md).
pub fn open_url(url: &str) -> Result<()> {
    if !url.starts_with("steam://") { return Err(Error::new(ErrorKind::InvalidInput, "not a steam:// URL")); }
    if deck_system::guard::deny(&format!("steam open_url {url}")) { return Ok(()); }
    steam_pipe::send(&[url])
}

/// Ask the running Steam to exit. With no running Steam nothing happens (`steam -shutdown` would start one).
pub fn request_shutdown() -> Result<()> { steam_pipe::send(&["-shutdown"]) }

pub fn launch(game: &Game) -> Result<()> { open_url(&format!("steam://rungameid/{}", game.rungame_id())) }
pub fn install(app_id: u32) -> Result<()> { open_url(&format!("steam://install/{app_id}")) }
pub fn uninstall(app_id: u32) -> Result<()> { open_url(&format!("steam://uninstall/{app_id}")) }
pub fn open(page: SteamPage) -> Result<()> { open_url(&url_for(&page)) }

// ------------------------------------------------------------ running game

fn proc_pids() -> Vec<u32> {
    fs::read_dir("/proc").map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect()).unwrap_or_default()
}

/// Parse a reaper command line (NUL separated): `... reaper SteamLaunch AppId=123 -- ...`.
pub fn parse_reaper_cmdline(cmdline: &[u8]) -> Option<u32> {
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    if !args.first().map(|a| a.ends_with(b"reaper")).unwrap_or(false) { return None; }
    let i = args.iter().position(|a| *a == b"SteamLaunch")?;
    let a = std::str::from_utf8(args.get(i + 1)?).ok()?;
    app_id_from(a.strip_prefix("AppId=")?)
}
fn app_id_from(s: &str) -> Option<u32> {
    let v: u64 = s.parse().ok()?;
    // non-Steam shortcuts may be given as full 64-bit game ids
    Some(if v > u32::MAX as u64 { (v >> 32) as u32 } else { v as u32 }).filter(|v| *v != 0)
}
fn reaper_pid_and_app() -> Option<(u32, u32)> {
    proc_pids().into_iter().find_map(|pid| {
        let c = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        parse_reaper_cmdline(&c).map(|app| (pid, app))
    })
}

pub fn running_game() -> Option<u32> {
    if let Some((_, app)) = reaper_pid_and_app() { return Some(app); }
    // fallback: environment of any own process (SteamAppId / SteamGameId)
    proc_pids().into_iter().find_map(|pid| {
        let env = fs::read(format!("/proc/{pid}/environ")).ok()?;
        for kv in env.split(|b| *b == 0) {
            let s = std::str::from_utf8(kv).ok()?;
            if let Some(v) = s.strip_prefix("SteamGameId=").or_else(|| s.strip_prefix("SteamAppId=")) {
                if let Some(id) = app_id_from(v) { return Some(id); }
            }
        }
        None
    })
}

fn pgid_of(pid: u32) -> Option<u32> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &s[s.rfind(')')? + 2..]; // after "(comm) "
    rest.split_whitespace().nth(2)?.parse().ok() // state ppid pgrp
}

/// SIGTERM the process group of the running game's `reaper`.
pub fn kill_running_game() -> Result<()> {
    let (pid, _) = reaper_pid_and_app().ok_or_else(|| Error::new(ErrorKind::NotFound, "no game running"))?;
    if deck_system::guard::deny(&format!("kill game pid {pid}")) { return Ok(()); }
    let target = match pgid_of(pid) { Some(g) if g > 1 => format!("-{g}"), _ => pid.to_string() };
    let st = Command::new("kill").args(["-TERM", "--", &target]).stdin(Stdio::null()).status()?;
    if st.success() { Ok(()) } else { Err(Error::new(ErrorKind::Other, format!("kill {target} failed"))) }
}

// ------------------------------------------------------------ downloads

#[derive(Debug, Clone)]
pub struct Download { pub app_id: u32, pub name: String, pub downloaded: u64, pub total: u64, pub paused: bool }

/// Top-level `"key"  "value"` fields of an ACF file (nested blocks are skipped).
fn acf_fields(text: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    let mut depth = 0i32;
    for line in text.lines() {
        let l = line.trim();
        if l == "{" { depth += 1; continue; }
        if l == "}" { depth -= 1; continue; }
        if depth != 1 { continue; }
        let q: Vec<&str> = l.split('"').collect(); // ["", k, "\t", v, ""]
        if q.len() >= 4 { m.insert(q[1].to_string(), q[3].to_string()); }
    }
    m
}

fn library_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v = vec![root.join("steamapps")];
    if let Ok(t) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        for l in t.lines() {
            let q: Vec<&str> = l.trim().split('"').collect();
            if q.len() >= 4 && q[1] == "path" {
                let d = Path::new(&q[3].replace("\\\\", "\\")).join("steamapps");
                if !v.contains(&d) { v.push(d); }
            }
        }
    }
    v
}

// StateFlags bits
const UPDATE_REQUIRED: u32 = 2;
const FULLY_INSTALLED: u32 = 4;
const UPDATE_RUNNING: u32 = 256;
const UPDATE_PAUSED: u32 = 512;
const UPDATE_STARTED: u32 = 1024;
const UNINSTALLING: u32 = 2048;

pub fn downloads(lib: &steam_data::Library) -> Vec<Download> {
    let mut out = Vec::new();
    for dir in library_dirs(&lib.steam_root) {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name();
            let fname = fname.to_string_lossy();
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) { continue; }
            let Ok(text) = fs::read_to_string(e.path()) else { continue };
            let f = acf_fields(&text);
            let num = |k: &str| f.get(k).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let Some(app_id) = f.get("appid").and_then(|v| v.parse::<u32>().ok()) else { continue };
            let flags = num("StateFlags") as u32;
            if flags & UNINSTALLING != 0 { continue; }
            let in_dl_dir = dir.join("downloading").join(app_id.to_string()).is_dir();
            let active = flags & (UPDATE_RUNNING | UPDATE_PAUSED | UPDATE_STARTED) != 0
                || (flags & UPDATE_REQUIRED != 0 && flags & FULLY_INSTALLED == 0)
                || in_dl_dir;
            if !active { continue; }
            let total = num("BytesToDownload");
            let downloaded = num("BytesDownloaded").min(if total > 0 { total } else { u64::MAX });
            let name = f.get("name").cloned()
                .or_else(|| lib.games.iter().find(|g| g.app_id == app_id).map(|g| g.name.clone()))
                .unwrap_or_else(|| app_id.to_string());
            out.push(Download { app_id, name, downloaded, total, paused: flags & UPDATE_PAUSED != 0 });
        }
    }
    out.sort_by_key(|d| d.app_id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls() {
        assert_eq!(url_for(&SteamPage::Store), "steam://store");
        assert_eq!(url_for(&SteamPage::StoreApp(220)), "steam://store/220");
        assert_eq!(url_for(&SteamPage::Friends), "steam://open/friends");
        assert_eq!(url_for(&SteamPage::Downloads), "steam://open/downloads");
        assert_eq!(url_for(&SteamPage::Settings), "steam://open/settings");
        assert_eq!(url_for(&SteamPage::Achievements(7)), "steam://url/SteamIDAchievementsPage/7");
        assert_eq!(url_for(&SteamPage::Properties(7)), "steam://gameproperties/7");
        assert_eq!(url_for(&SteamPage::Controller(7)), "steam://controllerconfig/7");
        assert_eq!(url_for(&SteamPage::Screenshots), "steam://open/screenshots");
        assert_eq!(url_for(&SteamPage::Keyboard), "steam://open/keyboard");
    }
    #[test]
    fn reaper() {
        let c = b"/home/deck/.steam/root/ubuntu12_32/reaper\0SteamLaunch\0AppId=227300\0--\0/bin/true\0";
        assert_eq!(parse_reaper_cmdline(c), Some(227300));
        assert_eq!(parse_reaper_cmdline(b"/usr/bin/bash\0SteamLaunch\0AppId=1\0"), None);
    }
    #[test]
    fn acf() {
        let t = "\"AppState\"\n{\n\t\"appid\"\t\t\"5\"\n\t\"StateFlags\"\t\t\"1026\"\n\t\"Depots\"\n\t{\n\t\t\"x\"\t\t\"1\"\n\t}\n}\n";
        let f = acf_fields(t);
        assert_eq!(f["appid"], "5");
        assert_eq!(f["StateFlags"], "1026");
        assert!(!f.contains_key("x"));
    }
}
