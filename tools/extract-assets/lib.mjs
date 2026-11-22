// Shared helpers: load Steam's webpack chunk files (steamui/*.js) WITHOUT running modules.
// Each chunk file only does self.webpackChunksteamui.push([[ids], {id: fn}]); we capture the push.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import vm from 'node:vm';

export const HOME = os.homedir();
export const STEAMUI = process.env.STEAMUI || path.join(HOME, '.local/share/Steam/steamui');
export const CACHE = process.env.DECKSHELL_ASSETS || path.join(process.env.XDG_CACHE_HOME || path.join(HOME, '.cache'), 'deckshell/assets');

/** yields {file, id, src} for every webpack module in steamui/*.js (skips localization/). */
export function* modules(files, steamui = STEAMUI) {
  files ??= fs.readdirSync(steamui).filter(f => f.endsWith('.js')).sort();
  for (const f of files) {
    const code = fs.readFileSync(path.join(steamui, f), 'utf8');
    if (!code.includes('webpackChunksteamui')) continue;
    const arr = [];
    const sandbox = { self: { webpackChunksteamui: arr }, window: {}, document: {}, console };
    sandbox.globalThis = sandbox;
    try { vm.runInNewContext(code, sandbox, { timeout: 20000, filename: f }); } catch (e) { process.stderr.write(`skip ${f}: ${e.message}\n`); continue; }
    for (const entry of arr) {
      const mods = entry[1] || {};
      for (const id of Object.keys(mods)) {
        let src; try { src = Function.prototype.toString.call(mods[id]); } catch { continue; }
        yield { file: f, id, src };
      }
    }
  }
}

/** names exported via webpack harmony `r.d(t,{A:()=>x,...})` or `t.X=`. */
export function exportNames(src) {
  const out = new Set();
  for (const m of src.matchAll(/\.d\(\s*\w+\s*,\s*\{([^}]*)\}/g))
    for (const k of m[1].matchAll(/([A-Za-z_$][\w$]*|"[^"]+")\s*:\s*\(\)\s*=>/g)) out.add(k[1].replace(/"/g, ''));
  for (const m of src.matchAll(/\b(?:t|exports)\.([A-Za-z_$][\w$]*)\s*=/g)) out.add(m[1]);
  return [...out];
}

/** Index of the matching closer for the opener at src[i], skipping string/template literals. */
export function matchBalanced(src, i) {
  const open = src[i], close = open === '{' ? '}' : open === '(' ? ')' : ']';
  let depth = 0;
  for (let p = i; p < src.length; p++) {
    const c = src[p];
    if (c === '"' || c === "'") { p++; while (p < src.length && src[p] !== c) { if (src[p] === '\\') p++; p++; } continue; }
    if (c === '`') { p++; while (p < src.length && src[p] !== '`') { if (src[p] === '\\') p++; p++; } continue; }
    if (c === open) depth++;
    else if (c === close) { depth--; if (depth === 0) return p; }
  }
  return -1;
}
