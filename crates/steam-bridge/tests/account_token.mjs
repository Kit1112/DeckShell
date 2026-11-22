import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';
const script = readFileSync(new URL('../js/account_token.js', import.meta.url), 'utf8');
function fixture(state = 1, reply = { result: 1 }) {
  const calls = [];
  const loginStore = { accountName: '', loginState: state };
  const context = vm.createContext({ window: { loginStore, settingsStore: { m_ClientSettings: { always_show_user_chooser: true } } }, SteamClient: {
    User: { StartLogin: () => calls.push('start-login'),
      RestartAsUser: () => { throw Error('Forbidden restart'); }, RestartToLoginScreen: () => { throw Error('Forbidden restart'); } },
    Auth: { SetLoginToken: async (t, a) => { calls.push(['token', t, a]); return reply; },
      SetSteamGuardData: async (a, g) => { calls.push(['guard', a, g]); },
      GetSteamGuardData: async a => ({ eresult: 1, data: 'GD-' + a }), GetLocalHostname: async () => 'steamdeck' },
    System: { GetOSType: async () => -203, GetSystemInfo: async () => ({ eGamingDeviceType: 544 }) },
  } });
  const run = c => vm.runInContext(script.replace('__COMMAND__', JSON.stringify(c)), context);
  return { calls, loginStore, run };
}
test('device info is read-only and includes the chooser setting', async () => {
  const f = fixture(5), d = await f.run({ kind: 'device' });
  assert.deepEqual({ ...d }, { os_type: -203, gaming_device_type: 544, hostname: 'steamdeck', always_show_user_chooser: true });
  assert.equal(f.calls.length, 0);
});
test('token login before sign-in calls SetLoginToken then guard data', async () => {
  const f = fixture(1), r = await f.run({ kind: 'token', account: 'fixture', refresh: 'R', guard: 'G' });
  assert.equal(r.phase, 'accepted');
  assert.deepEqual(f.calls, [['token', 'R', 'fixture'], ['guard', 'fixture', 'G']]);
});
test('state 0 starts the login UI first (stock StartLogin), empty guard is not written', async () => {
  const f = fixture(0); await f.run({ kind: 'token', account: 'fixture', refresh: 'R', guard: '' });
  assert.deepEqual(f.calls, ['start-login', ['token', 'R', 'fixture']]);
});
test('Ready session never receives a token and never restarts', async () => {
  const f = fixture(5), r = await f.run({ kind: 'token', account: 'fixture', refresh: 'R', guard: 'G' });
  assert.equal(r.phase, 'restart_required'); assert.equal(f.calls.length, 0);
});
test('rejected and malformed replies are distinct', async () => {
  assert.equal((await fixture(1, { result: 5 }).run({ kind: 'token', account: 'a', refresh: 'R', guard: '' })).phase, 'rejected');
  assert.equal((await fixture(1, null).run({ kind: 'token', account: 'a', refresh: 'R', guard: '' })).phase, 'unknown');
});
test('guard data read returns stored data only on OK', async () => {
  assert.equal((await fixture(5).run({ kind: 'guard', account: 'x' })).data, 'GD-x');
});
