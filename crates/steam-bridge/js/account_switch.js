(async () => {
  // Source: sp.js LoginStore (98638), cached sign-in (lt), constants 94601.
  // Deliberately never calls RestartAsUser/RestartToLoginScreen/FlipToLogin.
  const cmd = __COMMAND__;
  const S = window.loginStore, U = SteamClient.User, A = SteamClient.Auth;
  if (!S || !U || !A) throw new Error('Account API unavailable');
  const current = () => ({ account_name: S.accountName || '', state: S.loginState,
    result: S.loginResult, percentage: S.loginPercentage });
  const users = async () => (await U.GetLoginUsers()).map(u => {
    const avatar = String(u.avatarUrl || '');
    const id = /^\/avatarcache\/(\d{17})\.png$/.exec(avatar)?.[1] || '';
    return { account_name: u.accountName, persona_name: u.personaName || '', avatar_url: avatar,
      steamid: id, remembered: !!u.rememberPassword, has_pin: !!u.hasPin,
      current: S.loginState === 5 && u.accountName.toLowerCase() === (S.accountName || '').toLowerCase() };
  });
  if (cmd.kind === 'read') {
    const list = await users();
    const login = current();
    let steamid = null;
    try {
      const id = friendStore?.currentUserSteamID;
      if (login.state === 5 && id?.GetAccountID()) steamid = String(id.ConvertTo64BitString());
    } catch (_) { /* The Friends store is optional for login readiness. */ }
    return { users: list, ...login, steamid };
  }
  const R = window.__deckShellAccountSwitch ||= { operation: null };
  if (cmd.kind === 'start') {
    if (R.operation && ['starting', 'waiting', 'cancelling', 'unknown'].includes(R.operation.phase))
      return { ...R.operation, ...current() };
    const list = await users();
    // Recheck after await: two different CDP connections can enter this function.
    if (R.operation && ['starting', 'waiting', 'cancelling', 'unknown'].includes(R.operation.phase))
      return { ...R.operation, ...current() };
    const user = list.find(u => u.account_name === cmd.account);
    const op = R.operation = { id: cmd.id, target: cmd.account, phase: 'starting', auth_result: null, initial_result: S.loginResult, saw_loading: false };
    if (!user) { op.phase = 'missing'; return { ...op, ...current() }; }
    if (user.current) { op.phase = 'complete'; return { ...op, ...current() }; }
    if (user.has_pin) { op.phase = 'pin'; return { ...op, ...current() }; }
    if (!user.remembered) { op.phase = 'password'; return { ...op, ...current() }; }
    // Stock uses RestartAsUser for a live session. Cached sign-in is source-backed only
    // before login; do not pretend it can log out/switch an already Ready Steam client.
    if (S.loginState === 5) { op.phase = 'restart_required'; return { ...op, ...current() }; }
    if (![0, 1].includes(S.loginState)) { op.phase = 'unavailable'; return { ...op, ...current() }; }
    // Start once and observe separately: the native promise may outlive a CDP request.
    Promise.resolve().then(() => {
      if (S.loginState === 0) U.StartLogin();
      return A.StartSignInFromCache(user.account_name);
    }).then(reply => {
      op.auth_result = Number.isInteger(reply?.result) ? reply.result : null;
      // A void/malformed reply cannot establish that the write was rejected.
      op.phase = op.auth_result === null ? 'unknown' : op.auth_result !== 1 ? 'rejected' :
        op.cancel_requested ? 'unknown' : 'waiting';
    }, () => { op.phase = 'unknown'; });
    return { ...op, ...current() };
  }
  const op = R.operation;
  if (!op || op.id !== cmd.id) return { phase: 'unknown', ...current() };
  if ([2, 3, 4].includes(S.loginState)) op.saw_loading = true;
  if (['waiting', 'unknown'].includes(op.phase) && S.loginState === 1 && S.loginResult !== 1 &&
      (op.saw_loading || S.loginResult !== op.initial_result)) {
    op.auth_result = S.loginResult; op.phase = 'rejected';
  }
  // Success requires the requested identity AND Steam's Ready state (5), never just EResult=1.
  if (['waiting', 'unknown'].includes(op.phase) && op.auth_result === 1 && S.loginState === 5 &&
      (S.accountName || '').toLowerCase() === op.target.toLowerCase()) op.phase = 'complete';
  if (cmd.kind === 'cancel' && !op.cancel_requested && ['starting', 'waiting', 'unknown'].includes(op.phase)) {
    op.cancel_requested = true;
    op.phase = 'cancelling';
    // CancelLogin is exposed, but no acknowledgement contract was found. Its void
    // return is dispatch only: keep the duplicate-write guard until identity/Ready
    // or a native rejection resolves the outstanding login. Never replay cancel.
    try { await U.CancelLogin(); } catch (_) { /* outcome remains unknown */ }
    if (op.phase === 'cancelling') op.phase = 'unknown';
  }
  return { ...op, ...current() };
})()
