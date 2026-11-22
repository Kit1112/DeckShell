//! Quick Access Menu ("..." button): rail with 5 tabs (notifications, friends, quick settings, performance, Decky) and a content column.
//! Look: app/ui/generated/menus (compile-ui, dumps qam.json + QuickAccess Decky dump); this file builds the row model of every tab, owns
//! focus through a private instance of the Steam focus engine (`crate::nav::Tree`: Row[rail Column, content Column]) and the handlers
//! (docs/engine/qam.json): brightness / output + mic volume / airplane / Wi-Fi / UI scale, performance overlay + advanced (TDP, fps, refresh).
//! System writes use dry-run-aware actions or the corresponding settings backend.
//! Tabs hidden by the user's "QAM Hide Tabs" theme (Help, Soundtrack, VR, voice chat) are not part of the rail: only the 5 live tabs exist.
use steam_assets::i18n;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use deck_system::bluetooth::{self as bt, BluetoothBackend};
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use crate::actions::{self, Act};
use crate::app::App;
use crate::assets::{try_icon_image, t, t_or, tu};
use crate::data::DataSource;
use crate::input::Action;
use crate::nav::{self, FocusSource, Layout, NavEvent, NodeId, Props, Rect as NRect, Tree};
use crate::sound::{play, Snd};
use crate::{AppWindow, QamRow, QamState, TopBarState};

#[path = "qam_avatars.rs"]
pub mod qam_avatars;
#[path = "qam_friends.rs"]
pub mod qam_friends;
mod shortcuts;
use qam_avatars::{get_loader, resolve_avatar_url, should_rebuild_on_avatar_ready};
use qam_friends::{Entry, Lists};

// ---- row metrics (device px) = generated/menus GMenuGeo (css x 1.2) ----
const Y0: f32 = 76.78;
const H_HDR: f32 = 35.99;
const H_SLIDER: f32 = 52.8;
const H_TOGGLE: f32 = 50.39;
const H_SHORTCUT: f32 = 51.2;
const H_BT_DEVICE: f32 = 48.0;
const H_BTN: f32 = 72.0;
const H_SEP: f32 = 1.19;
const GAP: f32 = 28.8;
const H_NOTCH: f32 = 75.59;
const H_TALL: f32 = 96.0;
const H_ENTRY: f32 = 52.8;
const RAIL_W: f32 = 57.59;
const PANEL_W: f32 = 417.59;
const PANEL_H: f32 = 544.8;
// The five rail tabs are vertically centered in the visible QAM panel.
const TAB_Y0: f32 = (PANEL_H - TABS as f32 * TAB_H) / 2.0;
const TAB_H: f32 = 76.79;
const TITLE_H: f32 = 57.58;
const QUICK_TITLE_EDGE: f32 = 77.6;
const QUICK_BOTTOM_PAD: f32 = 28.0;
const TABS: usize = 5;

const WHITE_DIM: u32 = 0xff_dc_de_df;
const GREY: u32 = 0xff_96_96_96;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    Volume,
    Mic,
    Airplane,
    Wifi,
    Bluetooth,
    BluetoothDevice(u32),
    BluetoothAdd,
    NightMode,
    Brightness,
    UiAuto,
    TextSize,
    Controllers,
    Overlay,
    OverlayLevel,
    Advanced,
    Profile,
    ProfileReset,
    TdpOn,
    Tdp,
    GpuOn,
    Gpu,
    Vrs,
    /// per-app `is_vrr_enabled` (Steam SetVRREnabled), distinct from Vrs (`steamos_vrs_enabled`)
    Vrr,
    Tearing,
    FpsLimit,
    Refresh,
    Filter,
    Scaler,
    Sharp,
    Notif(usize),
    Friend(usize),
    /// Friends tab strip cell (Steam FriendsListTab: focus selects, A activates) = tab index
    FTab(usize),
    /// Friends header buttons (Steam FriendsInvitesButton, AddFriendButton, NewChatGroupButton)
    FInvite,
    FAdd,
    FNewGroup,
    /// Favorites / Recent messages / Groups rows: index into the matching list in State
    FFav(usize),
    FRecent(usize),
    FGroup(usize),
    Plugin(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BluetoothAction {
    Power(bool),
    Device { id: u32, connect: bool },
}

impl BluetoothAction {
    fn apply(self, backend: &dyn BluetoothBackend) -> Result<(), String> {
        match self {
            Self::Power(on) => backend.set_enabled(on),
            Self::Device { id, connect: true } => backend.connect(id),
            Self::Device { id, connect: false } => backend.disconnect(id),
        }
    }
}

#[derive(Default, Clone)]
pub struct Status {
    pub battery: Option<deck_system::Battery>,
    pub wifi: Option<bool>,
    wifi_epoch: u64,
    bluetooth: bt::State,
    bluetooth_epoch: u64,
    night_mode: Option<bool>,
    night_epoch: u64,
    pub brightness: Option<f32>,
    pub volume: Option<(f32, bool)>,
    pub profile: Option<String>,
    pub profiles: Vec<String>,
    pub tdp: Option<(u32, u32, u32)>,
    pub gpu_clock: Option<(bool, Option<u32>, u32, u32)>,
    pub running: Option<u32>,
    pub mic: Option<f32>,
    pub airplane: Option<bool>,
    /// SystemStoragStore.m_msgSettings.is_display_brightness_available (Steam hides the brightness section without it)
    pub brightness_avail: Option<bool>,
    /// PERF_READ result (js.rs): client settings, SteamOSManager limits, SystemPerfStore state; None = bridge down
    pub perf: Option<serde_json::Value>,
    /// gamescope root atoms (gamescope_atoms::read), only while the QAM is open; None = not a gamescope X server
    pub gs: Option<crate::gamescope_atoms::Snapshot>,
    pub mango_overlay: Option<i32>,
    /// true when this poll ran while the QAM was open: only then are the QAM-only reads taken (see `gather_status`)
    pub detailed: bool,
    quick: bool,
    overlay_epoch: u64,
    hardware_epoch: u64,
    brightness_generation: u64,
}

#[derive(Clone, Default)]
struct Notif {
    id: u64,
    label: String,
    appid: Option<u32>,
}

#[derive(Clone, Default)]
struct Friend {
    acc: u32,
    name: String,
    state: i64,
    game: String,
    avatar_available: bool,
    avatar_url: String,
}

/// Performance tab state Steam keeps in SystemPerfStore + client settings (SteamOSManager): mirrored here, refreshed from PERF_READ,
/// written through `Act::Perf` / `Act::ClientSetting` (steam-bridge js.rs; docs/engine/qam.json perf_tab).
struct PerfState {
    overlay: bool,
    level: i32,
    overlay_observed: Option<i32>,
    overlay_pending: Option<(u64, i32)>,
    overlay_write_failed: bool,
    advanced: bool,
    tdp_on: bool,
    tdp_pending: bool,
    tdp_failed: bool,
    gpu_pending: bool,
    gpu_failed: bool,
    /// index into `fps_opts`
    fps: usize,
    fps_opts: Vec<u32>,
    /// GAMESCOPE_FPS_LIMIT as read from gamescope (Some only when the atom exists); used when the Perf channel is absent
    gs_fps: Option<u32>,
    gs_refresh: Option<u32>,
    refresh_game_policy: bool,
    refresh_hz: f32,
    hz_min: f32,
    hz_max: f32,
    ui_auto: bool,
    text_size: f32,
    scale_min: f32,
    scale_max: f32,
    /// per-game profile switch (per_app.is_game_perf_profile_enabled), current game id (SystemPerfStore.nCurrentGameID) and its name
    game_profile: bool,
    game_id: u64,
    game_name: String,
    gpu_on: bool,
    gpu_mhz: f32,
    gpu_min: f32,
    gpu_max: f32,
    gpu_avail: bool,
    vrs: bool,
    /// per_app.is_vrr_enabled (readback only; written by Item::Vrr via SetVRREnabled)
    vrr: bool,
    /// limits.is_vrr_supported: Steam shows the VRR toggle only when true (absent = false)
    vrr_supported: bool,
    gs_vrr_capable: Option<bool>,
    vrs_avail: bool,
    /// gamescope_allow_tearing client setting
    tearing: bool,
    /// ESplitScalingFilter value (1 linear, 2 nearest, 3 sharp) / ESplitScalingScaler (1 auto, 2 integer, 3 fit, 5 stretch, 4 fill) / FSR 0..8
    filter: i32,
    scaler: i32,
    gs_filter: Option<u32>,
    gs_scaler: Option<u32>,
    sharp: i32,
    sharp_min: i32,
    sharp_max: i32,
    /// SteamClient.System.Perf exists (gamescope session). None = not asked yet / bridge down: assume yes (Deck game mode).
    perf_avail: Option<bool>,
    refresh_avail: bool,
}
impl Default for PerfState {
    fn default() -> Self {
        PerfState {
            overlay: false, level: 1, overlay_observed: None, overlay_pending: None, overlay_write_failed: false, advanced: false, tdp_on: false, tdp_pending: false, tdp_failed: false, gpu_pending: false, gpu_failed: false, fps: 0, fps_opts: FPS_OPTIONS.to_vec(), gs_fps: None, gs_refresh: None, refresh_game_policy: false, refresh_hz: 60.0, hz_min: 40.0, hz_max: 60.0,
            ui_auto: true, text_size: 0.5, scale_min: 0.5, scale_max: 2.5, game_profile: false, game_id: 0, game_name: String::new(),
            gpu_on: false, gpu_mhz: 1600.0, gpu_min: 200.0, gpu_max: 1600.0, gpu_avail: true, vrs: false, vrr: false, vrr_supported: false, gs_vrr_capable: None, vrs_avail: true, tearing: false, filter: 1, scaler: 1, gs_filter: None, gs_scaler: None, sharp: 0, sharp_min: 0,
            sharp_max: 8, perf_avail: None, refresh_avail: true,
        }
    }
}
fn accept_overlay_result(p: &mut PerfState, generation: u64, observed: Option<i32>, ambiguous: bool) -> bool {
    if p.overlay_pending.map(|(g, _)| g) != Some(generation) { return false; }
    p.overlay_pending = None;
    if ambiguous { p.overlay_write_failed = true; }
    p.overlay_observed = observed;
    if let Some(level) = observed {
        p.overlay = level > 0;
        if level > 0 { p.level = level; }
    }
    true
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn service_state_distinguishes_off_and_unknown() {
        assert_eq!(actions::observed_perf_overlay(&json!({"perf":{"global":{"perf_overlay_service_state":1,"perf_overlay_level":0}}})), Some(0));
        assert_eq!(actions::observed_perf_overlay(&json!({"perf":{"global":{"perf_overlay_service_state":2,"perf_overlay_level":3}}})), Some(4));
        assert_eq!(actions::observed_perf_overlay(&json!({"perf":{"global":{"perf_overlay_service_state":2}}})), None);
        assert_eq!(actions::observed_perf_overlay(&json!({"perf":null})), None);
    }
    #[test]
    fn stale_or_reversed_replies_do_not_replace_latest() {
        let mut p = PerfState::default();
        p.overlay_pending = Some((2, 4));
        assert!(!accept_overlay_result(&mut p, 1, Some(1), false));
        assert_eq!(p.overlay_observed, None);
        assert!(accept_overlay_result(&mut p, 2, Some(4), false));
        assert!(!accept_overlay_result(&mut p, 1, Some(1), false));
        assert_eq!(p.overlay_observed, Some(4));
        p.overlay_pending = Some((3, 0));
        assert!(accept_overlay_result(&mut p, 3, None, true));
        assert_eq!(p.overlay_observed, None);
        assert_eq!(p.level, 4);
    }
}
fn qam_max_scroll(content_h: f32) -> f32 { (content_h + 24.0 - PANEL_H).max(0.0) }
fn qam_wheel_scroll(scroll: f32, max_scroll: f32, dy: f32) -> f32 {
    (scroll - dy).clamp(0.0, max_scroll)
}
fn qam_focus_scroll(mut scroll: f32, content_h: f32, row: Option<(f32, f32)>, first: bool) -> f32 {
    if let Some((y, h)) = row {
        let top = scroll + TITLE_H + 8.0;
        let bottom = scroll + PANEL_H - 12.0;
        if y < top { scroll -= top - y; }
        else if y + h > bottom { scroll += y + h - bottom; }
        if first { scroll = 0.0; }
    }
    scroll.clamp(0.0, qam_max_scroll(content_h))
}

#[cfg(test)]
mod viewport_tests {
    use super::*;
    #[test]
    fn bottom_focus_and_wheel_stay_inside_viewport() {
        let content_h = 1000.0;
        let (y, h) = (940.0, 60.0);
        let scroll = qam_focus_scroll(0.0, content_h, Some((y, h)), false);
        assert!(y + h <= scroll + PANEL_H - 12.0);
        assert_eq!(qam_wheel_scroll(0.0, qam_max_scroll(content_h), -10000.0), qam_max_scroll(content_h));
        assert_eq!(qam_wheel_scroll(scroll, qam_max_scroll(content_h), 10000.0), 0.0);
    }
}

/// Friend row fields (name, status line, avatar); the avatar is requested when it is not cached yet.
fn friend_fields(r: &mut QamRow, f: &Friend, avatars: &HashMap<String, Image>, mut request: impl FnMut(&str)) {
    r.label = f.name.as_str().into();
    let (sub, tint) = if !f.game.is_empty() {
        (f.game.clone(), 0xff_90_ba_3c)
    } else if f.state == 0 {
        (t_or("#PersonaStateOffline", i18n::tr_ref("qam.offline")), GREY)
    } else {
        (t_or("#PersonaStateOnline", i18n::tr_ref("qam.online")), 0xff_57_cb_de)
    };
    r.sub = sub.into();
    r.tint = slint::Color::from_argb_encoded(tint);
    r.meta = "button||A,START,FOCUS,touch,mouse".into();
    let url = resolve_avatar_url(&f.avatar_url);
    if let Some(img) = avatars.get(&url) {
        r.icon = img.clone();
        r.has_icon = true;
    } else {
        r.has_icon = false;
        request(&url);
    }
}

/// Everything the Friends tab shows, borrowed from the QAM state (free function so the focus-table tests can build it).
struct FriendsView<'a> {
    pub ftab: usize,
    pub invites: u32,
    pub friends: &'a [Friend],
    pub favorites: &'a [Entry],
    pub groups: &'a [Entry],
    pub recent: &'a [Entry],
    pub avatars: &'a HashMap<String, Image>,
    /// icon by Steam asset name (Star, Friends, Groups, Recent, PendingInvites, AddFriend, CreateGroup)
    pub icons: &'a HashMap<&'static str, Image>,
}

