//! Reads the local Steam installation (no network, no CEF).
//! CONTRACT shared with `app` — keep these public types stable; add fields freely.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub mod appinfo;
pub mod art;
pub mod json;
pub mod vdf;

use vdf::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameKind { Steam, Shortcut }

#[derive(Debug, Clone, Default)]
pub struct Art {
    /// Portrait capsule 600x900 (library grid).
    pub capsule: Option<PathBuf>,
    /// Animated capsule (webp/apng/gif/webm/mp4), e.g. from SteamGridDB / grid folder.
    pub capsule_animated: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub header: Option<PathBuf>,
    pub icon: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Game {
    /// Steam appid, or shortcut appid (u32) for non-Steam games.
    pub app_id: u32,
    pub kind: GameKind,
    pub name: String,
    /// Lowercase name without "The"/punctuation, for sorting/search.
    pub sort_name: String,
    pub installed: bool,
    pub install_dir: Option<PathBuf>,
    pub size_on_disk: u64,
    pub playtime_minutes: u32,
    pub last_played: Option<u64>, // unix seconds
    pub update_pending: bool,
    pub collections: Vec<String>,
    pub art: Art,
}

impl Game {
    /// The id used in `steam://rungameid/<id>`.
    pub fn rungame_id(&self) -> u64 {
        match self.kind {
            GameKind::Steam => self.app_id as u64,
            GameKind::Shortcut => ((self.app_id as u64) << 32) | 0x0200_0000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub steam_id64: u64,
    pub account_id: u32, // userdata folder name
    pub account_name: String,
    pub persona_name: String,
    pub most_recent: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub steam_root: PathBuf,
    pub users: Vec<User>,
    pub active_user: Option<u32>,
    pub games: Vec<Game>,
    pub collections: Vec<String>,
}

#[derive(Debug)]
pub enum Error { NoSteam, Io(std::io::Error), Parse(String) }
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for Error {}

const STEAMID64_BASE: u64 = 76561197960265728;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

/// Default root: $STEAM_ROOT or ~/.local/share/Steam (or ~/.steam/steam).
pub fn default_root() -> Option<PathBuf> {
    if let Some(r) = std::env::var_os("STEAM_ROOT") {
        let p = PathBuf::from(r);
        if p.is_dir() {
            return Some(p);
        }
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [home.join(".local/share/Steam"), home.join(".steam/steam")].into_iter().find(|p| p.is_dir())
}

/// Lowercase name without leading "the", punctuation removed, whitespace collapsed.
pub fn sort_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut s = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_alphanumeric() {
            s.push(c);
        } else if c.is_whitespace() && !s.ends_with(' ') && !s.is_empty() {
            s.push(' ');
        }
    }
    let s = s.trim();
    let s = s.strip_prefix("the ").unwrap_or(s).trim();
    if s.is_empty() { lower.trim().to_owned() } else { s.to_owned() }
}

fn read_vdf(p: &Path) -> Option<Value> {
    let bytes = std::fs::read(p).ok()?;
    let s = String::from_utf8_lossy(&bytes);
    vdf::parse_text(&s).ok()
}

/// Runtimes, compatibility tools and redistributables that are not games.
fn is_tool(id: u32, name: &str) -> bool {
    const IDS: &[u32] = &[
        228980, 250820, 1070560, 1391110, 1628350, 1493710, 1113280, 1245040, 1161040, 1420170, 1580130,
        1826330, 1887720, 2180100, 2348590, 2805730, 3658110, 4183110, 4628710,
    ];
    if IDS.contains(&id) {
        return true;
    }
    let n = name.to_ascii_lowercase();
    n.starts_with("proton")
        || n.starts_with("steam linux runtime")
        || n.starts_with("steamworks common")
        || n.starts_with("steam controller configs")
        || n.starts_with("steamvr")
        || n.contains("proton easyanticheat")
        || n.contains("proton battleye")
        || n == "steam deck"
}

struct Manifest {
    app_id: u32,
    name: String,
    install_dir: PathBuf,
    size: u64,
    flags: u64,
    last_played: u64,
}

fn read_manifests(libs: &[PathBuf]) -> Vec<Manifest> {
    let mut out = Vec::new();
    for lib in libs {
        let steamapps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&steamapps) else { continue };
        for e in rd.flatten() {
            let Ok(fname) = e.file_name().into_string() else { continue };
            if !(fname.starts_with("appmanifest_") && fname.ends_with(".acf")) {
                continue;
            }
            let Some(v) = read_vdf(&e.path()) else { continue };
            let Some(st) = v.get("AppState") else { continue };
            let Some(app_id) = st.u64_at("appid").and_then(|n| u32::try_from(n).ok()) else { continue };
            let name = st.str_at("name").unwrap_or("").trim().to_owned();
            if name.is_empty() || is_tool(app_id, &name) {
                continue;
            }
            out.push(Manifest {
                app_id,
                name,
                install_dir: steamapps.join("common").join(st.str_at("installdir").unwrap_or("")),
                size: st.u64_at("SizeOnDisk").unwrap_or(0),
                flags: st.u64_at("StateFlags").unwrap_or(0),
                last_played: st.u64_at("LastPlayed").unwrap_or(0),
            });
        }
    }
    out
}

fn library_paths(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    let v = read_vdf(&root.join("config/libraryfolders.vdf")).or_else(|| read_vdf(&root.join("steamapps/libraryfolders.vdf")));
    if let Some(v) = v {
        if let Some(m) = v.get("libraryfolders").and_then(|v| v.as_map()) {
            for (k, e) in m {
                if !k.bytes().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let path = match e {
                    Value::Str(s) => Some(s.as_str()), // old format
                    Value::Map(_) => e.str_at("path"),
                };
                if let Some(p) = path {
                    let p = PathBuf::from(p);
                    if !libs.contains(&p) && p.is_dir() {
                        libs.push(p);
                    }
                }
            }
        }
    }
    libs
}

fn read_users(root: &Path) -> Vec<User> {
    let mut users = Vec::new();
    if let Some(v) = read_vdf(&root.join("config/loginusers.vdf")) {
        if let Some(m) = v.get("users").and_then(|v| v.as_map()) {
            for (k, u) in m {
                let Ok(id) = k.parse::<u64>() else { continue };
                if id < STEAMID64_BASE {
                    continue;
                }
                users.push(User {
                    steam_id64: id,
                    account_id: (id - STEAMID64_BASE) as u32,
                    account_name: u.str_at("AccountName").unwrap_or("").to_owned(),
                    persona_name: u.str_at("PersonaName").unwrap_or("").to_owned(),
                    most_recent: u.str_at("MostRecent").map(|s| s.trim() == "1").unwrap_or(false),
                });
            }
        }
    }
    users
}

fn pick_active(root: &Path, users: &[User]) -> Option<u32> {
    let ud = root.join("userdata");
    if let Some(u) = users.iter().find(|u| u.most_recent && ud.join(u.account_id.to_string()).is_dir()) {
        return Some(u.account_id);
    }
    let mut best: Option<(std::time::SystemTime, u32)> = None;
    for e in std::fs::read_dir(&ud).ok()?.flatten() {
        let Some(id) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if id == 0 {
            continue;
        }
        let Ok(m) = std::fs::metadata(e.path().join("config/localconfig.vdf")).and_then(|m| m.modified()) else { continue };
        if best.map_or(true, |(t, _)| m > t) {
            best = Some((m, id));
        }
    }
    best.map(|(_, id)| id)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn strip_quotes(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Parse a (possibly negative) app id key from localconfig into the u32 form.
fn key_to_id(k: &str) -> Option<u32> {
    let n: i64 = k.parse().ok()?;
    if n >= 0 { u32::try_from(n).ok() } else { i32::try_from(n).ok().map(|v| v as u32) }
}

/// Load everything. Must be fast (<150 ms on Deck for ~500 games).
pub fn load(root: &Path) -> Result<Library, Error> {
    if !root.join("steamapps").is_dir() && !root.join("config").is_dir() {
        return Err(Error::NoSteam);
    }
    let libs = library_paths(root);
    let manifests = read_manifests(&libs);
    let users = read_users(root);
    let active = pick_active(root, &users);

    let user_cfg = active.map(|id| root.join("userdata").join(id.to_string()).join("config"));

    // per-app playtime / last played
    let mut played: HashMap<u32, (u32, u64)> = HashMap::new();
    if let Some(cfg) = &user_cfg {
        if let Some(v) = read_vdf(&cfg.join("localconfig.vdf")) {
            let apps = v
                .path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"])
                .or_else(|| v.path(&["UserLocalConfigStore", "Software", "Valve", "Steam", "Apps"]));
            if let Some(m) = apps.and_then(|a| a.as_map()) {
                for (k, a) in m {
                    let Some(id) = key_to_id(k) else { continue };
                    let pt = a.u64_at("Playtime").unwrap_or(0).min(u32::MAX as u64) as u32;
                    let lp = a.u64_at("LastPlayed").unwrap_or(0);
                    if pt > 0 || lp > 0 {
                        played.insert(id, (pt, lp));
                    }
                }
            }
        }
    }

    // collections
    let mut collections: Vec<String> = Vec::new();
    let mut membership: HashMap<u32, Vec<String>> = HashMap::new();
    if let Some(cfg) = &user_cfg {
        if let Ok(text) = std::fs::read_to_string(cfg.join("cloudstorage/cloud-storage-namespace-1.json")) {
            if let Some(j) = json::parse(&text) {
                for entry in j.as_arr().unwrap_or(&[]) {
                    let Some(pair) = entry.as_arr() else { continue };
                    let (Some(key), Some(obj)) = (pair.first().and_then(|k| k.as_str()), pair.get(1)) else { continue };
                    if !key.starts_with("user-collections.") {
                        continue;
                    }
                    if matches!(obj.get("is_deleted"), Some(json::Json::Bool(true))) {
                        continue;
                    }
                    let Some(val) = obj.get("value").and_then(|v| v.as_str()).and_then(json::parse) else { continue };
                    let Some(name) = val.get("name").and_then(|n| n.as_str()) else { continue };
                    collections.push(name.to_owned());
                    for id in val.get("added").and_then(|a| a.as_arr()).unwrap_or(&[]) {
                        if let Some(n) = id.as_f64() {
                            if n >= 0.0 && n <= u32::MAX as f64 {
                                membership.entry(n as u32).or_default().push(name.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    collections.sort_by_key(|n| sort_name(n));
    collections.dedup();

    let cache = root.join("appcache/librarycache");
    let grid = user_cfg.as_ref().map(|c| art::Grid::scan(&c.join("grid"))).unwrap_or_default();
    let mk_art = |ids: &[String]| {
        let base = art::librarycache(&cache, &ids[0]);
        art::merge(base, grid.overrides(ids))
    };

    let mut games: HashMap<(bool, u32), Game> = HashMap::new();
    let mut put = |g: Game| {
        let key = (g.kind == GameKind::Shortcut, g.app_id);
        match games.get(&key) {
            Some(old) if old.installed && !g.installed => {}
            _ => {
                games.insert(key, g);
            }
        }
    };

    let mut installed_ids: HashSet<u32> = HashSet::new();
    for m in &manifests {
        installed_ids.insert(m.app_id);
        let (pt, lp) = played.get(&m.app_id).copied().unwrap_or((0, 0));
        let last = lp.max(m.last_played);
        put(Game {
            app_id: m.app_id,
            kind: GameKind::Steam,
            sort_name: sort_name(&m.name),
            name: m.name.clone(),
            installed: m.flags & 4 != 0,
            install_dir: Some(m.install_dir.clone()),
            size_on_disk: m.size,
            playtime_minutes: pt,
            last_played: (last > 0).then_some(last),
            update_pending: m.flags & (2 | 256 | 512 | 1024) != 0,
            collections: membership.get(&m.app_id).cloned().unwrap_or_default(),
            art: mk_art(&[m.app_id.to_string()]),
        });
    }

    // owned but not installed (played)
    let missing: HashSet<u32> = played.keys().copied().filter(|id| *id > 0 && !installed_ids.contains(id) && (*id as i32) > 0).collect();
    if !missing.is_empty() {
        let info = appinfo::lookup(&root.join("appcache/appinfo.vdf"), &missing);
        for (id, inf) in info {
            if inf.kind != "game" || is_tool(id, &inf.name) {
                continue;
            }
            let (pt, lp) = played[&id];
            put(Game {
                app_id: id,
                kind: GameKind::Steam,
                sort_name: sort_name(&inf.name),
                name: inf.name,
                installed: false,
                install_dir: None,
                size_on_disk: 0,
                playtime_minutes: pt,
                last_played: (lp > 0).then_some(lp),
                update_pending: false,
                collections: membership.get(&id).cloned().unwrap_or_default(),
                art: mk_art(&[id.to_string()]),
            });
        }
    }

    // non-Steam shortcuts
    if let Some(cfg) = &user_cfg {
        if let Ok(data) = std::fs::read(cfg.join("shortcuts.vdf")) {
            let v = vdf::parse_binary(&data, None);
            if let Some(m) = v.get("shortcuts").and_then(|s| s.as_map()) {
                for (_, e) in m {
                    let name = e.str_at("AppName").unwrap_or("").trim().to_owned();
                    if name.is_empty() {
                        continue;
                    }
                    let exe = e.str_at("Exe").unwrap_or("");
                    let id = e
                        .u64_at("appid")
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or_else(|| crc32(format!("{exe}{name}").as_bytes()) | 0x8000_0000);
                    let long = (((id as u64) << 32) | 0x0200_0000).to_string();
                    let (pt, lp) = played.get(&id).copied().unwrap_or((0, 0));
                    let lpt = e.u64_at("LastPlayTime").unwrap_or(0).max(lp);
                    let start = strip_quotes(e.str_at("StartDir").unwrap_or(""));
                    let mut art = mk_art(&[id.to_string(), long]);
                    if art.icon.is_none() {
                        let ic = strip_quotes(e.str_at("icon").unwrap_or(""));
                        if !ic.is_empty() && Path::new(ic).is_file() {
                            art.icon = Some(PathBuf::from(ic));
                        }
                    }
                    put(Game {
                        app_id: id,
                        kind: GameKind::Shortcut,
                        sort_name: sort_name(&name),
                        name,
                        installed: true,
                        install_dir: (!start.is_empty()).then(|| PathBuf::from(start)),
                        size_on_disk: 0,
                        playtime_minutes: pt,
                        last_played: (lpt > 0).then_some(lpt),
                        update_pending: false,
                        collections: membership.get(&id).cloned().unwrap_or_default(),
                        art,
                    });
                }
            }
        }
    }

    let mut games: Vec<Game> = games.into_values().collect();
    games.sort_by(|a, b| a.sort_name.cmp(&b.sort_name).then(a.app_id.cmp(&b.app_id)));

    Ok(Library { steam_root: root.to_path_buf(), users, active_user: active, games, collections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sort_names() {
        assert_eq!(sort_name("The Witcher® 3: Wild Hunt"), "witcher 3 wild hunt");
        assert_eq!(sort_name("Half-Life"), "halflife");
        assert_eq!(sort_name("!!!"), "!!!");
    }

    #[test]
    fn rungame_shortcut() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn end_to_end_fixture() {
        let dir = std::env::temp_dir().join(format!("steam-data-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let w = |p: &str, c: &[u8]| {
            let p = dir.join(p);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        };
        let sd = dir.join("sdcard");
        fs::create_dir_all(sd.join("steamapps")).unwrap();
        w("config/libraryfolders.vdf", format!("\"libraryfolders\"{{\"0\"{{\"path\" \"{}\"}}\"1\"{{\"path\" \"{}\"}}}}", dir.display(), sd.display()).as_bytes());
        w("steamapps/appmanifest_10.acf", b"\"AppState\"{\"appid\" \"10\" \"name\" \"The Game\" \"StateFlags\" \"4\" \"installdir\" \"g\" \"SizeOnDisk\" \"99\"}");
        w("steamapps/appmanifest_228980.acf", b"\"AppState\"{\"appid\" \"228980\" \"name\" \"Steamworks Common Redistributables\" \"StateFlags\" \"4\"}");
        fs::write(sd.join("steamapps/appmanifest_20.acf"), b"\"AppState\"{\"appid\" \"20\" \"name\" \"Alpha\" \"StateFlags\" \"6\" \"LastPlayed\" \"5\"}").unwrap();
        w("config/loginusers.vdf", b"\"users\"{\"76561198960265729\"{\"AccountName\" \"a\" \"PersonaName\" \"A\" \"MostRecent\" \"1\"}}");
        let uid = 76561198960265729u64 - STEAMID64_BASE;
        let cfg = format!("userdata/{uid}/config");
        w(&format!("{cfg}/localconfig.vdf"), b"\"UserLocalConfigStore\"{\"Software\"{\"Valve\"{\"Steam\"{\"apps\"{\"10\"{\"Playtime\" \"42\" \"LastPlayed\" \"100\"}}}}}}");
        w(&format!("{cfg}/cloudstorage/cloud-storage-namespace-1.json"), br#"[["user-collections.fav",{"key":"user-collections.fav","value":"{\"id\":\"fav\",\"name\":\"Fav\",\"added\":[10]}"}]]"#);
        w(&format!("{cfg}/grid/10p.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR0000000000000\0\0\0\x08acTL00000000\0\0\0\0IDAT");
        w(&format!("{cfg}/grid/10_hero.jpg"), b"x");
        w("appcache/librarycache/10/library_600x900.jpg", b"x");
        w("appcache/librarycache/10/logo.png", b"x");
        w("appcache/librarycache/20_library_600x900.jpg", b"x");

        let lib = load(&dir).unwrap();
        assert_eq!(lib.active_user, Some(uid as u32));
        assert_eq!(lib.games.len(), 2);
        assert_eq!(lib.games[0].name, "Alpha");
        assert!(lib.games[0].update_pending && lib.games[0].installed);
        assert!(lib.games[0].art.capsule.is_some());
        let g = &lib.games[1];
        assert_eq!((g.app_id, g.playtime_minutes, g.last_played, g.size_on_disk), (10, 42, Some(100), 99));
        assert_eq!(g.collections, vec!["Fav".to_string()]);
        assert_eq!(lib.collections, vec!["Fav".to_string()]);
        assert!(g.art.capsule.as_ref().unwrap().ends_with("grid/10p.png"));
        assert_eq!(g.art.capsule_animated, g.art.capsule);
        assert!(g.art.hero.is_some() && g.art.logo.is_some());
        let _ = fs::remove_dir_all(&dir);
    }
}
