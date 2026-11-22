//! Read-only getters for the lower Home page (What's New / Friends / Recommended). Sources (sp.js, build 11146165):
//! `libraryEventStore.GetWhatsNewEvents()` (sp.js:443719 -> events carousel, 384741), `downloadsStore.RecentlyCompleted`
//! (383736 "Недавно обновлено на этом устройстве"), `trendingStore.TrendingApps` + `StoreItemCache` ("Популярно у друзей", 383980),
//! `friendStore.m_FriendsUIFriendStore` (friends tab, 389269-389448), `appActivityStore.GetAppActivity(0)` (friend activity feed,
//! 503700), `playNextStore`. Nothing here changes Steam state (the store-item hint only asks Steam to fill its own cache).
use serde_json::Value;

/// One JSON document with every section; each part is wrapped so a missing store only empties its section.
pub const HOME_FEED: &str = r#"(()=>{
const T=(f,d)=>{try{const v=f();return v===undefined?d:v}catch(e){return d}};
const L=8,S=x=>String(x==null?'':x);
const events=T(()=>libraryEventStore.GetWhatsNewEvents().eventsToShow.slice(0,12).map(e=>({gid:S(e.GID),appid:e.appid,cat:T(()=>e.GetCategoryAsString(),''),type:T(()=>e.GetEventType(),0),name:T(()=>e.GetNameWithFallback(L),'')||S(e.name),sum:S(T(()=>e.GetSummaryWithFallback(L),'')).slice(0,240),post:T(()=>e.GetPostTimeAndDateUnixSeconds(),0),img:S(T(()=>e.GetImageURL('capsule',L),''))})),[]);
const completed=T(()=>downloadsStore.RecentlyCompleted.slice(0,16).map(t=>{let b=0;for(const u of t.update_type_info||[]){const p=(u.progress||[])[2]||{};b=Math.max(b,p.bytes_total||0)}return {appid:t.appid,time:t.completed_time,bytes:b}}),[]);
const tr=T(()=>trendingStore.TrendingApps.slice(0,24),[]);
const ids=tr.map(x=>x.appid);
T(()=>StoreItemCache.HintLoadStoreApps(ids,{include_assets:true,include_basic_info:true,include_release:true}),0);
const store=ids.map(id=>{const a=T(()=>StoreItemCache.GetApp(id),null);const o=T(()=>appStore.GetAppOverviewByAppID(id),null);const bp=a&&T(()=>a.GetBestPurchaseOption(),null);
 return {appid:id,name:S((a&&T(()=>a.GetName(),''))||(o&&o.display_name)),price:S(bp&&bp.formatted_final_price),orig:S(bp&&bp.formatted_original_price),pct:(bp&&bp.discount_pct)||0,free:!!(a&&a.m_bIsFree),soon:!!(a&&a.m_bIsComingSoon),owned:!!(o&&T(()=>o.BIsOwned(),false)),ready:!!a}});
const fs=T(()=>friendStore.m_FriendsUIFriendStore.all_friends,[]);
const friends=fs.map(f=>{const p=f.persona||f.m_persona||{};return {aid:f.m_unAccountID,steamid:S(T(()=>p.m_steamid.ConvertTo64BitString(),'')),name:S(p.m_strPlayerName),state:p.m_ePersonaState||0,game:S(T(()=>p.GetCurrentGameName(),'')),gameid:p.m_unGamePlayedAppID||0,seen:p.m_rtLastSeenOnline||0,avatar:S(p.m_strAvatarHash)}});
const act=[];T(()=>{const a=appActivityStore.GetAppActivity(0);if(!a)return;for(const d of a.appActivityByDay.slice(0,6)){for(const e of d.m_rgEvents||[]){if(act.length>=30)return;act.push({aid:T(()=>e.steamIDActor.m_ulSteamID.low,0),gameid:S(e.gameid),time:e.rtEventTime,type:e.eEventType,ach:(e.m_rgAchievements||[]).slice(0,8).map(x=>({name:S(x.strName),desc:S(x.strDescription),got:!!x.bAchieved}))})}}},0);
const next=T(()=>playNextStore.m_cachedPlayNext.appids.slice(0,16),[]);
return {now:Math.floor(Date.now()/1000),events,completed,store,friends,act,next}})()"#;


