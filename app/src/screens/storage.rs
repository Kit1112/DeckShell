//! Read-only storage view backed by Steam's InstallFolder snapshot.
use steam_assets::i18n;
use serde_json::Value;
use super::PRow;

// The Steam Storage page calls GetInstallFolders and reads these fields. Resolve app names
// from the same overview store. Folder paths are displayed in memory and never logged.
const SNAPSHOT: &str = r#"(async()=>{const f=await SteamClient.InstallFolder.GetInstallFolders();if(!Array.isArray(f))throw Error('invalid folders');return Promise.all(f.map(async d=>({id:d.nFolderIndex,label:d.strUserLabel,drive:d.strDriveName,path:d.strFolderPath,fixed:d.bIsFixed,mounted:d.bIsMounted,capacity:d.nCapacity,free:d.nFreeSpace,used:d.nUsedSize,game:d.nAppSize,dlc:d.nDLCSize,workshop:d.nWorkshopSize,staged:d.nStagedSize,shader:d.nShaderSize,media:await (async()=>{try {const s=await SteamClient.Screenshots.GetTotalDiskSpaceUsage(d.strFolderPath); const r=window.g_GRS; if(!r?.GetTotalDiskSpaceUsage)return null; const a=await r.GetTotalDiskSpaceUsage(d.strFolderPath,false),b=await r.GetTotalDiskSpaceUsage(d.strFolderPath,true); return Number(s)+Number(a)+Number(b)}catch(_){return null}})(),apps:Array.isArray(d.vecApps)?d.vecApps.map(a=>{const o=appStore.GetAppOverviewByAppID(a.nAppID);return {id:a.nAppID,name:a.strAppName||o?.display_name,sort:a.strSortAs,played:a.rtLastPlayed,size:a.nUsedSize}}):null})))})()"#;

#[derive(Clone, Debug)]
pub struct Drive { id: i64, label: String, drive: String, path: String, fixed: Option<bool>, mounted: Option<bool>, capacity: Option<u64>, free: Option<u64>, apps: Vec<Item>, categories: [Option<u64>; 7] }
#[derive(Clone, Debug)]
struct Item { id: u64, name: Option<String>, size: Option<u64>, sort: String, played: Option<u64> }

fn bytes(v: &Value) -> Option<u64> { v.as_u64().or_else(|| v.as_str()?.parse().ok()) }
fn parse(v: &Value) -> Result<Vec<Drive>, ()> {
    let folders = v.as_array().ok_or(())?;
    folders.iter().map(|d| {
        let id = d["id"].as_i64().ok_or(())?;
        let apps = d["apps"].as_array().ok_or(())?.iter().map(|a| Ok(Item {
            id: a["id"].as_u64().filter(|id| *id > 0).ok_or(())?,
            name: a["name"].as_str().filter(|s| !s.trim().is_empty()).map(str::to_owned),
            size: bytes(&a["size"]), sort: a["sort"].as_str().unwrap_or("").to_lowercase(), played: bytes(&a["played"]),
        })).collect::<Result<Vec<_>, ()>>()?;
        Ok(Drive { id, label: d["label"].as_str().unwrap_or("").to_owned(), drive: d["drive"].as_str().unwrap_or("").to_owned(), path: d["path"].as_str().unwrap_or("").to_owned(), fixed: d["fixed"].as_bool(), mounted: d["mounted"].as_bool(), capacity: bytes(&d["capacity"]), free: bytes(&d["free"]), apps, categories: [bytes(&d["game"]), bytes(&d["dlc"]), bytes(&d["workshop"]), bytes(&d["staged"]), bytes(&d["shader"]), bytes(&d["media"]), bytes(&d["used"])] })
    }).collect()
}
pub fn fetch() -> Result<Vec<Drive>, ()> {
    let b = crate::actions::bridge().ok_or(())?;
    let mut drives = parse(&b.eval_json(SNAPSHOT).map_err(|_| ())?)?;
    // lsblk is read-only and available on SteamOS. Limit extra entries to MMC/USB
    // disks: internal system partitions must not become phantom storage cards.
    if let Ok(output) = std::process::Command::new("lsblk").args(["--json", "--bytes", "--output", "NAME,PATH,TYPE,SIZE,TRAN,LABEL,MOUNTPOINTS"]).output() {
        if output.status.success() {
            if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) { merge_devices(&mut drives, &value); }
        }
    }
    Ok(drives)
}

