//! SteamGridDB API v2 client (curl, like the rest of DeckShell's network code), filter model and query builder.
//! The query string is a 1:1 port of the Decky plugin's `getApiParams` (hooks/useSGDB.tsx), constants from constants.ts.
//!
//! API key: the plugin ships a private key that its README says must not be reused ("will cause you to be automatically banned"),
//! so DeckShell never touches it. The user's own key (https://www.steamgriddb.com/profile/preferences/api) comes from
//! `DECKSHELL_SGDB_KEY` or `~/.config/deckshell/sgdb.json` {"api_key": "..."}; without one only the offline cache works.
use steam_assets::i18n::{tr_args, tr_ref};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use super::cache;

pub const DEFAULT_BASE: &str = "https://www.steamgriddb.com/api/v2";
/// Cached API answers younger than this are used without a request (saves quota); older ones only when offline.
const FRESH_SECS: u64 = 600;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AType {
    Capsule,
    Wide,
    Hero,
    Logo,
    Icon,
}

impl AType {
    pub const ALL: [AType; 5] = [AType::Capsule, AType::Wide, AType::Hero, AType::Logo, AType::Icon];
    /// eAssetType of SteamClient.Apps.SetCustomArtworkForApp.
    pub fn e_asset(self) -> i32 {
        match self {
            AType::Capsule => 0,
            AType::Hero => 1,
            AType::Logo => 2,
            AType::Wide => 3,
            AType::Icon => 4,
        }
    }
    /// Plugin setting / SGDBAssetType name.
    pub fn key(self) -> &'static str {
        match self {
            AType::Capsule => "grid_p",
            AType::Wide => "grid_l",
            AType::Hero => "hero",
            AType::Logo => "logo",
            AType::Icon => "icon",
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            AType::Capsule | AType::Wide => "grids",
            AType::Hero => "heroes",
            AType::Logo => "logos",
            AType::Icon => "icons",
        }
    }
    /// Russian plugin labels (LABEL_TAB_*).
    pub fn label(self) -> &'static str {
        match self {
            AType::Capsule => tr_ref("sgdb.capsule"),
            AType::Wide => tr_ref("sgdb.wide"),
            AType::Hero => tr_ref("sgdb.hero"),
            AType::Logo => tr_ref("sgdb.logo"),
            AType::Icon => tr_ref("sgdb.icon"),
        }
    }
    /// File name part in userdata/<acct>/config/grid/<appid><suffix>.<ext>.
    pub fn suffix(self) -> &'static str {
        match self {
            AType::Capsule => "p",
            AType::Wide => "",
            AType::Hero => "_hero",
            AType::Logo => "_logo",
            AType::Icon => "_icon",
        }
    }
    /// cell height / width of the result grid
    pub fn ratio(self) -> f32 {
        match self {
            AType::Capsule => 1.5,
            AType::Wide => 430.0 / 920.0,
            AType::Hero => 620.0 / 1920.0,
            AType::Logo => 0.5,
            AType::Icon => 1.0,
        }
    }
    /// Client-side tab split of the shared `grids` endpoint when the dimension filter is not sent: portrait -> capsule,
    /// landscape -> wide, squares in both (both tabs offer 512x512 / 1024x1024).
    pub fn fits_tab(self, a: &Asset) -> bool {
        match self {
            AType::Capsule => a.height >= a.width,
            AType::Wide => a.width >= a.height,
            _ => true,
        }
    }
    pub fn cols(self) -> usize {
        match self {
            AType::Capsule => 4,
            AType::Wide => 3,
            AType::Hero => 2,
            AType::Logo => 3,
            AType::Icon => 6,
        }
    }
    pub fn contain(self) -> bool {
        matches!(self, AType::Logo | AType::Icon)
    }
    /// (value, label) options of the filter dialog.
    pub fn style_opts(self) -> Vec<(&'static str, &'static str)> {
        match self {
            AType::Capsule | AType::Wide => vec![("alternate", tr_ref("sgdb.alternative")), ("white_logo", tr_ref("sgdb.white_logo")), ("no_logo", tr_ref("sgdb.no_logo")), ("blurred", tr_ref("sgdb.blurred")), ("material", tr_ref("sgdb.minimal"))],
            AType::Hero => vec![("alternate", tr_ref("sgdb.alternative")), ("blurred", tr_ref("sgdb.blurred")), ("material", tr_ref("sgdb.minimal"))],
            AType::Logo => vec![("official", tr_ref("sgdb.official")), ("white", tr_ref("sgdb.white")), ("black", tr_ref("sgdb.black")), ("custom", tr_ref("sgdb.custom"))],
            AType::Icon => vec![("official", tr_ref("sgdb.official")), ("custom", tr_ref("sgdb.custom"))],
        }
    }
    pub fn dim_opts(self) -> Vec<String> {
        const ICON: [u32; 37] = [1024, 768, 512, 310, 256, 194, 192, 180, 160, 152, 150, 144, 128, 120, 114, 100, 96, 90, 80, 76, 72, 64, 60, 57, 56, 54, 48, 40, 35, 32, 28, 24, 20, 16, 14, 10, 8];
        match self {
            AType::Capsule => ["600x900", "342x482", "660x930", "512x512", "1024x1024"].map(String::from).to_vec(),
            AType::Wide => ["460x215", "920x430", "512x512", "1024x1024"].map(String::from).to_vec(),
            AType::Hero => ["1920x620", "3840x1240", "1600x650"].map(String::from).to_vec(),
            AType::Logo => vec![],
            AType::Icon => ICON.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn default_dims(self) -> Vec<String> {
        match self {
            AType::Capsule => ["600x900", "342x482", "660x930"].map(String::from).to_vec(),
            AType::Wide => ["460x215", "920x430"].map(String::from).to_vec(),
            AType::Hero | AType::Icon => self.dim_opts(),
            AType::Logo => vec![],
        }
    }
    pub fn mime_opts(self) -> Vec<(&'static str, &'static str)> {
        match self {
            AType::Capsule | AType::Wide | AType::Hero => vec![("image/png", "PNG"), ("image/jpeg", "JPEG"), ("image/webp", "WebP")],
            AType::Logo => vec![("image/png", "PNG"), ("image/webp", "WebP")],
            AType::Icon => vec![("image/png", "PNG"), ("image/vnd.microsoft.icon", "ICO")],
        }
    }
}

/// User-visible filters of one art type (plugin settings `filters_<type>`).
#[derive(Clone, PartialEq, Debug)]
pub struct Filters {
    pub styles: Vec<String>,
    pub dimensions: Vec<String>,
    pub mimes: Vec<String>,
    pub animated: bool,
    pub stat: bool,
    pub adult: bool,
    pub humor: bool,
    pub epilepsy: bool,
    pub untagged: bool,
}

impl Filters {
    pub fn default_for(t: AType) -> Filters {
        Filters {
            styles: t.style_opts().iter().map(|s| s.0.to_string()).collect(),
            dimensions: t.default_dims(),
            mimes: t.mime_opts().iter().map(|s| s.0.to_string()).collect(),
            animated: true,
            stat: true,
            adult: false,
            humor: true,
            epilepsy: true,
            untagged: true,
        }
    }
    /// Missing keys fall back to defaults. Plugin keys: styles, dimensions, mimes, animated, _static, adult, humor, epilepsy, untagged.
    pub fn from_json(t: AType, v: &Value) -> Filters {
        let mut f = Filters::default_for(t);
        let list = |k: &str| -> Option<Vec<String>> {
            v.get(k)?.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from).or_else(|| x.as_u64().map(|n| n.to_string()))).collect())
        };
        if let Some(x) = list("styles") {
            f.styles = x;
        }
        if let Some(x) = list("dimensions") {
            f.dimensions = x;
        }
        if let Some(x) = list("mimes") {
            f.mimes = x;
        }
        for (k, slot) in [("animated", &mut f.animated), ("_static", &mut f.stat), ("adult", &mut f.adult), ("humor", &mut f.humor), ("epilepsy", &mut f.epilepsy), ("untagged", &mut f.untagged)] {
            if let Some(b) = v.get(k).and_then(Value::as_bool) {
                *slot = b;
            }
        }
        f
    }
    /// Only animated assets are asked for (the "static" box is off).
    pub fn animated_only(&self) -> bool {
        self.animated && !self.stat
    }
    pub fn to_json(&self) -> Value {
        json!({"styles": self.styles, "dimensions": self.dimensions, "mimes": self.mimes, "animated": self.animated, "_static": self.stat,
               "adult": self.adult, "humor": self.humor, "epilepsy": self.epilepsy, "untagged": self.untagged})
    }
}

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'*') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Port of getApiParams(): the nsfw/humor/epilepsy tri-state logic and `oneoftag` for "untagged off".
pub fn query(t: AType, f: &Filters, page: u32) -> String {
    let (mut adult, mut humor, mut epilepsy, mut oneoftag) = ("false", "any", "any", String::new());
    if f.untagged {
        if !f.humor {
            humor = "false";
        }
        if !f.adult {
            adult = "false";
        } else {
            adult = "any";
        }
        if !f.epilepsy {
            epilepsy = "false";
        }
    } else {
        let mut tags = vec![];
        if f.humor {
            humor = "any";
            tags.push("humor");
        }
        if f.adult {
            adult = "any";
            tags.push("nsfw");
        }
        if f.epilepsy {
            epilepsy = "any";
            tags.push("epilepsy");
        }
        oneoftag = tags.join(",");
    }
    let types = [f.stat.then_some("static"), f.animated.then_some("animated")].into_iter().flatten().collect::<Vec<_>>().join(",");
    let _ = t;
    // "Animated only": every animated asset must show up. `mimes` is about static formats (an APNG is image/png, a webm-thumbed
    // WebP image/webp: deselecting PNG/JPEG would drop them) and `dimensions` hides odd-sized animations, so both are left out
    // (capsule / wide are told apart client side, see `Filters::animated_only` + `fits_tab`).
    let (dims, mimes) = if f.animated_only() { (String::new(), String::new()) } else { (f.dimensions.join(","), f.mimes.join(",")) };
    [
        ("page", page.to_string()),
        ("styles", f.styles.join(",")),
        ("dimensions", dims),
        ("mimes", mimes),
        ("nsfw", adult.into()),
        ("humor", humor.into()),
        ("epilepsy", epilepsy.into()),
        ("oneoftag", oneoftag),
        ("types", types),
    ]
    .iter()
    .filter(|(k, v)| !(v.is_empty() && matches!(*k, "dimensions" | "mimes") && f.animated_only()))
    .map(|(k, v)| format!("{k}={}", pct(v)))
    .collect::<Vec<_>>()
    .join("&")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asset {
    pub id: u64,
    pub url: String,
    pub thumb: String,
    pub width: u32,
    pub height: u32,
    pub nsfw: bool,
    pub humor: bool,
    pub epilepsy: bool,
    pub notes: String,
    pub author: String,
    pub score: i64,
    pub mime: String,
    pub style: String,
    animation: Option<bool>,
}

impl Asset {
    pub fn from_json(v: &Value) -> Option<Asset> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let url = s("url");
        if url.is_empty() {
            return None;
        }
        Some(Asset {
            id: v.get("id").and_then(Value::as_u64)?,
            thumb: if s("thumb").is_empty() { url.clone() } else { s("thumb") },
            url,
            width: v.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
            height: v.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
            nsfw: b("nsfw"),
            humor: b("humor"),
            epilepsy: b("epilepsy"),
            notes: s("notes"),
            author: v.pointer("/author/name").and_then(Value::as_str).unwrap_or("").to_string(),
            score: v.get("score").and_then(Value::as_i64).unwrap_or(0),
            mime: s("mime"),
            style: s("style"),
            animation: None,
        })
    }
    pub fn ext_of(url: &str) -> String {
        let p = url.split(['?', '#']).next().unwrap_or("");
        p.rsplit('.').next().filter(|e| e.len() <= 5 && !e.contains('/')).unwrap_or("png").to_ascii_lowercase()
    }
    /// Classification comes from the SGDB types query or decoded cached full image.
    pub fn animated(&self) -> bool {
        self.animation.or_else(|| std::fs::read(cache::full_path(&self.url)).ok().and_then(|b| animation_from_content(&b))).unwrap_or(false)
    }
    pub fn format_tag(&self) -> String {
        match Self::ext_of(&self.url).as_str() {
            "webm" => "WEBM".into(),
            "apng" => "APNG".into(),
            "webp" => "WebP".into(),
            "jpg" | "jpeg" => "JPEG".into(),
            "ico" => "ICO".into(),
            _ => "PNG".into(),
        }
    }
    pub fn flags(&self) -> i32 {
        self.animated() as i32 | (self.nsfw as i32) << 1 | (self.humor as i32) << 2 | (self.epilepsy as i32) << 3 | (!self.notes.is_empty() as i32) << 4
    }
}

