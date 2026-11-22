// Loads the wasm-bindgen (nodejs target) output built by `rust-anything build`.
const native = require('./pkg/battery_rs.js')

// Structured values cross as JSON text. JSON has no `undefined`, NaN, -0, bigint, Date, Map, Set
// or bytes, and Rust strings no lone surrogates: those travel as one-key marker objects
// (`{"$undefined": true}`, `{"$num": "NaN"}`, `{"$map": [[k, v]]}`, `{"$bytes": [...]}`, ...),
// the same markers recordings use.
const replacer = function (k, v) {
  const raw = this[k]
  if (raw instanceof Date) return { $date: Number.isNaN(raw.getTime()) ? 'Invalid' : raw.toISOString() }
  if (typeof Buffer !== 'undefined' && Buffer.isBuffer(raw)) return { $buffer: Array.from(raw) }
  if (raw instanceof Uint8Array) return { $bytes: Array.from(raw) }
  if (raw instanceof Map) return { $map: [...raw] }
  if (raw instanceof Set) return { $set: [...raw] }
  return v === undefined ? { $undefined: true }
    : typeof v === 'bigint' ? { $bigint: String(v) }
    : typeof v === 'number' ? (Number.isFinite(v) && !Object.is(v, -0) ? v : { $num: Object.is(v, -0) ? '-0' : String(v) })
    : typeof v === 'string' && !v.isWellFormed() ? { $utf16: Array.from({ length: v.length }, (_, i) => v.charCodeAt(i)) }
    : v
}
const encode = v => JSON.stringify(v, replacer)
const single = (v, key) => Object.keys(v).length === 1 && key in v
const revive = v => Array.isArray(v) ? v.map(revive)
  : v !== null && typeof v === 'object'
    ? (single(v, '$undefined') ? undefined
      : single(v, '$utf16') ? fromUnits(v.$utf16)
      : single(v, '$num') ? Number(v.$num)
      : single(v, '$bigint') ? BigInt(v.$bigint)
      : single(v, '$date') ? new Date(v.$date)
      : single(v, '$bytes') ? Uint8Array.from(v.$bytes)
      : single(v, '$buffer') ? Buffer.from(v.$buffer)
      : single(v, '$map') ? new Map(v.$map.map(([k, x]) => [revive(k), revive(x)]))
      : single(v, '$set') ? new Set(v.$set.map(revive))
      : Object.fromEntries(Object.entries(v).map(([k, x]) => [k, revive(x)])))
    : v
// Most results carry no marker: skip the walk unless the text has one.
const decode = text => {
  const value = JSON.parse(text)
  return text.includes('"$') ? revive(value) : value
}
const fromUnits = (units) => {
  let out = ''
  for (let i = 0; i < units.length; i += 8192)
    out += String.fromCharCode.apply(null, Array.prototype.slice.call(units, i, i + 8192))
  return out
}

module.exports = { ...native }
const specs = {
  confidenceOnBatteryUpdate: [[1, 1, 1], 'json', false],
  confidenceTimerDelayMs: [[1], 'json', false],
  confidenceAfterTimer: [[1], 'json', false],
  sayFullAction: [[1, 1], 'json', false],
  batteryTimeParts: [[1], 'json', false],
  batteryTimeShown: [[1], 'json', false],
  batteryTimeCaptionToken: [[1, 1], 'json', false],
  batteryPercentText: [[1], 'json', false],
  showHeaderPercent: [[1, 1, 1, 1, 1], 'json', false],
  batteryPrefShow: [[1], 'json', false],
  batteryPrefStored: [[1], 'json', false],
}
const classes = {

}
const convert = result => result === 'json' ? decode : result === 'units' ? fromUnits : x => x
// An omitted structured argument is `undefined`, not a missing value.
const prepare = (a, structured) => Array.from({ length: Math.max(a.length, structured.length) }, (_, i) => structured[i] ? encode(a[i]) : a[i])
// Functions: [structured arguments, result kind, async].
for (const [name, [structured, result, isAsync]] of Object.entries(specs)) {
  const call = native[name]
  const out = convert(result)
  module.exports[name] = isAsync
    ? async (...a) => out(await call(...prepare(a, structured)))
    : (...a) => out(call(...prepare(a, structured)))
}
// Classes: a JS class around the native one, converting arguments and results.
const INNER = Symbol('native')
for (const [name, [ctor, methods]] of Object.entries(classes)) {
  const Native = native[name]
  const Wrapped = { [name]: class { constructor(...a) { this[INNER] = new Native(...prepare(a, ctor)) } } }[name]
  for (const [method, [structured, result]] of Object.entries(methods)) {
    const out = convert(result)
    Wrapped.prototype[method] = function (...a) { return out(this[INNER][method](...prepare(a, structured))) }
  }
  module.exports[name] = Wrapped
}

