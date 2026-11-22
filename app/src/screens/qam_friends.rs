//! QAM Friends data: one bridge expression (`qam_friends.js`, sources listed there) returns the four Steam Friends tab lists
//! (Favorites, Friends, Groups, Recent messages), the pending invite count and the list-ready flag. This module parses that
//! object and holds the tab model. Steam order and names: `FriendsListSteamDeckTopSection` / `rgTabs` in sp.js
//! (enum Favorites=0, FriendsList=1, GroupChats=2, RecentMessages=3; LB/RB step the active tab by one, clamped, no wrap).
use serde_json::Value;

use super::Friend;

pub const FRIENDS_JS: &str = include_str!("qam_friends.js");

/// Tab order of the strip (Steam `rgTabs` order == enum order).
#[cfg(test)]
pub const TAB_NAMES: [&str; 4] = ["Favorites", "Friends", "Groups", "Recent"];
pub const TAB_FAVORITES: usize = 0;
pub const TAB_FRIENDS: usize = 1;
pub const TAB_GROUPS: usize = 2;
pub const TAB_RECENT: usize = 3;
pub const TABS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Friend,
    Group,
    Clan,
}

/// One favorite / group / recent chat row. Friend rows keep the same fields as the Friends list (`Friend`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub name: String,
    pub state: i64,
    pub game: String,
    pub acc: u32,
    /// chat room group id (groups, recent group chats, favorite chats); empty for friends and clans
    pub gid: String,
    pub avatar_available: bool,
    pub avatar_url: String,
    /// unread messages (friend chat count, or 1 for an unread group)
    pub unread: u32,
    pub mention: bool,
    pub priority: bool,
}