// Unknown or malformed data stays unknown; filenames are not evidence of animation.
fn animation_from_content(bytes: &[u8]) -> Option<bool> {
    let reader = std::io::Cursor::new(bytes);
    match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => image::codecs::png::PngDecoder::new(reader).ok()?.is_apng().ok(),
        image::ImageFormat::WebP => Some(image::codecs::webp::WebPDecoder::new(reader).ok()?.has_animation()),
        _ => None,
    }
}

#[derive(Debug)]
pub enum ApiErr {
    NoKey,
    NotFound,
    Http(u16, String),
    Net(String),
}

impl std::fmt::Display for ApiErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiErr::NoKey => f.write_str(tr_ref("sgdb.api_key_missing")),
            ApiErr::NotFound => f.write_str(tr_ref("sgdb.api_game_missing")),
            ApiErr::Http(c, m) => write!(f, "SteamGridDB {c}: {m}"),
            ApiErr::Net(m) => write!(f, "{}", tr_args("sgdb.network_error", &[("m", &m.to_string())])),
        }
    }
}

// ------------------------------------------------------------------ user config (~/.config/deckshell/sgdb.json) + read-only plugin settings
pub fn config_path() -> PathBuf {
    std::env::var_os("DECKSHELL_SGDB_CONFIG").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("deckshell/sgdb.json")
    })
}
fn plugin_settings_path() -> PathBuf {
    std::env::var_os("DECKSHELL_SGDB_PLUGIN_SETTINGS").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("homebrew/settings/decky-steamgriddb/steamgriddb.json")
    })
}

