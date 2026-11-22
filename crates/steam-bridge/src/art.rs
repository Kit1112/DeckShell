//! Artwork exactly as Steam's UI resolves it: Steam gives the ordered URL list per asset (custom art first, then the
//! library cache, then CDN), Steam's loopback server maps URLs to files, we do the same mapping locally:
//!   /customimages/<name>.<ext>?v=..        -> userdata/<ACTIVE ACCOUNT>/config/grid/<name>.<ext>
//!   /assets/<appid>/<file>?c=..            -> appcache/librarycache/<appid>/<file>   (file may be "<hash>/x.jpg")
//!   /assets/<appid>_<file>?c=..            -> appcache/librarycache/<appid>_<file>   (legacy flat layout)
//!   https://...                            -> remote (Steam downloads into librarycache; we report the URL)
//! The first URL whose file exists wins, which is what the <img> error-walk does.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{js, Bridge, Error, Result, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ArtKind {
    Capsule,
    Header,
    Hero,
    Logo,
    Icon,
}

/// Steam's candidate URLs per asset for one app (in Steam's own order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppArtUrls {
    pub appid: u32,
    pub capsule: Vec<String>,
    pub header: Vec<String>,
    pub hero: Vec<String>,
    pub logo: Vec<String>,
    pub icon: Vec<String>,
}

impl AppArtUrls {
    pub fn list(&self, k: ArtKind) -> &[String] {
        match k {
            ArtKind::Capsule => &self.capsule,
            ArtKind::Header => &self.header,
            ArtKind::Hero => &self.hero,
            ArtKind::Logo => &self.logo,
            ArtKind::Icon => &self.icon,
        }
    }
}

/// Resolved art for one app. `None` = nothing local; `remote` then holds the first CDN URL Steam would use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtSet {
    pub capsule: Option<PathBuf>,
    /// Same file as `capsule` when it is an animated image (APNG/animated WebP/GIF): an <img> animates it, always.
    pub capsule_animated: Option<PathBuf>,
    pub header: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub logo: Option<PathBuf>,
    pub icon: Option<PathBuf>,
    /// grid/<appid>.json logo position (appDetailsStore.GetCustomLogoPosition reads /customimages/<appid>.json).
    pub logo_position: Option<PathBuf>,
    /// URLs that would have been used for kinds without a local file.
    pub remote: Vec<(ArtKind, String)>,
    /// Kinds whose chosen file is a user custom image (/customimages/).
    pub custom: Vec<ArtKind>,
    /// Kinds whose chosen file is an animated image (content-sniffed: the user's grid has WebP data in `*.png`
    /// names). Chromium animates any such <img> continuously while painted; there is no focus gating in BP.
    pub animated: Vec<ArtKind>,
}

impl ArtSet {
    pub fn get(&self, k: ArtKind) -> Option<&PathBuf> {
        match k {
            ArtKind::Capsule => self.capsule.as_ref(),
            ArtKind::Header => self.header.as_ref(),
            ArtKind::Hero => self.hero.as_ref(),
            ArtKind::Logo => self.logo.as_ref(),
            ArtKind::Icon => self.icon.as_ref(),
        }
    }
}

/// Maps Steam loopback URLs to local files for one Steam install + account.
#[derive(Debug, Clone)]
pub struct ArtResolver {
    pub steam_root: PathBuf,
    pub account: u32,
}

#[derive(Debug, PartialEq)]
pub enum Mapped {
    Local(PathBuf, bool),
    Remote(String),
    Skip,
}

impl ArtResolver {
    pub fn new(steam_root: impl Into<PathBuf>, account: u32) -> Self {
        ArtResolver { steam_root: steam_root.into(), account }
    }
    pub fn grid_dir(&self) -> PathBuf {
        self.steam_root.join("userdata").join(self.account.to_string()).join("config/grid")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.steam_root.join("appcache/librarycache")
    }

    /// URL -> file path (not checked for existence). Bool = custom image.
    pub fn map_url(&self, url: &str) -> Mapped {
        let path = url.split(['?', '#']).next().unwrap_or("");
        let safe = |s: &str| !s.is_empty() && !s.split('/').any(|c| c == ".." || c.is_empty());
        if let Some(rest) = path.strip_prefix("/customimages/") {
            return if safe(rest) { Mapped::Local(self.grid_dir().join(rest), true) } else { Mapped::Skip };
        }
        if let Some(rest) = path.strip_prefix("/assets/") {
            return if safe(rest) { Mapped::Local(self.cache_dir().join(rest), false) } else { Mapped::Skip };
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            return Mapped::Remote(url.to_string());
        }
        Mapped::Skip
    }