impl Entry {
    /// The friend view of a friend entry (same fields as the Friends list).
    pub(super) fn as_friend(&self) -> Friend {
        Friend {
            acc: self.acc,
            name: self.name.clone(),
            state: self.state,
            game: self.game.clone(),
            avatar_available: self.avatar_available,
            avatar_url: self.avatar_url.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct Lists {
    /// `FriendStore.friends_list_ready` (false: keep showing the previous lists)
    pub(super) ready: bool,
    pub(super) friends: Vec<Friend>,
    pub(super) favorites: Vec<Entry>,
    pub(super) groups: Vec<Entry>,
    pub(super) recent: Vec<Entry>,
    /// FriendStore.GetPendingInviteCount(): the Invites button shows only above 0
    pub(super) invites: u32,
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn friend_of(v: &Value, k: &str) -> Entry {
    Entry {
        kind: Kind::Friend,
        name: str_of(v, "n"),
        state: v.get("s").and_then(Value::as_i64).unwrap_or(0),
        game: str_of(v, "g"),
        acc: v.get("a").and_then(Value::as_u64).unwrap_or(0) as u32,
        gid: String::new(),
        avatar_available: v.get("av").and_then(Value::as_bool).unwrap_or(false),
        avatar_url: str_of(v, "u"),
        unread: v.get("c").and_then(Value::as_u64).unwrap_or(0) as u32,
        mention: v.get("m").and_then(Value::as_u64).unwrap_or(0) != 0,
        priority: v.get("p").and_then(Value::as_u64).unwrap_or(0) != 0,
    }
    .with_kind(k)
}

impl Entry {
    fn with_kind(mut self, k: &str) -> Entry {
        self.kind = match k {
            "g" => Kind::Group,
            "c" => Kind::Clan,
            _ => Kind::Friend,
        };
        self
    }
}

fn group_of(v: &Value) -> Entry {
    let mut e = Entry { kind: Kind::Group, name: str_of(v, "n"), gid: str_of(v, "id"), ..Default::default() };
    e.unread = v.get("c").and_then(Value::as_u64).unwrap_or(0) as u32;
    e.mention = v.get("m").and_then(Value::as_u64).unwrap_or(0) != 0;
    e.priority = v.get("p").and_then(Value::as_u64).unwrap_or(0) != 0;
    e
}

/// Parse the bridge object. `None` when the value is not an object (the caller keeps the previous lists and retries).
pub(super) fn parse(v: &Value) -> Option<Lists> {
    let obj = v.as_object()?;
    let arr = |k: &str| obj.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let friends = arr("friends")
        .iter()
        .map(|f| friend_of(f, "f").as_friend())
        .collect();
    let favorites = arr("favorites")
        .iter()
        .map(|f| match f.get("k").and_then(Value::as_str) {
            Some("g") => group_of(f),
            Some("c") => Entry { kind: Kind::Clan, name: str_of(f, "n"), ..Default::default() },
            _ => friend_of(f, "f"),
        })
        .collect();
    let groups = arr("groups").iter().map(group_of).collect();
    let recent = arr("recent")
        .iter()
        .map(|f| match f.get("k").and_then(Value::as_str) {
            Some("g") => group_of(f),
            _ => friend_of(f, "f"),
        })
        .collect();
    Some(Lists {
        ready: obj.get("ready").and_then(Value::as_bool).unwrap_or(true),
        friends,
        favorites,
        groups,
        recent,
        invites: obj.get("invites").and_then(Value::as_u64).unwrap_or(0) as u32,
    })
}

/// Unread badge on the Recent messages tab: a count below 100, "!" at 100 or more, "@" for a single unread mention.
pub fn badge_text(recent: &[Entry]) -> Option<String> {
    let total: u32 = recent.iter().map(|e| e.unread).sum();
    let mention = recent.iter().any(|e| e.mention);
    if total == 0 {
        return None;
    }
    if total >= 100 {
        Some("!".into())
    } else if mention && total == 1 {
        Some("@".into())
    } else {
        Some(total.to_string())
    }
}

/// LB / RB on the Friends list: the sibling tab, one step, clamped at both ends (Steam `SelectFriendsListSteamDeckSiblingTab`:
/// `B = active + v; if 0 <= B <= 3 then active = B`). `None` = no move.
pub fn sibling(ftab: usize, d: i32) -> Option<usize> {
    let t = ftab as i32 + d;
    (0..TABS as i32).contains(&t).then_some(t as usize)
}

/// Default tab when the lists first load: Steam opens Favorites when the account has favorites, else Friends.
pub(super) fn initial_tab(lists: &Lists) -> usize {
    if lists.favorites.is_empty() { TAB_FRIENDS } else { TAB_FAVORITES }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("qam_friends_fixture.json");

    fn fixture() -> Lists {
        let v: Value = serde_json::from_str(FIXTURE).expect("fixture json");
        parse(&v).expect("fixture object")
    }

    #[test]
    fn fixture_lists_keep_steam_order_and_kinds() {
        let l = fixture();
        assert!(l.ready);
        assert_eq!(l.invites, 2);
        let fav: Vec<_> = l.favorites.iter().map(|e| (e.kind, e.name.as_str())).collect();
        assert_eq!(fav, vec![(Kind::Friend, "Alpha"), (Kind::Group, "Raid Night"), (Kind::Clan, "Deck Club"), (Kind::Friend, "Bravo")]);
        assert_eq!(l.friends.len(), 3);
        assert_eq!(l.friends[1].name, "Charlie");
        assert_eq!(l.friends[1].game, "Portal 2");
        assert_eq!(l.groups.iter().map(|e| e.gid.as_str()).collect::<Vec<_>>(), vec!["103582791430000001", "103582791430000002"]);
        assert_eq!(l.recent.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![Kind::Friend, Kind::Group, Kind::Friend]);
    }

    #[test]
    fn recent_friend_chat_carries_unread_and_priority() {
        let l = fixture();
        let bravo = &l.recent[0];
        assert_eq!(bravo.name, "Bravo");
        assert_eq!(bravo.unread, 3);
        assert!(bravo.priority);
        assert!(!bravo.mention);
        assert_eq!(l.recent[1].gid, "103582791430000002");
        assert_eq!(l.recent[1].unread, 1);
    }

    #[test]
    fn badge_follows_source_rules() {
        let l = fixture();
        assert_eq!(badge_text(&l.recent).as_deref(), Some("5"));
        assert_eq!(badge_text(&[]), None);
        let mut one = Entry { unread: 1, ..Default::default() };
        assert_eq!(badge_text(std::slice::from_ref(&one)).as_deref(), Some("1"));
        one.mention = true;
        assert_eq!(badge_text(std::slice::from_ref(&one)).as_deref(), Some("@"));
        let many = Entry { unread: 150, ..Default::default() };
        assert_eq!(badge_text(std::slice::from_ref(&many)).as_deref(), Some("!"));
    }

    #[test]
    fn initial_tab_is_favorites_only_when_there_are_favorites() {
        let mut l = fixture();
        assert_eq!(initial_tab(&l), TAB_FAVORITES);
        l.favorites.clear();
        assert_eq!(initial_tab(&l), TAB_FRIENDS);
    }

    #[test]
    fn non_object_result_is_rejected_and_missing_lists_are_empty() {
        assert!(parse(&serde_json::json!([])).is_none());
        let l = parse(&serde_json::json!({})).unwrap();
        assert!(l.ready);
        assert!(l.favorites.is_empty() && l.groups.is_empty() && l.recent.is_empty() && l.friends.is_empty());
        assert_eq!(l.invites, 0);
    }

    #[test]
    fn lb_rb_step_one_tab_and_clamp_without_wrap() {
        assert_eq!(sibling(TAB_FAVORITES, -1), None);
        assert_eq!(sibling(TAB_FAVORITES, 1), Some(TAB_FRIENDS));
        assert_eq!(sibling(TAB_FRIENDS, -1), Some(TAB_FAVORITES));
        assert_eq!(sibling(TAB_GROUPS, 1), Some(TAB_RECENT));
        assert_eq!(sibling(TAB_RECENT, 1), None);
    }

    #[test]
    fn tab_order_matches_steam_enum() {
        assert_eq!(TAB_NAMES, ["Favorites", "Friends", "Groups", "Recent"]);
        assert_eq!((TAB_FAVORITES, TAB_FRIENDS, TAB_GROUPS, TAB_RECENT), (0, 1, 2, 3));
    }
}
