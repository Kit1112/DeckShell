// Offline contract harness: extract exact Rust JS templates; no Steam/network.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source=fs.readFileSync(new URL('../src/recording.rs',import.meta.url),'utf8');
const view=source.match(/const VIEW_JS: &str = r#"([\s\S]*?)"#;/)[1];
const operations=source.match(/let operation = if start \{\s*r#"([\s\S]*?)"#\s*\} else \{\s*r#"([\s\S]*?)"#/);
let template=source.match(/format!\("(\(async[\s\S]*?)"\)\n\}/)[1];
// Unescape only format's doubled braces; template contains no Rust escapes.
const js=start=>template.replaceAll('{{','{').replaceAll('}}','}').replace('{id}',JSON.stringify('730')).replace('{operation}',operations[start?1:2]);
function fixture({broadcast=false,mode=2,remote=false,active=null,code=1,asyncStatus=false}={}) {
 let calls=0,stops=0,unregisters=0;
 const store={m_recordingState:active,m_bEnoughDiskSpace:true,m_clips:new Map(),m_bClipLoadingTriggered:false,m_bLoadingClips:true,m_bEnableH265Recording:false,
  async StartRecording(id){calls++;assert.equal(id,'730');return {GetEResult:()=>code}},async StopRecording(id){stops++;assert.equal(id,'730')}};
 const window={g_GRS:store,settingsStore:{clientSettings:{gamerecording_background_mode:mode}},SteamUIStore:{ActiveWindowInstance:{MainRunningApp:{GetGameID:()=> '730'}}},
  RemotePlayStore_SteamUI:{devices:remote?[{status:'Streaming'}]:[]},g_FriendsUIApp:{BroadcastStore:{status:{bIsBroadcasting:broadcast}}},SteamClient:{Broadcast:{RegisterForBroadcastStatus(callback){if(asyncStatus)queueMicrotask(()=>callback({bIsBroadcasting:broadcast}));else callback({bIsBroadcasting:broadcast});return {unregister(){unregisters++}}}}}};
 return {ctx:vm.createContext({window,setTimeout,clearTimeout,Array}),store,counts:()=>[calls,stops,unregisters]};
}
let f=fixture();let r=await vm.runInContext(js(true),f.ctx);assert.equal(r.eresult,1);assert.deepEqual(f.counts(),[1,0,0]);
f=fixture({code:15,asyncStatus:true});r=await vm.runInContext(js(true),f.ctx);assert.equal(r.eresult,15);assert.deepEqual(f.counts(),[1,0,0]);
for(const opts of [{broadcast:true},{broadcast:null},{mode:1},{remote:true},{active:{m_gameID:'730'}}]) {
 f=fixture(opts);r=await vm.runInContext(js(true),f.ctx);assert.equal(r.outcome,'unavailable');assert.equal(f.counts()[0],0);
}
f=fixture();delete f.ctx.window.g_GRS;r=await vm.runInContext(js(true),f.ctx);assert.equal(r.outcome,'unavailable');assert.equal(f.counts()[0],0);
f=fixture({active:{m_gameID:'730'}});r=await vm.runInContext(js(false),f.ctx);assert.equal(r.outcome,'stop');assert.deepEqual(f.counts(),[0,1,0]);
f=fixture({active:{m_gameID:'999'}});r=await vm.runInContext(js(false),f.ctx);assert.equal(r.outcome,'unavailable');assert.equal(f.counts()[1],0);
f=fixture();r=vm.runInContext(view,f.ctx);assert.equal(r.mode,2);assert.equal(r.running_game,'730');assert.equal(r.cached_clip_count,0);assert.equal(r.can_start,true);
// Exceptions after dispatch have unknown outcome; the template never retries.
f=fixture();f.store.StartRecording=async()=>{throw Error('synthetic lost reply')};
await assert.rejects(vm.runInContext(js(true),f.ctx),/synthetic lost reply/);
f=fixture();delete f.ctx.window.g_FriendsUIApp;r=await vm.runInContext(js(true),f.ctx);
assert.equal(r.outcome,'unavailable');assert.equal(f.counts()[0],0);
// Native MobX session objects can serialize as {}, so read m_gameID explicitly.
f=fixture({active:Object.defineProperty({},'m_gameID',{get:()=> '730'})});
r=vm.runInContext(view,f.ctx);assert.equal(r.session.game_id,'730');
// Live test5 regression: direct MobX summaries serialize as [{}] over CDP.
// Model the actual sanitized shape using non-enumerable accessors, including
// the genuinely omitted optional fields. Roundtrip models returnByValue JSON.
const actualShape=JSON.parse(fs.readFileSync(new URL('./fixtures/native-clip-sanitized.json',import.meta.url),'utf8'));
const observableSummary=Object.defineProperties({},Object.fromEntries(Object.entries(actualShape).map(([k,value])=>[k,{get:()=>value}])));
assert.equal(JSON.stringify(observableSummary),'{}');
f=fixture();f.store.m_clips.set(actualShape.clip_id,observableSummary);
r=JSON.parse(JSON.stringify(vm.runInContext(view,f.ctx)));
assert.deepEqual(r.clips,[actualShape]);assert.equal(r.cached_clip_count,1);
assert.equal(typeof r.clips[0].game_id,'string');
assert.equal(r.clips[0].duration_ms,'21816');assert.equal(r.clips[0].temporary,false);
assert.equal(Object.hasOwn(r.clips[0],'original_device'),false);
// MainRunningApp can disappear after the launcher exits; absence is not a fake ID.
f.ctx.window.SteamUIStore.ActiveWindowInstance.MainRunningApp=undefined;
r=JSON.parse(JSON.stringify(vm.runInContext(view,f.ctx)));assert.equal(r.running_game,null);
console.log('Offline exact-template JS checks passed: start/rejection/guards/stop/view/cached preflight. No native actions.');
