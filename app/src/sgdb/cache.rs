//! On-disk caches of the cover changer under ~/.cache/deckshell/sgdb (override DECKSHELL_SGDB_CACHE):
//!   json/   API answers (offline mode + quota saving)      thumbs/  result thumbnails      full/  downloaded full-size images
//!   undo/   previous custom art of an app (quick revert)    - never pruned
//! `thumbs` + `full` share a byte budget (DECKSHELL_SGDB_CACHE_MB, default 160); pruning removes least recently used files
//! (mtime is bumped on every use).
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

pub fn root() -> PathBuf {
    std::env::var_os("DECKSHELL_SGDB_CACHE").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".cache")).join("deckshell/sgdb")
    })
}

pub fn fnv64(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn budget() -> u64 {
    std::env::var("DECKSHELL_SGDB_CACHE_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(160) << 20
}

pub fn json_path(key: &str) -> PathBuf {
    root().join("json").join(format!("{:016x}.json", fnv64(key)))
}
pub fn thumb_path(url: &str) -> PathBuf {
    root().join("thumbs").join(format!("{:016x}.{}", fnv64(url), super::api::Asset::ext_of(url)))
}
pub fn full_path(url: &str) -> PathBuf {
    root().join("full").join(format!("{:016x}.{}", fnv64(url), super::api::Asset::ext_of(url)))
}
pub fn undo_dir() -> PathBuf {
    root().join("undo")
}

/// Bump mtime (LRU order).
pub fn touch(p: &Path) {
    if let Ok(f) = fs::OpenOptions::new().append(true).open(p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

pub fn read_json(p: &Path) -> Option<(Value, u64)> {
    let md = fs::metadata(p).ok()?;
    let age = md.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).unwrap_or(Duration::ZERO).as_secs();
    let v = serde_json::from_slice(&fs::read(p).ok()?).ok()?;
    Some((v, age))
}

pub fn write_json(p: &Path, v: &Value) {
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    let tmp = p.with_extension("tmp");
    if serde_json::to_vec(v).ok().and_then(|b| fs::write(&tmp, b).ok()).is_some() {
        let _ = fs::rename(&tmp, p);
    }
}

fn dir_files(d: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    fs::read_dir(d)
        .map(|rd| rd.flatten().filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        }).collect())
        .unwrap_or_default()
}

/// Total bytes of thumbs + full.
pub fn used_bytes() -> u64 {
    ["thumbs", "full"].iter().flat_map(|d| dir_files(&root().join(d))).map(|f| f.1).sum()
}

/// Delete least-recently-used thumbs/full files until the budget holds; `keep` (files currently on screen) survive.
/// Returns the number of deleted files.
pub fn prune(budget: u64, keep: &[PathBuf]) -> usize {
    let mut all: Vec<_> = ["thumbs", "full"].iter().flat_map(|d| dir_files(&root().join(d))).collect();
    let mut total: u64 = all.iter().map(|f| f.1).sum();
    if total <= budget {
        return 0;
    }
    all.sort_by_key(|f| f.2);
    let mut n = 0;
    for (p, len, _) in all {
        if total <= budget {
            break;
        }
        if keep.contains(&p) {
            continue;
        }
        if fs::remove_file(&p).is_ok() {
            total -= len;
            n += 1;
        }
    }
    // json answers are small; cap by count
    let mut j = dir_files(&root().join("json"));
    if j.len() > 600 {
        j.sort_by_key(|f| f.2);
        for f in j.iter().take(j.len() - 600) {
            let _ = fs::remove_file(&f.0);
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_removes_oldest_first() {
        let d = std::env::temp_dir().join(format!("sgdbtest{}", std::process::id()));
        std::env::set_var("DECKSHELL_SGDB_CACHE", &d);
        fs::create_dir_all(d.join("thumbs")).unwrap();
        for i in 0..5u8 {
            let p = d.join("thumbs").join(format!("{i}.png"));
            fs::write(&p, vec![0u8; 1000]).unwrap();
            fs::File::options().append(true).open(&p).unwrap().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100 + i as u64)).unwrap();
        }
        assert_eq!(used_bytes(), 5000);
        let keep = vec![d.join("thumbs/0.png")];
        assert_eq!(prune(2500, &keep), 3);
        assert!(d.join("thumbs/0.png").exists() && !d.join("thumbs/1.png").exists() && d.join("thumbs/4.png").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
