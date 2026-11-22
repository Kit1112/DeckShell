//! Artwork discovery: appcache/librarycache + userdata/<id>/config/grid overrides.
use crate::Art;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const IMG_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "ico"];

fn ext_rank(ext: &str) -> Option<usize> {
    IMG_EXT.iter().position(|e| e.eq_ignore_ascii_case(ext))
}

/// Index of the custom grid folder: lowercase stem -> best image path.
#[derive(Default)]
pub struct Grid {
    images: HashMap<String, (usize, PathBuf)>,
    videos: HashMap<String, PathBuf>,
}

impl Grid {
    pub fn scan(dir: &Path) -> Grid {
        let mut g = Grid::default();
        let Ok(rd) = fs::read_dir(dir) else { return g };
        for e in rd.flatten() {
            let path = e.path();
            let (Some(stem), Some(ext)) = (path.file_stem().and_then(|s| s.to_str()), path.extension().and_then(|s| s.to_str())) else {
                continue;
            };
            let stem = stem.to_ascii_lowercase();
            if ext.eq_ignore_ascii_case("webm") || ext.eq_ignore_ascii_case("mp4") {
                g.videos.entry(stem).or_insert(path);
            } else if let Some(r) = ext_rank(ext) {
                match g.images.get(&stem) {
                    Some((old, _)) if *old <= r => {}
                    _ => {
                        g.images.insert(stem, (r, path));
                    }
                }
            }
        }
        g
    }

    fn img(&self, ids: &[String], suffix: &str) -> Option<PathBuf> {
        ids.iter().find_map(|id| self.images.get(&format!("{id}{suffix}")).map(|(_, p)| p.clone()))
    }
    fn vid(&self, ids: &[String], suffix: &str) -> Option<PathBuf> {
        ids.iter().find_map(|id| self.videos.get(&format!("{id}{suffix}")).cloned())
    }

    /// Overrides for a game. `ids` are all id spellings to try (short and long).
    pub fn overrides(&self, ids: &[String]) -> Art {
        let capsule = self.img(ids, "p");
        let mut animated = self.vid(ids, "p");
        if animated.is_none() {
            if let Some(c) = &capsule {
                if is_animated(c) {
                    animated = Some(c.clone());
                }
            }
        }
        Art {
            capsule,
            capsule_animated: animated,
            hero: self.img(ids, "_hero"),
            logo: self.img(ids, "_logo"),
            header: self.img(ids, ""),
            icon: self.img(ids, "_icon"),
        }
    }
}

/// Sniff the head of an image for animation (APNG acTL, WebP ANIM, GIF with >1 frame).
pub fn is_animated(p: &Path) -> bool {
    let Ok(mut f) = fs::File::open(p) else { return false };
    let mut buf = [0u8; 4096];
    let n = f.read(&mut buf).unwrap_or(0);
    let b = &buf[..n];
    if b.starts_with(b"\x89PNG") {
        // acTL must precede the first IDAT
        let idat = find(b, b"IDAT").unwrap_or(b.len());
        return find(&b[..idat], b"acTL").is_some();
    }
    if b.len() > 21 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        if &b[12..16] == b"VP8X" {
            return b[20] & 0x02 != 0 || find(b, b"ANIM").is_some();
        }
        return false;
    }
    if b.starts_with(b"GIF8") {
        // count image descriptors cheaply: read the entire file only if small
        drop(f);
        return fs::read(p).map(|d| gif_frames(&d) > 1).unwrap_or(false);
    }
    false
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// Count frames in a GIF by walking blocks.
fn gif_frames(d: &[u8]) -> usize {
    if d.len() < 13 {
        return 0;
    }
    let mut i = 13;
    if d[10] & 0x80 != 0 {
        i += 3 * (1 << ((d[10] & 7) + 1));
    }
    let mut frames = 0;
    while i < d.len() {
        match d[i] {
            0x3B => break,
            0x21 => {
                i += 2;
                while i < d.len() && d[i] != 0 {
                    i += d[i] as usize + 1;
                }
                i += 1;
            }
            0x2C => {
                frames += 1;
                if i + 10 > d.len() {
                    break;
                }
                let flags = d[i + 9];
                i += 10;
                if flags & 0x80 != 0 {
                    i += 3 * (1 << ((flags & 7) + 1));
                }
                i += 1; // LZW min code size
                while i < d.len() && d[i] != 0 {
                    i += d[i] as usize + 1;
                }
                i += 1;
            }
            _ => break,
        }
    }
    frames
}

fn is_hash_name(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) => stem.len() == 40 && stem.bytes().all(|c| c.is_ascii_hexdigit()) && ext_rank(ext).is_some(),
        None => false,
    }
}

fn fill_from_dir(art: &mut Art, dir: &Path, depth: u8) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut header_fallback: Option<PathBuf> = None;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let p = e.path();
        match name.as_str() {
            "library_600x900.jpg" | "library_600x900.png" => {
                art.capsule.get_or_insert(p);
            }
            "library_hero.jpg" | "library_hero.png" => {
                art.hero.get_or_insert(p);
            }
            "logo.png" | "logo.jpg" => {
                art.logo.get_or_insert(p);
            }
            "library_header.jpg" | "library_header.png" => {
                art.header.get_or_insert(p);
            }
            "header.jpg" | "header.png" => header_fallback = Some(p),
            _ if is_hash_name(&name) => {
                art.icon.get_or_insert(p);
            }
            _ => {
                if depth == 0 && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    subdirs.push(p);
                }
            }
        }
    }
    if art.header.is_none() {
        art.header = header_fallback;
    }
    for d in subdirs {
        fill_from_dir(art, &d, depth + 1);
    }
}

/// librarycache art for an app id (new per-app folder layout or old flat layout).
pub fn librarycache(cache: &Path, id: &str) -> Art {
    let mut art = Art::default();
    let dir = cache.join(id);
    if dir.is_dir() {
        fill_from_dir(&mut art, &dir, 0);
    }
    if art.capsule.is_none() || art.hero.is_none() || art.logo.is_none() || art.header.is_none() || art.icon.is_none() {
        // old flat layout; only stat what is still missing
        let flat = |suffix: &str| {
            for ext in ["jpg", "png"] {
                let p = cache.join(format!("{id}{suffix}.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
            None
        };
        if art.capsule.is_none() {
            art.capsule = flat("_library_600x900");
        }
        if art.hero.is_none() {
            art.hero = flat("_library_hero");
        }
        if art.logo.is_none() {
            art.logo = flat("_logo");
        }
        if art.header.is_none() {
            art.header = flat("_library_header").or_else(|| flat("_header"));
        }
        if art.icon.is_none() {
            art.icon = flat("_icon");
        }
    }
    art
}

/// Overrides win over librarycache.
pub fn merge(base: Art, over: Art) -> Art {
    Art {
        capsule: over.capsule.or(base.capsule),
        capsule_animated: over.capsule_animated.or(base.capsule_animated),
        hero: over.hero.or(base.hero),
        logo: over.logo.or(base.logo),
        header: over.header.or(base.header),
        icon: over.icon.or(base.icon),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_count() {
        // 1x1 gif, two frames
        let mut g = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
        for _ in 0..2 {
            g.extend(b"\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00");
        }
        g.push(0x3b);
        assert_eq!(gif_frames(&g), 2);
    }

    #[test]
    fn hash_names() {
        assert!(is_hash_name("6b0312cda02f5f777efa2f3318c307ff9acafbb5.jpg"));
        assert!(!is_hash_name("header.jpg"));
    }
}
