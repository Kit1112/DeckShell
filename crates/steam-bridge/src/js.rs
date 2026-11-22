//! JavaScript expressions evaluated inside SharedJSContext.
//! Reads trim the payload in JS (arrays of scalars) so transfers stay small.
//! Sources refer to the beautified bundle tools/cdp/build/pretty/sp.js (line numbers of that file).

/// All apps as arrays:
/// [appid, display_name, sort_as, installed, size_on_disk(string), minutes_forever, minutes_2weeks,
///  rt_last_time_played, app_type, display_status, status_percentage, is_shortcut, gameid(string),
///  visible_in_game_list, steam_hw_compat_category_packed, review_percentage, rt_steam_release_date, rt_recent_activity_time]
///
/// Union of `allAppsCollection` and the stock `all-games` collection (the All Games tab). The stock list holds the
/// family-shared (not owned, not installed) games and some installed games (e.g. Balatro 2379780) that
/// `allAppsCollection` omits; the stock collection has no tools, servers or SDKs, so only games are added. It is
/// read only when present, so a failing read leaves the list as before.
pub const APPS: &str = r#"(()=>{const m=new Map();let g=[];try{g=collectionStore.GetCollection('all-games').allApps}catch(e){}for(const o of [...collectionStore.allAppsCollection.allApps,...g]){if(!m.has(o.appid))m.set(o.appid,o)}return [...m.values()].map(o=>{const l=o.local_per_client_data||{};return [o.appid,o.display_name,o.sort_as,o.installed?1:0,String(o.size_on_disk||0),o.minutes_playtime_forever||0,o.minutes_playtime_last_two_weeks||0,o.rt_last_time_played||0,o.app_type,l.display_status||0,l.status_percentage||0,o.BIsShortcut()?1:0,String(o.GetGameID()),o.visible_in_game_list===false?0:1,o.steam_hw_compat_category_packed||0,o.review_percentage_with_bombs||0,o.rt_steam_release_date||0,o.rt_recent_activity_time||0]})})()"#;

#[cfg(test)]
mod apps_js_tests {
    use super::APPS;

    #[test]
    fn apps_include_stock_all_games_collection_guarded() {
        // A failing stock read must not empty the whole list, and the stock list is merged, not substituted.
        assert!(APPS.contains("GetCollection('all-games')"));
        assert!(APPS.contains("collectionStore.allAppsCollection.allApps"));
        assert!(APPS.contains("try{"));
    }
}

/// Collections (collectionStore.userCollections incl. favorites, dynamic `srm-*`/type collections):
/// [id, displayName, [appids]]
pub const COLLECTIONS: &str = r#"collectionStore.userCollections.map(c=>[c.id,c.displayName,c.allApps.map(a=>a.appid)])"#;

/// Recently played order (home "recent games"): appids.
pub const RECENT: &str = r#"collectionStore.allRecentAppsCollection.allApps.map(a=>a.appid)"#;

/// Downloads: {overview:{state,appid,net,peak,disc,upload,paused,throttling_suspended,lan_peer_hostname,percent,eta,dl_bytes,dl_total,is_shader,is_workshop}, history:[[rt_sample_time,net,disc,upload]],
/// items:[{appid,active,paused,completed,queue_index,percent,eta,bytes,total,launch_on_completion,name,deferred_time,completed_time,
/// update_result,update_error,auto_update,disabled_by_app,dl_bytes,dl_total,types:[[type,completed]]}]}. dl_* = progress[k_EAppUpdateProgress_Download=2] (stock H()/A()).
/// Stores: downloadsStore.LocalDownloadOverview, downloadsStore.AllTransfers (items from SteamClient.Downloads.RegisterForDownloadItems).
pub const DOWNLOADS: &str = r#"(()=>{const o=downloadsStore.LocalDownloadOverview||{};const nn=v=>v===undefined?null:v;const dp=o.progress&&o.progress[2];return {overview:{state:o.update_state,appid:o.update_appid,net:o.update_network_bytes_per_second||0,disc:o.update_disc_bytes_per_second||0,peak:o.update_peak_network_bytes_per_second||0,upload:!!o.update_is_upload,paused:nn(o.paused),throttling_suspended:nn(o.throttling_suspended),lan_peer_hostname:nn(o.lan_peer_hostname),percent:nn(o.overall_percent_complete),eta:nn(o.overall_estimated_time_remaining_sec),dl_bytes:dp?dp.bytes_in_progress:null,dl_total:dp?dp.bytes_total:null,is_shader:!!o.update_is_shader,is_workshop:!!o.update_is_workshop},history:(downloadsStore.DownloadHistory||[]).slice(-150).map(h=>[h.rt_sample_time||0,h.network_bytes_per_second||0,h.disc_bytes_per_second||0,+!!h.is_upload]),items:downloadsStore.AllTransfers.map(t=>{let pct=0,eta=-1,b=0,tot=0,db=null,dt=null;for(const u of t.update_type_info||[]){const d=u.progress&&u.progress[2];if(d){db=(db||0)+(d.bytes_in_progress||0);dt=(dt||0)+(d.bytes_total||0)}if(!u.has_update)continue;pct=Math.max(pct,u.overall_percent_complete||0);if(eta<0)eta=u.overall_estimated_time_remaining_sec}b=db||0;tot=dt||0;let ov=null,det=null;try{ov=appStore.GetAppOverviewByAppID(t.appid)}catch(e){}try{det=appDetailsStore.GetAppDetails(t.appid)}catch(e){}let pc=null;try{pc=ov&&ov.GetPerClientData("local")}catch(e){}return {appid:t.appid,active:!!t.active,paused:!!t.paused,completed:!!t.completed,queue_index:t.queue_index,percent:pct,eta:eta,bytes:b,total:tot,launch_on_completion:!!t.launch_on_completion,name:ov&&ov.display_name||null,deferred_time:t.deferred_time||0,completed_time:t.completed_time||0,update_result:nn(t.update_result),update_error:t.update_error||null,auto_update:det?nn(det.eAutoUpdateValue):null,disabled_by_app:pc?!!pc.update_available_but_disabled_by_app:null,dl_bytes:db,dl_total:dt,types:(t.update_type_info||[]).map((u,k)=>u&&u.has_update?[k,+!!u.completed]:null).filter(x=>x)}})}})()"#;