pub fn read_json(p: &Path) -> Value {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

#[derive(Default, Clone)]
pub struct Settings {
    pub own: Value,
    plugin: Value,
}

impl Settings {
    pub fn load() -> Settings {
        let own = read_json(&config_path());
        Settings { own: if own.is_object() { own } else { json!({}) }, plugin: read_json(&plugin_settings_path()) }
    }
    pub fn save(&self) {
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_vec_pretty(&self.own) {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, &p);
            }
        }
    }
    pub fn api_key(&self) -> Option<String> {
        std::env::var("DECKSHELL_SGDB_KEY").ok().filter(|k| !k.trim().is_empty()).or_else(|| self.own.get("api_key").and_then(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
    }
    pub fn set_api_key(&mut self, k: &str) {
        self.own["api_key"] = json!(k.trim());
        self.save();
    }
    pub fn base(&self) -> String {
        std::env::var("DECKSHELL_SGDB_API").unwrap_or_else(|_| DEFAULT_BASE.to_string())
    }
    /// Own filters first, then the plugin's saved ones (read-only seed), then defaults.
    pub fn filters(&self, t: AType) -> Filters {
        let k = format!("filters_{}", t.key());
        let v = self.own.get("filters").and_then(|f| f.get(t.key())).or_else(|| self.plugin.get(&k));
        v.map(|v| Filters::from_json(t, v)).unwrap_or_else(|| Filters::default_for(t))
    }
    pub fn set_filters(&mut self, t: AType, f: &Filters) {
        self.own["filters"][t.key()] = f.to_json();
        self.save();
    }
    /// Chosen SGDB game for an app (plugin `nonsteam_<appid>`: {id, name}).
    pub fn game_for(&self, appid: u32) -> Option<(u64, String)> {
        let v = self.own.get("games").and_then(|g| g.get(appid.to_string())).or_else(|| self.plugin.get(format!("nonsteam_{appid}")))?;
        Some((v.get("id")?.as_u64()?, v.get("name").and_then(Value::as_str).unwrap_or("").to_string()))
    }
    pub fn set_game_for(&mut self, appid: u32, g: Option<(u64, String)>) {
        if !self.own.get("games").map(Value::is_object).unwrap_or(false) {
            self.own["games"] = json!({});
        }
        match g {
            Some((id, name)) => self.own["games"][appid.to_string()] = json!({"id": id, "name": name}),
            None => {
                if let Some(o) = self.own["games"].as_object_mut() {
                    o.remove(&appid.to_string());
                }
            }
        }
        self.save();
    }
}

// ------------------------------------------------------------------ HTTP
#[derive(Clone)]
pub struct Client {
    pub base: String,
    pub key: Option<String>,
}

impl Client {
    pub fn from(s: &Settings) -> Client {
        Client { base: s.base(), key: s.api_key() }
    }

    fn curl(&self, url: &str, out: Option<&Path>, auth: bool, timeout: u32) -> Result<(u16, Vec<u8>), String> {
        // curl config on stdin: the key never shows up in `ps`
        let mut cfg = format!("url = \"{}\"\nsilent\nshow-error\nlocation\nmax-time = {timeout}\nuser-agent = \"DeckShell sgdb\"\nwrite-out = \"\\n%{{http_code}}\"\n", url.replace('"', "%22"));
        if auth {
            if let Some(k) = &self.key {
                cfg.push_str(&format!("header = \"Authorization: Bearer {}\"\nheader = \"Accept: application/json\"\n", k.replace('"', "")));
            }
        }
        if let Some(o) = out {
            cfg.push_str(&format!("output = \"{}\"\n", o.display()));
        }
        let mut c = Command::new("curl").args(["-K", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        c.stdin.take().unwrap().write_all(cfg.as_bytes()).map_err(|e| e.to_string())?;
        let o = c.wait_with_output().map_err(|e| e.to_string())?;
        let body = o.stdout;
        let nl = body.iter().rposition(|b| *b == b'\n').ok_or_else(|| String::from_utf8_lossy(&o.stderr).trim().to_string())?;
        let code: u16 = String::from_utf8_lossy(&body[nl + 1..]).trim().parse().unwrap_or(0);
        if code == 0 {
            return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
        }
        Ok((code, body[..nl].to_vec()))
    }

    /// GET `<base><path_q>`. Fresh cache hit -> no request. Network failure / missing key -> stale cache (second value = offline).
    pub fn get(&self, path_q: &str) -> Result<(Value, bool), ApiErr> {
        let ck = cache::json_path(&format!("{}{}", self.base, path_q));
        if let Some((v, age)) = cache::read_json(&ck) {
            if age < FRESH_SECS {
                return Ok((v, false));
            }
        }
        let stale = || cache::read_json(&ck).map(|(v, _)| (v, true));
        if self.key.is_none() {
            return stale().ok_or(ApiErr::NoKey);
        }
        match self.curl(&format!("{}{}", self.base, path_q), None, true, 20) {
            Err(e) => stale().ok_or(ApiErr::Net(e)),
            Ok((code, body)) => {
                let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let errs = v.get("errors").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                if code == 404 {
                    return Err(ApiErr::NotFound);
                }
                if code >= 500 {
                    return stale().ok_or(ApiErr::Http(code, errs));
                }
                if code != 200 || v.get("success").and_then(Value::as_bool) != Some(true) {
                    return Err(ApiErr::Http(code, errs));
                }
                cache::write_json(&ck, &v);
                Ok((v, false))
            }
        }
    }

    /// Autocomplete game search (plugin: encodeURIComponent twice).
    pub fn search_games(&self, term: &str) -> Result<(Vec<(u64, String)>, bool), ApiErr> {
        let (v, off) = self.get(&format!("/search/autocomplete/{}", pct(&pct(term))))?;
        let rows = v["data"].as_array().map(|a| a.iter().filter_map(|g| Some((g.get("id")?.as_u64()?, g.get("name")?.as_str()?.to_string()))).collect()).unwrap_or_default();
        Ok((rows, off))
    }

    /// One page of assets for a Steam appid (`game == None`) or an SGDB game id.
    pub fn assets(&self, t: AType, steam_appid: u32, game: Option<u64>, f: &Filters, page: u32) -> Result<(Vec<Asset>, bool), ApiErr> {
        let target = match game {
            Some(g) => format!("game/{g}"),
            None => format!("steam/{steam_appid}"),
        };
        // Separate SGDB's mutually exclusive types so each row carries the server's
        // classification, even when its thumbnail and full URL have ordinary extensions.
        let mut rows = Vec::new();
        let mut offline = false;
        for animated in [false, true] {
            if (animated && !f.animated) || (!animated && !f.stat) { continue; }
            let q = query(t, f, page).replace("types=static%2Canimated", if animated { "types=animated" } else { "types=static" });
            let (v, off) = self.get(&format!("/{}/{target}?{q}", t.endpoint()))?;
            offline |= off;
            if let Some(data) = v["data"].as_array() {
                rows.extend(data.iter().filter_map(Asset::from_json).map(|mut asset| {
                    asset.animation = Some(animated);
                    asset
                }));
            }
        }
        Ok((rows, offline))
    }

    /// Download `url` into `dest` (atomic). Thumbnails / full images are public CDN files: no key.
    pub fn download(&self, url: &str, dest: &Path) -> Result<(), String> {
        if let Some(d) = dest.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // One temp name per thread: the apply and a preview job may fetch the same URL at the same time, and a shared
        // name would let one rename the other's half-written file into place.
        let tmp = dest.with_extension(format!("part{:x}", cache::fnv64(&format!("{:?}", std::thread::current().id()))));
        let r = self.curl(url, Some(&tmp), false, 60);
        match r {
            Ok((200, _)) if std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false) => std::fs::rename(&tmp, dest).map_err(|e| e.to_string()),
            Ok((c, _)) => {
                let _ = std::fs::remove_file(&tmp);
                Err(format!("HTTP {c}"))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }
}

/// Map of appid -> (id, name) of all remembered matches (batch mode / debugging).
#[allow(dead_code)]
pub fn known_games(s: &Settings) -> HashMap<u32, (u64, String)> {
    let mut m = HashMap::new();
    if let Some(o) = s.own.get("games").and_then(Value::as_object) {
        for (k, v) in o {
            if let (Ok(a), Some(id)) = (k.parse(), v.get("id").and_then(Value::as_u64)) {
                m.insert(a, (id, v.get("name").and_then(Value::as_str).unwrap_or("").to_string()));
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_artwork_format_detection() {
        // Synthetic 2x2 still and two-frame images, encoded as PNG/APNG and WebP.
        assert_eq!(animation_from_content(&[137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,2,0,0,0,2,8,6,0,0,0,114,182,13,36,0,0,0,21,73,68,65,84,120,156,99,252,207,192,240,159,129,129,129,129,9,68,128,48,0,31,23,2,2,2,71,179,20,0,0,0,0,73,69,78,68,174,66,96,130]), Some(false)); // PNG
        assert_eq!(animation_from_content(&[137,80,78,71,13,10,26,10,0,0,0,13,73,72,68,82,0,0,0,2,0,0,0,2,8,6,0,0,0,114,182,13,36,0,0,0,8,97,99,84,76,0,0,0,2,0,0,0,0,243,141,147,112,0,0,0,26,102,99,84,76,0,0,0,0,0,0,0,2,0,0,0,2,0,0,0,0,0,0,0,0,0,1,0,10,0,0,232,84,220,0,0,0,0,21,73,68,65,84,120,156,99,252,207,192,240,159,129,129,129,129,9,68,128,48,0,31,23,2,2,2,71,179,20,0,0,0,26,102,99,84,76,0,0,0,1,0,0,0,2,0,0,0,2,0,0,0,0,0,0,0,0,0,1,0,10,0,0,115,39,54,212,0,0,0,25,102,100,65,84,0,0,0,2,120,156,99,100,96,248,255,159,129,129,129,129,9,68,128,48,0,29,25,2,2,69,110,158,87,0,0,0,0,73,69,78,68,174,66,96,130]), Some(true)); // PNG
        assert_eq!(animation_from_content(&[82,73,70,70,60,0,0,0,87,69,66,80,86,80,56,32,48,0,0,0,208,1,0,157,1,42,2,0,2,0,1,64,38,37,160,2,116,186,1,248,0,3,176,0,254,242,235,127,252,216,21,205,115,239,247,255,210,224,253,46,15,210,224,255,210,144,0,0]), Some(false)); // WEBP
        assert_eq!(animation_from_content(&[82,73,70,70,196,0,0,0,87,69,66,80,86,80,56,88,10,0,0,0,2,0,0,0,1,0,0,1,0,0,65,78,73,77,6,0,0,0,0,0,0,0,0,0,65,78,77,70,74,0,0,0,0,0,0,0,0,0,1,0,0,1,0,0,100,0,0,2,86,80,56,32,50,0,0,0,48,1,0,157,1,42,2,0,2,0,1,64,38,37,160,0,3,112,0,254,242,235,127,255,249,176,63,246,243,255,4,122,1,255,255,210,224,255,254,151,7,255,244,184,63,244,164,0,0,0,65,78,77,70,70,0,0,0,0,0,0,0,0,0,1,0,0,1,0,0,100,0,0,0,86,80,56,32,46,0,0,0,52,1,0,157,1,42,2,0,2,0,0,0,38,37,160,0,3,112,0,254,251,85,227,255,255,75,131,255,250,92,31,255,210,224,255,210,224,255,250,213,229,87,172,171,160,0]), Some(true)); // WEBP
        assert_eq!(animation_from_content(b"unknown format"), None);
        let mut asset = Asset { url: "https://example.invalid/art.png".into(), thumb: "https://example.invalid/thumb.webp".into(), ..Default::default() };
        asset.animation = Some(true);
        assert!(asset.animated());
        assert_eq!(asset.flags() & 1, 1);
        asset.animation = Some(false);
        assert!(!asset.animated());
        assert_eq!(asset.flags() & 1, 0);
    }

    #[test]
    fn defaults_match_plugin_query() {
        let f = Filters::default_for(AType::Capsule);
        let q = query(AType::Capsule, &f, 0);
        assert_eq!(
            q,
            "page=0&styles=alternate%2Cwhite_logo%2Cno_logo%2Cblurred%2Cmaterial&dimensions=600x900%2C342x482%2C660x930&mimes=image%2Fpng%2Cimage%2Fjpeg%2Cimage%2Fwebp&nsfw=false&humor=any&epilepsy=any&oneoftag=&types=static%2Canimated"
        );
    }

    #[test]
    fn animated_only_sends_no_dimensions_or_mimes() {
        let mut f = Filters::default_for(AType::Capsule);
        f.stat = false;
        let q = query(AType::Capsule, &f, 0);
        assert!(q.contains("types=animated") && !q.contains("dimensions") && !q.contains("mimes") && q.contains("styles="), "{q}");
        let mk = |w, h| Asset { width: w, height: h, ..Default::default() };
        assert!(AType::Capsule.fits_tab(&mk(600, 900)) && !AType::Capsule.fits_tab(&mk(920, 430)) && AType::Capsule.fits_tab(&mk(512, 512)));
        assert!(AType::Wide.fits_tab(&mk(920, 430)) && !AType::Wide.fits_tab(&mk(600, 900)));
    }

    #[test]
    fn tag_logic() {
        let mut f = Filters::default_for(AType::Hero);
        f.untagged = false;
        f.adult = true;
        f.humor = false;
        f.epilepsy = false;
        let q = query(AType::Hero, &f, 2);
        assert!(q.contains("nsfw=any") && q.contains("humor=any") && q.contains("oneoftag=nsfw") && q.contains("page=2"), "{q}");
        f.untagged = true;
        let q = query(AType::Hero, &f, 0);
        assert!(q.contains("nsfw=any") && q.contains("humor=false") && q.contains("epilepsy=false") && q.contains("oneoftag=&"), "{q}");
    }

    #[test]
    fn filters_from_plugin_json() {
        let v: Value = serde_json::from_str(r#"{"styles":["alternate"],"dimensions":["600x900"],"mimes":["image/png"],"animated":false,"_static":true,"adult":true,"humor":false,"epilepsy":true,"untagged":true}"#).unwrap();
        let f = Filters::from_json(AType::Capsule, &v);
        assert_eq!(f.styles, ["alternate"]);
        assert!(!f.animated && f.adult && !f.humor);
        assert_eq!(Filters::from_json(AType::Capsule, &f.to_json()), f);
    }

    #[test]
    fn asset_parse() {
        let v: Value = serde_json::from_str(r#"{"id":5,"url":"https://cdn/x/a.webp","thumb":"https://cdn/t/a.webm","width":600,"height":900,"nsfw":false,"humor":true,"epilepsy":false,"notes":"n","author":{"name":"bob"}}"#).unwrap();
        let a = Asset::from_json(&v).unwrap();
        assert!(!a.animated());
        assert_eq!(a.flags(), 4 | 16);
        assert_eq!(a.format_tag(), "WebP");
        assert_eq!(a.author, "bob");
    }
}