/// Steam Friends list (FriendsListSteamDeckTopSection + FriendsListTabPanel): the tab strip (Favorites, Friends, Groups,
/// Recent messages = `rgTabs` order; fids 0..3), the active tab's header with its buttons (one line), then the tab's panel.
fn build_friends_tab(b: &mut B, v: &FriendsView, request: &mut dyn FnMut(&str)) {
    // Stock geometry (task 145, docs/cdp/behavior/qam-friends.tree.json, FriendsListSteamDeckTabs): icon-only tabs,
    // 24 px icons in 30 px cells on a 46 px pitch, centred in the 300 px column, 8 px padding below the strip. Stock CSS px
    // map to DeckShell units by the QAM factor 1.2 (see H_ENTRY). Tabs have no text label and the panel has no title line.
    const SCALE: f32 = 1.2;
    const CELL_H: f32 = 30.0 * SCALE;
    const PITCH: f32 = 46.0 * SCALE;
    const STRIP_PAD: f32 = 8.0 * SCALE;
    const HDR_H: f32 = 34.0 * SCALE;
    const BTN_W: f32 = 48.0;
    let cw = PANEL_W - RAIL_W;
    let strip_x = (cw - PITCH * qam_friends::TABS as f32) / 2.0;
    let names = [
        t_or("#FriendGroup_Favorites", i18n::tr_ref("qam.favorites")),
        t_or("#Label_Friends", i18n::tr_ref("qam.friends")),
        t_or("#Label_Groups", i18n::tr_ref("qam.groups")),
        t_or("#FriendGroup_Recent", i18n::tr_ref("qam.recent")),
    ];
    let icon_names = ["Star", "Friends", "Groups", "Recent"];
    let badge = qam_friends::badge_text(v.recent).unwrap_or_default();
    let icon = |n: &str| v.icons.get(n).cloned().unwrap_or_default();
    let mut cells = vec![];
    for k in 0..qam_friends::TABS {
        let mut r = B::row(10);
        r.label = names[k].as_str().into();
        r.rtext = if k == qam_friends::TAB_RECENT { badge.as_str().into() } else { "".into() };
        r.on = (k == v.ftab) as i32;
        r.icon = icon(icon_names[k]);
        r.has_icon = true;
        r.meta = "tab||A,FOCUS,touch,mouse".into();
        cells.push((r, Some(Item::FTab(k)), strip_x + k as f32 * PITCH, PITCH));
    }
    b.push_cells(cells, CELL_H, 1);
    b.y += STRIP_PAD;

    // header: Steam shows Add friend and (with pending invites) Invites on Friends, New group on Groups; no title line
    let mut buttons: Vec<(Item, &'static str, String)> = vec![];
    match v.ftab {
        qam_friends::TAB_FRIENDS => {
            if v.invites > 0 {
                buttons.push((Item::FInvite, "PendingInvites", v.invites.to_string()));
            }
            buttons.push((Item::FAdd, "AddFriend", String::new()));
        }
        qam_friends::TAB_GROUPS => buttons.push((Item::FNewGroup, "CreateGroup", String::new())),
        _ => {}
    }
    if !buttons.is_empty() {
        let first_x = cw - buttons.len() as f32 * BTN_W;
        let mut hdr = vec![];
        for (i, (item, icon_name, count)) in buttons.into_iter().enumerate() {
            let mut r = B::row(12);
            r.icon = icon(icon_name);
            r.has_icon = true;
            r.rtext = count.as_str().into();
            r.meta = "button||A,FOCUS,touch,mouse".into();
            hdr.push((r, Some(item), first_x + i as f32 * BTN_W, BTN_W));
        }
        b.push_cells(hdr, HDR_H, 2);
    }

    // Panel top padding (Steam CSS): favorites and recent 8 px, groups 4 px, friends 0.
    match v.ftab {
        qam_friends::TAB_FAVORITES | qam_friends::TAB_RECENT => b.y += 8.0 * SCALE,
        qam_friends::TAB_GROUPS => b.y += 4.0 * SCALE,
        _ => {}
    }
    match v.ftab {
        qam_friends::TAB_FAVORITES => qam_entry_rows(b, v.favorites, v, request, Item::FFav, "#FriendsList_Favorites_Hint"),
        qam_friends::TAB_GROUPS => qam_entry_rows(b, v.groups, v, request, Item::FGroup, "#Chat_ChatRoomList_Empty"),
        qam_friends::TAB_RECENT => qam_entry_rows(b, v.recent, v, request, Item::FRecent, "#RecentChats_Empty"),
        _ => build_friend_rows(b, v.friends, v.avatars, |u| request(u)),
    }
}

/// Favorites / Groups / Recent rows. A friend row is focusable (opens its chat), a group row opens the group chat, a clan
/// row is shown but not focusable (Steam's clan chat has no DeckShell route yet).
fn qam_entry_rows(b: &mut B, list: &[qam_friends::Entry], v: &FriendsView, request: &mut dyn FnMut(&str), item: fn(usize) -> Item, empty_key: &str) {
    if list.is_empty() {
        // Steam's own empty-state line for the tab (friendsui localization); no line when the key is missing
        b.text(&t_or(empty_key, ""), H_TOGGLE, GREY);
        return;
    }
    let avatars = v.avatars;
    for (i, e) in list.iter().enumerate() {
        let mut r = B::row(7);
        match e.kind {
            qam_friends::Kind::Friend => {
                friend_fields(&mut r, &e.as_friend(), avatars, |u| request(u));
                b.push(r, H_ENTRY, Some(item(i)));
            }
            qam_friends::Kind::Group => {
                r.label = e.name.as_str().into();
                // groups have no avatar in the snapshot data: the Groups icon fills the avatar slot
                r.icon = v.icons.get("Groups").cloned().unwrap_or_default();
                r.has_icon = true;
                r.sub = if e.unread > 0 { e.unread.to_string().into() } else { "".into() };
                r.tint = slint::Color::from_argb_encoded(0xff_57_cb_de);
                r.meta = "button||A,FOCUS,touch,mouse".into();
                b.push(r, H_ENTRY, Some(item(i)));
            }
            qam_friends::Kind::Clan => {
                r.label = e.name.as_str().into();
                r.tint = slint::Color::from_argb_encoded(GREY);
                b.push(r, H_ENTRY, None);
            }
        }
    }
}

fn build_friend_rows(b: &mut B, friends: &[Friend], avatars: &HashMap<String, Image>, mut request: impl FnMut(&str)) {
    let mut online: Vec<usize> = vec![];
    let mut offline: Vec<usize> = vec![];
    for (i, f) in friends.iter().enumerate() {
        if f.state == 0 { offline.push(i) } else { online.push(i) }
    }
    online.sort_by_key(|&i| (friends[i].game.is_empty(), friends[i].name.to_lowercase()));
    offline.sort_by_key(|&i| friends[i].name.to_lowercase());
    if friends.is_empty() {
        b.text(&t_or("#Friends_Empty01", ""), H_TOGGLE, GREY);
    }

    let mut group = |b: &mut B, title: String, ids: &[usize], friends: &[Friend], divider: u32| {
        if ids.is_empty() {
            return;
        }
        let mut h = B::row(8);
        h.label = format!("{title} ({})", ids.len()).to_uppercase().into();
        h.tint = slint::Color::from_argb_encoded(divider);
        b.push(h, H_HDR, None);
        for &i in ids.iter() {
            let mut r = B::row(7);
            friend_fields(&mut r, &friends[i], avatars, &mut request);
            b.push(r, H_ENTRY, Some(Item::Friend(i)));
        }
    };
    group(b, t_or("#Friends_Online", i18n::tr_ref("qam.online_friends")), &online, friends, 0x4d_4c_b4_ff);
    group(b, t_or("#Friends_Offline", i18n::tr_ref("qam.offline")), &offline, friends, 0xff_3d_44_50);
}

#[cfg(test)]
mod friend_rows_tests {
    use super::*;
    use crate::nav::Button;

    #[test]
    fn qam_friend_groups_show_all_rows() {
        let mut friends = Vec::new();
        let mut avatars = HashMap::new();
        // Reverse input order and interleave groups; online players in a game sort first.
        for n in (0..64).rev() {
            for state in [0, 1] {
                let id = n * 2 + state + 1;
                let url = format!("{:040x}", id);
                let image = Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(id as u32, 1));
                avatars.insert(resolve_avatar_url(&url), image);
                friends.push(Friend { name: format!("Friend {n:02}"), state, game: if state == 1 && n % 2 == 0 { "Game".into() } else { String::new() }, avatar_url: url, ..Default::default() });
            }
        }
        let mut b = B::new();
        build_friend_rows(&mut b, &friends, &avatars, |_| panic!("unexpected avatar request"));
        assert_eq!(b.items.len(), 128);
        assert_eq!(b.rows.len(), 130);
        assert!(b.rows[0].label.ends_with("(64)"));
        assert!(b.rows[65].label.ends_with("(64)"));
        let expected_online = (0..64).filter(|n| n % 2 == 0).chain((0..64).filter(|n| n % 2 != 0));
        for (position, n) in expected_online.chain(0..64).enumerate() {
            let row = &b.rows[position + if position < 64 { 1 } else { 2 }];
            let Item::Friend(index) = b.items[position] else { panic!("not a friend") };
            let friend = &friends[index];
            assert_eq!(friend.name, format!("Friend {n:02}"));
            assert_eq!(row.label.as_str(), friend.name);
            assert!(row.has_icon);
            assert_eq!(row.icon.size(), avatars[&resolve_avatar_url(&friend.avatar_url)].size());
        }
        assert_eq!(b.rows.last().unwrap().label.as_str(), "Friend 63");

        // Exercise the same column navigation and viewport calculations used by QAM.
        let mut tree = Tree::new((1280.0, 800.0));
        let root = tree.root();
        let column = tree.add_node(root, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(RAIL_W, 0.0, PANEL_W - RAIL_W, PANEL_H)));
        let nodes: Vec<_> = b.rects.iter().map(|&(y, h)| tree.add_node(column, Props { focusable: true, ..Default::default() }, Some(NRect::xywh(RAIL_W, y, PANEL_W - RAIL_W, h)))).collect();
        tree.transfer_focus(FocusSource::Application, Some(nodes[0]), None);
        let mut scroll = 0.0;
        for (i, &node) in nodes.iter().enumerate().skip(1) {
            assert!(tree.handle_button(Button::DirDown, false));
            assert_eq!(tree.focused(), Some(node));
            scroll = qam_focus_scroll(scroll, b.y, Some(b.rects[i]), false);
            let (y, h) = b.rects[i];
            assert!(y + h <= scroll + PANEL_H - 12.0 + 0.01);
        }
        let max_scroll = qam_max_scroll(b.y);
        assert_eq!(qam_wheel_scroll(0.0, max_scroll, -100000.0), max_scroll);
        assert!(scroll > 0.0);
    }
}

const FPS_OPTIONS: [u32; 6] = [0, 15, 30, 40, 45, 60];
const FILTERS: [i32; 3] = [1, 2, 3];
const SCALERS: [i32; 5] = [1, 2, 3, 5, 4];

fn is_slider(item: Item) -> bool {
    matches!(item, Item::Volume | Item::Mic | Item::Brightness)
}

#[derive(Debug, PartialEq, Eq)]
enum CancelTarget { Slider, Rail, Close }
fn cancel_target(adjusting: bool, on_rail: bool) -> CancelTarget {
    if adjusting { CancelTarget::Slider } else if !on_rail { CancelTarget::Rail } else { CancelTarget::Close }
}

#[cfg(test)]
mod focus_level_tests {
    use super::*;
    #[test]
    fn brightness_readback_updates_qam_and_visible_popup() {
        let mut status = Status { brightness: Some(0.7), ..Default::default() };
        assert_eq!(brightness_readback(&mut status, Some((0.42, false)), true), Some(0.42));
        assert_eq!(status.brightness, Some(0.42));
        assert_eq!(brightness_readback(&mut status, Some((0.41, false)), false), None);
        assert_eq!(status.brightness, Some(0.41));
        assert_eq!(brightness_readback(&mut status, None, true), None);
        assert_eq!(status.brightness, None);
    }

    #[test]
    fn mic_gain_uses_steam_deck_range() {
        assert!((mic_to_slider(1.3) - 1.0).abs() < 1e-6);
        assert!((slider_to_mic(0.5) - 0.65).abs() < 1e-6);
        assert_eq!(slider_to_mic(2.0), 1.3);
    }

    #[test]
    fn focused_output_levels_adjust_without_entering_slider_mode() {
        assert!(direct_level_slider(Item::Brightness));
        assert!(direct_level_slider(Item::Volume));
        assert!(!direct_level_slider(Item::Mic));
        assert!(!direct_level_slider(Item::Airplane));
    }

    #[test]
    fn cancel_unwinds_slider_then_panel_then_menu() {
        assert_eq!(cancel_target(true, false), CancelTarget::Slider);
        assert_eq!(cancel_target(false, false), CancelTarget::Rail);
        assert_eq!(cancel_target(false, true), CancelTarget::Close);
        assert!(is_slider(Item::Brightness));
        assert!(!is_slider(Item::Airplane));
    }
}

fn direct_level_slider(item: Item) -> bool {
    matches!(item, Item::Brightness | Item::Volume)
}

/// Focus nodes for the content rows (shared by the live engine and the focus-table tests). Cells of one line (strip, header
/// buttons) share a Row container spanning them, so Left/Right stays on the line; full-width rows are direct children.
fn add_row_nodes(t: &mut Tree, content: NodeId, tab: usize, rects: &[(f32, f32)], spans: &[(f32, f32, u32)], items: &[Item]) -> Vec<NodeId> {
    let mut extent: HashMap<u32, (f32, f32, f32, f32)> = HashMap::new();
    for (fid, &(y, h)) in rects.iter().enumerate() {
        let (x, w, g) = spans.get(fid).copied().unwrap_or((0.0, PANEL_W - RAIL_W, 0));
        if g != 0 {
            let e = extent.entry(g).or_insert((x, x + w, y, h));
            e.0 = e.0.min(x);
            e.1 = e.1.max(x + w);
        }
    }
    let mut lines: HashMap<u32, NodeId> = HashMap::new();
    let mut row_nodes = vec![];
    for (fid, &(y, h)) in rects.iter().enumerate() {
        let (x, w, g) = spans.get(fid).copied().unwrap_or((0.0, PANEL_W - RAIL_W, 0));
        let parent = if g == 0 {
            content
        } else if let Some(&id) = lines.get(&g) {
            id
        } else {
            let (x0, x1, ly, lh) = extent.get(&g).copied().unwrap_or((x, x + w, y, h));
            let id = t.add_node(content, Props { layout: Layout::Row, ..Default::default() }, Some(NRect::xywh(RAIL_W + x0, ly, x1 - x0, lh)));
            lines.insert(g, id);
            id
        };
        let mut p = Props { focusable: true, nav_key: Some(format!("qamrow:{tab}:{fid}")), ..Default::default() };
        // Preserve existing controls until their individual source behavior is traced.
        if items.get(fid).is_some_and(|it| matches!(it, Item::TextSize | Item::OverlayLevel | Item::Tdp | Item::FpsLimit | Item::Refresh | Item::Gpu | Item::Filter | Item::Scaler | Item::Sharp)) {
            let mk = |d: f32| -> nav::tree::Handler {
                std::rc::Rc::new(move |_| { super::defer(move |a| a.qam_slide(fid as i32, d)); true })
            };
            p.on_move = [None, Some(mk(1.0)), None, Some(mk(-1.0))];
        }
        row_nodes.push(t.add_node(parent, p, Some(NRect::xywh(RAIL_W + x, y, w, h))));
    }
    row_nodes
}

pub struct State {
    pub open: bool,
    /// Signature value for app.rs (tab / zone / focused row): any change counts as a focus move.
    pub focus: i32,
    pub st: Status,
    pub st_stamp: Instant,
    /// Stock SystemPowerStore timers (time-remaining confidence, «battery full» latch).
    pub clock: battery_logic::Clock,
    pub tab: usize,
    pub rail_focus: Option<usize>,
    pub cursor: i32,
    slider_adjusting: bool,
    slider_repeat_start: Option<Instant>,
    items: Vec<Item>,
    rects: Vec<(f32, f32)>,
    /// per focusable item: (x, w, group) in content px; group 0 = full-width row, otherwise cells sharing one Row container
    spans: Vec<(f32, f32, u32)>,
    content_h: f32,
    scroll: f32,
    last_fid: [i32; TABS],
    last_quick_item: Option<Item>,
    bluetooth_pending: Option<BluetoothAction>,
    night_pending: bool,
    tree: Option<Tree>,
    rail_nodes: Vec<NodeId>,
    row_nodes: Vec<NodeId>,
    perf: PerfState,
    friends: Vec<Friend>,
    /// Steam Friends tabs besides the friend list (qam_friends.rs): favorites, groups, recent messages, pending invites
    favorites: Vec<Entry>,
    groups: Vec<Entry>,
    recent: Vec<Entry>,
    invites: u32,
    /// active Friends tab (Steam FriendsListSteamDeckActiveTab) and whether Steam's first-load default was applied
    ftab: usize,
    ftab_defaulted: bool,
    friends_loading: bool,
    notifs: Vec<Notif>,
    plugins: Vec<String>,
    icons: HashMap<&'static str, Image>,
    pub avatar_cache: HashMap<String, Image>,
}
impl Default for State {
    fn default() -> Self {
        State {
            open: false,
            focus: 0,
            st: Status::default(),
            st_stamp: Instant::now(),
            clock: battery_logic::Clock::default(),
            tab: 2,
            rail_focus: None,
            cursor: -1,
            slider_adjusting: false,
            slider_repeat_start: None,
            items: vec![],
            rects: vec![],
            spans: vec![],
            content_h: 0.0,
            scroll: 0.0,
            last_fid: [-1; TABS],
            last_quick_item: None,
            bluetooth_pending: None,
            night_pending: false,
            tree: None,
            rail_nodes: vec![],
            row_nodes: vec![],
            perf: PerfState::default(),
            friends: vec![],
            favorites: vec![],
            groups: vec![],
            recent: vec![],
            invites: 0,
            ftab: qam_friends::TAB_FRIENDS,
            ftab_defaulted: false,
            friends_loading: false,
            notifs: vec![],
            plugins: vec![],
            icons: HashMap::new(),
            avatar_cache: HashMap::new(),
        }
    }
}

fn clock_ms() -> f64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<QamState>();
    st.set_label_to_start_choose_a_friend_or_group(i18n::tr("qam.to_start_choose_a_friend_or_group").into());
    st.on_activate(|fid| super::defer(move |a| a.qam_click(fid)));
    st.on_changed(|fid, v| super::defer(move |a| a.qam_drag(fid, v)));
    st.on_select_tab(|i| super::defer(move |a| a.qam_select_tab(i as usize, true)));
    st.on_scrolled(|dy| super::defer(move |a| a.qam_wheel(dy)));
    st.on_close(|| super::defer(|a| a.close_overlays()));
}

pub fn init_strings(ui: &AppWindow) {
    let st = ui.global::<QamState>();
    let labels = [
        t_or("#NotificationsMenu_Title", i18n::tr_ref("qam.notifications")),
        t_or("#QuickAccess_Tab_Friends_Title", i18n::tr_ref("qam.friends")),
        t_or("#QuickAccess_Tab_Settings_Title", i18n::tr_ref("qam.quick_settings")),
        t_or("#EncodingOption_Performance", i18n::tr_ref("qam.best_performance")),
        "Decky".to_string(),
    ];
    // Decky's plug glyph (react-icons FaPlug look); Steam's icon set has none
    let plug = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="#ffffff" d="M7 2h2v5H7zM15 2h2v5h-2zM5 7h14v5a7 7 0 0 1-5 6.7V22h-4v-3.3A7 7 0 0 1 5 12z"/></svg>"##;
    if let Ok(img) = Image::load_from_svg_data(plug.as_bytes()) {
        st.set_decky_icon(img);
    }
    crate::plugins::init(ui);
    ui.global::<crate::DeckyState>().set_emit_rail_node(false);
    st.set_tab_labels(ModelRc::new(VecModel::from(labels.iter().map(|s| s.as_str().into()).collect::<Vec<slint::SharedString>>())));
}