/// Notifications: NotificationStore (unviewed counter + tray). Each tray group holds notifications[0].
pub const NOTIFICATIONS: &str = r#"(()=>({unviewed:NotificationStore.m_nUnviewedNotifications||0,items:NotificationStore.m_rgNotificationTray.map(g=>{const n=g.notifications[0]||{};let d='';try{d=JSON.stringify(n.data&&n.data.item||n.data||{}).slice(0,600)}catch(e){}return {id:n.notificationID,type:n.eType,source:n.eSource,is_new:!!n.bNewIndicator,read:!!n.rtMarkedRead,count:g.notifications.length,data:d}})}))()"#;

/// Network + battery + user. loginStore.currentUser = {personaName, accountName, avatarUrl:"/avatarcache/<steamid>.png"}.
pub const SYSTEM: &str = r#"(()=>{const N=window.SystemNetworkStore,P=window.SystemPowerStore;if(!N||!P)return null;const u=loginStore.currentUser||{};const ap=N.presentAccessPoints.find(a=>a.isConnected&&a.type!==undefined&&!a.isLoopback)||N.presentAccessPoints.find(a=>a.isConnected);return {network:{connected:!!N.hasNetworkConnection,internet:!!N.hasInternetConnection,steam:!!N.hasSteamConnection,wifi_enabled:!!N.isWifiEnabled,wifi_strength:N.presentAccessPoints.find(a=>a.isConnected&&a.wirelessAPInfo)?.wirelessAPInfo?.estrength??null,ssid:ap?ap.displayName:null,offline:!!(window.friendStore&&friendStore.BIsOfflineMode&&friendStore.BIsOfflineMode())},battery:{present:!!P.m_bHasBattery,level:P.m_flBatteryLevel,ac_state:P.m_eACState,state:P.m_eBatteryState,seconds_remaining:P.m_nBatterySecondsRemaining},user:{persona_name:u.personaName||null,account_name:u.accountName||null,avatar_url:u.avatarUrl||null,steamid:String(friendStore.currentUserSteamID.ConvertTo64BitString())}}})()"#;

/// Running app ids: SteamUIStore.ActiveWindowInstance.RunningApps (as used by ScopeRunningApps, sp.js:463775).
pub const RUNNING: &str = r#"[...((SteamUIStore.ActiveWindowInstance&&SteamUIStore.ActiveWindowInstance.RunningApps)||[])].map(a=>a.appid)"#;

/// Output devices: [{id,name,volume(0..1),default}]  (SteamClient.System.Audio.GetDevices)
pub const AUDIO: &str = r#"SteamClient.System.Audio.GetDevices().then(r=>r.vecDevices.filter(d=>d.bHasOutput).map(d=>({id:d.id,name:d.sName,volume:d.flOutputVolume,default:!!d.bIsDefaultOutputDevice})))"#;

/// App details via the same callback API the native UI uses (RegisterForAppDetails); unregisters afterwards.
/// `{APPID}` is substituted. Resolves null after 1.2 s.
pub const APP_DETAILS: &str = r#"new Promise(res=>{let h;const t=setTimeout(()=>{h&&h.unregister();res(null)},1200);h=SteamClient.Apps.RegisterForAppDetails({APPID},d=>{clearTimeout(t);setTimeout(()=>h&&h.unregister(),0);res({appid:d.unAppID,display_status:d.eDisplayStatus,last_played:d.rtLastTimePlayed,launch_options:d.strLaunchOptions,build_id:d.nBuildID,disk_usage:d.lDiskUsageBytes,dlc_usage:d.lDlcUsageBytes,install_folder:d.strInstallFolder,selected_beta:d.strSelectedBeta,compat_tool:d.strCompatToolName,compat_tool_name:d.strCompatToolDisplayName,cloud_enabled:!!d.bCloudEnabledForApp,cloud_available:!!d.bCloudAvailable,developer:d.strDeveloperName,homepage:d.strHomepageURL,playtime_forever:d.nPlaytimeForever,last_updated:d.rtLastUpdated,purchased:d.rtPurchased,store_header:d.strStoreHeaderImage,auto_update:d.eAutoUpdateValue,overlay_enabled:!!d.bOverlayEnabled})})})"#;

