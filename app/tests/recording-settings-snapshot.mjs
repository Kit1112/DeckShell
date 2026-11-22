import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import assert from 'node:assert/strict';
const js = readFileSync(new URL('../src/screens/recording_snapshot.js', import.meta.url), 'utf8');
// Realistic MobX properties must be read individually; no network store exists.
const values = {gamerecording_background_mode:2, gamerecording_background_a_m:false,
  gamerecording_video_bitrate:'preset_default', gamerecording_video_maxheight:0};
const settings = Object.defineProperties({}, Object.fromEntries(Object.entries(values).map(([key,value])=>[key,{get:()=>value}])));
assert.equal(JSON.stringify(settings),'{}');
const r = JSON.parse(JSON.stringify(runInNewContext(js,{window:{settingsStore:{clientSettings:settings}}})));
for (const [key,value] of Object.entries(values)) assert.equal(r.c[key],value);
assert.equal(r.c.gamerecording_ic_seconds,null);
assert.equal(runInNewContext(js,{window:{}}),null);
assert.equal(runInNewContext(js,{window:{settingsStore:{}}}),null);
assert.equal(runInNewContext(js,{window:{settingsStore:{m_ClientSettings:values}}}).c.gamerecording_background_mode,2);
console.log('Recording snapshot: absent optional stores, MobX accessors, false/zero, missing settings passed');

const broken = Object.defineProperty({}, 'gamerecording_background_mode', {get(){throw Error('not ready')}});
const recovered = runInNewContext(js,{window:{settingsStore:{clientSettings:broken,m_ClientSettings:values}}});
assert.equal(recovered.c.gamerecording_background_mode,2);
assert.equal(recovered.c.gamerecording_background_a_m,false);
const brokenStore = Object.defineProperty({m_ClientSettings:values}, 'clientSettings', {get(){throw Error('not ready')}});
assert.equal(runInNewContext(js,{window:{settingsStore:brokenStore}}).c.gamerecording_background_mode,2);
// The writer embeds this same reader in its bounded readback expression.
assert.equal(runInNewContext(`(${js})?.c?.gamerecording_background_a_m ?? null`,{window:{settingsStore:{clientSettings:settings}}}),false);

// Keep the hidden source readable by the unchanged generic Steam writer, without
// making its absence a required setting for page rendering.
assert.equal(runInNewContext(`(${js})?.c?.gamerecording_background_audio ?? null`, {window:{settingsStore:{clientSettings:{gamerecording_background_audio:2}}}}), 2);
