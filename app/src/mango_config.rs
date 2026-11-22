//! Steam's session MangoApp config. The file is watched in place by MangoApp.
use std::{fs, io, path::Path};

fn path() -> Option<std::path::PathBuf> {
    std::env::var_os("MANGOHUD_CONFIGFILE").filter(|s| !s.is_empty()).map(Into::into)
}

fn level_from(text: &str) -> Option<i32> {
    if !text.lines().any(|l| l.trim() == "mangoapp_steam") { return None; }
    if text.lines().any(|l| l.trim() == "no_display") { return Some(0); }
    let mut levels = text.lines().filter_map(|l| l.trim().strip_prefix("preset=").and_then(|v| v.trim().parse::<i32>().ok()));
    let level = levels.next()?;
    ((1..=4).contains(&level) && levels.next().is_none()).then_some(level)
}

pub fn read() -> Option<i32> { level_from(&fs::read_to_string(path()?).ok()?) }

fn update(path: &Path, level: i32) -> io::Result<()> {
    if !(0..=4).contains(&level) { return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid overlay level")); }
    let old = fs::read_to_string(path)?;
    if level_from(&old).is_none() { return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown MangoApp config")); }
    let mut next = String::new();
    for line in old.lines() {
        let l = line.trim();
        if l == "no_display" || l.starts_with("preset=") { continue; }
        next.push_str(line); next.push('\n');
    }
    if level == 0 { next.push_str("no_display\n"); } else { next.push_str(&format!("preset={level}\n")); }
    // Do not rename: MangoApp watches this inode. Check that Steam did not already replace the content.
    if fs::read_to_string(path)? != old { return Err(io::Error::new(io::ErrorKind::WouldBlock, "MangoApp config changed")); }
    fs::write(path, next)?;
    if level_from(&fs::read_to_string(path)?) != Some(level) { return Err(io::Error::other("MangoApp config readback mismatch")); }
    Ok(())
}

pub fn write(level: i32) -> io::Result<()> { update(&path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "MANGOHUD_CONFIGFILE"))?, level) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_other_config_and_reads_off_and_preset() {
        let p = std::env::temp_dir().join(format!("deckshell-mango-{}", std::process::id()));
        fs::write(&p, "control=mangohud\nmangoapp_steam\nfsr_steam_sharpness=5\nno_display\n").unwrap();
        update(&p, 4).unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert_eq!(level_from(&s), Some(4));
        assert!(s.contains("fsr_steam_sharpness=5\n"));
        update(&p, 0).unwrap();
        assert_eq!(level_from(&fs::read_to_string(&p).unwrap()), Some(0));
        fs::remove_file(p).unwrap();
    }
}