/// Cheap change fingerprint (one string per domain), polled every ~500 ms.
pub const FINGERPRINT: &str = r#"(()=>{const a=collectionStore.allAppsCollection.allApps;let inst=0,pl=0,lp=0,ds=0;for(let i=0;i<a.length;i++){const o=a[i],l=o.local_per_client_data||{};if(o.installed)inst++;pl+=o.minutes_playtime_forever||0;lp+=o.rt_last_time_played||0;ds+=(l.display_status||0)*(i+1)}const cs=collectionStore.userCollections.map(c=>c.id+'='+c.allApps.length).join(',');const T=downloadsStore.AllTransfers,O=downloadsStore.LocalDownloadOverview||{};const dl=T.map(t=>[t.appid,+!!t.active,+!!t.paused,+!!t.completed,t.queue_index,Math.round((t.update_type_info&&t.update_type_info[0]&&t.update_type_info[0].overall_percent_complete)||0)].join(':')).join(',')+'|'+O.update_state+'|'+O.update_appid+'|'+Math.round((O.update_network_bytes_per_second||0)/1024)+'|'+(+!!O.paused)+(+!!O.throttling_suspended)+'|'+T.map(t=>(t.deferred_time||0)+':'+(t.update_result||0)+':'+(+!!t.launch_on_completion)).join(',');const n=NotificationStore.m_nUnviewedNotifications+'|'+NotificationStore.m_rgNotificationTray.map(g=>g.notifications[0]&&g.notifications[0].notificationID).join(',');const w=SteamUIStore.ActiveWindowInstance;const run=[...((w&&w.RunningApps)||[])].map(x=>x.appid).join(',');return {account:window.loginStore?.accountName||'',lib:[a.length,inst,pl,lp,ds,cs].join('/'),dl:dl,n:n,run:run}})()"#;

// ---------------------------------------------------------------------------------------------
// Actions (state-changing). `{GID}` = gameid string, `{APPID}` = number, `{VOL}` = 0..1, `{DEV}` = device id.
// ---------------------------------------------------------------------------------------------

/// Play button / A on a game. Exactly the native handler (sp.js:498512 `wt`): guard on BHasRecentlyLaunched,
/// MarkAppAsRecentlyLaunched, SteamClient.Apps.RunGame(gameid,"",-1,500) (500 = launch source of the library
/// play button, chunk~2dcc5aaf7.js:364920 / sp.js:484525), then uiStore.SetRunningApp.
pub const ACTION_LAUNCH: &str = r#"(()=>{const o=appStore.GetAppOverviewByAppID({APPID});if(!o)return false;if(appDetailsStore.BHasRecentlyLaunched(o.appid))return false;appDetailsStore.MarkAppAsRecentlyLaunched(o.appid);SteamClient.Apps.RunGame(o.GetGameID(),"",-1,500);SteamUIStore.SetRunningApp(o.appid);return true})()"#;

/// Stop a running game. sp.js:393262 / 420753: SteamClient.Apps.TerminateApp(gameid, false).
pub const ACTION_STOP: &str = r#"(()=>{const o=appStore.GetAppOverviewByAppID({APPID});if(!o)return false;SteamClient.Apps.TerminateApp(String(o.GetGameID()),false);return true})()"#;

/// Cancel a launch in progress. sp.js:498633: SteamClient.Apps.CancelLaunch(gameid).
pub const ACTION_CANCEL_LAUNCH: &str = r#"(()=>{const o=appStore.GetAppOverviewByAppID({APPID});if(!o)return false;SteamClient.Apps.CancelLaunch(o.GetGameID());return true})()"#;

/// Stock wizard fallback. Passive idle guard avoids knowingly replacing a foreign wizard;
/// this does not claim exclusive ownership. Steam owns callbacks, config and EULAs.
pub const ACTION_INSTALL_STOCK: &str = include_str!("install_stock.js");

/// Install button: sp.js:442171 InstallApp -> SteamClient.Installs.OpenInstallWizard([appid], clientid "0")
/// (opens Steam's install config; follow with `install_continue`).
pub const ACTION_INSTALL: &str = r#"(SteamClient.Installs.OpenInstallWizard([{APPID}],"0"),true)"#;
/// sp.js:442166 StartRequestedInstall.
pub const ACTION_INSTALL_CONTINUE: &str = r#"SteamClient.Installs.ContinueInstall().then(()=>true)"#;
/// sp.js:442169 CancelRequestedInstall.
pub const ACTION_INSTALL_CANCEL: &str = r#"SteamClient.Installs.CancelInstall().then(()=>true)"#;
/// Confirmed local uninstall, sp.js ShowUninstallDialog OK (task375).
/// Void means dispatched, never completed. Keep guards and call in one JS turn.
pub const ACTION_UNINSTALL: &str = r#"(async()=>{const id={APPID};const o=appStore.GetAppOverviewByAppID(id);const local=o&&o.local_per_client_data;if(!local||local.clientid!=="0"||(local.installed!==true&&local.installed!==1))throw Error('Игра не установлена локально или состояние недоступно');if(o.app_type===1073741824)throw Error('Сторонняя игра не является локальной установкой Steam');if(typeof o.BIsAppBlocked!=='function'||o.BIsAppBlocked())throw Error('Доступ к игре заблокирован или не удалось проверить ограничения');const w=SteamUIStore.ActiveWindowInstance;if(!w||!w.RunningApps)throw Error('Список запущенных игр недоступен');if([...w.RunningApps].some(a=>a.appid===id))throw Error('Закройте игру перед удалением');const r=await SteamClient.Installs.OpenUninstallWizard([id],"0",true);if(r!==undefined&&r!==true)throw Error('Steam отклонил удаление: '+JSON.stringify(r));return true})()"#;
/// Read local data, never most_available (which may describe a remote PC).
/// Steam omits the protobuf false field after uninstall; require the local
/// ReadyToInstall status (9) as positive evidence, not merely a missing bool.
pub const UNINSTALL_STATE: &str = r#"(()=>{const o=appStore.GetAppOverviewByAppID({APPID});const p=o&&o.local_per_client_data;if(!p||p.clientid!=="0")return null;if(p.installed===true||p.installed===1)return true;if(p.installed===false||p.installed===0)return false;return p.installed===undefined&&p.display_status===9?false:null})()"#;

