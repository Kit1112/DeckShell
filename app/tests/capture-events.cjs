const fs = require('fs'), vm = require('vm'), assert = require('assert');
let shot, achievement, shotRegistrations = 0, achievementAttempts = 0;
const store = {friends_list_ready:true,m_bInitialPersonaStatesLoaded:true,all_friends:[]};
const window = {SteamClient:{GameSessions:{
  RegisterForScreenshotNotification(f){shotRegistrations++;shot=f},
  RegisterForAchievementNotification(f){achievementAttempts++;if(achievementAttempts===1)throw Error('unavailable');achievement=f}
}},friendStore:{m_FriendsUIFriendStore:store}};
const source = fs.readFileSync('app/src/screens/capture_events.js','utf8');
let now=100000;
const run = (reset,inGame=false) => vm.runInNewContext(source.replace('__WAIT__','false').replace('__RESET__',String(reset)).replace('__IN_GAME__',String(inGame)),{window,Map,Set,Number,Date:{now:()=>now}});
const friend = (game, ready=true) => ({accountid:1,display_name:'Друг',persona:{m_bStatusInitialized:ready,m_unGamePlayedAppID:game,m_strPlayerName:'Игрок',GetCurrentGameName:()=>game?'Игра':''}});
store.all_friends.push(friend(0)); assert.equal(run(true).length,0);
assert.equal(shotRegistrations,1); assert.equal(achievementAttempts,1);
store.all_friends[0]=friend(22); assert.equal(run(false)[0].body,'В игре: Игра'); assert.equal(run(false).length,0);
assert.equal(shotRegistrations,1); assert.equal(achievementAttempts,2);
shot({strOperation:'started'}); achievement({unAppID:1,achievement:{bAchieved:false}}); assert.equal(run(false).length,0);
shot({strOperation:'written',unAppID:1,hScreenshot:3}); shot({strOperation:'written',unAppID:1,hScreenshot:3});
achievement({unAppID:1,achievement:{bAchieved:true,strName:'Победа',strID:'WIN',rtUnlocked:5}});
achievement({unAppID:1,achievement:{bAchieved:true,strName:'Победа',strID:'WIN',rtUnlocked:5}});
assert.deepEqual(Array.from(run(false),e=>e.title),['Скриншот сделан','Победа']);
store.friends_list_ready=false; run(false); store.friends_list_ready=true; store.all_friends=[friend(33)]; assert.equal(run(false).length,0);
store.all_friends=[friend(0,false)]; run(false); store.all_friends=[friend(44)]; assert.equal(run(false).length,0);
window.settingsStore={clientSettings:{achievement_notification_toast:false,enable_screenshot_notification:false}};
shot({strOperation:'written',unAppID:1,hScreenshot:4}); achievement({unAppID:1,achievement:{bAchieved:true,strID:'X',rtUnlocked:6}}); assert.equal(run(false).length,0);
store.GetUserDoNotDisturb=()=>true; store.all_friends=[friend(0)]; run(false); store.all_friends=[friend(22)]; assert.equal(run(false).length,0);
now+=11000;store.GetUserDoNotDisturb=()=>false; store.all_friends=[friend(0)]; run(false); store.all_friends=[friend(22)]; assert.equal(run(false)[0].body,'В игре: Игра');
console.log('capture events: registration retry, deduplication and friend readiness OK');
// Preserve native visual fields instead of flattening them into one string.
window.settingsStore={clientSettings:{},friendSettings:{bNotifications_ShowIngame:false}};
store.all_friends=[friend(0)];run(false);store.all_friends=[friend(55)];assert.equal(run(false).length,0);
shot({strOperation:'written',details:{strGameID:'22',hHandle:99,strUrl:'screenshots/22/screenshots/test.jpg'}});
achievement({unAppID:22,achievement:{bAchieved:true,strName:'Заголовок',strDescription:'Описание',strImage:'https://steamcdn-a.akamaihd.net/icon.jpg'}});
let visual=Array.from(run(false));
assert.equal(visual[0].image,'screenshots/22/screenshots/test.jpg');
assert.equal(visual[1].title,'Заголовок');assert.equal(visual[1].body,'Описание');assert.equal(visual[1].kind,'achievement');
window.settingsStore.clientSettings.disable_all_toasts=true;
shot({strOperation:'written',unAppID:22,hScreenshot:100});assert.equal(run(false).length,0);
console.log('capture events: native images, split text and notification settings OK');

window.settingsStore.clientSettings={disable_toasts_in_game:true};
run(false,true);shot({strOperation:'written',unAppID:22,hScreenshot:101});assert.equal(run(false,true).length,0);
run(false,false);shot({strOperation:'written',unAppID:22,hScreenshot:102});assert.equal(run(false,false).length,1);

// Ожидание завершается самим callback, а не секундным таймером.
(async()=>{
 window.settingsStore.clientSettings={};
 const waiting=vm.runInNewContext(source.replace('__WAIT__','true').replace('__RESET__','true').replace('__IN_GAME__','false'),{window,Map,Set,Number,Date,setTimeout,clearTimeout,Promise});
 shot({strOperation:'written',unAppID:22,hScreenshot:103});
 const events=await Promise.race([waiting,new Promise((_,reject)=>setTimeout(()=>reject(Error('Событие ждало таймер')),100))]);
 assert.equal(events[0].kind,'screenshot');
 console.log('Ожидание события: callback немедленно передаёт скриншот');
})().catch(e=>{console.error(e);process.exitCode=1});
