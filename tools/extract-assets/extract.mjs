#!/usr/bin/env node
// Extract Steam's own gamepad-UI icons into $CACHE/icons/<name>.svg + index.json.
// Source of truth: Steam's webpack module registry as shipped on disk (steamui/sp.js etc.).
// We only *read* the files and run the module wrappers in a stubbed local sandbox; Steam is untouched.
//   node extract.mjs            static extraction (no Steam/CDP needed)
//   node extract.mjs --live     additionally read-only CDP pass: every <svg> in BP/MainMenu/QuickAccess -> live-svgs.json
// Output is Valve IP: it lives in the cache only and must never be committed.
import fs from 'node:fs';
import path from 'node:path';
import { modules, exportNames, CACHE, STEAMUI } from './lib.mjs';
import { evalModule, renderIcon } from './sandbox.mjs';

const OUT = path.join(CACHE, 'icons');
const svgs = new Map();
const index = { generated: new Date().toISOString(), steamui: STEAMUI, icons: {}, aliases: {}, files: {} };
const safe = n => n.replace(/[^A-Za-z0-9_.-]+/g, '_').replace(/\./g, '-');
function put(name, svg, meta) {
  const file = safe(name) + '.svg';
  if (index.icons[name]) return false;
  svgs.set(file, svg);
  index.icons[name] = { file, bytes: svg.length, ...meta };
  return true;
}