/// Pause all downloads: sp.js:498641 SteamClient.Downloads.EnableAllDownloads(false, clientid).
pub const ACTION_PAUSE_ALL: &str = r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#;
/// Resume all downloads (inverse of the above, same API with true).
pub const ACTION_RESUME_ALL: &str = r#"(SteamClient.Downloads.EnableAllDownloads(true,"0"),true)"#;
/// Pause one item: sp.js:529476 SteamClient.Downloads.PauseAppUpdate(appid, clientid).
pub const ACTION_PAUSE_APP: &str = r#"(SteamClient.Downloads.PauseAppUpdate({APPID},"0"),true)"#;
/// Resume one item: sp.js:498645 SteamClient.Downloads.ResumeAppUpdate(appid, clientid).
pub const ACTION_RESUME_APP: &str = r#"(SteamClient.Downloads.ResumeAppUpdate({APPID},"0"),true)"#;
/// Queue an app update: SteamClient.Downloads.QueueAppUpdate(appid, clientid) (same family as the above).
pub const ACTION_QUEUE_APP: &str = r#"(SteamClient.Downloads.QueueAppUpdate({APPID},"0"),true)"#;
/// Local client id of the Downloads API: module 12619 `Oy = "0"` (w.hj.CurrentViewingRemoteClientID when viewing this device).
pub const LOCAL_CLIENT: &str = "\"0\"";
/// Enable (resume) or disable (pause) all downloads: stock pause/resume button of the active item,
/// `SteamClient.Downloads.EnableAllDownloads(bEnable, clientid)`; the state is `overview.paused`.
pub fn enable_all_downloads(on: bool) -> String {
    format!("(SteamClient.Downloads.EnableAllDownloads({on},{LOCAL_CLIENT}),true)")
}
/// Stock "download now / move to top / retry" button of a queued, scheduled or unqueued item.
pub fn resume_app_update(appid: u32) -> String {
    format!("(SteamClient.Downloads.ResumeAppUpdate({appid},{LOCAL_CLIENT}),true)")
}
/// Stock drag of a queued or scheduled item into the "not queued" section.
pub fn pause_app_update(appid: u32) -> String {
    format!("(SteamClient.Downloads.PauseAppUpdate({appid},{LOCAL_CLIENT}),true)")
}
/// Stock drag inside the queue: `SetQueueIndex(appid, index, clientid)`, index = position in the queue (0 = active).
pub fn set_queue_index(appid: u32, index: u32) -> String {
    format!("(SteamClient.Downloads.SetQueueIndex({appid},{index},{LOCAL_CLIENT}),true)")
}
/// Stock remove button (function Be): remove, and re-enable downloads when the removed item was the paused head of the queue.
pub fn remove_from_download_list(appid: u32, reenable: bool) -> String {
    if reenable {
        format!("(SteamClient.Downloads.RemoveFromDownloadList({appid},{LOCAL_CLIENT}),SteamClient.Downloads.EnableAllDownloads(true,{LOCAL_CLIENT}),true)")
    } else {
        format!("(SteamClient.Downloads.RemoveFromDownloadList({appid},{LOCAL_CLIENT}),true)")
    }
}
/// Stock context menu "launch when complete": the appid sets it, 0 (module 23862 `sc`) clears it. One argument, no client id.
pub fn set_launch_on_update_complete(appid: u32) -> String {
    format!("(SteamClient.Downloads.SetLaunchOnUpdateComplete({appid}),true)")
}
/// Stock context menu "override download throttle": `SuspendDownloadThrottling(bSuspend, clientid)`.
pub fn suspend_download_throttling(on: bool) -> String {
    format!("(SteamClient.Downloads.SuspendDownloadThrottling({on},{LOCAL_CLIENT}),true)")
}
/// Download settings as the stock settings page reads them: `settingsStore.clientSettings[key]` (m_ClientSettings) and
/// `settingsStore.settings.vecValid*` (m_Settings). A key Steam does not report stays null.
pub const DOWNLOAD_SETTINGS: &str = r#"(()=>{const S=settingsStore,c=S.m_ClientSettings||{},s=S.m_Settings||{};const g=k=>c[k]===undefined?null:c[k];return {throttle_rate:g('download_throttle_rate'),rate_bits:g('download_rate_bits_per_s'),restrict:g('restrict_auto_updates'),restrict_start:g('restrict_auto_updates_start'),restrict_end:g('restrict_auto_updates_end'),while_running:g('download_while_app_running'),throttle_streaming:g('download_throttle_while_streaming'),update_behavior:g('default_app_update_behavior'),peer_content:g('download_peer_content'),region:g('download_region'),hours:(s.vecValidAutoUpdateRestrictHours||[]).map(h=>[h.nHour,h.strDisplay]),regions:(s.vecValidDownloadRegions||[]).map(r=>[r.nRegionID,r.strRegionName])}})()"#;
/// Remove from download list: SteamClient.Downloads.RemoveFromDownloadList(appid, clientid).
pub const ACTION_REMOVE_DOWNLOAD: &str = r#"(SteamClient.Downloads.RemoveFromDownloadList({APPID},"0"),true)"#;

/// Store page: Steam URL protocol through SteamClient.URL.ExecuteSteamURL (used e.g. at sp.js:46307 for settings URLs).
pub const ACTION_OPEN_STORE: &str = r#"(SteamClient.URL.ExecuteSteamURL("steam://store/{APPID}"),true)"#;

/// Volume: chunk of sp.js:423414 SteamClient.System.Audio.SetDeviceVolume(deviceId, AllOutput=1, volume 0..1).
pub const ACTION_SET_VOLUME: &str = r#"SteamClient.System.Audio.SetDeviceVolume({DEV},1,{VOL}).then(()=>true)"#;

