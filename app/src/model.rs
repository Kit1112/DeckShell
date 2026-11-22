//! Merge Steam bridge data into the steam-data `Library` the UI works with, and library sort/section logic
//! (docs/engine/library.json).
use std::cmp::Ordering;
use std::collections::HashMap;

use steam_data::{Game, GameKind, Library};

use crate::backend::LibData;
use crate::assets::t_or;

/// Per-app data the bridge knows and steam-data does not.
#[derive(Debug, Clone, Default)]
pub struct Extra {
    pub display_status: u32,
    pub status_pct: u32,
    pub release: u64,
    pub review_pct: u32,
    pub recent_activity: u64,
    pub app_type: u32,
    pub playtime_2w: u32,
    /// steam_deck_compat_category (steam_hw_compat_category_packed & 3, sp.js:436198): 0 unknown, 1 unsupported, 2 playable, 3 verified.
    pub compat: u8,
}

/// display_status values (Steam EAppDisplayStatus) the UI cares about.
pub const DS_UPDATE: u32 = 21;

// Bridge artwork can be partially resolved; preserve a local source when Steam
// did not resolve that slot. Artwork fallbacks must still exist on disk.
fn resolved_icon(bridge: Option<&std::path::PathBuf>, local: Option<&std::path::PathBuf>) -> Option<std::path::PathBuf> {
    bridge.or(local).cloned()
}

#[cfg(test)]
mod icon_tests {
    use super::resolved_icon;
    use std::path::PathBuf;
    #[test]
    fn partial_bridge_art_keeps_shortcut_icon() {
        let local = PathBuf::from("grid/123_icon.ico");
        let remote = PathBuf::from("cache/icon.jpg");
        assert_eq!(resolved_icon(None, Some(&local)), Some(local.clone()));
        assert_eq!(resolved_icon(Some(&remote), Some(&local)), Some(remote));
        assert_eq!(resolved_icon(None, None), None);
    }

    #[test]
    fn account_change_drops_previous_users_art_even_for_the_same_app() {
        use super::*;
        let mut data = LibData { account: 1, apps: vec![steam_bridge::App {
            appid: 123, app_type: 1, visible: true, ..Default::default()
        }], ..Default::default() };
        let (mut base, _) = from_bridge(&Library::default(), &data);
        let private_icon = PathBuf::from("userdata/1/config/grid/123_icon.png");
        base.games[0].art.icon = Some(private_icon.clone());
        assert_eq!(from_bridge(&base, &data).0.games[0].art.icon, Some(private_icon));
        data.account = 2;
        for partial_art in [false, true] {
            if partial_art { data.art.insert(123, Default::default()); }
            let (lib, _) = from_bridge(&base, &data);
            assert_eq!(lib.active_user, Some(2));
            assert_eq!(lib.games[0].art.icon, None, "old custom art must not cross identities");
        }
    }
}