/// Wi-Fi also feeds the top bar, so it is polled while the QAM is closed. Other costly
/// QAM-only fields are read while open and carried over on closed polls.
/// Opening the QAM wakes the poller at once (`set_open_flag`), so the rows are fresh on the first frame.
fn gather_status(ds: &DataSource) -> Status {
    let wifi_epoch = super::internet::radio_epoch();
    let overlay_epoch = OVERLAY_EPOCH.load(std::sync::atomic::Ordering::SeqCst);
    let open = QAM_OPEN.load(std::sync::atomic::Ordering::Relaxed);
    let quick = open && QUICK_TAB.load(Ordering::Relaxed);
    let bluetooth_epoch = BLUETOOTH_EPOCH.load(Ordering::SeqCst);
    let night_epoch = NIGHT_EPOCH.load(Ordering::SeqCst);
    Status {
        overlay_epoch,
        hardware_epoch: HARDWARE_EPOCH.load(std::sync::atomic::Ordering::SeqCst),
        brightness_generation: actions::brightness_generation(),
        battery: ds.battery(),
        wifi: super::internet::backend().radio(),
        wifi_epoch,
        bluetooth: if quick { super::bluetooth::backend().state() } else { bt::State::default() },
        bluetooth_epoch,
        night_mode: quick.then(|| crate::display::night_mode(ds.is_mock())).flatten(),
        night_epoch,
        quick,
        brightness: ds.brightness(),
        volume: open.then(|| ds.volume()).flatten(),
        profile: open.then(|| ds.profile()).flatten(),
        profiles: if open { ds.profiles() } else { Vec::new() },
        tdp: open.then(|| ds.tdp()).flatten(),
        gpu_clock: open.then(|| ds.gpu_clock()).flatten(),
        running: steam_control::running_game(),
        mic: open.then(mic_volume).flatten(),
        airplane: open.then(airplane).flatten(),
        brightness_avail: open.then(brightness_available).flatten(),
        perf: if open { actions::bridge().and_then(|b| b.perf_state().ok()) } else { None },
        gs: open.then(crate::gamescope_atoms::read).flatten(),
        mango_overlay: open.then(crate::mango_config::read).flatten(),
        detailed: open,
    }
}

/// SystemStoragStore.m_msgSettings.is_display_brightness_available through the bridge.
fn brightness_available() -> Option<bool> {
    actions::bridge().and_then(|b| b.eval_json("window.SystemStoragStore?.m_msgSettings?.is_display_brightness_available ?? null").ok()).and_then(|v| v.as_bool())
}

/// A poll that did not read the QAM-only fields keeps the previous values, so the rows do not blink out
/// while the QAM is closed and the first open poll shows them at once.
fn carry_closed_poll_fields(st: &mut Status, prev: &Status) {
    if !st.quick {
        st.bluetooth = prev.bluetooth.clone();
        st.night_mode = prev.night_mode;
    }
    if st.detailed { return; }
    st.mic = prev.mic;
    st.airplane = prev.airplane;
    st.brightness_avail = prev.brightness_avail;
    // wpctl / busctl fields: not read while closed (`gather_status`).
    st.volume = prev.volume;
    st.profile = prev.profile.clone();
    st.profiles = prev.profiles.clone();
    st.tdp = prev.tdp;
    st.gpu_clock = prev.gpu_clock;
}

/// A failed local read is unknown, not evidence that an adapter disappeared.
fn fallback_wifi(current: i32, observed: Option<bool>) -> i32 {
    observed.map(|v| v as i32).unwrap_or(current)
}

/// Apply confirmed sysfs readback to QAM and, when visible, the brightness popup.
fn brightness_readback(status: &mut Status, observed: Option<(f32, bool)>, popup_brightness_visible: bool) -> Option<f32> {
    status.brightness = observed.map(|v| v.0);
    if popup_brightness_visible { status.brightness } else { None }
}

// Steam Deck (unlike Steam Frame) exposes input gain up to 130%.
const MIC_MAX: f32 = 1.3;
fn mic_to_slider(v: f32) -> f32 { (v / MIC_MAX).clamp(0.0, 1.0) }
fn slider_to_mic(v: f32) -> f32 { v.clamp(0.0, 1.0) * MIC_MAX }

fn mic_volume() -> Option<f32> {
    let o = std::process::Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SOURCE@"]).output().ok()?;
    String::from_utf8_lossy(&o.stdout).split_whitespace().nth(1)?.parse::<f32>().ok()
}

/// Name of the internal panel as Steam shows it ("EDP-1 7\"": DRM connector + diagonal; the Deck LCD is 7 inch).
fn display_name() -> Option<String> {
    for e in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if let Some(pos) = n.find("eDP") {
            let conn = &n[pos..];
            if std::fs::read_to_string(e.path().join("status")).map(|s| s.trim() == "connected").unwrap_or(false) {
                return Some(format!("{conn} 7\""));
            }
        }
    }
    None
}

/// SystemStoragStore.AirplaneModeEnabled through the bridge (docs/engine/qam.json airplane_wifi).
fn airplane() -> Option<bool> {
    actions::bridge()?.eval_json("window.SystemStoragStore?.AirplaneModeEnabled?.Value ?? null").ok()?.as_bool()
}

/// Bridge JS or an external command; logged instead of executed under DECKSHELL_DRYRUN.
fn sys_write(what: &str, f: impl FnOnce() + Send + 'static) {
    if actions::dryrun() {
        eprintln!("[deckshell] DRYRUN qam {what}");
        return;
    }
    std::thread::spawn(f);
}

/// System status (sysfs / PipeWire / D-Bus) on a worker thread every 4 s; never blocks the UI thread.
pub fn spawn_status_poller(ds: std::sync::Arc<DataSource>) {
    let worker = std::thread::spawn(move || loop {
        let st = gather_status(&ds);
        let _ = slint::invoke_from_event_loop(move || {
            crate::with_app(|a| a.on_status(st));
        });
        // `park_timeout`: opening the QAM unparks the poller for an immediate full read.
        std::thread::park_timeout(Duration::from_secs(4));
    });
    let _ = STATUS_POLLER.set(worker.thread().clone());
}

/// The status poller's thread, woken when the QAM opens.
static STATUS_POLLER: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();


fn qam_friends_diag(event: &str, fields: serde_json::Value) {
    if crate::diag::enabled() {
        crate::diag::log(event, fields);
    }
}

fn friend_counts(friends: &[Friend]) -> (usize, usize, usize) {
    let online = friends.iter().filter(|f| f.state != 0).count();
    let avatars = friends.iter().filter(|f| f.avatar_available).count();
    (online, friends.len() - online, avatars)
}

/// QAM visible: the status poller only reads the Perf/Quick-settings state (a webpack round trip) while it is open.
static HARDWARE_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OVERLAY_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static QAM_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static QUICK_TAB: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BLUETOOTH_EPOCH: AtomicU64 = AtomicU64::new(0);
static NIGHT_EPOCH: AtomicU64 = AtomicU64::new(0);

struct B {
    rows: Vec<QamRow>,
    items: Vec<Item>,
    rects: Vec<(f32, f32)>,
    spans: Vec<(f32, f32, u32)>,
    y: f32,
}
/// Hardware TDP cap for the stock switch: on keeps the shown cap (clamped to the hardware range), off restores the maximum.
pub fn tdp_toggle_target(on: bool, cap: u32, lo: u32, hi: u32) -> u32 {
    if on { cap.clamp(lo, hi) } else { hi }
}

/// Performance tab rows. Steam hides the Perf section when the SystemPerfStore channel is absent, so rows appear only
/// after `perf_avail == Some(true)`. Displayed values come from the PERF_READ mirror (`apply_perf`), not from the
/// request, so a row shows what the system reports. Overlay needs an observed level; TDP and GPU need sysfs readback.
fn performance_rows(b: &mut B, p: &PerfState, tdp: Option<(u32, u32, u32)>, gpu: Option<(bool, Option<u32>, u32, u32)>) {
    let perf = p.perf_avail == Some(true);
    match p.overlay_observed {
        Some(level) if !p.overlay_write_failed => {
            b.toggle(Item::Overlay, None, &t_or("#QuickAccess_Tab_Perf_Overlay", i18n::tr_ref("qam.performance_overlay")), (level > 0) as i32, H_TOGGLE, "#quickaccess_tab_perf_overlay");
            let v = p.overlay_pending.map(|(_, l)| l).unwrap_or(level) as f32 / 4.0;
            b.notch(Item::OverlayLevel, &t_or("#QuickAccess_Tab_Perf_Overlay_Level", i18n::tr_ref("qam.overlay_level")), v, &t_or("#QuickAccess_Tab_Perf_Overlay_Level_OFF", i18n::tr_ref("qam.off")), "4", 0);
        }
        _ => {},
    }
    b.button(Item::Advanced, None, &if p.advanced { t_or("#Common_Basic_View", i18n::tr_ref("qam.show_the_main_parameters")) } else { t_or("#Common_Advanced_View", i18n::tr_ref("qam.advanced_view")) }, "", "#common_advanced_view");
    if !p.advanced {
        return;
    }
    if let Some((cur, min, max)) = tdp.filter(|_| !p.tdp_failed) {
        // steamos_tdp_limit_enabled (PERF_READ tdp.on): stock «enable limit» switch; the cap goes through set_tdp_verified, Steam keeps the stored switch
        b.toggle(Item::TdpOn, None, i18n::tr_ref("qam.limitation_of_tdp"), p.tdp_on as i32, H_TOGGLE, "tdp_limit");
        if p.tdp_pending {
            b.text(&i18n::tr_args("qam.tdp_limit_w_applying", &[("cur", &(cur).to_string())]), H_TOGGLE, WHITE_DIM);
        } else {
            b.text(&i18n::tr_args("qam.tdp_limit_w", &[("cur", &(cur).to_string())]), H_TOGGLE, WHITE_DIM);
            b.notch(Item::Tdp, "", (cur - min) as f32 / (max - min).max(1) as f32, &i18n::tr_args("qam.w", &[("min", &(min).to_string())]), &i18n::tr_args("qam.w_2", &[("max", &(max).to_string())]), 0);
        }
    } else {
        b.text(i18n::tr_ref("qam.tdp_limitation_unavailable"), H_TOGGLE, GREY);
    }
    if let Some((on, clock, min, max)) = gpu {
        b.toggle(Item::GpuOn, None, &t_or("#QuickAccess_Tab_Perf_GPUClockControl", i18n::tr_ref("qam.manual_frequency_gpu")), on as i32, H_TOGGLE, "gpu_manual");
        if let Some(clock) = clock {
            b.text(&i18n::tr_args("qam.gpu_clock_mhz", &[("clock", &(clock).to_string()), ("gpu_pending", &(if p.gpu_pending { i18n::tr_ref("qam.application_of") } else { "" }).to_string())]), H_TOGGLE, WHITE_DIM);
            b.notch(Item::Gpu, "", (clock - min) as f32 / (max - min).max(1) as f32, &min.to_string(), &max.to_string(), 0);
        }
        if p.gpu_failed {
            b.text(i18n::tr_ref("qam.failed_to_apply_the_gpu_frequency_try"), H_TOGGLE, GREY);
        }
    } else {
        b.text(i18n::tr_ref("qam.gpu_frequency_not_available"), H_TOGGLE, GREY);
    }
    if !perf {
        // No Perf channel: the only source is the gamescope FPS atom (gamescope_atoms). Other rows stay hidden.
        match p.gs_fps {
            Some(v) if p.fps_opts.contains(&v) => fps_row(b, p),
            Some(v) => b.text(&i18n::tr_args("qam.not_listed", &[("QuickAccess_Tab_Perf_LimitFrameRate", &(t_or("#QuickAccess_Tab_Perf_LimitFrameRate", i18n::tr_ref("qam.limitation_of_staff_s"))).to_string()), ("value", &(v).to_string())]), H_TOGGLE, WHITE_DIM),
            None => b.text(i18n::tr_ref("qam.performance_options_not_available_outside_the_gamescope"), H_TOGGLE, GREY),
        }
        refresh_row(b, p, false);
        scaling_rows(b, p, false);
        return;
    }
    fps_row(b, p);
    refresh_row(b, p, true);
    if p.vrr_supported && p.gs_vrr_capable != Some(false) {
        b.toggle(Item::Vrr, None, &t_or("#QuickAccess_Tab_Perf_EnableVRR", "VRR"), p.vrr as i32, H_TOGGLE, "#quickaccess_tab_perf_enablevrr");
    }
    b.toggle(Item::Tearing, None, &t_or("#QuickAccess_Tab_Perf_EnableTearing", i18n::tr_ref("qam.staffing_gap")), p.tearing as i32, H_TOGGLE, "#quickaccess_tab_perf_enabletearing");
    if p.vrs_avail {
        b.toggle(Item::Vrs, None, &t_or("#QuickAccess_Tab_Perf_VariableResolution", i18n::tr_ref("qam.variable_authorization")), p.vrs as i32, H_TOGGLE, "#quickaccess_tab_perf_variableresolution");
    }
    scaling_rows(b, p, true);
    let sharp_span = (p.sharp_max - p.sharp_min).max(1);
    b.notch(Item::Sharp, &format!("{}: {}", t_or("#QuickAccess_Tab_Perf_Sharpness", i18n::tr_ref("qam.sharp_fsr")), p.sharp), (p.sharp - p.sharp_min) as f32 / sharp_span as f32, &p.sharp_min.to_string(), &p.sharp_max.to_string(), 0);
    b.toggle(Item::Profile, None, &t_or("#QuickAccess_Tab_Perf_GameSpecificSettings", i18n::tr_ref("qam.settings_for_the_game")), p.game_profile as i32, H_TOGGLE, "#quickaccess_tab_perf_gamespecificsettings");
    b.button(Item::ProfileReset, None, &t_or("#QuickAccess_Tab_Perf_ResetToDefault", i18n::tr_ref("qam.reset_by_default")), "", "#quickaccess_tab_perf_resettodefault");
}

fn refresh_row(b: &mut B, p: &PerfState, perf: bool) {
    if (perf && p.refresh_avail) || (!perf && p.gs_refresh.is_some()) {
        let hz_span = (p.hz_max - p.hz_min).max(1.0);
        let policy = if !perf && p.refresh_game_policy { i18n::tr_ref("qam.sets_the_game") } else { "" };
        let label = i18n::tr_args("qam.hz", &[("QuickAccess_Tab_Perf_RefreshRate", &(t_or("#QuickAccess_Tab_Perf_RefreshRate", i18n::tr_ref("qam.refresh_rate"))).to_string()), ("i32", &(p.refresh_hz.round() as i32).to_string()), ("policy", &(policy).to_string())]);
        b.text(&label, H_TOGGLE, WHITE_DIM);
        b.notch(Item::Refresh, &label, (p.refresh_hz - p.hz_min) / hz_span, &i18n::tr_args("qam.hz_2", &[("i32", &(p.hz_min.round() as i32).to_string())]), &i18n::tr_args("qam.hz_2", &[("i32", &(p.hz_max.round() as i32).to_string())]), 0);
    }
}

fn scaling_rows(b: &mut B, p: &PerfState, perf: bool) {
    if perf || p.gs_filter.is_some() {
    let filter_name = match p.filter { 2 => i18n::tr_ref("qam.nearest"), 3 => i18n::tr_ref("qam.sharp"), _ => i18n::tr_ref("qam.linear") };
    let fidx = FILTERS.iter().position(|&x| x == p.filter).unwrap_or(0);
    let label = format!("{}: {filter_name}", t_or("#QuickAccess_Tab_Perf_ScalingFilter", i18n::tr_ref("qam.scaling_filter")));
    b.text(&label, H_TOGGLE, WHITE_DIM);
    b.notch(Item::Filter, &label, fidx as f32 / (FILTERS.len() - 1) as f32, "", "", 0);
    }
    if perf || p.gs_scaler.is_some() {
    let scaler_name = match p.scaler { 2 => i18n::tr_ref("qam.integer"), 3 => i18n::tr_ref("qam.fit"), 5 => i18n::tr_ref("qam.stretch"), 4 => i18n::tr_ref("qam.fill_in"), _ => i18n::tr_ref("qam.automotive") };
    let sidx = SCALERS.iter().position(|&x| x == p.scaler).unwrap_or(0);
    let label = format!("{}: {scaler_name}", t_or("#QuickAccess_Tab_Perf_ScalingScaler", i18n::tr_ref("qam.scaling")));
    b.text(&label, H_TOGGLE, WHITE_DIM);
    b.notch(Item::Scaler, &label, sidx as f32 / (SCALERS.len() - 1) as f32, "", "", 0);
    }
}