/// Power: sp.js:404771 ShutdownPC, 420720 SuspendPC; RestartPC from the same SteamClient.System API.
pub const ACTION_SHUTDOWN: &str = r#"(SteamClient.System.ShutdownPC(),true)"#;
pub const ACTION_RESTART: &str = r#"(SteamClient.System.RestartPC(),true)"#;
pub const ACTION_SUSPEND: &str = r#"(SteamClient.System.SuspendPC(),true)"#;

/// Mark notifications as seen (NotificationStore.Viewed, sp.js:451212).
pub const ACTION_NOTIFICATIONS_VIEWED: &str = r#"(NotificationStore.Viewed(),true)"#;

/// Native call used by SystemNetworkStore.SetWifiEnabled(bool). Await it so
/// a rejected Steam request reaches the caller.
pub const ACTION_SET_WIFI: &str = r#"(async () => (await SteamClient.System.Network.SetWifiEnabled({ON})) !== false)()"#;

// ---------------------------------------------------------------------------------------------
// Artwork (context/archive/docs/fidelity-strategy.md "Обложки"). Steam's UI builds an ordered URL list per asset and the <img>
// walks it on error (SteamImage* component, sp.js:341040 GetSourcesForAsset + stores 434762/436674).
// ---------------------------------------------------------------------------------------------

/// Active account id (32-bit) exactly as the UI knows it: friendStore.currentUserSteamID. Custom art lives in
/// userdata/<this>/config/grid and is served by Steam as /customimages/.
pub const ACTIVE_ACCOUNT: &str = r#"friendStore.currentUserSteamID.GetAccountID()"#;

// Icon asset (VYj) used by Steam's header. Keep both cache layouts before the CDN.
pub const APP_ICON_URLS: &str = r#"(o)=>{if(!o)return [];const id=o.appid,lc=o.local_cache_version??0;const ic=[];if(o.icon_hash){ic.push(`/assets/${id}/${o.icon_hash}.jpg?c=${lc}`)}ic.push(`/assets/${id}_icon.jpg?c=${lc}`);const u=appStore.GetIconURLForApp(o);if(u)ic.push(u.startsWith('data:')?'data:':u);return ic}"#;

/// Ordered candidate URLs per asset for `{IDS}` (JSON array of appids): [appid, capsule[], header[], hero[], logo[], icon[]].
/// Uses Steam's own functions: appStore.GetCustom*URLs / GetCachedVerticalCapsuleURL / GetVerticalCapsuleURLForApp /
/// GetPregeneratedVerticalCapsuleForApp (sp.js:436674-436730) and appDetailsStore.GetHeaderImages / GetHeroImages /
/// GetLogoImages (sp.js:435041-435140). The latter need AppDetails, so we register exactly like the UI does
/// (appDetailsStore.RegisterForAppData), wait <=1.2 s, read, unregister. Capsule order = sp.js:341060 (non-mod branch;
/// mods/shortcuts: custom only). Icon order = sp.js:341076 (cached hash jpg, legacy, then community URL; data: URIs
/// are reduced to "data:" to keep the payload small). null = app unknown.
pub const ART_URLS: &str = r#"(async()=>{const ids={IDS},D=appDetailsStore,S=appStore;const hs=ids.map(i=>D.RegisterForAppData(i,()=>{}));for(let k=0;k<12&&!ids.every(i=>D.GetAppDetails(i));k++)await new Promise(r=>setTimeout(r,100));const out=ids.map(id=>{const o=S.GetAppOverviewByAppID(id);if(!o)return null;const mod=o.BIsModOrShortcut();const cap=mod?[...S.GetCustomVerticalCapsuleURLs(o)]:[...S.GetCustomVerticalCapsuleURLs(o),...S.GetCachedVerticalCapsuleURL(o),S.GetVerticalCapsuleURLForApp(o),S.GetPregeneratedVerticalCapsuleForApp(o)];return [id,cap,D.GetHeaderImages(o,false),D.GetHeroImages(o).rgHeroImages,D.GetLogoImages(o).rgLogoImages,({ICON_URLS})(o)]});hs.forEach(h=>h.unregister());return out})()"#;

/// Audit helper: for `{ID}` walk each list like an <img> (first URL answering 200), return per asset
/// [url, bytes, fnv1a32] or null (nothing local answered). Lists come from ART_URLS.
pub const ART_FETCH: &str = r#"(async()=>{const lists={LISTS};const fnv=b=>{let h=0x811c9dc5;for(let i=0;i<b.length;i++){h^=b[i];h=Math.imul(h,16777619)}return h>>>0};const res=[];for(const l of lists){let r=null;for(const u of l){if(!u.startsWith('/'))break;try{const f=await fetch('https://steamloopback.host'+u);if(f.ok){const b=new Uint8Array(await f.arrayBuffer());r=[u,b.length,fnv(b)];break}}catch(e){}}res.push(r)}return res})()"#;

// ---------------------------------------------------------------------------------------------
// QAM Performance / Quick settings / Friends / Notifications writers (docs/engine/qam.json bridge_js; sp.js refs in comments).
// `@W@` = webpack require helper (WEBPACK_W), substituted by `Bridge::perf_call` etc.
// ---------------------------------------------------------------------------------------------

/// webpack module require helper (docs/engine/qam.json helper_W).
pub const WEBPACK_W: &str = "const W=id=>{let r;window.webpackChunksteamui.push([[Symbol()],{},x=>{r=x}]);return r(id)};";

