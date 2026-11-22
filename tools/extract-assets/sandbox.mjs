// Evaluate a webpack module function with stub requires and a fake JSX runtime that builds
// plain virtual nodes, so exported icon components can be rendered to SVG strings offline.
// Nothing here talks to Steam; it only runs Steam's own module source locally.
const CAMEL_KEEP = new Set(['viewBox', 'preserveAspectRatio', 'gradientUnits', 'gradientTransform', 'patternUnits', 'patternContentUnits', 'patternTransform', 'clipPathUnits', 'maskUnits', 'maskContentUnits', 'filterUnits', 'primitiveUnits', 'stdDeviation', 'baseFrequency', 'numOctaves', 'markerWidth', 'markerHeight', 'refX', 'refY', 'textLength', 'startOffset', 'spreadMethod', 'pathLength', 'tableValues', 'xChannelSelector', 'yChannelSelector']);
export const node = (t, p) => ({ __vnode: true, t, p: p || {} });
const flat = c => (Array.isArray(c) ? c.flat(Infinity) : [c]);
export const jsx = (t, p, k) => node(t, p);
export const createElement = (t, p, ...ch) => node(t, ch.length ? { ...(p || {}), children: ch.length === 1 ? ch[0] : ch } : p);

export function stub(name = 'stub') {
  const target = function () {};
  const cache = new Map();
  const p = new Proxy(target, {
    get(_, k) {
      if (k === 'prototype') return target.prototype;
      if (k === Symbol.toPrimitive) return () => '';
      if (k === 'then') return undefined;
      if (k === 'jsx' || k === 'jsxs') return jsx;
      if (k === 'createElement') return createElement;
      if (k === 'Fragment') return 'Fragment';
      if (k === '__esModule') return true;
      if (k === Symbol.iterator) return function* () { while (true) yield stub('it'); };
      if (typeof k === 'symbol') return undefined;
      if (!cache.has(k)) cache.set(k, stub(String(k)));
      return cache.get(k);
    },
    apply: (_, __, args) => (typeof args[0] === 'function' && /^(memo|forwardRef|observer|memo2|withStyles)/.test(name) ? args[0] : p),
    construct: () => p,
    set: () => true,
  });
  return p;
}

export function makeRequire(exportsRegistry) {
  const req = function (id) { return exportsRegistry?.(id) ?? stub('mod' + id); };
  req.r = e => { Object.defineProperty(e, '__esModule', { value: true }); };
  req.d = (e, defs) => { for (const k in defs) Object.defineProperty(e, k, { enumerable: true, configurable: true, get: defs[k] }); };
  req.n = m => { const g = () => (m && m.__esModule ? m.default : m); g.a = g; return g; };
  req.o = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
  req.g = globalThis; req.p = ''; req.e = () => Promise.resolve(); req.t = m => m; req.nmd = m => m; req.u = () => ''; req.l = () => {}; req.f = {}; req.m = {}; req.c = {};
  return req;
}

export function evalModule(fn, registry) {
  const module = { exports: {} };
  fn.call(module.exports, module, module.exports, makeRequire(registry));
  return module.exports;
}

function attrName(k) {
  if (k === 'className') return 'class';
  if (k === 'xlinkHref') return 'xlink:href';
  if (k === 'htmlFor') return 'for';
  if (CAMEL_KEEP.has(k) || !/[A-Z]/.test(k) || k.startsWith('data-') || k.startsWith('aria-')) return k;
  return k.replace(/[A-Z]/g, c => '-' + c.toLowerCase());
}
const esc = s => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');

export function toSvg(n, depth = 0) {
  if (depth > 40) throw new Error('deep');
  if (n == null || n === false || n === true) return '';
  if (typeof n === 'string' || typeof n === 'number') return esc(n).replace(/&quot;/g, '"');
  if (Array.isArray(n)) return n.map(c => toSvg(c, depth + 1)).join('');
  if (!n.__vnode) return '';
  let { t, p } = n;
  if (typeof t === 'function') { const r = t({ ...p }); return toSvg(r, depth + 1); }
  if (t && typeof t === 'object' && typeof t.render === 'function') return toSvg(t.render({ ...p }, null), depth + 1);
  if (t === 'Fragment' || typeof t !== 'string') return toSvg(p.children, depth + 1);
  let a = '';
  for (const [k, v] of Object.entries(p)) {
    if (k === 'children' || k === 'ref' || k === 'key' || k === 'dangerouslySetInnerHTML' || typeof v === 'function' || v == null || v === false) continue;
    if (k.startsWith('on')) continue;
    if (k === 'style' && typeof v === 'object') {
      const s = Object.entries(v).filter(([, x]) => typeof x === 'string' || typeof x === 'number').map(([x, y]) => `${x.replace(/[A-Z]/g, c => '-' + c.toLowerCase())}:${y}`).join(';');
      if (s) a += ` style="${esc(s)}"`;
      continue;
    }
    if (typeof v === 'object') continue;
    a += ` ${attrName(k)}="${esc(v === true ? 'true' : v)}"`;
  }
  const inner = toSvg(p.children, depth + 1);
  return inner ? `<${t}${a}>${inner}</${t}>` : `<${t}${a}/>`;
}

/** Render a component function to an <svg> string, or null if it does not produce a root <svg>. */
export function renderIcon(fn, props = {}) {
  let r;
  try { r = fn(props); } catch { return null; }
  // unwrap wrappers (div/span around svg are not icons)
  if (!r || !r.__vnode) return null;
  let cur = r, hops = 0;
  while (cur && cur.__vnode && typeof cur.t === 'function' && hops++ < 5) { try { cur = cur.t({ ...cur.p }); } catch { return null; } }
  if (!cur || !cur.__vnode || cur.t !== 'svg') return null;
  try {
    let s = toSvg(cur);
    if (!/xmlns=/.test(s.slice(0, 200))) s = s.replace('<svg', '<svg xmlns="http://www.w3.org/2000/svg"');
    return s.length < 200000 ? s : null;
  } catch { return null; }
}