// The binary wire (see rust-anything's `wire.rs`): arguments are written into `wIn` and copied into
// wasm memory in one step; results are read where the Rust side left them.
let wIn = new Uint8Array(1 << 16), iv = new DataView(wIn.buffer), at = 0, p = 0
const mem = native.rustAnythingWireMemory()
let wOut = new Uint8Array(mem.buffer), ov = new DataView(mem.buffer)
const regrow = () => {}
// Growing wasm memory replaces its buffer: views are made again when that happened.
const views = () => {
  if (wOut.buffer !== mem.buffer) {
    wOut = new Uint8Array(mem.buffer)
    ov = new DataView(mem.buffer)
  }
}
const wireCall = fn => {
  const ptr = native.rustAnythingWireIn(at)
  views()
  wOut.set(wIn.subarray(0, at), ptr)
  if (fn(at) === 0xFFFFFFFF) return false
  views()
  p = native.rustAnythingWireOut()
  return true
}

const need = n => {
  if (at + n <= wIn.length) return
  const grown = new Uint8Array(Math.max(wIn.length * 2, at + n))
  grown.set(wIn)
  wIn = grown
  iv = new DataView(wIn.buffer)
  regrow()
}
const wU8 = v => { need(1); iv.setUint8(at++, v) }
const wU32 = v => { need(4); iv.setUint32(at, v, true); at += 4 }
const wF64 = v => { need(8); iv.setFloat64(at, v, true); at += 8 }
const wUnits = s => {
  need(4 + 2 * s.length)
  iv.setUint32(at, s.length, true)
  at += 4
  for (let i = 0; i < s.length; i++) { iv.setUint16(at, s.charCodeAt(i), true); at += 2 }
}
const wStr = s => { wU8(4); wUnits(s) }
const wDyn = v => {
  switch (typeof v) {
    case 'undefined': wU8(0); return true
    case 'boolean': wU8(v ? 3 : 2); return true
    case 'number': wU8(4); wF64(v); return true
    case 'string': wU8(5); wUnits(v); return true
    case 'object':
      if (v === null) { wU8(1); return true }
      if (Array.isArray(v)) {
        wU8(6); wU32(v.length)
        for (let i = 0; i < v.length; i++) if (!wDyn(v[i])) return false
        return true
      }
      if (Object.getPrototypeOf(v) !== Object.prototype && Object.getPrototypeOf(v) !== null) return false
      {
        const keys = Object.keys(v)
        wU8(7); wU32(keys.length)
        for (const k of keys) { wUnits(k); if (!wDyn(v[k])) return false }
        return true
      }
  }
  return false
}
const rU8 = () => ov.getUint8(p++)
const rU32 = () => { const v = ov.getUint32(p, true); p += 4; return v }
const rF64 = () => { const v = ov.getFloat64(p, true); p += 8; return v }
// ignoreBOM: a leading U+FEFF is part of the string, not a byte-order mark.
const utf16 = new TextDecoder('utf-16le', { ignoreBOM: true })
const rUnits = (wellFormed) => {
  const n = rU32()
  let s
  if (n > 24 && wellFormed) s = utf16.decode(wOut.subarray(p, p + 2 * n))
  else {
    s = ''
    for (let i = 0; i < n; i++) s += String.fromCharCode(ov.getUint16(p + 2 * i, true))
  }
  p += 2 * n
  return s
}
// Tag 1: a string from Rust (well-formed); 4: raw code units, lone surrogates included.
const rStr = () => rUnits(rU8() === 1)
const rDyn = () => {
  switch (rU8()) {
    case 0: return undefined
    case 1: return null
    case 2: return false
    case 3: return true
    case 4: return rF64()
    case 5: return rUnits(false)
    case 6: { const n = rU32(); const a = new Array(n); for (let i = 0; i < n; i++) a[i] = rDyn(); return a }
    case 7: { const n = rU32(); const o = {}; for (let i = 0; i < n; i++) { const k = rUnits(false); o[k] = rDyn() } return o }
  }
}