fn fps_row(b: &mut B, p: &PerfState) {
    let fps = p.fps_opts.get(p.fps).copied().unwrap_or(0);
    let fps_n = p.fps_opts.len().max(2) - 1;
    let fps_text = if fps == 0 { t_or("#QuickAccess_Tab_Perf_LimitFrameRate_Off", i18n::tr_ref("qam.unlimited")) } else { fps.to_string() };
    let label = format!("{}: {fps_text}", t_or("#QuickAccess_Tab_Perf_LimitFrameRate", i18n::tr_ref("qam.limitation_of_staff_s")));
    b.text(&label, H_TOGGLE, WHITE_DIM);
    b.notch(Item::FpsLimit, &label, p.fps as f32 / fps_n as f32, "0", p.fps_opts.last().map(|x| x.to_string()).unwrap_or_default().as_str(), 0);
}

#[cfg(test)]
#[path = "qam_performance_tests.rs"]
mod performance_availability_tests;

impl B {
    fn new() -> B {
        B { rows: vec![], items: vec![], rects: vec![], spans: vec![], y: Y0 }
    }
    fn push(&mut self, mut r: QamRow, h: f32, item: Option<Item>) {
        r.y = self.y;
        r.h = h;
        if let Some(it) = item {
            r.fid = self.items.len() as i32;
            self.items.push(it);
            self.rects.push((self.y, h));
            self.spans.push((0.0, PANEL_W - RAIL_W, 0));
        } else {
            r.fid = -1;
        }
        self.y += h;
        self.rows.push(r);
    }
    /// One horizontal line of cells (Steam: a Row flow container). Each cell is `(row, item, x, w)`; all share `h` and `group`.
    fn push_cells(&mut self, cells: Vec<(QamRow, Option<Item>, f32, f32)>, h: f32, group: u32) {
        let y = self.y;
        for (mut r, item, x, w) in cells {
            r.y = y;
            r.h = h;
            r.cx = x;
            r.cw = w;
            if let Some(it) = item {
                r.fid = self.items.len() as i32;
                self.items.push(it);
                self.rects.push((y, h));
                self.spans.push((x, w, group));
            } else {
                r.fid = -1;
            }
            self.rows.push(r);
        }
        self.y += h;
    }
    fn row(kind: i32) -> QamRow {
        QamRow {
            kind,
            fid: -1,
            cx: 0.0,
            cw: 0.0,
            label: "".into(),
            sub: "".into(),
            rtext: "".into(),
            cap_l: "".into(),
            cap_r: "".into(),
            icon: Image::default(),
            has_icon: false,
            value: 0.0,
            on: 0,
            notches: 0,
            y: 0.0,
            h: 0.0,
            tint: slint::Color::from_argb_encoded(WHITE_DIM),
            meta: "".into(),
        }
    }
    fn header(&mut self, text: &str, h: f32) {
        let mut r = B::row(0);
        r.label = text.into();
        self.push(r, h, None);
    }
    fn gap(&mut self) {
        self.y += GAP;
    }
    fn text(&mut self, text: &str, h: f32, tint: u32) {
        let mut r = B::row(6);
        r.label = text.into();
        r.tint = slint::Color::from_argb_encoded(tint);
        self.push(r, h, None);
    }
    fn slider(&mut self, item: Item, icon: Image, label: &str, v: f32, token: &str) {
        let mut r = B::row(1);
        r.icon = icon;
        r.label = label.into();
        r.value = v;
        r.meta = format!("slider|{token}|FOCUS").into();
        self.push(r, H_SLIDER, Some(item));
    }
    fn toggle(&mut self, item: Item, icon: Option<Image>, label: &str, on: i32, h: f32, token: &str) {
        let quick_shortcut = matches!(item, Item::Airplane | Item::Wifi | Item::Bluetooth | Item::NightMode);
        let mut r = B::row(if quick_shortcut { 15 } else { 2 });
        r.notches = i32::from(matches!(item, Item::Airplane | Item::Wifi));
        r.has_icon = icon.is_some();
        r.icon = icon.unwrap_or_default();
        r.label = label.into();
        r.on = on;
        r.meta = format!("toggle|{token}|A,FOCUS,touch,mouse").into();
        self.push(r, h, Some(item));
    }
    fn button(&mut self, item: Item, icon: Option<Image>, label: &str, rtext: &str, token: &str) {
        let mut r = B::row(3);
        r.has_icon = icon.is_some();
        r.icon = icon.unwrap_or_default();
        r.label = label.into();
        r.rtext = rtext.into();
        r.meta = format!("button|{token}|A,START,FOCUS,touch,mouse,long_press").into();
        self.push(r, H_BTN, Some(item));
    }
    fn notch(&mut self, item: Item, label: &str, v: f32, cl: &str, cr: &str, notches: i32) {
        let mut r = B::row(4);
        r.label = label.into();
        r.value = v;
        r.cap_l = cl.into();
        r.cap_r = cr.into();
        r.notches = notches;
        r.meta = "slider||FOCUS".into();
        self.push(r, H_NOTCH, Some(item));
    }
}

impl App {
    pub fn on_gpu_clock_result(&mut self, observed: Option<(bool, Option<u32>, u32, u32)>, ok: bool) {
        self.qm.perf.gpu_pending = false;
        self.qm.perf.gpu_failed = !ok;
        self.qm.st.gpu_clock = observed;
        if self.qm.open { self.qam_rebuild(false); }
    }

    pub fn on_tdp_result(&mut self, observed: Option<(u32, u32, u32)>, ok: bool) {
        self.qm.perf.tdp_pending = false;
        self.qm.perf.tdp_failed = !ok;
        self.qm.st.tdp = observed;
        if self.qm.open { self.qam_rebuild(false); }
    }

    pub fn on_perf_overlay_result(&mut self, generation: u64, observed: Option<i32>, ambiguous: bool) {
        if !accept_overlay_result(&mut self.qm.perf, generation, observed, ambiguous) { return; }
        OVERLAY_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.qam_refresh_light();
    }

    fn qam_icon(&mut self, name: &'static str) -> Image {
        if let Some(icon) = self.qm.icons.get(name) { return icon.clone(); }
        let Some(icon) = try_icon_image(name) else { return Image::default() };
        self.qm.icons.insert(name, icon.clone());
        icon
    }

    pub(crate) fn qam_icons_ready(&mut self) {
        self.qm.icons.clear();
        if self.qm.open { self.qam_rebuild(false); }
    }

