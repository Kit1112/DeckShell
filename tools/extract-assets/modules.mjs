#!/usr/bin/env node
// Dump an index of every webpack module of the local Steam UI -> $CACHE/modules.json
// Static (reads steamui/*.js, executes only the chunk wrappers; no CDP, no Steam interaction).
import fs from 'node:fs';
import path from 'node:path';
import { modules, exportNames, CACHE } from './lib.mjs';

function components(src) {
  const names = new Set();
  for (const m of src.matchAll(/class\s+([A-Za-z_$][\w$]*)\s+extends\s+[\w$.()]*?(?:Component|PureComponent)\b/g)) names.add(m[1]);
  for (const m of src.matchAll(/displayName\s*=\s*"([^"]+)"/g)) names.add(m[1]);
  for (const m of src.matchAll(/function\s+([A-Z][\w$]*)\s*\([^)]*\)\s*\{[^]{0,400}?\.jsxs?\)\(/g)) names.add(m[1]);
  return [...names].slice(0, 20);
}

const out = [];
for (const { file, id, src } of modules()) {
  out.push({
    file, id,
    len: src.length,
    exports: exportNames(src).slice(0, 40),
    components: components(src),
    classNames: [...new Set([...src.matchAll(/\bclass\s+([A-Za-z_$][\w$]*)/g)].map(m => m[1]))].slice(0, 10),
    strings: [...new Set([...src.matchAll(/"((?:Steam|Gamepad|Library|Home|MainMenu|QuickAccess|Settings)[A-Za-z]{3,40})"/g)].map(m => m[1]))].slice(0, 12),
    head: src.slice(0, 200),
  });
}
fs.mkdirSync(CACHE, { recursive: true });
fs.writeFileSync(path.join(CACHE, 'modules.json'), JSON.stringify(out));
console.log(`modules.json: ${out.length} modules from ${new Set(out.map(m => m.file)).size} files`);