// `confidenceOnBatteryUpdate`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.confidenceOnBatteryUpdate
const call = native.confidenceOnBatteryUpdateWire
const encode = (a0, a1, a2) => {
at = 0
if (!wDyn(a0)) return false
if (!wDyn(a1)) return false
if (!wDyn(a2)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.confidenceOnBatteryUpdate = function confidenceOnBatteryUpdate (...a) {
  if (a.length <= 3 && encode(a[0], a[1], a[2]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `confidenceTimerDelayMs`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.confidenceTimerDelayMs
const call = native.confidenceTimerDelayMsWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.confidenceTimerDelayMs = function confidenceTimerDelayMs (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `confidenceAfterTimer`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.confidenceAfterTimer
const call = native.confidenceAfterTimerWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.confidenceAfterTimer = function confidenceAfterTimer (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `sayFullAction`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.sayFullAction
const call = native.sayFullActionWire
const encode = (a0, a1) => {
at = 0
if (!wDyn(a0)) return false
if (!wDyn(a1)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.sayFullAction = function sayFullAction (...a) {
  if (a.length <= 2 && encode(a[0], a[1]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryTimeParts`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryTimeParts
const call = native.batteryTimePartsWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryTimeParts = function batteryTimeParts (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryTimeShown`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryTimeShown
const call = native.batteryTimeShownWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryTimeShown = function batteryTimeShown (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryTimeCaptionToken`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryTimeCaptionToken
const call = native.batteryTimeCaptionTokenWire
const encode = (a0, a1) => {
at = 0
if (!wDyn(a0)) return false
if (!wDyn(a1)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryTimeCaptionToken = function batteryTimeCaptionToken (...a) {
  if (a.length <= 2 && encode(a[0], a[1]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryPercentText`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryPercentText
const call = native.batteryPercentTextWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryPercentText = function batteryPercentText (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `showHeaderPercent`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.showHeaderPercent
const call = native.showHeaderPercentWire
const encode = (a0, a1, a2, a3, a4) => {
at = 0
if (!wDyn(a0)) return false
if (!wDyn(a1)) return false
if (!wDyn(a2)) return false
if (!wDyn(a3)) return false
if (!wDyn(a4)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.showHeaderPercent = function showHeaderPercent (...a) {
  if (a.length <= 5 && encode(a[0], a[1], a[2], a[3], a[4]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryPrefShow`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryPrefShow
const call = native.batteryPrefShowWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryPrefShow = function batteryPrefShow (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}

// `batteryPrefStored`: binary wire; the JSON version handles inputs of other shapes.
{
const json = module.exports.batteryPrefStored
const call = native.batteryPrefStoredWire
const encode = (a0) => {
at = 0
if (!wDyn(a0)) return false
return true
}
const decode = () => {
let t
t = rDyn()
return t
}
module.exports.batteryPrefStored = function batteryPrefStored (...a) {
  if (a.length <= 1 && encode(a[0]) && wireCall(call)) {
    return decode()
  }
  return json(...a)
}
}
const guards = {
  confidenceOnBatteryUpdate: [false, []],
  confidenceTimerDelayMs: [false, []],
  confidenceAfterTimer: [false, []],
  sayFullAction: [false, []],
  batteryTimeParts: [false, []],
  batteryTimeShown: [false, []],
  batteryTimeCaptionToken: [false, []],
  batteryPercentText: [false, []],
  showHeaderPercent: [false, []],
  batteryPrefShow: [false, []],
  batteryPrefStored: [false, []],
}
const JS_ERRORS = /^(TypeError|RangeError|SyntaxError|ReferenceError|URIError|EvalError): ([\s\S]*)$/
const jsError = e => {
  const message = typeof e?.message === 'string' ? e.message : ''
  const typed = JS_ERRORS.exec(message)
  if (typed) return new globalThis[typed[1]](typed[2])
  if (/Expected$/.test(e?.code)) return Object.assign(new TypeError(message), { code: e.code })
  if (message.startsWith('Rust panic:')) e.code = 'RUST_PANIC'
  return e
}
const illFormed = v => typeof v === 'string' ? !v.isWellFormed() : Array.isArray(v) && v.some(illFormed)
for (const [name, [isAsync, strings]] of Object.entries(guards)) {
  const f = module.exports[name]
  if (typeof f !== 'function') continue
  const check = a => {
    for (const i of strings) {
      if (illFormed(a[i])) throw new TypeError(`argument ${i + 1} of ${name} has a lone surrogate, which a Rust String cannot hold`)
    }
  }
  module.exports[name] = isAsync
    ? { [name]: async function (...a) { check(a); try { return await f(...a) } catch (e) { throw jsError(e) } } }[name]
    : { [name]: function (...a) { check(a); try { return f(...a) } catch (e) { throw jsError(e) } } }[name]
}