    pub fn on_status(&mut self, mut st: Status) {
        if let Some(b) = st.battery.as_ref() { self.qm.clock.update(Instant::now(), ac_state(b), battery_state(b)); }
        st.brightness = actions::brightness_for_snapshot(st.brightness_generation, st.brightness);
        carry_closed_poll_fields(&mut st, &self.qm.st);
        if super::internet::radio_pending() || st.wifi_epoch != super::internet::radio_epoch() {
            st.wifi = self.qm.st.wifi;
        }
        if self.qm.bluetooth_pending.is_some() || st.bluetooth_epoch != BLUETOOTH_EPOCH.load(Ordering::SeqCst) {
            st.bluetooth = self.qm.st.bluetooth.clone();
        }
        if self.qm.night_pending || st.night_epoch != NIGHT_EPOCH.load(Ordering::SeqCst) {
            st.night_mode = self.qm.st.night_mode;
        }
        if st.hardware_epoch != HARDWARE_EPOCH.load(std::sync::atomic::Ordering::SeqCst) || self.qm.perf.tdp_pending || self.qm.perf.gpu_pending {
            st.tdp = self.qm.st.tdp;
            st.gpu_clock = self.qm.st.gpu_clock;
        }
        let keep_qam = self.qm.open && self.qm.st_stamp.elapsed() < Duration::from_secs(2);
        if keep_qam {
            self.qm.st.battery = st.battery;
            self.qm.st.wifi = st.wifi;
            self.qm.st.bluetooth = st.bluetooth;
            self.qm.st.night_mode = st.night_mode;
        } else {
            if let Some(p) = st.perf.clone() {
                self.apply_perf(&p, st.overlay_epoch == OVERLAY_EPOCH.load(std::sync::atomic::Ordering::SeqCst));
            }
            self.apply_gs_fps(st.gs.as_ref());
            if self.qm.perf.perf_avail != Some(true) && st.overlay_epoch == OVERLAY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) && self.qm.perf.overlay_pending.is_none() {
                self.qm.perf.overlay_observed = st.mango_overlay;
                if let Some(level) = st.mango_overlay.filter(|v| *v > 0) { self.qm.perf.level = level; }
            }
            if st.perf.is_none() && st.mango_overlay.is_none() && self.qm.open && st.overlay_epoch == OVERLAY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) && self.qm.perf.overlay_pending.is_none() {
                self.qm.perf.overlay_observed = None;
            }
            self.qm.st = st;
            // A system-side change (for example Steam's adaptive brightness) can arrive
            // after our write completed. Keep an already visible popup on the same
            // accepted snapshot as the QAM; do not open a popup for a poll.
            let ui = self.ui();
            let shell = ui.global::<crate::Shell>();
            let popup_visible = shell.get_level_shown() && shell.get_level_kind() == 0;
            let observed = self.qm.st.brightness.map(|v| (v, false));
            if let Some(value) = brightness_readback(&mut self.qm.st, observed, popup_visible) {
                self.level_update_value(value);
            }
        }
        // without the bridge the running game comes from the process scan
        if !self.live.bridge_up {
            let run: Vec<u32> = self.qm.st.running.into_iter().collect();
            if run != self.live.running {
                self.live.running = run;
                self.recompute_recent();
                self.home_refresh();
                self.refresh_game_page();
            }
        }
        self.sync_status();
    }

    pub fn on_friend_avatar_ready(
        &mut self,
        url: &str,
        buf: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
        from_disk: bool,
        duration_ms: u64,
    ) {
        let img = slint::Image::from_rgba8(buf);
        if self.qm.avatar_cache.len() > 300 {
            let keys: Vec<String> = self.qm.avatar_cache.keys().take(100).cloned().collect();
            for k in keys {
                self.qm.avatar_cache.remove(&k);
            }
        }
        self.qm.avatar_cache.insert(url.to_string(), img);

        if crate::diag::enabled() {
            crate::diag::log(
                "qam_avatar_loaded",
                serde_json::json!({
                    "from_disk": from_disk,
                    "duration_ms": duration_ms
                }),
            );
        }
        // If QAM is open and we are on Friends tab (1), refresh rows in-place without resetting scroll or cursor
        let should_rebuild = should_rebuild_on_avatar_ready(
            self.qm.open,
            self.qm.tab,
            self.qam_friend_avatar_urls(),
            url,
        );
        if should_rebuild {
            let b = self.qam_build_rows();
            self.qm.items = b.items;
            self.qm.rects = b.rects;
            self.qm.spans = b.spans;
            self.qm.content_h = b.y;
            let ui = self.ui();
            let g = ui.global::<QamState>();
            g.set_rows(ModelRc::new(VecModel::from(b.rows)));
        }
    }

    /// Mirror PERF_READ (client settings + SteamOSManager limits + SystemPerfStore) into the Performance / Quick settings state.
    fn apply_perf(&mut self, v: &serde_json::Value, overlay_fresh: bool) {
        let mut gid: Option<u64> = None;
        let p = &mut self.qm.perf;
        let f = |x: &serde_json::Value| x.as_f64();
        p.tdp_on = v["tdp"]["on"].as_bool().unwrap_or(p.tdp_on);
        p.gpu_on = v["gpu"]["on"].as_bool().unwrap_or(p.gpu_on);
        if let Some(m) = f(&v["gpu"]["mhz"]) {
            p.gpu_mhz = m as f32;
        }
        if let Some(l) = v["limits"].as_object() {
            p.gpu_avail = l.get("gpu").and_then(|g| g[2].as_bool()).unwrap_or(true);
            p.vrs_avail = l.get("vrs").and_then(|g| g.as_bool()).unwrap_or(true);
            if let (Some(lo), Some(hi)) = (l.get("gpu").and_then(|g| f(&g[0])), l.get("gpu").and_then(|g| f(&g[1]))) {
                if hi > lo {
                    p.gpu_min = lo as f32;
                    p.gpu_max = hi as f32;
                }
            }
        }
        p.vrs = v["vrs"].as_bool().unwrap_or(p.vrs);
        p.tearing = v["tearing"].as_bool().unwrap_or(p.tearing);
        if let Some(a) = v["scale"]["auto"].as_bool() {
            p.ui_auto = a;
        }
        if let (Some(lo), Some(hi)) = (f(&v["scale"]["min"]), f(&v["scale"]["max"])) {
            if hi > lo {
                p.scale_min = lo as f32;
                p.scale_max = hi as f32;
            }
        }
        if let (Some(cur), true) = (f(&v["scale"]["factor"]), p.scale_max > p.scale_min) {
            p.text_size = ((cur as f32 - p.scale_min) / (p.scale_max - p.scale_min)).clamp(0.0, 1.0);
        }
        let perf = &v["perf"];
        if perf.is_null() {
            // bridge answered but there is no Perf channel (not the gamescope session): Steam hides these controls
            p.perf_avail = Some(false);
            if overlay_fresh && p.overlay_pending.is_none() { p.overlay_observed = None; }
        } else {
            p.perf_avail = Some(true);
            let g = &perf["global"];
            if overlay_fresh && p.overlay_pending.is_none() {
                p.overlay_observed = actions::observed_perf_overlay(v);
                if let Some(on) = g["perf_overlay_service_state"].as_i64().and_then(|n| match n { 1 => Some(false), 2 => Some(true), _ => None }) { p.overlay = on; }
                if let Some(level) = p.overlay_observed.filter(|n| *n > 0) { p.level = level; }
            }
            let a = &perf["per_app"];
            let l = &perf["limits"];
            if let Some(o) = l["fps_limit_options"].as_array() {
                let opts: Vec<u32> = o.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect();
                if !opts.is_empty() {
                    p.fps_opts = opts;
                }
            }
            let fl = a["fps_limit"].as_u64().unwrap_or(0) as u32;
            p.fps = p.fps_opts.iter().position(|&x| x == fl).unwrap_or(0);
            if let (Some(lo), Some(hi)) = (f(&l["display_refresh_manual_hz_min"]), f(&l["display_refresh_manual_hz_max"])) {
                if hi > lo {
                    p.hz_min = lo as f32;
                    p.hz_max = hi as f32;
                }
            }
            if let Some(h) = f(&a["display_refresh_manual_hz"]) {
                p.refresh_hz = (h as f32).clamp(p.hz_min, p.hz_max);
            }
            p.refresh_avail = l["is_manual_display_refresh_rate_available"].as_bool().unwrap_or(true) && !l["disable_refresh_rate_management"].as_bool().unwrap_or(false);
            p.filter = a["split_scaling_filter"].as_i64().map(|x| x as i32).filter(|x| FILTERS.contains(x)).unwrap_or(p.filter);
            p.scaler = a["split_scaling_scaler"].as_i64().map(|x| x as i32).filter(|x| SCALERS.contains(x)).unwrap_or(p.scaler);
            p.sharp_min = l["fsr_sharpness_min"].as_i64().unwrap_or(0) as i32;
            p.sharp_max = l["fsr_sharpness_max"].as_i64().filter(|&x| x > 0).unwrap_or(8) as i32;
            p.sharp = a["fsr_sharpness"].as_i64().map(|x| x as i32).unwrap_or(p.sharp).clamp(p.sharp_min, p.sharp_max);
            p.game_profile = a["is_game_perf_profile_enabled"].as_bool().unwrap_or(p.game_profile);
            p.vrr = a["is_vrr_enabled"].as_bool().unwrap_or(p.vrr);
            p.vrr_supported = l["is_vrr_supported"].as_bool().unwrap_or(false);
            p.game_id = perf["gameid"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
            gid = Some(p.game_id);
        }
        if let Some(g) = gid {
            let appid = (g & 0xffff_ffff) as u32;
            let name = if g == 0 { String::new() } else { self.gidx.get(&appid).map(|&gi| self.games()[gi].name.clone()).unwrap_or_default() };
            self.qm.perf.game_name = name;
        }
    }

    /// GAMESCOPE_FPS_LIMIT mirror. Used only without the Perf channel: with it, the per-app `fps_limit` from apply_perf stays.
    /// The index is taken only when the value is one of the listed options; anything else is shown as a raw value (performance_rows).
    fn apply_gs_fps(&mut self, g: Option<&crate::gamescope_atoms::Snapshot>) {
        let p = &mut self.qm.perf;
        p.gs_vrr_capable = g.and_then(|g| g.vrr_capable);
        p.gs_fps = g.and_then(|g| g.fps_limit);
        p.gs_refresh = g.and_then(|g| g.refresh_feedback);
        p.refresh_game_policy = g.map_or(false, |g| g.refresh_game_policy == Some(true) && g.dynamic_refresh != g.refresh_feedback);
        p.gs_filter = g.and_then(|g| g.scaling_filter).filter(|v| matches!(v, 0 | 1 | 4));
        p.gs_scaler = g.and_then(|g| g.scaling_scaler).filter(|v| *v <= 4);
        if p.perf_avail == Some(true) {
            return;
        }
        if let Some(hz) = p.gs_refresh {
            if let Some((lo, hi)) = std::env::var("STEAM_DISPLAY_REFRESH_LIMITS").ok().and_then(|s| {
                let (lo, hi) = s.split_once(',')?;
                let (lo, hi) = (lo.trim().parse::<u32>().ok()?, hi.trim().parse::<u32>().ok()?);
                (lo < hi).then_some((lo, hi))
            }) { p.hz_min = lo as f32; p.hz_max = hi as f32; }
            p.refresh_hz = hz as f32;
        }
        if let Some(v) = p.gs_filter { p.filter = match v { 0 => 1, 1 => 2, 4 => 3, _ => unreachable!() }; }
        if let Some(v) = p.gs_scaler { p.scaler = v as i32 + 1; }
        let idx = p.gs_fps.and_then(|v| p.fps_opts.iter().position(|&x| x == v));
        if let Some(i) = idx {
            p.fps = i;
        }
    }

    /// Top-bar battery/Wi-Fi + the open tab's model.
    pub fn sync_status(&mut self) {
        {
            let ui = self.ui();
            let s = &self.qm.st;
            let tb = ui.global::<TopBarState>();
            if !self.live.bridge_up || self.live.system.is_none() {
                tb.set_wifi(fallback_wifi(tb.get_wifi(), s.wifi));
            }
        }
        if self.qm.open {
            self.qam_rebuild(false);
            self.update_hint();
        }
    }

    pub fn search_friend_ids<'a>(&'a self, query: &'a str) -> impl Iterator<Item = u32> + 'a {
        self.qm.friends.iter()
            .filter(move |f| !query.is_empty() && f.name.to_lowercase().contains(query))
            .map(|f| f.acc)
    }

    pub fn search_friend_label(&self, id: u32) -> Option<(String, String)> {
        let f = self.qm.friends.iter().find(|f| f.acc == id)?;
        let status = if !f.game.is_empty() {
            f.game.clone()
        } else if f.state == 0 {
            t_or("#PersonaStateOffline", i18n::tr_ref("qam.offline"))
        } else {
            t_or("#PersonaStateOnline", i18n::tr_ref("qam.online"))
        };
        Some((f.name.clone(), status))
    }

    /// OpenQuickAccessMenu(tab): header bell -> Notifications, Steam menu "Друзья и чат" -> Friends (leaves search / other overlays first).
    pub fn open_qam_tab(&mut self, tab: usize) {
        self.ui().global::<QamState>().set_expanded(false);
        self.header_clear();
        if self.lb.search {
            self.close_search();
            if matches!(self.router.current(), crate::router::Route::Search) {
                self.go_back();
            }
        }
        self.sm.open = false;
        self.lb.sort_open = false;
        self.qm.tab = tab.min(TABS - 1);
        if self.qm.open {
            self.qm.rail_focus = Some(self.qm.tab);
            self.qam_select_tab(self.qm.tab, false);
            self.qam_rebuild(true);
            self.sync_overlays();
            self.update_hint();
        } else {
            self.toggle_qam();
        }
    }

    pub fn toggle_qam(&mut self) {
        self.header_clear();
        self.dismiss_confirm();
        self.qm.open = !self.qm.open;
        if self.qm.open {
            self.ui().global::<QamState>().set_expanded(false);
            self.sm.open = false;
            self.lb.sort_open = false;
            // the flag must be set before the poll reads it, or this first poll skips the QAM-only fields
            QUICK_TAB.store(self.qm.tab == 2, Ordering::Relaxed);
            set_open_flag(true);
            let ds = self.ds.clone();
            std::thread::spawn(move || {
                let st = gather_status(&ds);
                let _ = slint::invoke_from_event_loop(move || {
                    crate::with_app(|a| a.on_status(st));
                });
            });
            // opens on the remembered tab, focus on the vertical rail tabs like Steam
            self.qm.rail_focus = Some(self.qm.tab);
            self.qam_enter_tab();
        }
        self.sync_overlays();
        self.update_hint();
    }

    /// Entering a tab: load its data (notifications are marked viewed when their tab opens), rebuild rows + engine nodes.
    fn qam_enter_tab(&mut self) {
        let quick = self.qm.tab == 2;
        if !QUICK_TAB.swap(quick, Ordering::Relaxed) && quick {
            if let Some(t) = STATUS_POLLER.get() { t.unpark(); }
        }
        self.qm.slider_adjusting = false;
        crate::plugins::set_active(self.qm.tab == 4 && self.qm.open);
        if self.qm.tab == 4 {
            crate::plugins::open();
        }
        match self.qm.tab {
            0 => {
                self.act(Act::NotificationsViewed);
                self.qam_load_notifs();
            }
            1 => self.qam_load_friends(),
            4 => self.qam_load_plugins(),
            _ => {}
        }
        self.qam_rebuild(true);
    }

    fn qam_load_notifs(&mut self) {
        let items: Vec<Notif> = actions::bridge()
            .and_then(|b| b.notifications().ok())
            .map(|n| {
                n.items
                    .iter()
                    .map(|i| {
                        let d: serde_json::Value = serde_json::from_str(&i.data).unwrap_or_default();
                        let label = ["strTitle", "title", "strBody", "body", "name", "game_name"].iter().find_map(|k| d[*k].as_str()).map(str::to_owned).unwrap_or_else(|| t_or("#NotificationsMenu_Title", i18n::tr_ref("qam.notification")));
                        let appid = ["appid", "app_id", "nAppID", "unAppID"].iter().find_map(|k| d[*k].as_u64()).map(|x| x as u32);
                        Notif { id: i.id, label, appid }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.qm.notifs = items;
    }

    fn qam_load_friends(&mut self) {
        if self.qm.friends_loading {
            qam_friends_diag("qam_friends_request_skipped", serde_json::json!({"reason":"already_loading"}));
            return;
        }
        let Some(b) = actions::bridge() else {
            qam_friends_diag("qam_friends_request_skipped", serde_json::json!({"reason":"bridge_unavailable","previous_count":self.qm.friends.len()}));
            return;
        };
        self.qm.friends_loading = true;
        let started = Instant::now();
        let previous_count = self.qm.friends.len();
        qam_friends_diag("qam_friends_request_start", serde_json::json!({"previous_count":previous_count}));
        std::thread::spawn(move || {
            // Steam started with -silent (gaming mode) fills friendStore a while after login, and a single eval can time
            // out while Steam is busy: retry for ~10 s instead of caching an empty list (test 3: "friends empty").
            let mut result: Option<Lists> = None;
            for attempt in 0..6 {
                let attempt_started = Instant::now();
                match b.eval_json(qam_friends::FRIENDS_JS) {
                    Ok(v) => match qam_friends::parse(&v) {
                        Some(lists) => {
                            let empty = lists.friends.is_empty();
                            let (online, offline, avatars) = friend_counts(&lists.friends);
                            qam_friends_diag("qam_friends_attempt_result", serde_json::json!({"attempt":attempt+1,"duration_ms":attempt_started.elapsed().as_millis(),"count":lists.friends.len(),"online":online,"offline":offline,"avatar_hash_available":avatars,"empty":empty,"favorites":lists.favorites.len(),"groups":lists.groups.len(),"recent":lists.recent.len(),"invites":lists.invites,"ready":lists.ready}));
                            result = Some(lists);
                            if !empty {
                                break;
                            }
                            qam_friends_diag("qam_friends_retry", serde_json::json!({"attempt":attempt+1,"reason":"empty_result"}));
                            eprintln!("[deckshell] qam friends: empty result (attempt {attempt}), retrying");
                        }
                        None => {
                            qam_friends_diag("qam_friends_attempt_invalid", serde_json::json!({"attempt":attempt+1,"duration_ms":attempt_started.elapsed().as_millis(),"result_type":"non_object"}));
                            qam_friends_diag("qam_friends_retry", serde_json::json!({"attempt":attempt+1,"reason":"invalid_result"}));
                            eprintln!("[deckshell] qam friends: invalid result type (attempt {attempt}), retrying");
                        }
                    },
                    Err(e) => {
                        let kind = match &e { steam_bridge::Error::Timeout => "timeout", steam_bridge::Error::Unavailable(_) => "unavailable", steam_bridge::Error::Protocol(_) => "protocol", steam_bridge::Error::Js(_) => "javascript" };
                        qam_friends_diag("qam_friends_attempt_error", serde_json::json!({"attempt":attempt+1,"duration_ms":attempt_started.elapsed().as_millis(),"kind":kind}));
                        qam_friends_diag("qam_friends_retry", serde_json::json!({"attempt":attempt+1,"reason":"eval_error","kind":kind}));
                        eprintln!("[deckshell] qam friends: eval failed (attempt {attempt}): {e}");
                    }
                }
                if attempt < 5 {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
            let elapsed_ms = started.elapsed().as_millis();
            let _ = slint::invoke_from_event_loop(move || {
                crate::with_app(|a| {
                    a.qm.friends_loading = false;
                    if let Some(lists) = result {
                        let (online, offline, avatars) = friend_counts(&lists.friends);
                        qam_friends_diag("qam_friends_refresh_apply", serde_json::json!({"duration_ms":elapsed_ms,"count":lists.friends.len(),"online":online,"offline":offline,"avatar_hash_available":avatars,"previous_count":a.qm.friends.len(),"favorites":lists.favorites.len(),"groups":lists.groups.len(),"recent":lists.recent.len(),"invites":lists.invites}));
                        if lists.ready || !lists.friends.is_empty() {
                            if !a.qm.ftab_defaulted {
                                a.qm.ftab = qam_friends::initial_tab(&lists);
                                a.qm.ftab_defaulted = true;
                            }
                            a.qm.friends = lists.friends;
                            a.search_catalog_changed();
                            a.qm.favorites = lists.favorites;
                            a.qm.groups = lists.groups;
                            a.qm.recent = lists.recent;
                            a.qm.invites = lists.invites;
                        }
                        let loader = get_loader();
                        for url in a.qam_friend_avatar_urls() {
                            loader.request(url);
                        }
                    } else {
                        let (online, offline, avatars) = friend_counts(&a.qm.friends);
                        qam_friends_diag("qam_friends_keep_previous", serde_json::json!({"duration_ms":elapsed_ms,"previous_count":a.qm.friends.len(),"online":online,"offline":offline,"avatar_hash_available":avatars,"reason":"all_evals_failed"}));
                        eprintln!("[deckshell] qam friends: keeping previous list; all evals failed");
                    }
                    if a.qm.open && a.qm.tab == 1 {
                        a.qam_rebuild(false);
                    }
                });
            });
        });
    }

    /// Decky plugins in the user's order are not readable without the loader token: list the installed plugin folders.
    /// Avatar URLs of every friend-shaped row (Friends, Favorites, Recent messages), for the avatar loader.
    pub fn qam_friend_avatar_urls(&self) -> Vec<&str> {
        self.qm.friends.iter().map(|f| f.avatar_url.as_str())
            .chain(self.qm.favorites.iter().chain(self.qm.recent.iter()).filter(|e| e.kind == qam_friends::Kind::Friend).map(|e| e.avatar_url.as_str()))
            .filter(|u| !u.is_empty())
            .collect()
    }

    fn qam_load_plugins(&mut self) {
        let dir = std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join("homebrew/plugins")).unwrap_or_default();
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let j = std::fs::read_to_string(e.path().join("plugin.json")).ok()?;
                        let v: serde_json::Value = serde_json::from_str(&j).ok()?;
                        v["name"].as_str().map(str::to_owned).or_else(|| e.file_name().to_str().map(str::to_owned))
                    })
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        self.qm.plugins = names;
    }

    // ------------------------------------------------------------------------------------------------ model
    /// Steam Friends list rows (see `build_friends_tab`): icons are cached like the other QAM icons.
    fn qam_build_friends_tab(&mut self, b: &mut B) {
        let mut icons: HashMap<&'static str, Image> = HashMap::new();
        for name in ["Star", "Friends", "Groups", "Recent", "PendingInvites", "AddFriend", "CreateGroup"] {
            let img = self.qam_icon(name);
            icons.insert(name, img);
        }
        let view = FriendsView {
            ftab: self.qm.ftab,
            invites: self.qm.invites,
            friends: &self.qm.friends,
            favorites: &self.qm.favorites,
            groups: &self.qm.groups,
            recent: &self.qm.recent,
            avatars: &self.qm.avatar_cache,
            icons: &icons,
        };
        build_friends_tab(b, &view, &mut |url| get_loader().request(url));
    }

    fn qam_build_rows(&mut self) -> B {
        let mut b = B::new();
        let s = self.qm.st.clone();
        match self.qm.tab {
            0 => {
                if self.qm.notifs.is_empty() {
                    b.text(&t_or("#QuickAccess_Tab_Notifications_None", i18n::tr_ref("qam.no_new_notifications")), H_TOGGLE, GREY);
                }
                for (i, n) in self.qm.notifs.iter().enumerate() {
                    let mut r = B::row(7);
                    r.label = n.label.as_str().into();
                    r.meta = "button||A,START,FOCUS,touch,mouse".into();
                    b.push(r, H_ENTRY, Some(Item::Notif(i)));
                }
            }
            1 => self.qam_build_friends_tab(&mut b),
            2 => {
                let muted_or_zero = s.volume.map(|(v, m)| m || v <= 0.001).unwrap_or(false);
                let ic_vol = self.qam_icon(if muted_or_zero { "VolumeMuted" } else { "AudioVolumeIcon" });
                let ic_mic = self.qam_icon("Microphone");
                let ic_air = self.qam_icon("AirplaneMode");
                let ic_wifi = self.qam_icon("WiFi");
                let ic_br = self.qam_icon("Brightness");
                // Steam hides the brightness section when is_display_brightness_available is false
                if let (Some(v), true) = (s.brightness, s.brightness_avail.unwrap_or(true)) {
                    b.header(&tu("#QuickAccess_Tab_Settings_Section_Brightness_Title"), H_HDR);
                    b.slider(Item::Brightness, ic_br, &t("#QuickAccess_Tab_Settings_Section_Brightness_Title"), v, "#quickaccess_tab_settings_section_brightness_title");
                    b.gap();
                }
                b.header(&tu("#QuickAccess_Tab_Settings_Section_Audio_Title"), H_HDR);
                let vol = s.volume.map(|(v, m)| if m { 0.0 } else { v }).unwrap_or(-1.0);
                b.slider(Item::Volume, ic_vol, &t_or("#Audio_Volume", i18n::tr_ref("qam.volume")), vol, "#voicevolume");
                b.slider(Item::Mic, ic_mic, &t_or("#Audio_InputVolume", i18n::tr_ref("qam.microphone_volume")), s.mic.map(mic_to_slider).unwrap_or(-1.0), "#audio_inputvolume");
                b.gap();
                b.header(&tu("#QuickAccess_Tab_Settings_Section_Other_Title"), H_HDR);
                b.button(Item::Controllers, None, &t_or("#QuickAccess_Tab_Settings_Section_Controllers_ReorderButton", steam_assets::i18n::tr_ref("controller.rearrange_controller_order")), "", "#quickaccess_tab_settings_section_controllers_reorderbutton");
                let air = s.airplane.map(|x| x as i32).unwrap_or(-1);
                b.toggle(Item::Airplane, Some(ic_air), &t_or("#QuickAccess_Tab_Settings_Section_Shortcuts_AirplaneMode", i18n::tr_ref("qam.airplane_mode")), air, H_SHORTCUT, "#quickaccess_tab_settings_section_shortcuts_airplanemode");
                // The status sub-row in Steam's Oe() is gated on ON_FRAME.
                b.toggle(Item::Wifi, Some(ic_wifi), &t_or("#QuickAccess_Tab_Settings_Section_Shortcuts_Wifi", "Wi-Fi"), s.wifi.map(i32::from).unwrap_or(-1), H_SHORTCUT, "#quickaccess_tab_settings_section_shortcuts_wifi");
                self.qam_bluetooth_rows(&mut b, &s.bluetooth);
                let night_icon = self.qam_icon("NightMode");
                b.toggle(Item::NightMode, Some(night_icon),
                    &t_or("#QuickAccess_Tab_Settings_Section_Shortcuts_NightMode", i18n::tr_ref("qam.night_mode")),
                    s.night_mode.map(i32::from).unwrap_or(-1), H_SHORTCUT, "#quickaccess_tab_settings_section_shortcuts_nightmode");
                // Display scaling belongs to the desktop Quick Access layout.
                if s.gs.is_some() || std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some() {
                    return b;
                }
                b.y += H_SEP;
                b.gap();
                // "Масштабирование экрана eDP-1 7"" (two lines like Steam: ScalingForTVName with the internal panel's name)
                let scaling = display_name().map(|n| crate::assets::tf("#Settings_Display_DisplayScaling_ScalingForTVName", &[&n]).to_uppercase()).unwrap_or_else(|| tu("#Settings_Display_DisplayScaling_Scaling"));
                b.header(&scaling, 62.4);
                b.toggle(
                    Item::UiAuto,
                    None,
                    &t_or("#Settings_Display_DisplayScaling_Automatically_Scale_UI_Verbose", i18n::tr_ref("qam.automatically_scale_user_interface")),
                    self.qm.perf.ui_auto as i32,
                    H_TALL,
                    "#settings_display_displayscaling_automatically_scale_ui_verb",
                );
                b.notch(
                    Item::TextSize,
                    "",
                    self.qm.perf.text_size,
                    &tu("#Settings_Display_DisplayScaling_SmallerText"),
                    &tu("#Settings_Display_DisplayScaling_LargerText"),
                    0,
                );
            }
            3 => {
                // Stock SystemPowerStore/QAM logic (sp.js B/M): decisions come from the rust-anything-gated battery-logic crate.
                let bl = |x: &deck_system::Battery| (battery_state(x), x.minutes_left.map(|m| f64::from(m) * 60.0));
                let (state, secs) = s.battery.as_ref().map(bl).unwrap_or((0, None));
                let conf = self.qm.clock.confidence();
                let pct = match s.battery.as_ref() {
                    None => "?".to_string(),
                    Some(_) if self.qm.clock.say_full() => t("#QuickAccess_Tab_Perf_BatteryFull"),
                    Some(x) => battery_logic::percent_text(f64::from(x.percent) / 100.0),
                };
                let caption = battery_logic::caption_token(state, conf);
                let time = if caption.is_some() && battery_logic::time_shown(conf) {
                    let (h, m) = battery_logic::time_parts(secs);
                    let tok = "#QuickAccess_Tab_Perf_BatteryTimeRemaining";
                    if steam_assets::has_loc(tok) { crate::assets::tf(tok, &[&h, &m]) } else { i18n::tr_args("qam.h_min", &[("hours", &(h).to_string()), ("message", &(m).to_string())]) }
                } else {
                    "\u{200B}".to_string()
                };
                let mut r = B::row(9);
                r.label = pct.into();
                r.sub = time.into();
                r.cap_l = caption.map(|c| tu(&c)).unwrap_or_default().into();
                b.push(r, 111.6, None);
                // Steam's settings store confirms requests, not that gamescope or
                // the hardware applied them. Until independent readback is wired
                // up, expose these controls as unavailable and never dispatch.
                performance_rows(&mut b, &self.qm.perf, self.qm.st.tdp, self.qm.st.gpu_clock);

            }
            _ => {
                let ic = self.qam_icon("Settings");
                let names = self.qm.plugins.clone();
                if names.is_empty() {
                    b.text("Decky", H_TOGGLE, GREY);
                }
                for (i, n) in names.iter().enumerate() {
                    let _ = &ic;
                    let plug = self.qam_icon("Settings");
                    b.button(Item::Plugin(i), Some(plug), n, "", &n.to_lowercase());
                }
            }
        }
        b
    }

    /// Rebuild model + engine nodes; `reset` = entering a tab (restore its remembered row).
    pub(crate) fn qam_rebuild(&mut self, reset: bool) {
        let b = self.qam_build_rows();
        let old_item = self.qm.items.get(self.qm.cursor as usize).copied();
        let layout_changed = self.qm.items != b.items || self.qm.rects != b.rects;
        self.qm.items = b.items;
        self.qm.rects = b.rects;
        self.qm.spans = b.spans;
        self.qm.content_h = b.y;
        if reset {
            // Friends: entering the list lands on the active strip tab (Steam preferredFocus), other tabs on their last row
            let want = if self.qm.tab == 1 { self.qm.ftab as i32 }
                else if self.qm.tab == 2 { self.qam_quick_cursor(self.qm.last_quick_item).unwrap_or(0) }
                else { self.qm.last_fid[self.qm.tab] };
            self.qm.cursor = if want >= 0 && (want as usize) < self.qm.items.len() { want } else if self.qm.items.is_empty() { -1 } else { 0 };
            self.qm.scroll = 0.0;
        } else if self.qm.tab == 2 && layout_changed && self.qm.cursor >= 0 {
            self.qm.cursor = self.qam_quick_cursor(old_item).unwrap_or(0);
        } else if self.qm.cursor >= self.qm.items.len() as i32 {
            self.qm.cursor = self.qm.items.len() as i32 - 1;
        }
        self.qam_build_nav();
        if self.qm.tab != 2 || reset || layout_changed { self.qam_scroll_into_view(); }
        self.qam_push(b.rows);
    }

    fn qam_push(&mut self, rows: Vec<QamRow>) {
        let ui = self.ui();
        let g = ui.global::<QamState>();
        g.set_rows(ModelRc::new(VecModel::from(rows)));
        g.set_tab(self.qm.tab as i32);
        g.set_rail_focus(if self.pad_mode { self.qm.rail_focus.map(|x| x as i32).unwrap_or(-1) } else { -1 });
        g.set_cursor(if self.pad_mode { self.qm.cursor } else { -1 });
        g.set_slider_adjusting(self.pad_mode && self.qm.slider_adjusting);
        g.set_scroll(self.qm.scroll);
        g.set_shortcut_top(self.qm.items.iter().position(|it| *it == Item::Airplane)
            .map_or(0.0, |idx| self.qm.rects[idx].0));
        g.set_title(self.qam_title().into());
        self.qm.focus = self.qm.tab as i32 * 1000 + self.qm.rail_focus.map(|x| 100 + x as i32).unwrap_or(0) + (self.qm.cursor.max(0) % 100);
    }

    pub(crate) fn qam_refresh_expanded_title(&self) {
        self.ui().global::<QamState>().set_title(self.qam_title().into());
    }

    fn qam_title(&self) -> String {
        match self.qm.tab {
            0 => t_or("#QuickAccess_Tab_Notifications_Title", i18n::tr_ref("qam.notifications")),
            1 if self.ui().global::<QamState>().get_expanded() => match self.qm.ftab {
                qam_friends::TAB_FAVORITES => t_or("#FriendGroup_Favorites", i18n::tr_ref("qam.favorites_2")),
                qam_friends::TAB_FRIENDS => t_or("#Label_Friends", i18n::tr_ref("qam.friends")),
                qam_friends::TAB_GROUPS => t_or("#Label_Groups", i18n::tr_ref("qam.groups")),
                _ => t_or("#FriendGroup_Recent", i18n::tr_ref("qam.recent")),
            },
            1 => t_or("#QuickAccess_Tab_Friends_Title", i18n::tr_ref("qam.friends")),
            2 => t_or("#QuickAccess_Tab_Settings_Title", i18n::tr_ref("qam.quick_settings")),
            3 => t_or("#QuickAccess_Tab_Perf_Title", i18n::tr_ref("qam.performance")),
            _ => "Decky".into(),
        }
    }

    fn qam_scroll_into_view(&mut self) {
        let row = if self.qm.cursor >= 0 { self.qm.rects.get(self.qm.cursor as usize).copied() } else { None };
        if self.qm.tab == 2 {
            let scroll = self.qm.scroll;
            if self.qm.cursor == 0 {
                self.qm.scroll = 0.0;
            } else if let Some((y, h)) = row {
                self.qm.scroll = if y < scroll + QUICK_TITLE_EDGE { y - QUICK_TITLE_EDGE }
                    else { scroll.max(y + h - PANEL_H + QUICK_BOTTOM_PAD) };
                self.qm.scroll = self.qm.scroll.clamp(0.0, self.qam_scroll_max());
            }
            return;
        }
        self.qm.scroll = qam_focus_scroll(self.qm.scroll, self.qm.content_h, row, self.qm.cursor == 0);
    }

    // ------------------------------------------------------------------------------------------------ focus engine
    fn qam_build_nav(&mut self) {
        let mut t = Tree::new((1280.0, 800.0));
        let root = t.root();
        let panel = t.add_node(root, Props { layout: Layout::Row, ..Default::default() }, Some(NRect::xywh(0.0, 0.0, PANEL_W, PANEL_H)));
        let rail = t.add_node(panel, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(0.0, 0.0, RAIL_W, PANEL_H)));
        let mut rail_nodes = vec![];
        for i in 0..TABS {
            rail_nodes.push(t.add_node(
                rail,
                Props { focusable: true, nav_key: Some(format!("qamtab:{i}")), ..Default::default() },
                Some(NRect::xywh(0.0, TAB_Y0 + TAB_H * i as f32, RAIL_W, TAB_H)),
            ));
        }
        let content = t.add_node(panel, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(RAIL_W, 0.0, PANEL_W - RAIL_W, PANEL_H)));
        let row_nodes = add_row_nodes(&mut t, content, self.qm.tab, &self.qm.rects, &self.qm.spans, &self.qm.items);
        t.now_ms = clock_ms();
        let focus_node = match self.qm.rail_focus {
            Some(i) => rail_nodes.get(i).copied(),
            None => row_nodes.get(self.qm.cursor.max(0) as usize).copied().or_else(|| rail_nodes.get(self.qm.tab).copied()),
        };
        t.transfer_focus(FocusSource::Application, focus_node, None);
        let _ = t.drain_events();
        self.qm.rail_nodes = rail_nodes;
        self.qm.row_nodes = row_nodes;
        self.qm.tree = Some(t);
    }

    pub fn handle_qam(&mut self, a: Action, is_repeat: bool) {
        // Decky tab: the plugin UI owns the content (B on a panel returns to the list; false = not handled -> default QAM behaviour)
        if self.qm.tab == 4 && self.qm.rail_focus.is_none() && crate::plugins::active() && !matches!(a, Action::LB | Action::RB) && crate::plugins::handle(a) {
            return;
        }
        match a {
            Action::B => {
                match cancel_target(self.qm.slider_adjusting, self.qm.rail_focus.is_some()) {
                    CancelTarget::Slider => { self.qm.slider_adjusting = false; self.qm.slider_repeat_start = None; self.qam_refresh_light(); },
                    CancelTarget::Rail => self.qam_return_to_rail(),
                    CancelTarget::Close => {
                        self.qm.open = false;
                        crate::plugins::set_active(false);
                        self.sync_overlays();
                        self.update_hint();
                    }
                }
                return;
            }
            Action::LB | Action::RB => {
                let d = if a == Action::LB { -1 } else { 1 };
                // Friends: LB/RB step the Steam friends-list tab; on the rail they still switch the QAM tab
                if self.qm.tab == 1 && self.qm.rail_focus.is_none() {
                    self.qam_friends_step(d);
                    return;
                }
                if let Some(n) = nav::tab_step(self.qm.tab, d, TABS, None) {
                    self.qam_select_tab(n, false);
                }
                return;
            }
            Action::A => return self.qam_press(),
            Action::Menu => return self.qam_menu(), // START: friend menu / dismiss notification
            Action::Up | Action::Down | Action::Left | Action::Right => {}
            _ => return,
        }
        // Brightness and output volume respond directly while their row is focused.
        if self.qm.rail_focus.is_none() && matches!(a, Action::Left | Action::Right)
            && self.qm.items.get(self.qm.cursor.max(0) as usize).is_some_and(|item| direct_level_slider(*item)) {
            let elapsed = if is_repeat { self.qm.slider_repeat_start.get_or_insert_with(Instant::now).elapsed().as_secs_f32() * 1000.0 } else { self.qm.slider_repeat_start = None; 0.0 };
            self.qam_slide_repeat(self.qm.cursor, if a == Action::Right { 1.0 } else { -1.0 }, elapsed);
            return;
        }
        if self.qm.slider_adjusting {
            match a {
                Action::Left | Action::Right => {
                    let elapsed = if is_repeat { self.qm.slider_repeat_start.get_or_insert_with(Instant::now).elapsed().as_secs_f32() * 1000.0 } else { self.qm.slider_repeat_start = None; 0.0 };
                    self.qam_slide_repeat(self.qm.cursor, if a == Action::Right { 1.0 } else { -1.0 }, elapsed);
                },
                _ => play(Snd::FailedNav),
            }
            return;
        }
        if self.qm.rail_focus.is_some() {
            match a {
                Action::Up => {
                    if self.qm.tab > 0 {
                        self.qam_select_tab(self.qm.tab - 1, false);
                    } else {
                        play(Snd::FailedNav);
                    }
                    return;
                }
                Action::Down => {
                    if self.qm.tab + 1 < TABS {
                        self.qam_select_tab(self.qm.tab + 1, false);
                    } else {
                        play(Snd::FailedNav);
                    }
                    return;
                }
                Action::Right => {
                    self.qam_enter_content();
                    return;
                }
                Action::Left => {
                    play(Snd::FailedNav);
                    return;
                }
                _ => {}
            }
        } else if a == Action::Left {
            let item = self.qm.items.get(self.qm.cursor.max(0) as usize).copied();
            let legacy_adjustable = item.is_some_and(|it| matches!(it, Item::TextSize | Item::OverlayLevel | Item::Tdp | Item::FpsLimit | Item::Refresh | Item::Gpu | Item::Filter | Item::Scaler | Item::Sharp) && self.qam_value(it).is_some_and(|v| v > 0.001));
            // strip cells: the tree moves between tabs first; the failed move returns to the rail below
            let on_strip_cell = matches!(item, Some(Item::FTab(_)));
            if !legacy_adjustable && !on_strip_cell {
                self.qam_return_to_rail();
                return;
            }
        }
        let Some(b) = nav::button_of(a) else { return };
        let mut moved = None;
        let mut failed = false;
        if let Some(t) = self.qm.tree.as_mut() {
            t.now_ms = clock_ms();
            let handled = t.handle_button(b, false);
            for ev in t.drain_events() {
                match ev {
                    NavEvent::Focus { to: Some(n), .. } => moved = Some(n),
                    NavEvent::FailedNav => failed = true,
                    _ => {}
                }
            }
            let _ = handled;
        }
        if let Some(n) = moved {
            if let Some(i) = self.qm.rail_nodes.iter().position(|&x| x == n) {
                // entering the rail lands on the selected tab (Steam keeps the active tab focused)
                let want = self.qm.tab;
                self.qm.rail_focus = Some(want);
                if i != want {
                    if let (Some(t), Some(&id)) = (self.qm.tree.as_mut(), self.qm.rail_nodes.get(want)) {
                        t.transfer_focus(FocusSource::Application, Some(id), None);
                        let _ = t.drain_events();
                    }
                }
            } else if let Some(i) = self.qm.row_nodes.iter().position(|&x| x == n) {
                self.qm.rail_focus = None;
                self.qm.cursor = i as i32;
                self.qm.last_fid[self.qm.tab] = i as i32;
                if self.qm.tab == 2 { self.qm.last_quick_item = self.qm.items.get(i).copied(); }
                if let Some(&Item::FTab(k)) = self.qm.items.get(i) {
                    // focus selects the tab; the panel changes, focus stays on the strip cell
                    self.qm.ftab = k;
                    self.qam_rebuild(false);
                }
            }
            self.qam_scroll_into_view();
            self.qam_refresh_light();
        } else if failed {
            if a == Action::Left && self.qm.rail_focus.is_none() {
                self.qam_return_to_rail();
            } else {
                play(Snd::FailedNav);
            }
        }
    }

    /// LB / RB on the Friends list: the sibling tab (SelectFriendsListSteamDeckSiblingTab: one step, clamped, no wrap).
    /// Focus stays on the strip when it was there, otherwise it moves to the first row of the new panel.
    fn qam_friends_step(&mut self, d: i32) {
        let Some(next) = qam_friends::sibling(self.qm.ftab, d) else {
            play(Snd::FailedNav);
            return;
        };
        let in_strip = self.qm.cursor >= 0 && (self.qm.cursor as usize) < qam_friends::TABS;
        self.qm.ftab = next;
        self.qam_rebuild(false);
        let target = if in_strip {
            next
        } else {
            self.qm.items.iter().position(|it| matches!(it, Item::Friend(_) | Item::FFav(_) | Item::FRecent(_) | Item::FGroup(_))).unwrap_or(next)
        };
        self.qam_focus_fid(target as i32);
        play(Snd::ChangeTabs);
    }

    /// Give gamepad focus to a content row (tree + cursor + scroll), without a rebuild.
    fn qam_focus_fid(&mut self, fid: i32) {
        self.qm.rail_focus = None;
        self.qm.cursor = fid;
        if let (Some(t), Some(&id)) = (self.qm.tree.as_mut(), self.qm.row_nodes.get(fid.max(0) as usize)) {
            t.transfer_focus(FocusSource::Gamepad, Some(id), None);
            let _ = t.drain_events();
        }
        self.qam_scroll_into_view();
        self.qam_refresh_light();
    }

    /// Open a favorite / recent / group entry: friends open their chat, groups open the group chat (clans are not focusable).
    fn qam_open_entry(&mut self, e: &qam_friends::Entry) {
        match e.kind {
            qam_friends::Kind::Friend => self.act(Act::OpenChat(e.acc)),
            qam_friends::Kind::Group if !e.gid.is_empty() => self.act(Act::OpenChatGroup(e.gid.clone())),
            _ => {}
        }
    }

    /// Move focus from vertical rail into the active tab content.
    fn qam_enter_content(&mut self) {
        if self.qm.rects.is_empty() && self.qm.tab != 4 {
            play(Snd::FailedNav);
            return;
        }
        self.qm.rail_focus = None;
        let fid = if self.qm.tab == 1 {
            self.qm.ftab
        } else if self.qm.tab == 2 {
            self.qam_quick_cursor(self.qm.last_quick_item).unwrap_or(0) as usize
        } else if self.qm.last_fid[self.qm.tab] >= 0 && (self.qm.last_fid[self.qm.tab] as usize) < self.qm.rects.len() {
            self.qm.last_fid[self.qm.tab] as usize
        } else {
            0
        };
        self.qm.cursor = fid as i32;
        if let (Some(t), Some(&id)) = (self.qm.tree.as_mut(), self.qm.row_nodes.get(fid)) {
            t.transfer_focus(FocusSource::Gamepad, Some(id), None);
            let _ = t.drain_events();
        }
        self.qam_scroll_into_view();
        self.qam_refresh_light();
        play(Snd::BasicNav);
    }

    /// Return focus from tab content back to the vertical rail.
    fn qam_return_to_rail(&mut self) {
        self.qm.slider_adjusting = false;
        let want = self.qm.tab;
        self.qm.rail_focus = Some(want);
        if let (Some(t), Some(&id)) = (self.qm.tree.as_mut(), self.qm.rail_nodes.get(want)) {
            t.transfer_focus(FocusSource::Gamepad, Some(id), None);
            let _ = t.drain_events();
        }
        self.qam_refresh_light();
        play(Snd::BasicNav);
    }

    /// Cheap sync (cursor, scroll, rail focus) without rebuilding rows.
    pub(crate) fn qam_refresh_light(&mut self) {
        let ui = self.ui();
        let g = ui.global::<QamState>();
        g.set_rail_focus(if self.pad_mode { self.qm.rail_focus.map(|x| x as i32).unwrap_or(-1) } else { -1 });
        g.set_cursor(if self.pad_mode { self.qm.cursor } else { -1 });
        g.set_slider_adjusting(self.pad_mode && self.qm.slider_adjusting);
        g.set_scroll(self.qm.scroll);
        self.qm.focus = self.qm.tab as i32 * 1000 + self.qm.rail_focus.map(|x| 100 + x as i32).unwrap_or(0) + (self.qm.cursor.max(0) % 100);
        if self.qm.open { self.update_hint(); }
    }

    pub fn qam_select_tab(&mut self, i: usize, by_pointer: bool) {
        if i >= TABS {
            return;
        }
        let changed = i != self.qm.tab;
        self.qm.tab = i;
        if by_pointer {
            self.qm.rail_focus = Some(i);
        } else if self.qm.rail_focus.is_some() {
            self.qm.rail_focus = Some(i);
        }
        if changed {
            play(Snd::ChangeTabs);
        }
        self.qam_enter_tab();
    }

    fn qam_press(&mut self) {
        if self.qm.slider_adjusting {
            self.qm.slider_adjusting = false;
            self.qm.slider_repeat_start = None;
            self.qam_refresh_light();
            return;
        }
        if self.qm.rail_focus.is_some() {
            // A on a rail tab: enter its content
            self.qam_enter_content();
            return;
        }
        let fid = self.qm.cursor;
        if fid < 0 { return; }
        if let Some(&item) = self.qm.items.get(fid as usize) {
            if is_slider(item) && self.qam_value(item).is_some_and(|v| v >= 0.0) {
                self.qm.slider_adjusting = true;
                self.qm.slider_repeat_start = None;
                self.qam_refresh_light();
                return;
            }
        }
        self.qam_click(fid);
    }

    pub fn qam_click(&mut self, fid: i32) {
        self.qm.slider_adjusting = false;
        if fid < 0 {
            return;
        }
        self.qm.rail_focus = None;
        self.qm.cursor = fid;
        self.qm.last_fid[self.qm.tab] = fid;
        let Some(&item) = self.qm.items.get(fid as usize) else { return };
        if self.qm.tab == 2 { self.qm.last_quick_item = Some(item); }
        self.qm.st_stamp = Instant::now();
        let toggle = |on: bool| play(if on { Snd::ToggleOn } else { Snd::ToggleOff });
        match item {
            // Pointer activation is separate from gamepad slider adjustment.
            Item::Volume | Item::Mic | Item::Brightness | Item::TextSize | Item::OverlayLevel | Item::Tdp | Item::FpsLimit | Item::Refresh | Item::Gpu | Item::Filter | Item::Scaler | Item::Sharp => {}
            Item::Controllers => {
                // Steam opens its controller-order dialog; the closest in-shell page is Settings / Controller
                self.navigate(crate::router::Route::Settings("controller".into()));
                return;
            }
            Item::Airplane => {
                if let Some(on) = self.qm.st.airplane {
                    self.qm.st.airplane = Some(!on);
                    toggle(!on);
                    self.act(Act::SetAirplane(!on));
                }
            }
            Item::Wifi => {
                if let Some(on) = self.qm.st.wifi {
                    super::internet::request_radio(self, !on);
                }
            }
            Item::Bluetooth | Item::BluetoothDevice(_) => {
                if let Some(op) = self.qam_bluetooth_action(item) {
                    if let BluetoothAction::Power(on) = op { toggle(on); }
                    self.qam_write_bluetooth(op, super::bluetooth::backend());
                }
            }
            Item::BluetoothAdd => {
                self.qam_open_bluetooth_settings();
                return;
            }
            Item::NightMode => {
                if let Some(on) = self.qm.st.night_mode.filter(|_| !self.qm.night_pending) {
                    toggle(!on);
                    self.qam_write_night_mode(!on);
                }
            }
            Item::UiAuto => {
                self.qm.perf.ui_auto = !self.qm.perf.ui_auto;
                toggle(self.qm.perf.ui_auto);
                self.act(Act::UiAutoScale(self.qm.perf.ui_auto));
            }
            Item::Overlay => {
                if self.qm.perf.overlay_observed.is_none() || self.qm.perf.overlay_write_failed { return; }
                let p = &mut self.qm.perf;
                let level = if p.overlay_observed == Some(0) { p.level } else { 0 };
                let generation = OVERLAY_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                p.overlay_pending = Some((generation, level));
                actions::set_perf_overlay(generation, level, p.perf_avail != Some(true));
            }
            Item::Advanced => {
                self.qm.perf.advanced = !self.qm.perf.advanced;
                toggle(self.qm.perf.advanced);
            }
            Item::Profile => {
                self.qm.perf.game_profile = !self.qm.perf.game_profile;
                toggle(self.qm.perf.game_profile);
                self.act(Act::Perf { method: "SetGameSpecificProfileEnabled", args: self.qm.perf.game_profile.to_string() });
            }
            Item::ProfileReset => {
                if self.qm.perf.overlay_pending.is_some() { return; }
                self.qm.perf = PerfState { overlay: self.qm.perf.overlay, level: self.qm.perf.level, overlay_observed: self.qm.perf.overlay_observed, overlay_pending: self.qm.perf.overlay_pending, overlay_write_failed: self.qm.perf.overlay_write_failed, advanced: self.qm.perf.advanced, perf_avail: self.qm.perf.perf_avail, game_id: self.qm.perf.game_id, game_name: self.qm.perf.game_name.clone(), ui_auto: self.qm.perf.ui_auto, text_size: self.qm.perf.text_size,
                    // capability limits come from the system, not from the profile being reset: keep them until the next PERF_READ
                    fps_opts: self.qm.perf.fps_opts.clone(), hz_min: self.qm.perf.hz_min, hz_max: self.qm.perf.hz_max, refresh_avail: self.qm.perf.refresh_avail,
                    vrr_supported: self.qm.perf.vrr_supported, vrs_avail: self.qm.perf.vrs_avail, sharp_min: self.qm.perf.sharp_min, sharp_max: self.qm.perf.sharp_max, ..PerfState::default() };
                self.act(Act::Perf { method: "ResetCurrentPerfProfileSettings", args: String::new() });
            }
            Item::Vrs => {
                self.qm.perf.vrs = !self.qm.perf.vrs;
                toggle(self.qm.perf.vrs);
                self.act(Act::ClientSetting { name: "steamos_vrs_enabled", value: self.qm.perf.vrs.to_string() });
            }
            Item::Vrr => {
                self.qm.perf.vrr = !self.qm.perf.vrr;
                toggle(self.qm.perf.vrr);
                self.act(Act::Perf { method: "SetVRREnabled", args: self.qm.perf.vrr.to_string() });
            }
            Item::Tearing => {
                self.qm.perf.tearing = !self.qm.perf.tearing;
                toggle(self.qm.perf.tearing);
                self.act(Act::ClientSetting { name: "gamescope_allow_tearing", value: self.qm.perf.tearing.to_string() });
            }
            Item::GpuOn => {
                if !self.qm.perf.gpu_pending {
                    if let Some((on, clock, min, _)) = self.qm.st.gpu_clock {
                        self.qm.slider_adjusting = false;
                        self.qm.perf.gpu_pending = true;
                        HARDWARE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        actions::set_gpu_clock_verified(!on, clock.unwrap_or(min));
                    }
                }
            }
            Item::TdpOn => {
                if self.qm.perf.tdp_pending { return; }
                if let Some((c, lo, hi)) = self.qm.st.tdp {
                    self.qm.perf.tdp_on = !self.qm.perf.tdp_on;
                    toggle(self.qm.perf.tdp_on);
                    // One hardware writer: the cap goes through SteamOS Manager with sysfs readback, like the slider.
                    // Steam's settings only store the switch and the value.
                    let on = self.qm.perf.tdp_on;
                    let target = tdp_toggle_target(on, c, lo, hi);
                    self.qm.perf.tdp_pending = true;
                    HARDWARE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    actions::set_tdp_verified(target);
                    self.act(Act::ClientSetting { name: "steamos_tdp_limit_enabled", value: on.to_string() });
                    if on { self.act(Act::ClientSetting { name: "steamos_tdp_limit", value: target.to_string() }); }
                }
            }
            Item::Plugin(i) => {
                let n = self.qm.plugins.get(i).cloned().unwrap_or_default();
                // HOOK(decky): the DeckyTab integration opens the plugin panel here
                self.toast(&format!("Decky: {n}"));
            }
            // notification row: open its target (game page when it names an app), otherwise just dismiss (RemoveGroupFromTray)
            Item::Notif(i) => {
                if let Some(n) = self.qm.notifs.get(i).cloned() {
                    self.act(Act::NotifDismiss(n.id));
                    self.qm.notifs.remove(i);
                    if let Some(gi) = n.appid.and_then(|id| self.gidx.get(&id).copied()) {
                        self.qm.open = false;
                        self.sync_overlays();
                        self.open_game(gi);
                        return;
                    }
                }
            }
            // friend row: OpenChatDialog + FocusChatTextEntryByAccountID (chat lives in Steam's own QAM/chat window)
            Item::Friend(i) => {
                if let Some(f) = self.qm.friends.get(i).cloned() {
                    self.act(Act::OpenChat(f.acc));
                }
            }
            Item::FTab(k) => self.qm.ftab = k,
            Item::FInvite | Item::FAdd => self.act(Act::FriendsInvites),
            Item::FNewGroup => self.act(Act::CreateChatGroup),
            Item::FFav(i) => {
                if let Some(e) = self.qm.favorites.get(i).cloned() {
                    self.qam_open_entry(&e);
                }
            }
            Item::FRecent(i) => {
                if let Some(e) = self.qm.recent.get(i).cloned() {
                    self.qam_open_entry(&e);
                }
            }
            Item::FGroup(i) => {
                if let Some(e) = self.qm.groups.get(i).cloned() {
                    self.qam_open_entry(&e);
                }
            }
        }
        self.qam_rebuild(false);
        self.qam_refresh_light();
    }

    /// START on the focused row: friend -> profile (Steam: friend context menu), notification -> dismiss.
    fn qam_menu(&mut self) {
        if self.qm.rail_focus.is_some() {
            return;
        }
        let Some(&item) = self.qm.items.get(self.qm.cursor.max(0) as usize) else { return };
        match item {
            Item::Bluetooth | Item::BluetoothDevice(_) => self.qam_open_bluetooth_settings(),
            Item::Friend(i) => {
                if let Some(f) = self.qm.friends.get(i).cloned() {
                    self.act(Act::FriendProfile(f.acc));
                }
            }
            Item::FFav(i) | Item::FRecent(i) => {
                let entry = if matches!(item, Item::FFav(_)) { self.qm.favorites.get(i) } else { self.qm.recent.get(i) };
                if let Some(e) = entry.filter(|e| e.kind == qam_friends::Kind::Friend).cloned() {
                    self.act(Act::FriendProfile(e.acc));
                }
            }
            Item::Notif(i) => {
                if let Some(n) = self.qm.notifs.get(i).cloned() {
                    self.act(Act::NotifDismiss(n.id));
                    self.qm.notifs.remove(i);
                    self.qam_rebuild(false);
                    self.qam_refresh_light();
                }
            }
            _ => {}
        }
    }

    /// Mouse wheel over the content: scroll the column (Steam scrolls the tab's ScrollPanel).
    pub fn qam_wheel(&mut self, dy: f32) {
        self.qm.scroll = qam_wheel_scroll(self.qm.scroll, self.qam_scroll_max(), dy);
        self.qam_refresh_light();
    }

    /// Pointer drag / click on a slider: absolute value 0..1.
    pub fn qam_drag(&mut self, fid: i32, v: f32) {
        self.qm.slider_adjusting = false;
        self.qm.rail_focus = None;
        self.qm.cursor = fid;
        self.qam_set_slider(fid, v.clamp(0.0, 1.0), true);
    }

    /// Normalised value (0..1) of a slider row, `None` = unavailable.
    fn qam_value(&self, item: Item) -> Option<f32> {
        let p = &self.qm.perf;
        Some(match item {
            Item::Brightness => self.qm.st.brightness?,
            Item::Volume => self.qm.st.volume.map(|x| x.0)?,
            Item::Mic => mic_to_slider(self.qm.st.mic?),
            Item::TextSize => p.text_size,
            Item::OverlayLevel => p.overlay_pending.map(|(_, l)| l).or(p.overlay_observed)? as f32 / 4.0,
            Item::Tdp => {
                let (c, lo, hi) = self.qm.st.tdp?;
                c.saturating_sub(lo) as f32 / (hi - lo).max(1) as f32
            }
            Item::FpsLimit => p.fps as f32 / (p.fps_opts.len().max(2) - 1) as f32,
            Item::Refresh => (p.refresh_hz - p.hz_min) / (p.hz_max - p.hz_min).max(1.0),
            Item::Gpu => { let (_, clock, min, max) = self.qm.st.gpu_clock?; (clock? - min) as f32 / (max - min) as f32 },
            Item::Filter => FILTERS.iter().position(|&x| x == p.filter).unwrap_or(0) as f32 / (FILTERS.len() - 1) as f32,
            Item::Scaler => SCALERS.iter().position(|&x| x == p.scaler).unwrap_or(0) as f32 / (SCALERS.len() - 1) as f32,
            Item::Sharp => (p.sharp - p.sharp_min) as f32 / (p.sharp_max - p.sharp_min).max(1) as f32,
            _ => return None,
        })
    }

    /// One D-pad step of a slider (fraction of its range).
    fn qam_step(&self, item: Item) -> f32 {
        let p = &self.qm.perf;
        match item {
            Item::OverlayLevel => 0.25,
            Item::FpsLimit => 1.0 / (p.fps_opts.len().max(2) - 1) as f32,
            Item::Filter => 1.0 / (FILTERS.len() - 1) as f32,
            Item::Scaler => 1.0 / (SCALERS.len() - 1) as f32,
            Item::Sharp => 1.0 / (p.sharp_max - p.sharp_min).max(1) as f32,
            Item::Tdp => self.qm.st.tdp.map(|(_, lo, hi)| 1.0 / (hi - lo).max(1) as f32).unwrap_or(0.05),
            Item::Refresh => 1.0 / (p.hz_max - p.hz_min).max(1.0),
            Item::Gpu => self.qm.st.gpu_clock.map(|(_, _, min, max)| 100.0 / (max - min) as f32).unwrap_or(0.05),
            Item::Brightness | Item::Volume | Item::Mic => 0.01,
            _ => 0.05,
        }
    }

    pub(crate) fn qam_level_result(&mut self, brightness: bool, observed: Option<(f32, bool)>, result: Result<(), String>) {
        if brightness {
            let ui = self.ui();
            let shell = ui.global::<crate::Shell>();
            let popup_visible = shell.get_level_shown() && shell.get_level_kind() == 0;
            if let Some(value) = brightness_readback(&mut self.qm.st, observed, popup_visible) {
                self.level_update_value(value);
            }
        } else { self.qm.st.volume = observed; }
        self.qm.st_stamp = Instant::now();
        self.qam_refresh_light();
        // the QAM rows show the stored level: rebuild so the slider lands on the confirmed value
        if self.qm.open { self.qam_rebuild(false); }
        if result.is_err() || observed.is_none() {
            let name = if brightness { i18n::tr_ref("qam.brightness") } else { i18n::tr_ref("qam.volume") };
            self.toast(&i18n::tr_args("qam.change_unconfirmed_or_unavailable", &[("name", &(name).to_string())]));
        }
    }

    /// Gamepad Left/Right on a slider (engine onMoveLeft/Right).
    pub fn qam_slide(&mut self, fid: i32, dir: f32) { self.qam_slide_repeat(fid, dir, 0.0); }

    fn qam_slide_repeat(&mut self, fid: i32, dir: f32, elapsed_ms: f32) {
        let Some(&item) = self.qm.items.get(fid.max(0) as usize) else { return };
        let Some(cur) = self.qam_value(item) else {
            play(Snd::FailedNav);
            return;
        };
        let step = self.qam_step(item);
        let step = if matches!(item, Item::Brightness | Item::Volume | Item::Mic) { crate::level::hold_step(elapsed_ms) * step / crate::level::SLIDER_STEP } else { step };
        let nv = (cur + dir * step).clamp(0.0, 1.0);
        if (nv - cur).abs() < 1e-4 {
            play(Snd::FailedNav);
            return;
        }
        play(if dir > 0.0 { Snd::SliderUp } else { Snd::SliderDown });
        self.qam_set_slider(fid, nv, false);
    }

    /// Brightness and volume rows show the stored value at once, as the hardware chord does
    /// (`level_changed`). Waiting for the write confirmation (`qam_level_result`) made the slider lag by the write time.
    fn qam_slider_shown(&mut self) {
        if self.qm.open {
            self.qam_rebuild(false);
            self.qam_refresh_light();
        }
    }

    fn qam_set_slider(&mut self, fid: i32, v: f32, _drag: bool) {
        let Some(&item) = self.qm.items.get(fid.max(0) as usize) else { return };
        if !self.qam_value(item).is_some_and(|value| value >= 0.0) { return; }
        self.qm.st_stamp = Instant::now();
        match item {
            // The QAM slider never shows the level popup: it is for hardware actions only (STEAM / "..." + stick,
            // physical volume buttons). Its own volume echo from pactl is matched in level_external and skipped.
            Item::Brightness => {
                self.qm.st.brightness = Some(v);
                crate::actions::set_brightness(&self.ds, v);
                self.qam_slider_shown();
            }
            Item::Volume => {
                if let Some((_, muted)) = self.qm.st.volume { self.qm.st.volume = Some((v, muted)); }
                crate::level::note_own_volume(v);
                crate::actions::set_volume(&self.ds, v);
                self.qam_slider_shown();
            }
            Item::Mic => {
                let v = slider_to_mic(v);
                self.qm.st.mic = Some(v);
                sys_write(&format!("mic volume {v:.2}"), move || {
                    if deck_system::guard::deny("wpctl set-volume mic") { return; }
                    let _ = std::process::Command::new("wpctl").args(["set-volume", "@DEFAULT_AUDIO_SOURCE@", &format!("{v:.2}")]).status();
                });
            }
            Item::TextSize => {
                let p = &mut self.qm.perf;
                p.text_size = v;
                p.ui_auto = false;
                // SetGamepadUIManualDisplayScaleFactor(min + v * (max - min)) (sp.js:377485)
                let f = p.scale_min + v * (p.scale_max - p.scale_min);
                self.act(Act::UiScaleFactor(f));
            }
            Item::OverlayLevel => {
                let l = (v * 4.0).round() as i32;
                if self.qm.perf.overlay_observed.is_none() || self.qm.perf.overlay_write_failed { return; }
                let p = &mut self.qm.perf;
                let generation = OVERLAY_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                p.overlay_pending = Some((generation, l));
                crate::actions::set_perf_overlay(generation, l, p.perf_avail != Some(true));
            }
            Item::Tdp => {
                if self.qm.perf.tdp_pending || self.qm.perf.tdp_failed { return; }
                if let Some((current, lo, hi)) = self.qm.st.tdp {
                    let target = lo + (v * (hi - lo) as f32).round() as u32;
                    if target != current {
                        self.qm.slider_adjusting = false;
                        self.qm.perf.tdp_pending = true;
                        HARDWARE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        actions::set_tdp_verified(target);
                        self.act(Act::ClientSetting { name: "steamos_tdp_limit", value: target.to_string() });
                    }
                }
            }
            Item::FpsLimit => {
                let n = self.qm.perf.fps_opts.len().max(2) - 1;
                self.qm.perf.fps = (v * n as f32).round() as usize;
                let f = self.qm.perf.fps_opts.get(self.qm.perf.fps).copied().unwrap_or(0);
                // One writer per setting: the Perf channel when Steam has it, otherwise the gamescope atom.
                if self.qm.perf.perf_avail == Some(true) {
                    self.act(Act::Perf { method: "SetFPSLimit", args: format!("false,{f}") });
                } else {
                    self.act(Act::GsAtom { name: crate::gamescope_atoms::FPS_LIMIT, value: f });
                }
            }
            Item::Refresh => {
                let p = &mut self.qm.perf;
                p.refresh_hz = (p.hz_min + v * (p.hz_max - p.hz_min)).round();
                let hz = p.refresh_hz as i32;
                if self.qm.perf.perf_avail == Some(true) {
                    self.act(Act::Perf { method: "SetDisplayRefreshRateManualHz", args: format!("false,{hz}") });
                } else {
                    self.act(Act::GsAtom { name: crate::gamescope_atoms::DYNAMIC_REFRESH, value: hz as u32 });
                }
            }
            Item::Gpu => {
                if self.qm.perf.gpu_pending { return; }
                if let Some((true, Some(current), min, max)) = self.qm.st.gpu_clock {
                    let target = (((min as f32 + v * (max - min) as f32) / 100.0).round() as u32 * 100).clamp(min, max);
                    if target != current {
                        self.qm.slider_adjusting = false;
                        self.qm.perf.gpu_pending = true;
                        HARDWARE_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        actions::set_gpu_clock_verified(true, target);
                    }
                }
            }
            Item::Filter => {
                let i = (v * (FILTERS.len() - 1) as f32).round() as usize;
                self.qm.perf.filter = FILTERS[i.min(FILTERS.len() - 1)];
                if self.qm.perf.perf_avail == Some(true) {
                    self.act(Act::Perf { method: "SetSplitScalingFilter", args: self.qm.perf.filter.to_string() });
                } else {
                    let value = [0, 1, 4][i.min(2)];
                    self.act(Act::GsAtom { name: crate::gamescope_atoms::SCALING_FILTER, value });
                }
            }
            Item::Scaler => {
                let i = (v * (SCALERS.len() - 1) as f32).round() as usize;
                self.qm.perf.scaler = SCALERS[i.min(SCALERS.len() - 1)];
                if self.qm.perf.perf_avail == Some(true) {
                    self.act(Act::Perf { method: "SetSplitScalingScaler", args: self.qm.perf.scaler.to_string() });
                } else {
                    let value = (self.qm.perf.scaler - 1) as u32;
                    self.act(Act::GsAtom { name: crate::gamescope_atoms::SCALING_SCALER, value });
                }
            }
            Item::Sharp => {
                let p = &mut self.qm.perf;
                p.sharp = p.sharp_min + (v * (p.sharp_max - p.sharp_min) as f32).round() as i32;
                let n = p.sharp;
                self.act(Act::Perf { method: "SetFSRSharpness", args: n.to_string() });
            }
            _ => return,
        }
        self.qam_rebuild(false);
        self.qam_refresh_light();
    }
}

/// Called by `App::sync_overlays` so the status poller knows whether to read the Perf state.
pub fn set_open_flag(open: bool) {
    let was_open = QAM_OPEN.swap(open, std::sync::atomic::Ordering::Relaxed);
    if open && !was_open {
        if let Some(t) = STATUS_POLLER.get() { t.unpark(); }
    }
}

#[cfg(test)]
mod friends_focus_tests {
    use super::*;
    use crate::nav::Button;

    fn lists() -> qam_friends::Lists {
        let v: serde_json::Value = serde_json::from_str(include_str!("qam_friends_fixture.json")).expect("fixture");
        qam_friends::parse(&v).expect("fixture object")
    }

    /// Rows of the Friends tab exactly as the live QAM builds them (same builder, no App).
    fn build(l: &qam_friends::Lists, ftab: usize, invites: u32) -> B {
        let avatars = HashMap::new();
        let icons = HashMap::new();
        let view = FriendsView { ftab, invites, friends: &l.friends, favorites: &l.favorites, groups: &l.groups, recent: &l.recent, avatars: &avatars, icons: &icons };
        let mut b = B::new();
        build_friends_tab(&mut b, &view, &mut |_| {});
        b
    }

    /// Engine tree with the same content column as `qam_build_nav`; focus starts on fid 0.
    fn engine(b: &B) -> (Tree, Vec<NodeId>) {
        let mut t = Tree::new((1280.0, 800.0));
        let root = t.root();
        let panel = t.add_node(root, Props { layout: Layout::Row, ..Default::default() }, Some(NRect::xywh(0.0, 0.0, PANEL_W, PANEL_H)));
        let content = t.add_node(panel, Props { layout: Layout::Column, ..Default::default() }, Some(NRect::xywh(RAIL_W, 0.0, PANEL_W - RAIL_W, PANEL_H)));
        let nodes = add_row_nodes(&mut t, content, 1, &b.rects, &b.spans, &b.items);
        t.transfer_focus(FocusSource::Application, nodes.first().copied(), None);
        let _ = t.drain_events();
        (t, nodes)
    }

    /// One d-pad press: the fid focus moved to (as handle_qam maps it), or None when the engine failed the move.
    fn press(t: &mut Tree, nodes: &[NodeId], btn: Button) -> Option<usize> {
        t.handle_button(btn, false);
        let mut moved = None;
        for ev in t.drain_events() {
            match ev {
                NavEvent::Focus { to: Some(n), .. } => moved = nodes.iter().position(|&x| x == n),
                NavEvent::FailedNav => moved = None,
                _ => {}
            }
        }
        moved
    }

    #[test]
    fn strip_moves_between_tabs_and_fails_at_the_ends() {
        let l = lists();
        let b = build(&l, qam_friends::TAB_FRIENDS, 2);
        let (mut t, nodes) = engine(&b);
        assert_eq!(b.items[0], Item::FTab(0));
        assert_eq!(press(&mut t, &nodes, Button::DirLeft), None, "Left on the first tab fails (rail return happens in handle_qam)");
        assert_eq!(press(&mut t, &nodes, Button::DirRight), Some(1));
        assert_eq!(b.items[1], Item::FTab(1), "focus selects the tab it lands on");
        assert_eq!(press(&mut t, &nodes, Button::DirRight), Some(2));
        assert_eq!(press(&mut t, &nodes, Button::DirRight), Some(3));
        assert_eq!(press(&mut t, &nodes, Button::DirRight), None, "Right on the last tab fails");
        assert_eq!(press(&mut t, &nodes, Button::DirLeft), Some(2));
    }

    #[test]
    fn strip_down_reaches_header_then_first_row_and_back() {
        let l = lists();
        let b = build(&l, qam_friends::TAB_FRIENDS, 2);
        let (mut t, nodes) = engine(&b);
        assert_eq!(press(&mut t, &nodes, Button::DirRight), Some(1));
        let header = press(&mut t, &nodes, Button::DirDown).expect("Down from the strip");
        assert!(matches!(b.items[header], Item::FInvite | Item::FAdd), "header button, got {:?}", b.items[header]);
        let row = press(&mut t, &nodes, Button::DirDown).expect("Down from the header");
        assert!(matches!(b.items[row], Item::Friend(_)), "first friend row, got {:?}", b.items[row]);
        let back = press(&mut t, &nodes, Button::DirUp).expect("Up from the first row");
        assert!(matches!(b.items[back], Item::FInvite | Item::FAdd));
        let top = press(&mut t, &nodes, Button::DirUp).expect("Up from the header");
        assert!(matches!(b.items[top], Item::FTab(_)), "back on the strip, got {:?}", b.items[top]);
    }

    #[test]
    fn header_buttons_follow_the_active_tab() {
        let l = lists();
        let friends = build(&l, qam_friends::TAB_FRIENDS, 2);
        assert!(friends.items.contains(&Item::FInvite) && friends.items.contains(&Item::FAdd));
        let no_invites = build(&l, qam_friends::TAB_FRIENDS, 0);
        assert!(!no_invites.items.contains(&Item::FInvite), "Invites only with pending invites");
        let groups = build(&l, qam_friends::TAB_GROUPS, 2);
        assert!(groups.items.contains(&Item::FNewGroup) && !groups.items.contains(&Item::FAdd));
        assert_eq!(groups.items.iter().filter(|i| matches!(i, Item::FGroup(_))).count(), 2);
        let favorites = build(&l, qam_friends::TAB_FAVORITES, 2);
        assert!(!favorites.items.iter().any(|i| matches!(i, Item::FInvite | Item::FAdd | Item::FNewGroup)), "no header buttons on Favorites");
        assert_eq!(favorites.items.iter().filter(|i| matches!(i, Item::FFav(_))).count(), 3, "Favorites shows friend and group rows; the clan is not focusable");
        let recent = build(&l, qam_friends::TAB_RECENT, 2);
        assert_eq!(recent.items.iter().filter(|i| matches!(i, Item::FRecent(_))).count(), 3);
    }

    #[test]
    fn recent_tab_badge_shows_total_unread() {
        let l = lists();
        let b = build(&l, qam_friends::TAB_FAVORITES, 0);
        assert_eq!(b.rows[qam_friends::TAB_RECENT].rtext.as_str(), "5");
        assert_eq!(b.rows[qam_friends::TAB_FAVORITES].rtext.as_str(), "");
    }

    /// Layout lint over every Friends tab: strip cells tile the content width, header cells do not overlap, no focusable
    /// row overlaps another row outside its own line, and every row stays inside the content column and panel.
    #[test]
    fn friends_tab_layout_lint() {
        let l = lists();
        let cw = PANEL_W - RAIL_W;
        let mut problems = Vec::new();
        for ftab in 0..qam_friends::TABS {
            for invites in [0u32, 2] {
                let b = build(&l, ftab, invites);
                let tag = format!("ftab={ftab} invites={invites}");
                let strip: Vec<_> = b.items.iter().enumerate().filter(|(_, it)| matches!(it, Item::FTab(_))).map(|(i, _)| b.spans[i]).collect();
                if strip.len() != qam_friends::TABS {
                    problems.push(format!("{tag}: strip has {} cells", strip.len()));
                }
                // Stock strip: cells are contiguous on a 46 px (x1.2) pitch and centred in the column.
                let first = strip.first().map(|s| s.0).unwrap_or(0.0);
                let mut x = first;
                for (sx, sw, _) in &strip {
                    if (sx - x).abs() > 0.01 {
                        problems.push(format!("{tag}: strip gap at {sx}"));
                    }
                    x = sx + sw;
                }
                if (first - (cw - x)).abs() > 0.01 {
                    problems.push(format!("{tag}: strip not centred: left {first}, right {}", cw - x));
                }
                for r in &b.rows {
                    if !(r.h > 0.0 && r.y >= Y0 - 0.01 && r.y.is_finite()) {
                        problems.push(format!("{tag}: bad row y={} h={}", r.y, r.h));
                    }
                    if r.cw > 0.0 && (r.cx < -0.01 || r.cx + r.cw > cw + 0.01) {
                        problems.push(format!("{tag}: cell outside content x={} w={}", r.cx, r.cw));
                    }
                }
                // rects of focusable items: cells of one line are side by side, everything else is one full-width row
                for (i, &(y, h)) in b.rects.iter().enumerate() {
                    let (x, w, g) = b.spans[i];
                    for (j, &(y2, h2)) in b.rects.iter().enumerate().skip(i + 1) {
                        let (x2, w2, g2) = b.spans[j];
                        let overlap = x < x2 + w2 && x2 < x + w && y < y2 + h2 && y2 < y + h;
                        if overlap && !(g != 0 && g == g2) {
                            problems.push(format!("{tag}: focusable {:?} overlaps {:?}", b.items[i], b.items[j]));
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "friends layout lint: {} problem(s):\n{}", problems.len(), problems.join("\n"));
    }
}

#[cfg(test)]
mod closed_poll_tests {
    use super::*;

    #[test]
    fn closed_poll_keeps_the_last_qam_only_values() {
        let prev = Status { wifi: Some(true), night_mode: Some(false), mic: Some(0.4), airplane: Some(true), brightness_avail: Some(true), ..Default::default() };
        let mut closed = Status { detailed: false, wifi: Some(false), ..Default::default() };
        carry_closed_poll_fields(&mut closed, &prev);
        assert_eq!((closed.wifi, closed.night_mode, closed.mic, closed.airplane, closed.brightness_avail), (Some(false), Some(false), Some(0.4), Some(true), Some(true)));
    }

    #[test]
    fn fallback_keeps_last_known_wifi_on_read_failure() {
        assert_eq!(fallback_wifi(-1, None), -1);
        assert_eq!(fallback_wifi(1, None), 1);
        assert_eq!(fallback_wifi(1, Some(false)), 0);
        assert_eq!(fallback_wifi(0, Some(true)), 1);
    }

    #[test]
    fn open_poll_replaces_the_values_it_read() {
        let prev = Status { wifi: Some(true), mic: Some(0.4), ..Default::default() };
        let mut open = Status { detailed: true, wifi: Some(false), mic: None, ..Default::default() };
        carry_closed_poll_fields(&mut open, &prev);
        assert_eq!((open.wifi, open.mic), (Some(false), None));
    }
}

/// Steam `eBatteryState` for the system reading: full 3, charging 2, otherwise discharging 1.
fn battery_state(b: &deck_system::Battery) -> i64 {
    if b.full { battery_logic::FULL } else if b.charging { battery_logic::CHARGING } else { battery_logic::DISCHARGING }
}
/// Steam `eACState` (module 98146): 0 unknown, 1 unplugged, 2 plugged (3 = plugged but slow, not detectable from sysfs).
fn ac_state(b: &deck_system::Battery) -> i64 {
    match b.online { Some(true) => 2, Some(false) => 1, None => 0 }
}