/// Perf store call: SystemPerfStore (module 83761 `Hn`), sp.js:425520-425650. Each setter builds a CMsgSystemPerfUpdateSettingsRequest for
/// `nActiveProfileGameID` and sends it with SteamClient.System.Perf.UpdateSettings. Returns false when the Perf channel does not
/// exist (only the gamescope session has it). `{METHOD}` = SetFPSLimit | SetDisplayRefreshRateManualHz | SetPerfOverlayEnabled |
/// SetPerfOverlayLevel | SetSplitScalingFilter | SetSplitScalingScaler | SetFSRSharpness | SetVRREnabled | SetGameSpecificProfileEnabled |
/// SaveGameProfile | ResetCurrentPerfProfileSettings | SetAdvancedSettingsEnabled | SetLowLatencyModeEnabled ...; `{ARGS}` = JS argument list.
pub const PERF_CALL: &str = r#"(()=>{@W@if(!SteamClient.System.Perf)return false;const S=W(83761).Hn.Get();S.@METHOD@(@ARGS@);return true})()"#;

/// SteamOSManager state fields the power page needs, read the way the stock hooks do (sp.js module 47269 `z`/`S`/`L`: `GetState` of module 39970,
/// `staleTime: Infinity`). Read-only RPC. -> {charge_limit_available, charge_limit_min, charge_limit_max, charge_limit_default,
/// cec_wake_supported, cec_available}; null when the service is not reachable (the page then keeps its fallbacks).
pub const POWER_LIMITS: &str = r#"(async()=>{@W@try{const r=await W(39970).Bd.GetState({});if(!r.BSuccess())return null;const x=r.Body().toObject().state;if(!x)return null;
const n=v=>typeof v==='number'?v:null,b=v=>typeof v==='boolean'?v:null;
return {charge_limit_available:b(x.is_charge_limit_available),charge_limit_min:n(x.charge_limit_min),charge_limit_max:n(x.charge_limit_max),charge_limit_default:n(x.charge_limit_default),
cec_wake_supported:b(x.is_extended_cec_wake_device_supported),cec_available:b(x.is_extended_cec_available)}}catch(e){return null}})()"#;

/// Whole Performance/Quick-settings state in one round trip (docs/engine/qam.json read_quick + read_limits + read_perf).
/// -> {tdp:{on,w}, gpu:{on,mhz}, charge, vrs, tearing, profile, limits:{tdp:[min,max,avail], gpu:[min,max,avail], vrs}, perf:null|{gameid, profile_gameid, limits, global, per_app},
///     scale:{auto, factor, min, max}, airplane, night}
// Optional stores may be absent in SharedJSContext. Get() is NOT a read-only
// fallback: sp.js:425467/427317 lazily constructs stores and registers callbacks.
// Read existing globals only; missing fields remain null rather than fake off.
pub const PERF_READ: &str = r#"(async()=>{
const read=f=>{try{return f()??null}catch(e){return null}};
const bool=x=>typeof x==='boolean'?x:null;
const c=read(()=>settingsStore.clientSettings),d=read(()=>settingsStore.settings);
// The previous limits RPC required webpack fake-chunk injection. There is no
// proven already-exposed read path here, so it stays unknown without that hook.
const lim=null;
const perf=read(()=>{const S=window.SystemPerfStore;if(!S||!SteamClient.System.Perf)return null;
return JSON.parse(JSON.stringify({gameid:S.nCurrentGameID==null?null:String(S.nCurrentGameID),profile_gameid:S.nActiveProfileGameID==null?null:String(S.nActiveProfileGameID),limits:S.msgLimits,global:S.msgSettingsGlobal,per_app:S.msgSettingsPerApp}))});
return {tdp:{on:bool(read(()=>c.steamos_tdp_limit_enabled)),w:read(()=>c.steamos_tdp_limit)},
gpu:{on:bool(read(()=>c.steamos_manual_gpu_clock_enabled)),mhz:read(()=>c.steamos_manual_gpu_clock_hz)},
vrs:bool(read(()=>c.steamos_vrs_enabled)),tearing:bool(read(()=>c.gamescope_allow_tearing)),profile:read(()=>c.steamos_platform_performance_profile),limits:lim,perf:perf,
scale:{auto:bool(read(()=>d.bDisplayIsUsingAutoScale)),factor:read(()=>d.flCurrentDisplayScaleFactor),min:read(()=>d.flMinDisplayScaleFactor),max:read(()=>d.flMaxDisplayScaleFactor)},
airplane:bool(read(()=>SystemStoragStore.AirplaneModeEnabled.Value))}
})()"#;

/// settingsStore client setting write (serialization CMsgClientSettings, module 29303 `Ne`; docs/engine/qam.json set_client_setting).
/// `{NAME}` = steamos_tdp_limit_enabled | steamos_tdp_limit | steamos_manual_gpu_clock_enabled | steamos_manual_gpu_clock_hz |
/// steamos_charge_limit_enabled | steamos_charge_limit | steamos_vrs_enabled | steamos_platform_performance_profile | gamescope_* | enable_ui_sounds.
pub const SET_CLIENT_SETTING: &str = r#"(()=>{@W@const P=W(29303),N=W(58663),w=new N.BinaryWriter();P.Ne.serializeBinaryToWriter(P.Ne.fromObject({[@NAME@]:@VALUE@}),w);SteamClient.Settings.SetSetting(w.getResultBase64String());return true})()"#;

