#!/usr/bin/env node
// Resolve identifiers referenced by docs/cdp/behavior/handlers.json (read-only) to
// webpack module / enclosing class / export names. Writes $CACHE/handlers-resolved.json and prints markdown rows.
import fs from 'node:fs';
import path from 'node:path';
import { modules, exportNames, CACHE } from './lib.mjs';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '../..');
const handlers = JSON.parse(fs.readFileSync(path.join(root, 'docs/cdp/behavior/handlers.json'), 'utf8'));
const text = JSON.stringify(handlers);
// Candidate identifiers: CamelCase method/store names with >=2 humps (OnHomeButtonPressed, OpenPowerMenu ...), plus *Store/*Manager
const ids = [...new Set([...text.matchAll(/\b((?:On|Open|Set|Show|Close|Toggle|Select|Focus|Start|Run|Resume|Check|Ensure|Report|Take|Change|Play|Add|Remove|Switch|Is)[A-Z][A-Za-z]{3,}|[A-Z][A-Za-z]+(?:Store|Manager))\b/g)].map(m => m[1]))]
  .filter(i => !['SteamClient', 'OpenVR', 'IsAnyVRWindow'].includes(i));
const res = Object.fromEntries(ids.map(i => [i, []]));
for (const { file, id, src } of modules()) {
  for (const i of ids) {
    let at = src.indexOf(i);
    if (at < 0) continue;
    // enclosing class: last `class X` before the hit within this module
    const before = src.slice(0, at);
    const cls = [...before.matchAll(/class\s+([A-Za-z_$][\w$]*)/g)].pop()?.[1] ?? null;
    const isDef = new RegExp(`(?:^|[;{},\\s])(?:async\\s+)?${i}\\s*\\([^)]*\\)\\s*\\{`).test(src) || new RegExp(`${i}\\s*=\\s*(?:async\\s*)?\\(`).test(src);
    if (res[i].length < 4) res[i].push({ module: `${file}#${id}`, class: cls, defined: isDef, exports: exportNames(src).slice(0, 6) });
  }
}
// prefer modules where the identifier is defined
for (const i of ids) res[i].sort((a, b) => b.defined - a.defined);
fs.writeFileSync(path.join(CACHE, 'handlers-resolved.json'), JSON.stringify(res, null, 1));
for (const i of ids) if (res[i].length) { const r = res[i][0]; console.log(`| \`${i}\` | ${r.module} | ${r.class ?? '-'} | ${r.defined ? 'def' : 'use'} | ${r.exports.join(', ')} |`); }