/// Library from bridge data: ALL owned games/shortcuts (installed or not), collections incl. dynamic ones.
/// Art paths come from the file-based `base` library (cold start) or `librarycache` for apps it does not know.
pub fn from_bridge(base: &Library, d: &LibData) -> (Library, HashMap<u32, Extra>) {
    // A stale disk snapshot can belong to the previous user. Its custom grid art
    // and shortcuts must never fill gaps in the new user's bridge artwork.
    let same_account = d.account == 0 || base.active_user == Some(d.account);
    let by_id: HashMap<u32, &Game> = base.games.iter().filter(|_| same_account).map(|g| (g.app_id, g)).collect();
    let cache = base.steam_root.join("appcache/librarycache");
    let mut coll_of: HashMap<u32, Vec<String>> = HashMap::new();
    for c in &d.collections {
        for &a in &c.apps {
            coll_of.entry(a).or_default().push(c.name.clone());
        }
    }
    let mut extras = HashMap::new();
    let mut games = Vec::with_capacity(d.apps.len());
    for a in d.apps.iter().filter(|a| a.is_game() && a.visible) {
        // Steam's own resolution (URL lists -> files of the bridge's active account) wins; file-based guess only until it arrives.
        let art = match (d.art.get(&a.appid), by_id.get(&a.appid)) {
            (Some(s), _) => steam_data::Art {
                capsule: resolved_icon(s.capsule.as_ref(), by_id.get(&a.appid).and_then(|g| g.art.capsule.as_ref()).filter(|p| p.is_file())),
                capsule_animated: if s.capsule.is_some() { s.capsule_animated.clone() } else { by_id.get(&a.appid).and_then(|g| g.art.capsule_animated.as_ref()).filter(|p| p.is_file()).cloned() },
                hero: resolved_icon(s.hero.as_ref(), by_id.get(&a.appid).and_then(|g| g.art.hero.as_ref()).filter(|p| p.is_file())),
                logo: resolved_icon(s.logo.as_ref(), by_id.get(&a.appid).and_then(|g| g.art.logo.as_ref()).filter(|p| p.is_file())),
                header: resolved_icon(s.header.as_ref(), by_id.get(&a.appid).and_then(|g| g.art.header.as_ref()).filter(|p| p.is_file())),
                icon: resolved_icon(s.icon.as_ref(), by_id.get(&a.appid).and_then(|g| g.art.icon.as_ref())),
            },
            (None, Some(g)) => g.art.clone(),
            (None, None) => steam_data::art::librarycache(&cache, &a.appid.to_string()),
        };
        let name = a.display_name.clone();
        games.push(Game {
            app_id: a.appid,
            kind: if a.is_shortcut { GameKind::Shortcut } else { GameKind::Steam },
            sort_name: if a.sort_as.is_empty() { steam_data::sort_name(&name) } else { a.sort_as.to_lowercase() },
            name,
            installed: a.installed,
            install_dir: by_id.get(&a.appid).and_then(|g| g.install_dir.clone()),
            size_on_disk: a.size_on_disk,
            playtime_minutes: a.playtime_minutes,
            last_played: (a.last_played > 0).then_some(a.last_played),
            update_pending: a.display_status == DS_UPDATE,
            collections: coll_of.remove(&a.appid).unwrap_or_default(),
            art,
        });
        extras.insert(
            a.appid,
            Extra {
                display_status: a.display_status,
                status_pct: a.status_percentage,
                release: a.release_date,
                review_pct: a.review_percentage,
                recent_activity: a.recent_activity_time,
                app_type: a.app_type,
                playtime_2w: a.playtime_2weeks_minutes,
                compat: (a.deck_compat_packed & 3) as u8,
            },
        );
    }
    let lib = Library {
        steam_root: base.steam_root.clone(),
        users: base.users.clone(),
        active_user: if d.account != 0 { Some(d.account) } else { base.active_user },
        games,
        collections: d.collections.iter().map(|c| c.name.clone()).collect(),
    };
    (lib, extras)
}

// ------------------------------------------------------------------ sorting (library.json "sort")

/// Gamepad sort menu order (LastUpdated is not offered). Index = persisted setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Alphabetical,
    FriendsPlaying,
    PctAchievementsComplete,
    HoursPlayed,
    LastPlayed,
    ReleaseDate,
    AddedToLibrary,
    SizeOnDisk,
    Metacritic,
    SteamReview,
}

pub const SORTS: [Sort; 10] = [
    Sort::Alphabetical, Sort::FriendsPlaying, Sort::PctAchievementsComplete, Sort::HoursPlayed, Sort::LastPlayed,
    Sort::ReleaseDate, Sort::AddedToLibrary, Sort::SizeOnDisk, Sort::Metacritic, Sort::SteamReview,
];

impl Sort {
    pub fn token(self) -> &'static str {
        match self {
            Sort::Alphabetical => "#Library_SortByAlphabetical",
            Sort::FriendsPlaying => "#Library_SortByFriendsPlaying",
            Sort::PctAchievementsComplete => "#Library_SortByPctAchievementsComplete",
            Sort::HoursPlayed => "#Library_SortByHoursPlayed",
            Sort::LastPlayed => "#Library_SortByLastPlayed",
            Sort::ReleaseDate => "#Library_SortByReleaseDate",
            Sort::AddedToLibrary => "#Library_SortByAddedToLibrary",
            Sort::SizeOnDisk => "#Library_SortBySizeOnDisk",
            Sort::Metacritic => "#Library_SortByMetacriticScore",
            Sort::SteamReview => "#Library_SortBySteamReview",
        }
    }
}