    /// First URL of the list backed by an existing non-empty file (what the <img> error-walk ends on).
    pub fn pick(&self, urls: &[String]) -> (Option<(PathBuf, bool)>, Option<String>) {
        let mut remote = None;
        for u in urls {
            match self.map_url(u) {
                Mapped::Local(p, c) => {
                    if std::fs::metadata(&p).map(|m| m.is_file() && m.len() > 0).unwrap_or(false) {
                        return (Some((p, c)), None);
                    }
                }
                Mapped::Remote(r) => {
                    remote.get_or_insert(r);
                }
                Mapped::Skip => {}
            }
        }
        (None, remote)
    }

    pub fn resolve(&self, a: &AppArtUrls) -> ArtSet {
        let mut s = ArtSet::default();
        for k in [ArtKind::Capsule, ArtKind::Header, ArtKind::Hero, ArtKind::Logo, ArtKind::Icon] {
            let (loc, rem) = self.pick(a.list(k));
            let slot = match k {
                ArtKind::Capsule => &mut s.capsule,
                ArtKind::Header => &mut s.header,
                ArtKind::Hero => &mut s.hero,
                ArtKind::Logo => &mut s.logo,
                ArtKind::Icon => &mut s.icon,
            };
            if let Some((p, custom)) = loc {
                if custom {
                    s.custom.push(k);
                }
                if is_animated(&p) {
                    s.animated.push(k);
                }
                *slot = Some(p);
            } else if let Some(r) = rem {
                s.remote.push((k, r));
            }
        }
        if s.animated.contains(&ArtKind::Capsule) {
            s.capsule_animated = s.capsule.clone();
        }
        let j = self.grid_dir().join(format!("{}.json", a.appid));
        if j.is_file() {
            s.logo_position = Some(j);
        }
        s
    }
}

/// Sniff the head of an image for animation (APNG acTL before IDAT, WebP VP8X animation flag, GIF).
pub fn is_animated(p: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return false };
    let mut buf = [0u8; 4096];
    let n = f.read(&mut buf).unwrap_or(0);
    let b = &buf[..n];
    let find = |h: &[u8], n: &[u8]| h.windows(n.len()).position(|w| w == n);
    if b.starts_with(b"\x89PNG") {
        let idat = find(b, b"IDAT").unwrap_or(b.len());
        return find(&b[..idat], b"acTL").is_some();
    }
    if b.len() > 21 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" && &b[12..16] == b"VP8X" {
        return b[20] & 0x02 != 0;
    }
    b.starts_with(b"GIF8")
}

fn parse_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn parse_art_urls(v: &Value) -> Result<Vec<AppArtUrls>> {
    let rows = v.as_array().ok_or_else(|| Error::Protocol("art urls: not an array".into()))?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(r) = r.as_array() else { continue };
        if r.len() < 6 {
            return Err(Error::Protocol("art urls: short row".into()));
        }
        out.push(AppArtUrls {
            appid: r[0].as_u64().unwrap_or(0) as u32,
            capsule: parse_list(&r[1]),
            header: parse_list(&r[2]),
            hero: parse_list(&r[3]),
            logo: parse_list(&r[4]),
            icon: parse_list(&r[5]),
        });
    }
    Ok(out)
}

impl Bridge {
    /// Active account (32-bit) from the UI itself (never guessed from userdata/).
    pub fn active_account(&self) -> Result<u32> {
        self.eval_json(js::ACTIVE_ACCOUNT)?.as_u64().map(|v| v as u32).filter(|v| *v != 0).ok_or_else(|| Error::Protocol("no active account".into()))
    }