#[derive(Debug, Clone, Default)]
pub struct FeedEvent {
    pub gid: String,
    pub appid: u32,
    pub category: String,
    pub etype: i64,
    pub name: String,
    pub summary: String,
    pub post: i64,
    pub img: String,
}
#[derive(Debug, Clone, Default)]
pub struct FeedCompleted {
    pub appid: u32,
    pub time: i64,
    pub bytes: u64,
}
#[derive(Debug, Clone, Default)]
pub struct FeedStoreApp {
    pub appid: u32,
    pub name: String,
    pub price: String,
    pub orig: String,
    pub pct: i32,
    pub free: bool,
    pub soon: bool,
    pub owned: bool,
    /// Steam's store cache had the item (price/name valid); false = it is still loading.
    pub ready: bool,
}
#[derive(Debug, Clone, Default)]
pub struct FeedFriend {
    pub aid: u32,
    pub steamid: String,
    pub name: String,
    /// 0 offline, 1 online, 2 busy, 3 away, 4 snooze, 5 looking to trade, 6 looking to play
    pub state: i64,
    pub game: String,
    pub gameid: u32,
    pub seen: i64,
    pub avatar: String,
}
#[derive(Debug, Clone, Default)]
pub struct FeedAchievement {
    pub name: String,
    pub desc: String,
    pub got: bool,
}
#[derive(Debug, Clone, Default)]
pub struct FeedActivity {
    pub aid: u32,
    pub gameid: u32,
    pub time: i64,
    /// Steam EActivityEventType (2 = achievements, ...)
    pub etype: i64,
    pub ach: Vec<FeedAchievement>,
}
#[derive(Debug, Clone, Default)]
pub struct HomeFeed {
    pub now: i64,
    pub events: Vec<FeedEvent>,
    pub completed: Vec<FeedCompleted>,
    pub store: Vec<FeedStoreApp>,
    pub friends: Vec<FeedFriend>,
    pub activity: Vec<FeedActivity>,
    pub play_next: Vec<u32>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}
fn i(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64))).unwrap_or(0)
}
fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(false)
}
fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(|x| x.as_array()).map(|a| a.as_slice()).unwrap_or(&[])
}

pub fn parse_feed(v: &Value) -> Option<HomeFeed> {
    if !v.is_object() {
        return None;
    }
    let g = |gid: &Value| gid.as_str().and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
    Some(HomeFeed {
        now: i(v, "now"),
        events: arr(v, "events")
            .iter()
            .map(|e| FeedEvent { gid: s(e, "gid"), appid: i(e, "appid") as u32, category: s(e, "cat"), etype: i(e, "type"), name: s(e, "name"), summary: s(e, "sum"), post: i(e, "post"), img: s(e, "img") })
            .collect(),
        completed: arr(v, "completed").iter().map(|c| FeedCompleted { appid: i(c, "appid") as u32, time: i(c, "time"), bytes: i(c, "bytes") as u64 }).collect(),
        store: arr(v, "store")
            .iter()
            .map(|a| FeedStoreApp { appid: i(a, "appid") as u32, name: s(a, "name"), price: s(a, "price"), orig: s(a, "orig"), pct: i(a, "pct") as i32, free: b(a, "free"), soon: b(a, "soon"), owned: b(a, "owned"), ready: b(a, "ready") })
            .collect(),
        friends: arr(v, "friends")
            .iter()
            .map(|f| FeedFriend { aid: i(f, "aid") as u32, steamid: s(f, "steamid"), name: s(f, "name"), state: i(f, "state"), game: s(f, "game"), gameid: i(f, "gameid") as u32, seen: i(f, "seen"), avatar: s(f, "avatar") })
            .collect(),
        activity: arr(v, "act")
            .iter()
            .map(|a| FeedActivity {
                aid: i(a, "aid") as u32,
                gameid: g(a.get("gameid").unwrap_or(&Value::Null)),
                time: i(a, "time"),
                etype: i(a, "type"),
                ach: arr(a, "ach").iter().map(|x| FeedAchievement { name: s(x, "name"), desc: s(x, "desc"), got: b(x, "got") }).collect(),
            })
            .collect(),
        play_next: arr(v, "next").iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect(),
    })
}