fn merge_devices(drives: &mut Vec<Drive>, inventory: &Value) {
    fn mounts<'a>(device: &'a Value, out: &mut Vec<&'a str>) {
        if let Some(paths) = device["mountpoints"].as_array() {
            out.extend(paths.iter().filter_map(Value::as_str).filter(|p| p.starts_with('/')));
        }
        if let Some(children) = device["children"].as_array() {
            for child in children { mounts(child, out); }
        }
    }
    let Some(devices) = inventory["blockdevices"].as_array() else { return };
    for disk in devices {
        if disk["type"].as_str() != Some("disk") { continue; }
        let path = disk["path"].as_str().unwrap_or("");
        let transport = disk["tran"].as_str().unwrap_or("");
        if !(transport == "mmc" || transport == "usb" || path.starts_with("/dev/mmcblk")) { continue; }
        let mut paths = Vec::new(); mounts(disk, &mut paths);
        let matched = drives.iter_mut().find(|d| !d.path.is_empty() && paths.iter().any(|p| d.path == *p || d.path.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))));
        if let Some(d) = matched {
            // The block-device path identifies microSD even when Steam's drive name is a mount path.
            if path.starts_with("/dev/mmcblk") { d.drive = path.to_owned(); }
            continue;
        }
        // Stable negative identifier avoids collision with Steam folder indices.
        let hash = path.bytes().fold(14695981039346656037u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
        let id = -1 - (hash & (i64::MAX as u64 - 1)) as i64;
        drives.push(Drive { id, label: disk["label"].as_str().unwrap_or("").to_owned(), drive: path.to_owned(), path: paths.first().copied().unwrap_or("").to_owned(), fixed: Some(false), mounted: Some(false), capacity: bytes(&disk["size"]), free: None, apps: vec![], categories: [None; 7] });
    }
}
fn size(v: Option<u64>) -> String { v.map(|n| format!("{:.1} GB", n as f64 / 1_073_741_824.0)).unwrap_or_else(|| "—".into()) }
fn row(k: i32, label: String, x: f32, y: f32, w: f32, h: f32) -> PRow {
    PRow { k, lab: label, x, y, w, h, ..Default::default() }
}
pub fn rows(drives: &[Drive], selected: Option<i64>, sort: usize, menu: bool) -> (Vec<PRow>, Option<i64>) {
    if drives.is_empty() { return (vec![row(12, i18n::tr_ref("storage.there_are_no_steam_libraries_available").into(), 270., 125., 650., 32.)], None); }
    let selected = selected.filter(|id| drives.iter().any(|d| d.id == *id)).unwrap_or(drives[0].id);
    let mut rows = Vec::new();
    for (i, d) in drives.iter().enumerate() {
        let label = if !d.label.trim().is_empty() { d.label.clone() }
            else if d.id == 0 { i18n::tr_ref("storage.internal_storage").into() }
            else if d.drive.contains("mmcblk") { i18n::tr_ref("storage.microsd_card").into() }
            else if !d.drive.is_empty() { d.drive.clone() }
            else if d.fixed == Some(false) { i18n::tr_ref("storage.removable_drive").into() }
            else { i18n::tr_args("storage.drive", &[("index", &(i + 1).to_string())]) };
        // Device cards use a two-column layout; the wider Steam rail is applied
        // by the settings screen when converting these rows.
        // Gaming Mode stock capture places the device selector at y=168 px.
        // These rows still use the legacy 1.1985 scale, so its logical y is 140.
        let mut r = row(24, label, 270. + (i % 2) as f32 * 378., 140. + (i / 2) as f32 * 82., 358., 76.);
        // Steam sets the card capacity caption in uppercase bold (task 123 comparison); the label stays mixed case.
        let caption: String = if d.id < 0 { i18n::tr_args("storage.no_steam_library", &[("capacity", &(size(d.capacity)).to_string())]) } else if d.mounted == Some(true) { format!("{} / {}", size(d.free), size(d.capacity)) } else { i18n::tr_ref("storage.unavailable").into() };
        r.desc = caption.to_uppercase();
        r.full = if d.drive.contains("mmcblk") { "sd" } else { "drive" }.into();
        r.role = "tab".into(); r.tok = format!("storage:{}", d.id); r.f = true; r.pad = "A,FOCUS,touch,mouse".into();
        rows.push(r);
    }
    let d = drives.iter().find(|d| d.id == selected).unwrap();
    let mut y = 112. + drives.len().div_ceil(2) as f32 * 82. + 64.;
    // Steam's legend includes Other and Free; Other is the value the bar draws, when the breakdown is shown.
    let mut legend_other: Option<u64> = None;
    let unadopted = d.id < 0;
    let status = match d.mounted { Some(true) => "", Some(false) => i18n::tr_ref("storage.unconnected"), None => i18n::tr_ref("storage.state_of_unknown_state") };
    let space = if unadopted { i18n::tr_ref("storage.there_is_no_steam_library_on_the").into() } else if d.mounted == Some(true) && !d.path.is_empty() { d.path.to_uppercase() } else if d.mounted == Some(true) { i18n::tr_args("storage.free_of", &[("free", &(size(d.free)).to_string()), ("capacity", &(size(d.capacity)).to_string())]) } else { i18n::tr_args("storage.capacity_information_unavailable", &[("status", &(status).to_string())]) };
    rows.push(row(11, space, 270., y, 768., 20.));
    if let (Some(capacity), Some(free)) = (d.capacity, d.free) {
        if d.mounted == Some(true) && capacity > 0 && free <= capacity {
            let start = rows.len();
            let mut x = 270.;
            // Only draw the breakdown when all component totals are available and fit.
            let parts = &d.categories[..6];
            let total = parts.iter().try_fold(0u64, |sum, v| sum.checked_add((*v)?));
            if let Some(total) = total.filter(|n| *n <= capacity - free && d.categories[6].is_some()) {
                for (i, amount) in parts.iter().enumerate() {
                    let width = 768. * amount.unwrap() as f32 / capacity as f32;
                    if width > 0. { rows.push(row(if i == 5 { 25 } else { 18 + i as i32 }, String::new(), x, y + 25., width, 12.)); x += width; }
                }
                let other_bytes = d.categories[6].and_then(|used| used.checked_add(d.categories[3]?)).map(|used| (capacity - free).saturating_sub(used)).unwrap_or_else(|| (capacity - free).saturating_sub(total));
                let other_bytes = other_bytes.min((capacity - free).saturating_sub(total));
                legend_other = Some(other_bytes);
                let other = 768. * other_bytes as f32 / capacity as f32;
                if other > 0. { rows.push(row(23, String::new(), x, y + 25., other, 12.)); x += other; }
            } else {
                let used = 768. * (capacity - free) as f32 / capacity as f32;
                if used > 0. { rows.push(row(16, String::new(), x, y + 25., used, 12.)); x += used; }
            }
            if x < 1038. { rows.push(row(17, String::new(), x, y + 25., 1038. - x, 12.)); }
            let end = rows.len();
            if end > start {
                rows[start].full = if end == start + 1 { "both" } else { "first" }.into();
                if end > start + 1 { rows[end - 1].full = "last".into(); }
            }
        }
    }
    // Legend, in Steam's order: every nonzero category, then Other (only with the breakdown bar) and Free.
    // Swatch kinds match the bar colours in generated/settings/rows.slint; Free uses the track colour.
    // Steam draws round dots and bold uppercase labels (task 123 comparison): swatch sub "dot", label sub "legend".
    let mut legend_end = y;
    if d.mounted == Some(true) {
        let mut legend: Vec<(i32, &str, u64)> = [i18n::tr_ref("storage.games"), i18n::tr_ref("storage.supplements"), i18n::tr_ref("storage.workshop"), i18n::tr_ref("storage.updates"), i18n::tr_ref("storage.shaders"), i18n::tr_ref("storage.media")].iter().zip(d.categories[..6].iter()).enumerate()
            .filter_map(|(i, (name, value))| value.filter(|v| *v > 0).map(|v| (if i == 5 { 25 } else { 18 + i as i32 }, *name, v))).collect();
        if let Some(other) = legend_other.filter(|v| *v > 0) { legend.push((23, i18n::tr_ref("storage.more"), other)); }
        if let (Some(capacity), Some(free)) = (d.capacity, d.free) {
            if free <= capacity { legend.push((17, i18n::tr_ref("storage.free"), free)); }
        }
        for (n, (kind, name, value)) in legend.iter().enumerate() {
            let lx = 270. + (n % 3) as f32 * 250.;
            let ly = y + 45. + (n / 3) as f32 * 28.;
            let mut dot = row(*kind, String::new(), lx, ly + 8., 10., 10.);
            dot.full = "dot".into();
            rows.push(dot);
            let mut text = row(12, format!("{}  {}", name, size(Some(*value))).to_uppercase(), lx + 16., ly, 225., 26.);
            text.full = "legend".into();
            rows.push(text);
            legend_end = ly + 26.;
        }
    }
    // The app header sits 40 px above the list; keep it clear of a third legend row.
    y = (y + 132.).max(442.).max(legend_end + 48.);
    rows.push(row(11, if d.mounted != Some(true) { i18n::tr_ref("storage.applications_data_not_available").into() } else { i18n::tr_args("storage.apps", &[("len", &(d.apps.len()).to_string())]) }, 270., y - 40., 440., 32.));
    if d.mounted == Some(true) && d.apps.is_empty() {
        rows.push(row(12, i18n::tr_ref("storage.no_installed_applications").into(), 270., y, 650., 32.));
    }
    // Steam defaults to size on disk, descending. Keep unknown sizes after known
    // sizes and retain their input order instead of inventing a value.
    let mut sort_row = row(9, i18n::tr_args("storage.sort", &[("sort", &([i18n::tr_ref("storage.size"), i18n::tr_ref("storage.title"), i18n::tr_ref("storage.last_played")][sort % 3]).to_string())]), 730., y - 40., 250., 32.);
    sort_row.tok = "storage-sort".into(); sort_row.f = true; sort_row.pad = "A,FOCUS,touch,mouse".into(); rows.push(sort_row);
    let mut direction = row(9, if sort >= 3 { "↑" } else { "↓" }.into(), 990., y - 40., 40., 32.);
    direction.tok = "storage-direction".into(); direction.f = true; direction.pad = "A,FOCUS,touch,mouse".into(); rows.push(direction);
    if menu {
        for (i, (mode, label)) in [(1, i18n::tr_ref("storage.alphabetical")), (2, i18n::tr_ref("storage.last_played_2")), (0, i18n::tr_ref("storage.size_on_disk")), (3, i18n::tr_ref("storage.cancel"))].iter().enumerate() {
            let mut option = row(4, (*label).into(), 730., y + i as f32 * 40., 300., 36.);
            option.tok = format!("storage-sort-option:{mode}"); option.f = true; option.pad = "A,FOCUS,touch,mouse".into(); rows.push(option);
        }
    }
    let mut apps: Vec<_> = d.apps.iter().filter(|_| d.mounted == Some(true)).collect();
    match sort % 3 {
        1 => apps.sort_by_key(|a| if a.sort.is_empty() { a.name.as_deref().unwrap_or("").to_lowercase() } else { a.sort.clone() }),
        2 => apps.sort_by_key(|a| std::cmp::Reverse(a.played)),
        _ => apps.sort_by_key(|a| std::cmp::Reverse(a.size)),
    }
    if sort >= 3 {
        // Reverse known values only; missing metadata always remains last.
        let known = apps.iter().take_while(|a| match sort % 3 { 0 => a.size.is_some(), 2 => a.played.is_some(), _ => true }).count();
        apps[..known].reverse();
    }
    for app in apps {
        let name = app.name.clone().unwrap_or_else(|| format!("App {}", app.id));
        let mut art = row(6, String::new(), 270., y, 136., 58.);
        art.tok = format!("storage-art:{}", app.id);
        rows.push(art);
        let mut item = row(11, name, 420., y, 490., 58.);
        item.role = "listitem".into(); item.f = true; item.pad = "FOCUS".into();
        rows.push(item);
        let mut amount = row(12, size(app.size), 920., y, 110., 58.); amount.full = "r".into(); rows.push(amount);
        y += 58.;
    }
    (rows, Some(selected))
}
#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