/// Primary comparator; the caller falls back to `sort_as` (Alphabetical = no primary), like `CompareSortAs`.
/// Data we do not have (friends playing, achievements %, metacritic, purchase date) compares equal -> alphabetical.
pub fn primary(s: Sort, a: &Game, b: &Game, ea: &Extra, eb: &Extra) -> Ordering {
    match s {
        Sort::HoursPlayed => b.playtime_minutes.cmp(&a.playtime_minutes),
        Sort::LastPlayed => b.last_played.cmp(&a.last_played),
        // unreleased (date 0) first, then newer first
        Sort::ReleaseDate => (ea.release == 0).cmp(&(eb.release == 0)).reverse().then(eb.release.cmp(&ea.release)),
        // installed first, then bigger size first
        Sort::SizeOnDisk => b.installed.cmp(&a.installed).then(b.size_on_disk.cmp(&a.size_on_disk)),
        Sort::SteamReview => eb.review_pct.cmp(&ea.review_pct),
        Sort::AddedToLibrary => eb.recent_activity.cmp(&ea.recent_activity),
        Sort::Alphabetical | Sort::FriendsPlaying | Sort::PctAchievementsComplete | Sort::Metacritic => Ordering::Equal,
    }
}

fn current_year() -> i32 {
    use chrono::Datelike;
    chrono::Local::now().year()
}

/// Section header text for a game under a sort ("" = no header) - library.json grouping_section_headers.
pub fn section(s: Sort, g: &Game, e: &Extra) -> String {
    match s {
        Sort::HoursPlayed => {
            let m = g.playtime_minutes;
            let tok = match m {
                6000.. => "#GridSection_Over100Hours",
                3600.. => "#GridSection_Over60Hours",
                2400.. => "#GridSection_Over40Hours",
                1200.. => "#GridSection_Over20Hours",
                600.. => "#GridSection_Over10Hours",
                60.. => "#GridSection_Over1Hour",
                1.. => "#GridSection_LessThan1Hour",
                0 => "#GridSection_NoPlaytime",
            };
            t_or(tok, "")
        }
        Sort::LastPlayed => match g.last_played {
            None => t_or("#GridSection_Never", "Никогда"),
            Some(ts) if ts < 31_536_000 => t_or("#GridSection_Never", "Никогда"),
            Some(ts) => {
                use chrono::Datelike;
                let Some(dt) = chrono::DateTime::from_timestamp(ts as i64, 0) else { return String::new() };
                let l = dt.with_timezone(&chrono::Local);
                if l.year() < current_year() - 10 {
                    return t_or("#GridSection_MoreThan10YearsAgo", "");
                }
                format!("{} {}", month_name(l.month0() as usize), l.year())
            }
        },
        Sort::ReleaseDate => {
            if e.release == 0 {
                t_or("#AppBox_ComingSoon", "")
            } else {
                use chrono::Datelike;
                chrono::DateTime::from_timestamp(e.release as i64, 0).map(|d| d.year().to_string()).unwrap_or_default()
            }
        }
        Sort::SizeOnDisk => {
            let gb = g.size_on_disk as f64 / 1e9;
            let tok = if gb > 100.0 {
                "#GridSection_GreaterThan100GB"
            } else if gb > 50.0 {
                "#GridSection_GreaterThan50GB"
            } else if gb > 20.0 {
                "#GridSection_GreaterThan20GB"
            } else if g.size_on_disk > 0 {
                "#GridSection_LessThan20GB"
            } else if g.installed {
                "#GridSection_NoSizeOnDisk"
            } else {
                "#GridSection_NotInstalled"
            };
            t_or(tok, "")
        }
        _ => String::new(),
    }
}

fn month_name(m0: usize) -> String {
    const FALLBACK: [&str; 12] = ["январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь"];
    FALLBACK[m0 % 12].to_string()
}
