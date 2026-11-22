//! Read-only storage view backed by Steam's InstallFolder snapshot.
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
    if drives.is_empty() { return (vec![row(12, "Нет доступных библиотек Steam".into(), 270., 125., 650., 32.)], None); }
    let selected = selected.filter(|id| drives.iter().any(|d| d.id == *id)).unwrap_or(drives[0].id);
    let mut rows = Vec::new();
    for (i, d) in drives.iter().enumerate() {
        let label = if !d.label.trim().is_empty() { d.label.clone() }
            else if d.id == 0 { "Внутренний накопитель".into() }
            else if d.drive.contains("mmcblk") { "Карта microSD".into() }
            else if !d.drive.is_empty() { d.drive.clone() }
            else if d.fixed == Some(false) { "Съёмный накопитель".into() }
            else { format!("Диск {}", i + 1) };
        // Device cards use a two-column layout; the wider Steam rail is applied
        // by the settings screen when converting these rows.
        // Gaming Mode stock capture places the device selector at y=168 px.
        // These rows still use the legacy 1.1985 scale, so its logical y is 140.
        let mut r = row(24, label, 270. + (i % 2) as f32 * 378., 140. + (i / 2) as f32 * 82., 358., 76.);
        // Steam sets the card capacity caption in uppercase bold (task 123 comparison); the label stays mixed case.
        let caption: String = if d.id < 0 { format!("Нет библиотеки Steam · {}", size(d.capacity)) } else if d.mounted == Some(true) { format!("{} / {}", size(d.free), size(d.capacity)) } else { "Недоступно".into() };
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
    let status = match d.mounted { Some(true) => "", Some(false) => " · не подключён", None => " · состояние неизвестно" };
    let space = if unadopted { "На устройстве нет библиотеки Steam".into() } else if d.mounted == Some(true) && !d.path.is_empty() { d.path.to_uppercase() } else if d.mounted == Some(true) { format!("Свободно: {} из {}", size(d.free), size(d.capacity)) } else { format!("Данные о пространстве недоступны{}", status) };
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
        let mut legend: Vec<(i32, &str, u64)> = ["Игры", "Дополнения", "Мастерская", "Обновления", "Шейдеры", "Медиа"].iter().zip(d.categories[..6].iter()).enumerate()
            .filter_map(|(i, (name, value))| value.filter(|v| *v > 0).map(|v| (if i == 5 { 25 } else { 18 + i as i32 }, *name, v))).collect();
        if let Some(other) = legend_other.filter(|v| *v > 0) { legend.push((23, "Другое", other)); }
        if let (Some(capacity), Some(free)) = (d.capacity, d.free) {
            if free <= capacity { legend.push((17, "Свободно", free)); }
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
    rows.push(row(11, if d.mounted != Some(true) { "Приложения: данные недоступны".into() } else { format!("Приложений: {}", d.apps.len()) }, 270., y - 40., 440., 32.));
    if d.mounted == Some(true) && d.apps.is_empty() {
        rows.push(row(12, "Нет установленных приложений".into(), 270., y, 650., 32.));
    }
    // Steam defaults to size on disk, descending. Keep unknown sizes after known
    // sizes and retain their input order instead of inventing a value.
    let mut sort_row = row(9, format!("Сортировка: {}  ▾", ["Размер", "Название", "Последний запуск"][sort % 3]), 730., y - 40., 250., 32.);
    sort_row.tok = "storage-sort".into(); sort_row.f = true; sort_row.pad = "A,FOCUS,touch,mouse".into(); rows.push(sort_row);
    let mut direction = row(9, if sort >= 3 { "↑" } else { "↓" }.into(), 990., y - 40., 40., 32.);
    direction.tok = "storage-direction".into(); direction.f = true; direction.pad = "A,FOCUS,touch,mouse".into(); rows.push(direction);
    if menu {
        for (i, (mode, label)) in [(1, "По алфавиту"), (2, "По дате запуска"), (0, "По размеру на диске"), (3, "Отмена")].iter().enumerate() {
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
mod tests {
    use super::*; use serde_json::json;
    #[test] fn inventory_matches_mounts_and_keeps_unadopted_removable_disks() {
        let mut drives = parse(&json!([{ "id": 0, "path": "/home/deck/.local/share/Steam", "mounted": true, "apps": [] },
            { "id": 1, "path": "/run/media/card/steamapps", "mounted": true, "apps": [] }])).unwrap();
        let inventory = json!({"blockdevices": [
            {"type":"disk", "path":"/dev/nvme0n1", "tran":"nvme", "size":1000},
            {"type":"disk", "path":"/dev/mmcblk0", "tran":"mmc", "size":2000,
             "children":[{"mountpoints":["/run/media/card"]}]},
            {"type":"disk", "path":"/dev/sda", "tran":"usb", "size":3000, "mountpoints":[]}
        ]});
        merge_devices(&mut drives, &inventory);
        assert_eq!(drives.len(), 3);
        assert_eq!(drives[1].drive, "/dev/mmcblk0");
        assert!(drives[2].id < 0);
        assert_eq!(drives[2].capacity, Some(3000));
        assert!(rows(&drives, Some(drives[2].id), 0, false).0.iter().any(|r| r.lab == "На устройстве нет библиотеки Steam"));
    }

    #[test] fn device_cards_and_art_use_their_own_data() {
        let d = parse(&json!([
            {"id":0,"drive":"/home/deck","mounted":true,"capacity":100000000000u64,"free":25000000000u64,"apps":[{"id":42,"name":"A game"}]},
            {"id":1,"drive":"/run/media/mmcblk0p1","mounted":true,"capacity":200000000000u64,"free":150000000000u64,"apps":[]}
        ])).unwrap();
        let (r, _) = rows(&d, None, 0, false);
        let cards: Vec<_> = r.iter().filter(|r| r.role == "tab").collect();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].lab, "Внутренний накопитель");
        assert_eq!(cards[1].lab, "Карта microSD");
        assert_eq!(cards[1].full, "sd");
        assert_ne!(cards[0].desc, cards[1].desc);
        assert!(r.iter().any(|r| r.tok == "storage-art:42" && r.k == 6));
    }

    // Row geometry only: this does not measure rendered text or effective clipping.
    #[test] fn storage_fixture_row_geometry_has_no_overlaps() {
        let d = parse(&json!([
            {"id":0,"label":"A long device name that must be elided at the card boundary", "mounted":true,"capacity":1000,"free":400,"game":200,"dlc":100,"workshop":50,"staged":20,"shader":30,"apps":[{"id":1,"name":"A long game title that must be elided at the row boundary","size":42}]},
            {"id":1,"drive":"/run/media/mmcblk0p1","mounted":true,"apps":[]}
        ])).unwrap();
        let (r, _) = rows(&d, None, 0, false);
        for (i, a) in r.iter().enumerate() {
            assert!(a.x >= 270. && a.x + a.w <= 1038. && a.y >= 0. && a.w > 0. && a.h > 0.);
            for b in &r[i + 1..] {
                assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, "overlap: {:?} / {:?}", a.lab, b.lab);
            }
        }
    }

    #[test] fn selected_folder_displays_its_path() {
        let drives = parse(&json!([{"id": 0, "path": "/home/deck/Steam", "mounted": true, "apps": []}])).unwrap();
        let (rows, _) = rows(&drives, None, 0, false);
        assert!(rows.iter().any(|row| row.lab == "/HOME/DECK/STEAM"));
    }

    #[test] fn selecting_drive_filters_apps() {
        let d = parse(&json!([
            {"id":1,"mounted":true,"apps":[{"id":11,"name":"First"}]},
            {"id":2,"mounted":true,"apps":[{"id":22,"name":"Second"}]}
        ])).unwrap();
        let (r, selected) = rows(&d, Some(2), 0, false);
        assert_eq!(selected, Some(2));
        assert!(r.iter().any(|r| r.lab == "Second"));
        assert!(!r.iter().any(|r| r.lab == "First"));
        assert_eq!(rows(&d, Some(99), 0, false).1, Some(1));
    }
    #[test] fn parse_states_and_geometry() {
        assert!(parse(&json!([])).unwrap().is_empty());
        assert!(parse(&json!([{"id":0,"apps":null}])).is_err());
        let d = parse(&json!([{"id":0,"mounted":true,"apps":[{"id":42,"name":"Game","size":"1200"}],"capacity":2000,"free":800}])).unwrap();
        let (r, selected) = rows(&d, None, 0, false); assert_eq!(selected, Some(0));
        assert_eq!(r.iter().filter(|r| r.f).count(), 4);
        let tab = r.iter().find(|r| r.role == "tab").unwrap();
        assert_eq!((tab.x, tab.y, tab.w, tab.h), (270., 140., 358., 76.));
        let game = r.iter().find(|r| r.lab == "Game").unwrap();
        assert_eq!((game.y, game.h), (442., 58.));
        assert!(r.iter().all(|r| r.w > 0. && r.h > 0.));
        for (i, a) in r.iter().enumerate() {
            for b in &r[i + 1..] {
                assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y);
            }
        }
        assert!(r.iter().any(|r| r.lab == "Game"));

    }
    #[test] fn storage_unknown_mount_regression() {
        for mounted in [None, Some(false), Some(true)] {
            for populated in [false, true] {
                let apps = if populated { json!([{"id":42,"name":"Game","size":1000000000}]) } else { json!([]) };
                let d = parse(&json!([{"id":1,"mounted":mounted,"capacity":2000000000u64,"free":1000000000u64,"apps":apps}])).unwrap();
                let (r, _) = rows(&d, None, 0, false);
                assert_eq!(r.iter().any(|r| r.lab == "Game"), mounted == Some(true) && populated);
                assert_eq!(r.iter().any(|r| r.lab == "Нет установленных приложений"), mounted == Some(true) && !populated);
                assert_eq!(r.iter().any(|r| r.lab.contains("Свободно: 0.9 GB из 1.9 GB")), mounted == Some(true));
                assert_eq!(r.iter().any(|r| r.lab == "Приложения: данные недоступны"), mounted != Some(true));
                assert_eq!(r.iter().filter(|r| r.f).count(), 3 + usize::from(mounted == Some(true) && populated));
                assert!(r.iter().filter(|r| r.role == "listitem").all(|r| r.f));
            }
        }
    }

    #[test] fn navigation_moves_between_tabs_and_app_rows_in_both_directions() {
        use crate::screens::settings::spatial;
        let d = parse(&json!([{ "id": 1, "mounted": true, "apps": [
            {"id": 11, "name": "First"}, {"id": 12, "name": "Second"}
        ] }])).unwrap();
        let (rows, _) = rows(&d, None, 0, false);
        let tab = rows.iter().position(|r| r.role == "tab").unwrap();
        let first = rows.iter().position(|r| r.lab == "First").unwrap();
        let second = rows.iter().position(|r| r.lab == "Second").unwrap();
        let sort = rows.iter().position(|r| r.tok == "storage-sort").unwrap();
        assert_eq!(spatial(&rows, tab, 1), Some(sort));
        assert_eq!(spatial(&rows, sort, 1), Some(first));
        assert_eq!(spatial(&rows, first, 1), Some(second));
        assert_eq!(spatial(&rows, second, 0), Some(first));
        assert_eq!(spatial(&rows, first, 0), Some(sort));
        assert_eq!(spatial(&rows, second, 1), None);
    }

    #[test] fn defaults_to_descending_size_with_unknown_sizes_last() {
        let d = parse(&json!([{ "id": 1, "mounted": true, "apps": [
            {"id": 1, "name": "Small", "size": 10},
            {"id": 2, "name": "Unknown"},
            {"id": 3, "name": "Large", "size": 30},
            {"id": 4, "name": "Equal", "size": 10}
        ] }])).unwrap();
        let (rows, _) = rows(&d, None, 0, false);
        let names: Vec<_> = rows.iter().filter(|r| r.role == "listitem").map(|r| r.lab.as_str()).collect();
        assert_eq!(names, ["Large", "Small", "Equal", "Unknown"]);
    }

    #[test] fn sort_menu_choices_and_direction_preserve_unknown_sizes() {
        let d = parse(&json!([{ "id": 1, "mounted": true, "apps": [
            {"id": 1, "name": "Small", "size": 10}, {"id": 2, "name": "Unknown"},
            {"id": 3, "name": "Large", "size": 30}
        ] }])).unwrap();
        let (menu, _) = rows(&d, None, 0, true);
        assert_eq!(menu.iter().filter(|r| r.tok.starts_with("storage-sort-option:")).count(), 4);
        assert!(menu.iter().any(|r| r.tok == "storage-direction"));
        let names: Vec<_> = rows(&d, None, 3, false).0.into_iter().filter(|r| r.role == "listitem").map(|r| r.lab).collect();
        assert_eq!(names, ["Small", "Large", "Unknown"]);
    }

    #[test] fn storage_bar_uses_only_confirmed_capacity_and_free_space() {
        for (mounted, capacity, free, widths) in [
            (Some(true), Some(100), Some(25), Some((576., 192.))),
            (Some(true), Some(100), Some(101), None),
            (Some(true), None, Some(25), None),
            (Some(false), Some(100), Some(25), None),
        ] {
            let d = Drive { id: 1, label: String::new(), drive: String::new(), path: String::new(), fixed: None, mounted, capacity, free, apps: vec![], categories: [None; 7] };
            let (rows, _) = rows(&[d], None, 0, false);
            // Bar segments are 12 px high; the legend's Free swatch shares kind 17 but is 10 px.
            let bar: Vec<_> = rows.iter().filter(|r| (r.k == 16 || r.k == 17) && r.h == 12.).map(|r| r.w).collect();
            match widths { Some((used, free)) => assert_eq!(bar, vec![used, free]), None => assert!(bar.is_empty()) }
        }
    }

    #[test] fn categories_and_sort_modes_use_steam_data() {
        let d = parse(&json!([{ "id": 1, "mounted": true, "capacity": 1000, "free": 400,
            "game": 200, "dlc": 100, "workshop": 50, "staged": 20, "shader": 30, "media": 20, "used": 580,
            "apps": [{"id": 1, "name": "Zulu", "sort": "zulu", "size": 100, "played": 10},
                     {"id": 2, "name": "Alpha", "sort": "alpha", "size": 10, "played": 20}] }])).unwrap();
        let names = |sort| rows(&d, None, sort, false).0.into_iter().filter(|r| r.role == "listitem").map(|r| r.lab).collect::<Vec<_>>();
        assert_eq!(names(0), ["Zulu", "Alpha"]);
        assert_eq!(names(1), ["Alpha", "Zulu"]);
        assert_eq!(names(2), ["Alpha", "Zulu"]);
        let r = rows(&d, None, 0, false).0;
        assert!(r.iter().any(|r| r.lab.starts_with("ИГРЫ")));
        assert_eq!(r.iter().filter(|r| (18..=22).contains(&r.k) && r.h == 12.).count(), 5);
    }

    // Layout lint for the legend: eight entries fill three rows, and with three device cards the
    // lowest row must still end above the app-list header. Sizes are binary GiB.
    #[test] fn legend_has_other_and_free_and_stays_clear_of_the_app_header() {
        let d = parse(&json!([
            {"id":0,"mounted":true,"capacity":10737418240u64,"free":5368709120u64,"used":3221225472u64,
             "game":1073741824,"dlc":268435456,"workshop":268435456,"staged":536870912,"shader":268435456,"media":268435456,
             "apps":[{"id":1,"name":"Game","size":1073741824}]},
            {"id":1,"drive":"/run/media/mmcblk0p1","mounted":true,"apps":[]},
            {"id":2,"label":"Third","mounted":true,"apps":[]}
        ])).unwrap();
        let (r, _) = rows(&d, Some(0), 0, false);
        let legend: Vec<_> = r.iter().filter(|r| r.k == 12 && r.lab.contains("  ")).collect();
        assert_eq!(legend.len(), 8);
        assert!(r.iter().any(|r| r.lab == "ДРУГОЕ  1.5 GB" && r.k == 12));
        assert!(r.iter().any(|r| r.lab == "СВОБОДНО  5.0 GB" && r.k == 12));
        assert!(r.iter().any(|r| r.k == 23 && r.w == 10. && r.h == 10.), "other swatch");
        assert!(r.iter().any(|r| r.k == 17 && r.w == 10. && r.h == 10.), "free swatch");
        let header = r.iter().find(|r| r.lab.starts_with("Приложений")).unwrap();
        assert!(legend.iter().all(|l| l.y + l.h <= header.y), "legend overlaps the app header");
        for (i, a) in r.iter().enumerate() {
            for b in &r[i + 1..] {
                assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, "overlap: {:?} / {:?}", a.lab, b.lab);
            }
        }
    }

    /// Row lint for the legend (task 143): every swatch is a 10 px round dot (sub "dot") centred on its text row, the text
    /// starts 16 px right of its dot, is bold (sub "legend") and uppercase, and rows never overlap.
    #[test] fn legend_rows_are_round_dots_with_bold_uppercase_labels() {
        let d = parse(&json!([
            {"id":0,"mounted":true,"capacity":10737418240u64,"free":5368709120u64,"used":3221225472u64,
             "game":1073741824,"dlc":268435456,"workshop":268435456,"staged":536870912,"shader":268435456,"media":268435456,
             "apps":[{"id":1,"name":"Game","size":1073741824}]},
            {"id":1,"drive":"/run/media/mmcblk0p1","mounted":true,"apps":[]}
        ])).unwrap();
        let (r, _) = rows(&d, Some(0), 0, false);
        let dots: Vec<_> = r.iter().filter(|r| r.full == "dot").collect();
        let texts: Vec<_> = r.iter().filter(|r| r.full == "legend").collect();
        assert_eq!(dots.len(), 8, "one dot per legend entry");
        assert_eq!(texts.len(), 8, "one bold label per legend entry");
        for dot in &dots {
            assert!(matches!(dot.k, 17 | 18..=23 | 25), "dot kind {}", dot.k);
            assert_eq!((dot.w, dot.h), (10., 10.), "dot size");
            // the dot's centre (y + 5) lines up with the text row's centre (y + 13 of a 26 px row)
            let text = texts.iter().find(|t| (t.y + 13.) == (dot.y + 5.) && (t.x - (dot.x + 16.)).abs() < 0.5)
                .unwrap_or_else(|| panic!("no label beside dot at {}, {}", dot.x, dot.y));
            assert_eq!(text.lab, text.lab.to_uppercase(), "legend label must be uppercase: {}", text.lab);
            assert!(text.lab.contains("  "), "label has a value: {}", text.lab);
        }
        for (i, a) in r.iter().enumerate() {
            for b in &r[i + 1..] {
                assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, "overlap: {:?} / {:?}", a.lab, b.lab);
            }
        }
    }

    // Without the breakdown (no category data), the bar is used/free only and Other is not listed.
    #[test] fn legend_omits_other_without_the_breakdown() {
        let d = parse(&json!([{"id":0,"mounted":true,"capacity":2000,"free":500,"apps":[]}])).unwrap();
        let (r, _) = rows(&d, None, 0, false);
        assert!(!r.iter().any(|r| r.lab.starts_with("ДРУГОЕ")));
        assert!(r.iter().any(|r| r.lab == "СВОБОДНО  0.0 GB" && r.k == 12));
    }

}