/** Expose top-level function declarations of a module: append a table before the final brace. */
function withFnTable(src) {
  const names = [...new Set([...src.matchAll(/function\s+([A-Za-z_$][\w$]*)\s*\(/g)].map(m => m[1]))];
  const tbl = names.map(n => `${JSON.stringify(n)}:(typeof ${n}!=="undefined"?${n}:undefined)`).join(',');
  const i = src.lastIndexOf('}');
  return src.slice(0, i) + `;globalThis.__fnTable={${tbl}}` + src.slice(i);
}

const PROP_TRIES = [{}, { uniformSizing: true, batteryLevel: 0.8 }, { uniformSizing: true }, { size: 36 }];
function renderAny(fn) { for (const p of PROP_TRIES) { const s = renderIcon(fn, p); if (s) return s; } return null; }

function variants(name, fn, src, table) {
  const out = [];
  const fsrc = fn.toString();
  // switch(prop) { case "up": ... } -> Name.up etc.
  for (const sw of fsrc.matchAll(/switch\(([\w$]+)\)\{/g)) {
    const alias = sw[1];
    const key = (fsrc.match(new RegExp(`([\\w$]+):${alias.replace(/\$/g, '\\$')}[,=}]`)) || [])[1];
    if (!key) continue;
    const body = fsrc.slice(sw.index, sw.index + 20000);
    for (const c of body.matchAll(/case\s*"([^"]+)"\s*:/g)) {
      const s = renderIcon(fn, { [key]: c[1] });
      if (s) out.push([`${name}.${c[1]}`, s]);
    }
  }
  // case X.Y.Level: ...jsx)(Inner, -> Name.Level via the function table
  for (const m of fsrc.matchAll(/case\s*[\w$]+\.[\w$]+\.([\w$]+):[^;]*?\)\(([\w$]+),\{/g)) {
    const inner = table?.[m[2]];
    const s = inner && renderAny(inner);
    if (s) out.push([`${name}.${m[1]}`, s]);
  }
  if (name === 'Battery' || name === 'BatteryFull' || name === 'BatteryWhite' || name === 'BatteryLow')
    for (const l of [0.05, 0.2, 0.5, 0.8, 1]) { const s = renderIcon(fn, { uniformSizing: true, batteryLevel: l }); if (s) out.push([`${name}.${Math.round(l * 100)}`, s]); }
  return out;
}

// ---- 1. the named icon library (module exporting ABXYButton + Home + WiFi ...) ----
let libMod = null;
let controllerEnum = null;
let svgIdHook = null;
for (const m of modules()) {
  const ex = exportNames(m.src);
  if (ex.includes('ABXYButton') && ex.includes('Home') && ex.includes('WiFi')) { if (!libMod || m.file === 'sp.js') libMod = m; }
  if (m.file !== 'sp.js') continue;
  if (['AE', 'D$', 'rb', 'HD', 'UK'].every(key => ex.includes(key))) controllerEnum = m;
  if (m.src.includes('"svgid_') && ex.includes('l')) svgIdHook = m;
}
if (!libMod || !controllerEnum || !svgIdHook) throw new Error('Steam icon library or controller glyph dependencies not found');
{
  delete globalThis.__fnTable;
  const fn = (0, eval)('(' + withFnTable(libMod.src) + ')');
  const controllerTypes = evalModule((0, eval)('(' + controllerEnum.src + ')'));
  const ex = evalModule(fn, id => {
    if (String(id) === controllerEnum.id) return controllerTypes;
    if (String(id) === svgIdHook.id) return { l: () => ['controller-clip', 'url(#controller-clip)'] };
  });
  const table = globalThis.__fnTable;
  let n = 0;
  for (const k of Object.keys(ex).sort()) {
    if (typeof ex[k] !== 'function') continue;
    const s = renderAny(ex[k]);
    if (s && put(k, s, { module: `${libMod.file}#${libMod.id}`, export: k })) n++;
    for (const [vn, vs] of variants(k, ex[k], libMod.src, table)) if (put(vn, vs, { module: `${libMod.file}#${libMod.id}`, export: k, variant: vn })) n++;
  }
  for (const [name, typeKey] of Object.entries({
    Deck: 'AE', Xbox: 'D$', Steam: 'rb', Triton: 'VD', PS4: '_X', PS5: 'HD', Switch: 'gW',
    Hori: 'tE', Mobile: 'kL', Frame: 'My', Generic: 'UK',
  })) {
    const controllerType = controllerTypes[typeKey];
    const svg = renderIcon(ex.ControllerType, { controllerType });
    if (!svg) throw new Error(`Cannot extract ControllerType.${name}`);
    put(`ControllerType.${name}`, svg, { module: `${libMod.file}#${libMod.id}`, export: 'ControllerType', controllerType });
  }
  console.log(`icon library ${libMod.file}#${libMod.id}: ${n} icons`);
}

// (step 2 removed: evaluating every module exhausts the heap; inline unexported icons are not addressable by name)

// ---- 3. standalone svg files shipped in steamui/images ----
function walk(d) { return fs.readdirSync(d, { withFileTypes: true }).flatMap(e => e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]); }
for (const f of walk(path.join(STEAMUI, 'images')).filter(f => f.endsWith('.svg'))) {
  put('img.' + path.basename(f, '.svg'), fs.readFileSync(f, 'utf8'), { module: path.relative(STEAMUI, f) });
}
// bitmap controller glyphs etc: just record paths (not copied)
const GL = path.join(STEAMUI, '..', 'controller_base/images/api');
index.glyphs = {};
if (fs.existsSync(GL)) for (const variant of fs.readdirSync(GL)) {
  const d = path.join(GL, variant);
  if (!fs.statSync(d).isDirectory()) continue;
  for (const f of fs.readdirSync(d)) {
    const m = f.match(/^(.+?)(?:_(sm|md|lg))?\.(svg|png)$/); if (!m) continue;
    const e = ((index.glyphs[m[1]] ??= {})[variant] ??= {});
    e[m[3] === 'svg' ? 'svg' : m[2]] = path.join(d, f);
  }
}
index.files.controller_glyphs = walk(path.join(STEAMUI, 'images/controller')).map(f => path.relative(STEAMUI, f)).slice(0, 2000);

// ---- 4. curated semantic aliases used by DeckShell (alias -> icon name) ----
const A = {
  home: 'Home', library: 'Library', store: 'Store', friends: 'Friends', media: 'MediaGallery', downloads: 'Download',
  settings: 'Settings', power: 'Power', search: 'Search', bell: 'Notifications', notifications: 'Notifications',
  wifi: 'WiFi', 'wifi.excellent': 'ConnectionMeter.Excellent', 'wifi.good': 'ConnectionMeter.Good', 'wifi.ok': 'ConnectionMeter.Ok', 'wifi.weak': 'ConnectionMeter.Weak',
  battery: 'Battery', 'battery.20': 'Battery.20', 'battery.50': 'Battery.50', 'battery.80': 'Battery.80', 'battery.100': 'Battery.100',
  sdcard: 'SdCard', 'sdcard.warning': 'SdCardWarning', controller: 'Controller', play: 'Play', pause: 'Pause', 'big-picture': 'BigPicture',
  steam: 'SteamLogo', deck: 'SteamDeck', 'deck-logo': 'DeckLogo', bluetooth: 'Bluetooth', volume: 'Volume', 'volume.muted': 'VolumeMuted',
  brightness: 'Brightness', lock: 'Lock', close: 'Close', add: 'Add', check: 'Checkmark', info: 'Information', help: 'Help',
  keyboard: 'Keyboard', 'qam.performance': 'QAMPerformance', 'qam.notifications': 'Notifications', 'qam.friends': 'Friends',
  'qam.settings': 'Settings', 'qam.help': 'Help', 'qam.controller': 'Controller', 'qam.media': 'RecordingsScreenshots',
  'qam.sound': 'Volume', 'qam.display': 'Display', 'qam.power': 'Power', 'qam.guide': 'GuideButton',
  'btn.abxy': 'ABXYButton', 'btn.guide': 'GuideButton', 'badge.verified': 'DeckLogoColor', 'deck.playtime': 'PlayTime', 'achievements': 'Achievement',
  recent: 'Recent', 'cloud.sync': 'CloudSync', 'friends.add': 'AddFriend', 'airplane': 'AirplaneMode', 'dnd': 'DoNotDisturb', 'chat': 'Chat',
  'dots': 'Dots', 'trash': 'Trash', 'star': 'Star', 'filter': 'Filter', 'harddrive': 'HardDrive', 'refresh': 'Refresh',
};
for (const [a, n] of Object.entries(A)) if (index.icons[n]) index.aliases[a] = n;
index.count = Object.keys(index.icons).length;
// Keep the previous cache available if the installed bundle cannot be extracted.
fs.mkdirSync(OUT, { recursive: true });
for (const [file, svg] of svgs) fs.writeFileSync(path.join(OUT, file), svg);
fs.writeFileSync(path.join(CACHE, 'icons', 'index.json'), JSON.stringify(index, null, 1));
fs.writeFileSync(path.join(CACHE, 'index.json'), JSON.stringify(index, null, 1));
console.log(`wrote ${index.count} icons, ${Object.keys(index.aliases).length} aliases -> ${OUT}`);

// ---- 5. optional read-only live pass ----
if (process.argv.includes('--live')) {
  const { targets, connect } = await import('../cdp/cdp.mjs');
  const live = [];
  for (const t of (await targets()).filter(t => /Big Picture|MainMenu_|QuickAccess_|SharedJSContext/.test(t.title))) {
    const c = await connect(t);
    try {
      const r = await c.eval(`JSON.stringify([...document.querySelectorAll('svg')].map(s=>{let p=s.closest('[class]');return {html:s.outerHTML.slice(0,4000),aria:s.getAttribute('aria-label')||s.parentElement?.getAttribute('aria-label')||'',cls:(s.getAttribute('class')||''),parent:p&&p!==s?p.className:''}}))`);
      live.push({ target: t.title, svgs: JSON.parse(r) });
    } catch (e) { live.push({ target: t.title, error: String(e) }); }
    c.close();
  }
  fs.writeFileSync(path.join(CACHE, 'live-svgs.json'), JSON.stringify(live));
  console.log('live:', live.map(l => `${l.target}=${l.svgs?.length ?? 'err'}`).join(' '));
}
