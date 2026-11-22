import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { test } from 'node:test';
import assert from 'node:assert/strict';
const script = readFileSync(new URL('../js/account_switch.js', import.meta.url), 'utf8');
function fixture(overrides = {}) {
  const calls = [];
  const users = Array.from({length: 12}, (_, i) => ({accountName:`account${i}`, personaName:`Persona ${i}`,
    avatarUrl:`/avatarcache/${76561198000000000n + BigInt(i)}.png`, rememberPassword:i!==10, hasPin:i===11}));
  const loginStore = { accountName:'account0', loginState:5, loginResult:1, loginPercentage:0 };
  const context = vm.createContext({ window:{loginStore}, SteamClient:{
    User: { GetLoginUsers:async()=>users, CancelLogin:async()=>{calls.push('cancel')},
      StartLogin:()=>{calls.push('start-login')},
      RestartAsUser:()=>{throw Error('Forbidden restart')}, RestartToLoginScreen:()=>{throw Error('Forbidden restart')} },
    Auth: { StartSignInFromCache:async name=>{calls.push(name);return {result:1}}, ...overrides },
  }});
  const run = command => vm.runInContext(script.replace('__COMMAND__', JSON.stringify(command)), context);
  const settle = () => new Promise(resolve=>setImmediate(resolve));
  return {calls, users, loginStore, context, run, settle};
}
test('reads every account, identity stays an exact decimal string, current highlighted', async()=>{
  const f=fixture(), s=await f.run({kind:'read'});
  assert.equal(s.users.length,12); assert.equal(s.users[9].steamid,'76561198000000009');
  assert.equal(s.users.filter(u=>u.current).length,1); assert.equal(f.calls.length,0);
  f.loginStore.loginState=1; assert.equal((await f.run({kind:'read'})).users.filter(u=>u.current).length,0);
});
test('reads the live SteamID only after login is ready', async () => {
  const f = fixture();
  f.context.friendStore = { currentUserSteamID: {
    GetAccountID: () => 1,
    ConvertTo64BitString: () => '76561198000000000',
  } };
  assert.equal((await f.run({ kind: 'read' })).steamid, '76561198000000000');
  f.loginStore.accountName = 'account9';
  f.loginStore.loginState = 4;
  assert.equal((await f.run({ kind: 'read' })).steamid, null);
  f.context.friendStore.currentUserSteamID.ConvertTo64BitString = () => '76561198000000009';
  f.loginStore.loginState = 5;
  const snapshot = await f.run({ kind: 'read' });
  assert.equal(snapshot.steamid, '76561198000000009');
  assert.equal(snapshot.users.find(user => user.current).persona_name, 'Persona 9');
});
test('cached success is not completion until target identity and Ready; duplicate suppressed', async()=>{
  const f=fixture(); f.loginStore.loginState=1; const command={kind:'start',id:'one',account:'account9'};
  await Promise.all([f.run(command),f.run({...command,id:'two'})]);await f.settle();
  assert.equal(f.calls.length,1);
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'waiting');
  f.loginStore.accountName='account9'; f.loginStore.loginState=4;
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'waiting');
  f.loginStore.loginState=5;
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'complete');
});
test('PIN, password and missing accounts never dispatch a write',async()=>{
  for(const [account,phase] of [['account10','password'],['account11','pin'],['missing','missing']]) {
    const f=fixture(); assert.equal((await f.run({kind:'start',id:'one',account})).phase,phase);assert.equal(f.calls.length,0);
  }
});
test('void cancellation dispatches once; late acceptance alone cannot unlock another write',async()=>{
  let resolve;const f=fixture({StartSignInFromCache:name=>{f.calls.push(name);return new Promise(r=>resolve=r)}});
  await (f.loginStore.loginState=1, f.run({kind:'start',id:'one',account:'account1'}));await f.settle();
  await f.run({kind:'cancel',id:'one'});await f.run({kind:'cancel',id:'one'});
  resolve({result:1});await f.settle();
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'unknown');
  assert.equal((await f.run({kind:'start',id:'two',account:'account2'})).id,'one');
  assert.deepEqual(f.calls,['account1','cancel']);
  f.loginStore.accountName='account1';f.loginStore.loginState=5;
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'complete');
});
test('rejections, malformed replies, and thrown errors do not imply success',async()=>{
  for (const reply of [{result:63},{result:5},undefined]) {
    const f=fixture({StartSignInFromCache:async()=>reply});await (f.loginStore.loginState=1, f.run({kind:'start',id:'one',account:'account1'}));await f.settle();
    assert.equal((await f.run({kind:'poll',id:'one'})).phase,reply ? 'rejected' : 'unknown');
  }
  const f=fixture({StartSignInFromCache:async()=>{throw Error('network')}});await (f.loginStore.loginState=1, f.run({kind:'start',id:'one',account:'account1'}));await f.settle();
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'unknown');
});
test('lost operation is unknown; name is serialized rather than executable',async()=>{
  const f=fixture(); assert.equal((await f.run({kind:'poll',id:'lost'})).phase,'unknown');
  assert.equal((await f.run({kind:'start',id:'one',account:'"; SteamClient.User.RestartAsUser(); //'})).phase,'missing');
  assert.equal(f.calls.length,0);
});
test('uncertain native failure stays blocked after unconfirmed native cancel',async()=>{
  const f=fixture({StartSignInFromCache:async()=>{f.calls.push('attempt');throw Error('lost')}});
  await (f.loginStore.loginState=1, f.run({kind:'start',id:'one',account:'account1'}));await f.settle();
  assert.equal((await f.run({kind:'start',id:'two',account:'account2'})).id,'one');
  assert.deepEqual(f.calls,['attempt']);
  assert.equal((await f.run({kind:'cancel',id:'one'})).phase,'unknown');
  assert.equal((await f.run({kind:'start',id:'two',account:'account2'})).id,'one');
});
test('cancel arriving after readiness does not cancel a completed login',async()=>{
  const f=fixture();await (f.loginStore.loginState=1, f.run({kind:'start',id:'one',account:'account1'}));await f.settle();
  f.loginStore.accountName='account1';f.loginStore.loginState=5;
  assert.equal((await f.run({kind:'cancel',id:'one'})).phase,'complete');
  assert.deepEqual(f.calls,['account1']);
});

