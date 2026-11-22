import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../../app/src/actions.rs', import.meta.url), 'utf8');
const template = source.match(/const STEAM_YIELD_JS: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(template, 'test executes the actual production RPC');
function run(layer, { store = true, browser = true } = {}) {
  const calls = [];
  let menu = 1;
  const g = { MenuStore: { GetOpenSideMenu: () => menu } };
  if (browser) g.BrowserWindow = { SteamClient: { Window: {
    SetComposition: (...args) => calls.push(['composition', ...args]),
    HideWindow: () => { throw Error('unmap reintroduces retained overlay'); },
  } } };
  const s = {
    WindowStore: { GamepadUIMainWindowInstance: g },
    CloseSideMenus() { calls.push(['close']); menu = 0; },
    NavigateToRunningApp() { calls.push(['running']); },
  };
  const script = template.replaceAll('{LAYER}', JSON.stringify(layer))
    .replaceAll('{RESUME}', layer.length === 2 ? 'true' : 'false');
  const result = vm.runInNewContext(script, { window: { SteamUIStore: store ? s : undefined } });
  return { calls: JSON.parse(JSON.stringify(calls)), result };
}
test('HOME clears native composition/input without unmapping or changing native route', () => {
  const { calls, result } = run([233594385]);
  assert.deepEqual(calls, [['close'], ['composition', 0, [233594385], 0]]);
  assert.equal(result.menu_before, 1);
  assert.equal(result.menu_after, 0);
});
test('Play/Resume moves native UI to running route and selects game before shell fallback', () => {
  assert.deepEqual(run([42, 233594385]).calls,
    [['close'], ['running'], ['composition', 0, [42, 233594385], 0]]);
});
test('missing native store does not create or initialize one', () => {
  const { calls, result } = run([233594385], { store: false });
  assert.deepEqual(calls, []);
  assert.equal(result.store, false);
});
test('missing browser reports no composition request and still closes existing menus', () => {
  const { calls, result } = run([233594385], { browser: false });
  assert.deepEqual(calls, [['close']]);
  assert.equal(result.composition_requested, false);
});