/// GPU mode and frequency must travel in the same protobuf update.
pub const SET_MANUAL_GPU_CLOCK: &str = r#"(()=>{@W@const P=W(29303),N=W(58663),w=new N.BinaryWriter();P.Ne.serializeBinaryToWriter(P.Ne.fromObject({steamos_manual_gpu_clock_enabled:@ON@,steamos_manual_gpu_clock_hz:@MHZ@}),w);SteamClient.Settings.SetSetting(w.getResultBase64String());return true})()"#;

/// Airplane mode: SteamClient.System.SetAirplaneMode(bool) (docs/engine/qam.json airplane_wifi).
pub const SET_AIRPLANE: &str = r#"(SteamClient.System.SetAirplaneMode({ON}),true)"#;
/// Night Mode settings callback (System store 75191). Get() also subscribes in a headless Steam session.
pub const NIGHT_MODE_SETTINGS: &str = r#"(()=>{@W@
const s=(window.SystemStoragStore ?? W(75191).mG.Get()).msgSettings;
return {enabled:s.display_nightmode_enabled, strength:s.display_nightmode_tintstrength,
  hue:s.display_nightmode_maxhue, saturation:s.display_nightmode_maxsat};
})()"#;
/// Night mode: CMsgSystemSettings.display_nightmode_enabled via SteamClient.System.UpdateSettings (module 78544 `sH`).
pub const SET_NIGHT_MODE: &str = r#"(async()=>{@W@const m=W(78544),a=new m.sH();a.set_display_nightmode_enabled({ON});await SteamClient.System.UpdateSettings(a.serializeBase64String());return true})()"#;
/// "Автоматически масштабировать интерфейс": SteamClient.Window.SetGamepadUIAutoDisplayScale(bool) (sp.js:377520).
pub const SET_UI_AUTO_SCALE: &str = r#"(SteamClient.Window.SetGamepadUIAutoDisplayScale({ON}),true)"#;
/// МЕЛКИЙ/КРУПНЫЙ ТЕКСТ slider: SteamClient.Window.SetGamepadUIManualDisplayScaleFactor(float) (sp.js:377485).
pub const SET_UI_SCALE_FACTOR: &str = r#"(SteamClient.Window.SetGamepadUIManualDisplayScaleFactor({VAL}),true)"#;

/// Friend row A (sp.js:64201): friend.OpenChatDialog(browserContext) + UIStore.FocusChatTextEntryByAccountID, and, like the Deck menu entry
/// (handlers.json "Друзья и чат"), the QAM chat expanded flag. `{ACC}` = accountid.
pub const FRIEND_OPEN_CHAT: &str = r#"(()=>{const f=friendStore.GetFriend?friendStore.GetFriend({ACC}):friendStore.allFriends.find(x=>x.accountid=={ACC});if(!f)return false;const ctx=(window.SteamUIStore&&SteamUIStore.ActiveWindowInstance)||undefined;try{f.OpenChatDialog(ctx&&ctx.BrowserWindow?ctx:undefined)}catch(e){f.OpenChatDialog()}try{const U=window.UIStore||(window.g_FriendsUIApp&&g_FriendsUIApp.UIStore);U&&U.FocusChatTextEntryByAccountID&&U.FocusChatTextEntryByAccountID(ctx,{ACC});U&&U.SetQAMFriendsChatExpanded&&U.SetQAMFriendsChatExpanded(true)}catch(e){}return true})()"#;
/// Friend row START "Просмотреть профиль": steam://url/SteamIDPage/<steamid64> through SteamClient.URL.ExecuteSteamURL.
pub const FRIEND_PROFILE: &str = r#"(()=>{const f=friendStore.allFriends.find(x=>x.accountid=={ACC});if(!f)return false;SteamClient.URL.ExecuteSteamURL("steam://url/SteamIDPage/"+f.steamid.ConvertTo64BitString());return true})()"#;
/// Notification row: NotificationStore.RemoveGroupFromTray(group) (sp.js:451441) = dismiss. `{NID}` = notificationID of the group's first item.
pub const NOTIFICATION_DISMISS: &str = r#"(()=>{const g=NotificationStore.m_rgNotificationTray.find(g=>g.notifications[0].notificationID=={NID});if(!g)return false;NotificationStore.RemoveGroupFromTray(g);return true})()"#;
/// Account page ("Открыть аккаунт"): Navigator.Account() is an in-Steam web route; DeckShell has no web view, so the same page is handed to Steam.
pub const OPEN_ACCOUNT: &str = r#"(SteamClient.URL.ExecuteSteamURL("steam://openurl/https://store.steampowered.com/account/"),true)"#;
/// Store search for the Search page МАГАЗИН tab: `{TERM}` = JSON string.
pub const OPEN_STORE_SEARCH: &str = r#"(SteamClient.URL.ExecuteSteamURL("steam://openurl/https://store.steampowered.com/search/?term="+encodeURIComponent({TERM})),true)"#;

#[cfg(test)]
mod volume_contract_test {
    #[test]
    fn output_volume_includes_steam_channel() {
        assert!(super::ACTION_SET_VOLUME.contains("SetDeviceVolume({DEV},1,{VOL})"));
    }
}

