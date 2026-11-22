//! Name/type lookup from appcache/appinfo.vdf (binary, v40/v41) for owned-but-not-installed games.
use crate::vdf::{parse_binary, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct Info {
    pub name: String,
    pub kind: String,
}

fn u32le(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

pub fn lookup(path: &Path, wanted: &HashSet<u32>) -> HashMap<u32, Info> {
    let mut out = HashMap::new();
    if wanted.is_empty() {
        return out;
    }
    let Ok(d) = std::fs::read(path) else { return out };
    let Some(magic) = u32le(&d, 0) else { return out };
    let (header, table): (usize, Option<Vec<String>>) = match magic {
        0x0756_4429 => {
            let Some(off) = d.get(8..16).map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize) else { return out };
            let Some(count) = u32le(&d, off) else { return out };
            let mut t = Vec::with_capacity(count as usize);
            let mut i = off + 4;
            for _ in 0..count {
                let Some(rest) = d.get(i..) else { break };
                let Some(n) = rest.iter().position(|&c| c == 0) else { break };
                t.push(String::from_utf8_lossy(&rest[..n]).into_owned());
                i += n + 1;
            }
            (16, Some(t))
        }
        0x0756_4428 => (8, None),
        _ => return out,
    };
    let end = match &table {
        Some(_) => d.get(8..16).map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize).unwrap_or(d.len()),
        None => d.len(),
    };
    let mut i = header;
    while i + 8 <= end {
        let Some(id) = u32le(&d, i) else { break };
        if id == 0 {
            break;
        }
        let Some(size) = u32le(&d, i + 4) else { break };
        let body = i + 8;
        let next = body + size as usize;
        if next > d.len() {
            break;
        }
        if wanted.contains(&id) && size >= 60 {
            let v = parse_binary(&d[body + 60..next], table.as_deref());
            let root: &Value = v.get("appinfo").unwrap_or(&v);
            if let Some(common) = root.get("common") {
                if let Some(name) = common.str_at("name") {
                    out.insert(
                        id,
                        Info { name: name.to_owned(), kind: common.str_at("type").unwrap_or("").to_ascii_lowercase() },
                    );
                }
            }
        }
        i = next;
    }
    out
}