test('a logged-in session cannot be passed off as a working hot-switch',async()=>{
  const f=fixture();
  assert.equal((await f.run({kind:'start',id:'one',account:'account1'})).phase,'restart_required');
  assert.equal(f.calls.length,0);
});
test('new cached login does not interrupt another native login or shutdown',async()=>{
  for(const state of [2,3,4,6]) {
    const f=fixture();f.loginStore.loginState=state;
    assert.equal((await f.run({kind:'start',id:'one',account:'account1'})).phase,'unavailable');
    assert.equal(f.calls.length,0);
  }
});
test('later login-state Guard rejection is surfaced after cached API accepts',async()=>{
  const f=fixture();f.loginStore.loginState=1;
  await f.run({kind:'start',id:'one',account:'account1'});await f.settle();
  f.loginStore.loginState=3;await f.run({kind:'poll',id:'one'});
  f.loginStore.loginState=1;f.loginStore.loginResult=63;
  const status=await f.run({kind:'poll',id:'one'});
  assert.equal(status.phase,'rejected');assert.equal(status.auth_result,63);
});

test('cancel while cache promise is pending retains its guard even if CancelLogin returns',async()=>{
  let resolve;const f=fixture({StartSignInFromCache:()=>new Promise(r=>resolve=r)});
  f.loginStore.loginState=1;
  await f.run({kind:'start',id:'one',account:'account1'});await f.settle();
  await f.run({kind:'cancel',id:'one'});
  assert.equal((await f.run({kind:'start',id:'two',account:'account2'})).id,'one');
  resolve({result:5});await f.settle();
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'rejected');
});
test('throwing native StartLogin is unknown rather than permanently starting',async()=>{
  const f=fixture();f.loginStore.loginState=0;
  f.context.SteamClient.User.StartLogin=()=>{throw Error('native failure')};
  await f.run({kind:'start',id:'one',account:'account1'});await f.settle();
  assert.equal((await f.run({kind:'poll',id:'one'})).phase,'unknown');
  assert.deepEqual(f.calls,[]);
});
test('failed cancellation is never replayed and does not unlock another login',async()=>{
  const f=fixture();f.loginStore.loginState=1;
  f.context.SteamClient.User.CancelLogin=()=>{f.calls.push('cancel');throw Error('lost')};
  await f.run({kind:'start',id:'one',account:'account1'});await f.settle();
  await f.run({kind:'cancel',id:'one'});await f.run({kind:'cancel',id:'one'});
  assert.equal((await f.run({kind:'start',id:'two',account:'account2'})).id,'one');
  assert.deepEqual(f.calls,['account1','cancel']);
});
test('current account succeeds without write, including accounts protected by PIN',async()=>{
  const f=fixture();f.users[0].hasPin=true;
  assert.equal((await f.run({kind:'start',id:'one',account:'account0'})).phase,'complete');
  assert.deepEqual(f.calls,[]);
});