// QAM Friends tabs (app/src/screens/qam_friends.rs). Chat room group: UIStore.ShowAndOrActivateChatRoomGroup(defaultBrowserContext,
// group, false) (sp.js ChatStore/UIStore). `{GID}` = JSON string of the group id.
pub const OPEN_CHAT_GROUP: &str = r#"(()=>{const U=window.g_FriendsUIApp;if(!U)return false;const C=U.ChatStore||U.m_ChatStore;const g=C&&C.GetChatRoomGroup&&C.GetChatRoomGroup({GID});if(!g)return false;U.UIStore.ShowAndOrActivateChatRoomGroup(U.GetDefaultBrowserContext(),g,false);return true})()"#;
/// Friends header "Invites" and "Add friend" buttons: both post NavigateToInvites (sp.js FriendsListSteamDeckTopSection `Ce`).
pub const FRIENDS_NAVIGATE_TO_INVITES: &str = r#"(()=>{SteamClient.Messaging.PostMessage("FriendsUI","NavigateToInvites","{}");return true})()"#;
/// Groups header "New group": CreateChatDialog popup = module 64932 export UA(browserContext, host) (sp.js `Dt`).
pub const FRIENDS_CREATE_GROUP: &str = r#"(()=>{@W@const m=W(64932);if(!m||!m.UA)return false;const U=window.g_FriendsUIApp;const host=(window.SteamUIStore&&SteamUIStore.ActiveWindowInstance)||window;m.UA(U.GetDefaultBrowserContext(),host);return true})()"#;

/// Brightness write (task 270): Steam's display API, the same call the stock Quick Access slider makes.
/// `v` is the scale position 0..1; Steam maps it to the backlight and refuses positions above its top step.
pub fn brightness_set(v: f32) -> String {
    format!("(SteamClient.System.Display.SetBrightness({:.6}),true)", v.clamp(0.0, 1.0))
}
/// Brightness read (task 270): the scale position from Steam's display callback, registered once per
/// SharedJSContext. The callback carries the scale position (flBrightness), not a raw sysfs value.
pub const BRIGHTNESS_READ: &str = r#"(()=>{const s=window.__dsDisplay||(window.__dsDisplay={pos:null,reg:null});if(!s.reg&&SteamClient.System.Display.RegisterForBrightnessChanges){s.reg=SteamClient.System.Display.RegisterForBrightnessChanges(v=>{s.pos=v.flBrightness})}return s.pos})()"#;

#[cfg(test)]
mod brightness_js_tests {
    use super::brightness_set;

    #[test]
    fn write_clamps_the_scale_position_to_0_1() {
        assert_eq!(brightness_set(-0.5), "(SteamClient.System.Display.SetBrightness(0.000000),true)");
        assert_eq!(brightness_set(1.5), "(SteamClient.System.Display.SetBrightness(1.000000),true)");
        assert_eq!(brightness_set(0.5), "(SteamClient.System.Display.SetBrightness(0.500000),true)");
    }
}

#[cfg(test)]
mod downloads_stage_tests {
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Настоящий `DOWNLOADS` в node на заглушках сторов: у игры заполнены все 7 стадий progress[], а размер загрузки
    /// должен быть только стадией k_EAppUpdateProgress_Download = 2 (стоковые H()/A()), суммой по типам содержимого.
    /// Раньше bytes/total были суммой всех стадий и только по типам с has_update — размер и «осталось» после нативной установки врали.
    #[test]
    fn items_bytes_are_the_download_stage_only() {
        let stages = |dl: (u64, u64)| format!("[{{bytes_in_progress:50,bytes_total:100}},{{bytes_in_progress:70,bytes_total:900}},{{bytes_in_progress:{},bytes_total:{}}},{{bytes_in_progress:10,bytes_total:800}},{{bytes_in_progress:1,bytes_total:800}},{{bytes_in_progress:0,bytes_total:800}},{{bytes_in_progress:0,bytes_total:5}}]", dl.0, dl.1);
        let script = format!(
            r#"globalThis.downloadsStore={{LocalDownloadOverview:{{update_state:"Downloading",update_appid:7,update_network_bytes_per_second:10,update_peak_network_bytes_per_second:99,progress:{ov}}},
AllTransfers:[{{appid:7,active:true,queue_index:0,update_type_info:[{{has_update:true,overall_percent_complete:20,progress:{a}}},{{has_update:false,progress:{b}}}]}},
{{appid:8,queue_index:1,update_type_info:[]}}]}};
globalThis.appStore={{GetAppOverviewByAppID:id=>({{display_name:"G"+id,GetPerClientData:()=>null}})}};globalThis.appDetailsStore={{GetAppDetails:()=>null}};
process.stdout.write(JSON.stringify({code}))"#,
            ov = stages((300, 1000)),
            a = stages((200, 800)),
            b = stages((30, 100)),
            code = super::DOWNLOADS
        );
        let Ok(mut node) = Command::new("node").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
            eprintln!("node not installed: skipped");
            return;
        };
        node.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let out = node.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let d = crate::parse_downloads(&serde_json::from_slice(&out.stdout).unwrap()).unwrap();
        let i = &d.items[0];
        assert_eq!((i.dl_bytes, i.dl_total), (Some(230), Some(900)), "сумма progress[2] по двум типам содержимого");
        assert_eq!((i.bytes, i.total), (230, 900), "bytes/total — не сумма стадий 0..6");
        assert_eq!((d.overview.dl_bytes, d.overview.dl_total), (Some(300), Some(1000)));
        assert_eq!(d.overview.peak_net_bytes_per_sec, 99);
        assert_eq!((d.items[1].bytes, d.items[1].total, d.items[1].dl_total), (0, 0, None), "нет таблицы стадий — размер неизвестен");
    }
}

#[cfg(test)]
mod power_limits_contract_test {
    #[test]
    fn power_limits_reads_the_stock_state_fields_read_only() {
        let j = super::POWER_LIMITS;
        for f in ["W(39970).Bd.GetState", "charge_limit_min", "charge_limit_max", "charge_limit_default", "is_charge_limit_available", "is_extended_cec_wake_device_supported"] { assert!(j.contains(f), "{f}"); }
        assert!(!j.contains("Set") && !j.contains("Update"), "read-only");
    }
}
