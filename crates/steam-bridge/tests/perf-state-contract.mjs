// Exact production-template contract: no Steam/network, no store initialization.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const source=fs.readFileSync(new URL('../src/js.rs',import.meta.url),'utf8');
const template=source.match(/pub const PERF_READ: &str = r#"([\s\S]*?)"#;/)[1];
const helper=source.match(/pub const WEBPACK_W: &str = "(.*?)";/)[1];
const script=template.replace('@W@',helper);
function fixture(extra={}) {
 let calls=[];
 const window={webpackChunksteamui:{push:()=>{calls.push('injection');assert.fail('read must not inject webpack chunks')}},...extra};
 const context=vm.createContext({window,SteamClient:{System:{Perf:{}}},...extra});
 return {context,calls};
}
const evaluate=async f=>JSON.parse(JSON.stringify(await vm.runInContext(script,f.context)));
// Regression: the old expression threw ReferenceError for SystemStoragStore,
// poisoning all unrelated performance/scale reads in the live shared context.
let f=fixture({settingsStore:{clientSettings:{steamos_tdp_limit_enabled:false,steamos_tdp_limit:8,steamos_vrs_enabled:true},settings:{bDisplayIsUsingAutoScale:false,flCurrentDisplayScaleFactor:1.5}}});
let r=await evaluate(f);assert.equal(r.airplane,null);assert.equal(r.perf,null);
assert.equal(r.tdp.on,false);assert.equal(r.tdp.w,8);assert.equal(r.vrs,true);
assert.equal(r.scale.auto,false);assert.equal(r.scale.factor,1.5);
assert.equal(r.tearing,null);assert.equal(r.gpu.on,null);assert.equal(r.profile,null);
assert.deepEqual(f.calls,[]);
// Missing all stores is unknown, not an invented false or blank profile.
f=fixture();r=await evaluate(f);assert.equal(r.tdp.on,null);assert.equal(r.profile,null);assert.equal(r.scale.auto,null);
// Native stores expose prototype accessors, not enumerable fields.
class Perf {
 get nCurrentGameID(){return '730'} get nActiveProfileGameID(){return '0'}
 get msgLimits(){return {fps_limit_options:[30,60]}}
 get msgSettingsGlobal(){return {perf_overlay_service_state:2,perf_overlay_level:3}}
 get msgSettingsPerApp(){return {fps_limit:60}}
}
class Storage {get AirplaneModeEnabled(){return {Value:false}}}
f=fixture({SystemPerfStore:new Perf(),SystemStoragStore:new Storage(),settingsStore:{clientSettings:{gamescope_allow_tearing:false,steamos_platform_performance_profile:''},settings:{}}});
r=await evaluate(f);assert.equal(r.airplane,false);assert.equal(r.tearing,false);assert.equal(r.profile,'');
assert.equal(r.perf.global.perf_overlay_level,3);assert.equal(r.perf.gameid,'730');
// Failure of one optional getter cannot poison known values from other domains.
Object.defineProperty(f.context.window,'SystemPerfStore',{get(){throw Error('unavailable')}});
r=await evaluate(f);assert.equal(r.perf,null);assert.equal(r.airplane,false);
Object.defineProperty(f.context,'SystemStoragStore',{get(){throw Error('unavailable')}});
r=await evaluate(f);assert.equal(r.airplane,null);assert.equal(r.tearing,false);
// No webpack access is required even when the injector is unavailable.
f.context.window.webpackChunksteamui.push=()=>{throw Error('injection forbidden')};
r=await evaluate(f);assert.equal(r.limits,null);assert.equal(r.tearing,false);
console.log('Performance state exact-template contract PASS: absent stores, genuine false, prototype getters, isolated failures; no webpack injection/Require/Init/Get/write.');
