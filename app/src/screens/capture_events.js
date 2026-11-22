// Steam handlers: sp.js 435553–561, 435765–777 and 453736–744.
// One singleton per SharedJSContext; each registration is tracked independently.
(()=>{
 const api=window.SteamClient?.GameSessions;
 if(!api || typeof api.RegisterForScreenshotNotification!=='function' || typeof api.RegisterForAchievementNotification!=='function') return null;
 let s=window.__deckShellCaptureEventsV3;
 if(!s) s=window.__deckShellCaptureEventsV3={queue:[],friends:null,seen:new Set(),seenOrder:[],shotRegistered:false,achievementRegistered:false};
 s.inGame=__IN_GAME__;
 if(!s.friendToasts) s.friendToasts=new Map();
 const add=(event,key)=>{
   const settings=window.settingsStore?.clientSettings;
   if(settings?.disable_all_toasts || (settings?.disable_toasts_in_game && s.inGame)) return;
   if(key && s.seen.has(key)) return;
   if(key){s.seen.add(key);s.seenOrder.push(key);if(s.seenOrder.length>128)s.seen.delete(s.seenOrder.shift());}
   if(s.queue.length<128){s.queue.push(event);s.wake?.();}
 };
 if(!s.shotRegistered){
   try {
     api.RegisterForScreenshotNotification(e=>{
       if(e?.strOperation!=='written' || window.settingsStore?.clientSettings?.enable_screenshot_notification===false)return;
       const handle=e.details?.hHandle??e.hScreenshot, appid=e.unAppID||Number(e.details?.strGameID)||0;
       const key=appid && handle!=null?`s:${appid}:${handle}`:null;
       add({kind:'screenshot',body:window.appStore?.GetAppOverviewByAppID?.(appid)?.display_name||'',image:e.details?.strUrl||''},key);
     });
     s.shotRegistered=true;
   } catch(_) {}
 }
 if(!s.achievementRegistered){
   try {
     api.RegisterForAchievementNotification(e=>{
       if(!e?.unAppID || !e?.achievement?.bAchieved || window.settingsStore?.clientSettings?.achievement_notification_toast===false)return;
       const a=e.achievement;
       const key=a.strID && a.rtUnlocked?`a:${e.unAppID}:${a.strID}:${a.rtUnlocked}`:null;
       add({kind:'achievement',title:a.strName||'',body:a.strDescription||'',image:a.strImage||''},key);
     });
     s.achievementRegistered=true;
   } catch(_) {}
 }
 if(__RESET__) {s.queue=[];s.friends=null;}
 const store=window.friendStore?.m_FriendsUIFriendStore;
 const fs=store?.friends_list_ready===true && store?.m_bInitialPersonaStatesLoaded===true ? store.all_friends : null;
 if(!Array.isArray(fs)) s.friends=null;
 else {
   const now=new Map();
   for(const f of fs){
     try {
       const p=f?.persona||f?.m_persona, id=f?.accountid||f?.m_unAccountID;
       if(!p || !id || p.m_bStatusInitialized!==true || !Number.isFinite(p.m_unGamePlayedAppID)) continue;
       const game=p.m_unGamePlayedAppID;
       now.set(id,game);
       if(s.friends?.has(id) && s.friends.get(id)!==game && game>0){
         const name=f.display_name||p.m_strPlayerName, title=typeof p.GetCurrentGameName==='function'?p.GetCurrentGameName():'';
         const manager=window.g_FriendsUIApp?.NotificationManager;
         const allowed=(!manager?.BNotificationsPermitted || manager.BNotificationsPermitted()) && (!store.GetUserDoNotDisturb || !store.GetUserDoNotDisturb());
         const key=`${id}_${title}`, last=s.friendToasts.get(key), at=Date.now()/1000;
         if(name && title && allowed && window.settingsStore?.friendSettings?.bNotifications_ShowIngame!==false && (last===undefined || at>last+10)) {
           s.friendToasts.set(key,at);
           for(const [k,t] of s.friendToasts) if(at>t+10)s.friendToasts.delete(k);
           add({kind:'friend',title:name,game:title,image:p.avatar_url_medium||''},null);
         }
       }
     } catch(_) { /* Missing persona data is not an idle observation. */ }
   }
   s.friends=now;
 }
 if(!__WAIT__ || s.queue.length) return s.queue.splice(0,32);
 // Отдельный CDP канал ждёт событие: callback разрешает Promise без поллинга.
 // Таймаут нужен только для persona/reconnect и не задерживает скриншоты.
 return new Promise(resolve=>{
   let timer;
   const wake=()=>{clearTimeout(timer);if(s.wake===wake)s.wake=null;resolve(s.queue.splice(0,32));};
   s.wake=wake;
   timer=setTimeout(wake,1000);
 });
})()