    /// Steam's candidate URL lists for the given apps (chunked: each chunk waits <=1.2 s for AppDetails).
    pub fn art_urls(&self, appids: &[u32]) -> Result<Vec<AppArtUrls>> {
        let mut out = Vec::with_capacity(appids.len());
        for ch in appids.chunks(80) {
            let ids = format!("[{}]", ch.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
            out.extend(parse_art_urls(&self.eval_json(&js::ART_URLS.replace("{IDS}", &ids).replace("{ICON_URLS}", js::APP_ICON_URLS))?)?);
        }
        Ok(out)
    }

    /// The download header needs only the app overview's icon, with no AppDetails wait.
    pub fn app_icon_urls(&self, appid: u32) -> Result<Vec<String>> {
        let query = format!("({})(appStore.GetAppOverviewByAppID({appid}))", js::APP_ICON_URLS);
        Ok(parse_list(&self.eval_json(&query)?))
    }

    /// Everything the UI needs: active account -> resolver -> resolved art per app.
    pub fn art_all(&self, steam_root: &Path, appids: &[u32]) -> Result<(u32, HashMap<u32, ArtSet>)> {
        let acct = self.active_account()?;
        let r = ArtResolver::new(steam_root, acct);
        let map = self.art_urls(appids)?.into_iter().map(|u| (u.appid, r.resolve(&u))).collect();
        Ok((acct, map))
    }

    /// Audit helper: what the live UI really loads for these URL lists: per list `[url, bytes, fnv1a32]` or null.
    pub fn art_fetch(&self, lists: &[&[String]]) -> Result<Vec<Option<(String, u64, u32)>>> {
        let l = serde_json::to_string(&lists).map_err(|e| Error::Protocol(e.to_string()))?;
        let v = self.eval_json(&js::ART_FETCH.replace("{LISTS}", &l))?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .map(|r| r.as_array().and_then(|r| Some((r.first()?.as_str()?.to_string(), r.get(1)?.as_u64()?, r.get(2)?.as_u64()? as u32))))
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// FNV-1a 32 over a file, same function as ART_FETCH.
pub fn fnv1a32_file(p: &Path) -> Option<(u64, u32)> {
    let d = std::fs::read(p).ok()?;
    let mut h = 0x811c9dc5u32;
    for b in &d {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    Some((d.len() as u64, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_urls() {
        let r = ArtResolver::new("/s", 42);
        assert_eq!(r.map_url("/customimages/1p.png?v=3"), Mapped::Local("/s/userdata/42/config/grid/1p.png".into(), true));
        assert_eq!(r.map_url("/assets/5/abc/library_600x900.jpg?c=1"), Mapped::Local("/s/appcache/librarycache/5/abc/library_600x900.jpg".into(), false));
        assert_eq!(r.map_url("/assets/5_header.jpg?c=1"), Mapped::Local("/s/appcache/librarycache/5_header.jpg".into(), false));
        assert_eq!(r.map_url("/assets/../x"), Mapped::Skip);
        assert!(matches!(r.map_url("https://x/y.jpg"), Mapped::Remote(_)));
        assert_eq!(r.map_url("data:"), Mapped::Skip);
    }

    /// Sniffs by content, not by name: Steam's grid files often carry WebP or APNG data under a `.png` name.
    #[test]
    fn animation_is_sniffed_from_the_file_head() {
        let dir = std::env::temp_dir().join(format!("steam-bridge-anim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let check = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            is_animated(&p)
        };
        let chunk = |tag: &[u8]| { let mut v = vec![0, 0, 0, 0]; v.extend_from_slice(tag); v.extend_from_slice(&[0; 4]); v };
        // APNG: acTL must come before the first IDAT
        let apng: Vec<u8> = [b"\x89PNG\r\n\x1a\n".as_slice(), &chunk(b"IHDR"), &chunk(b"acTL"), &chunk(b"IDAT")].concat();
        assert!(check("apng.png", &apng));
        let png: Vec<u8> = [b"\x89PNG\r\n\x1a\n".as_slice(), &chunk(b"IHDR"), &chunk(b"IDAT")].concat();
        assert!(!check("still.png", &png));
        // a WebP with a VP8X chunk: flags byte at offset 20, animation bit 0x02
        let webp = |flags: u8| -> Vec<u8> {
            let mut v = b"RIFF\x00\x00\x00\x00WEBPVP8X\x0a\x00\x00\x00".to_vec();
            v.push(flags);
            v.extend_from_slice(&[0; 9]);
            v
        };
        assert!(check("anim.webp", &webp(0x12)));
        assert!(!check("still-vp8x.webp", &webp(0x10)));
        assert!(check("anim.gif", b"GIF89a\x01\x00"));
        assert!(!check("notes.txt", b"hello world, not an image at all"));
        assert!(!is_animated(&dir.join("missing.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
