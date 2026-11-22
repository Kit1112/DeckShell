use crate::screens::expected_text;
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
    assert!(rows(&drives, Some(drives[2].id), 0, false).0.iter().any(|r| r.lab == expected_text("На устройстве нет библиотеки Steam", "There is no Steam library on the device.")));
}

#[test] fn device_cards_and_art_use_their_own_data() {
    let d = parse(&json!([
        {"id":0,"drive":"/home/deck","mounted":true,"capacity":100000000000u64,"free":25000000000u64,"apps":[{"id":42,"name":"A game"}]},
        {"id":1,"drive":"/run/media/mmcblk0p1","mounted":true,"capacity":200000000000u64,"free":150000000000u64,"apps":[]}
    ])).unwrap();
    let (r, _) = rows(&d, None, 0, false);
    let cards: Vec<_> = r.iter().filter(|r| r.role == "tab").collect();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].lab, expected_text("Внутренний накопитель", "Internal storage"));
    assert_eq!(cards[1].lab, expected_text("Карта microSD", "MicroSD card"));
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
            assert_eq!(r.iter().any(|r| r.lab == expected_text("Нет установленных приложений", "No installed applications")), mounted == Some(true) && !populated);
            assert_eq!(r.iter().any(|r| r.lab.contains(expected_text("Свободно: 0.9 GB из 1.9 GB", "Free: 0.9 GB of 1.9 GB"))), mounted == Some(true));
            assert_eq!(r.iter().any(|r| r.lab == expected_text("Приложения: данные недоступны", "Applications: data not available")), mounted != Some(true));
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
    assert!(r.iter().any(|r| r.lab.starts_with(expected_text("ИГРЫ", "GAMES"))));
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
    assert!(r.iter().any(|r| r.lab == expected_text("ДРУГОЕ  1.5 GB", "OTHER  1.5 GB") && r.k == 12));
    assert!(r.iter().any(|r| r.lab == expected_text("СВОБОДНО  5.0 GB", "FREE  5.0 GB") && r.k == 12));
    assert!(r.iter().any(|r| r.k == 23 && r.w == 10. && r.h == 10.), "other swatch");
    assert!(r.iter().any(|r| r.k == 17 && r.w == 10. && r.h == 10.), "free swatch");
    let header = r.iter().find(|r| r.lab.starts_with(expected_text("Приложений", "Apps"))).unwrap();
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
    assert!(!r.iter().any(|r| r.lab.starts_with(expected_text("ДРУГОЕ", "OTHER"))));
    assert!(r.iter().any(|r| r.lab == expected_text("СВОБОДНО  0.0 GB", "FREE  0.0 GB") && r.k == 12));
}
