import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import assert from 'node:assert/strict';
const js = readFileSync(new URL('../src/screens/recording_capability.js', import.meta.url), 'utf8');
async function probe({value=true, result=1, missing=false, duplicate=false}={}) {
  const calls=[];
  function factory() { return 'GameRecording.GetPlatformCapabilities#1'; }
  const req = () => ({service:{async GetPlatformCapabilities(request) {
    calls.push(request);
    return {GetEResult:()=>result, Body:()=>({per_process_audio_capture:()=>value})};
  }}});
  req.m = missing ? {} : duplicate ? {a:factory, b:factory} : {a:factory};
  const window = {webpackChunksteamui:{push:([,,f])=>f(req)}};
  return {value:await runInNewContext(js, {window, Symbol}),calls};
}
assert.equal((await probe()).value, true);
assert.equal((await probe({value:false})).value, false);
assert.equal((await probe({value:'true'})).value, null);
assert.equal((await probe({result:2})).value, null);
assert.equal((await probe({missing:true})).value, null);
assert.equal((await probe({duplicate:true})).calls.length, 0);
console.log('recording capability: 6 cases passed');
