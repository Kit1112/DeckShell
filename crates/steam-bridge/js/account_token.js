(async () => {
  // Вход по токену после QR / пароля — как стоковый экран входа (sp.js 414798, 535708):
  // SteamClient.Auth.SetLoginToken(refresh, account) → {result, message}; затем SetSteamGuardData(account, guard).
  // Только до входа (loginState 0/1): в Ready (5) сток меняет аккаунт перезапуском (RestartAsUser /
  // RestartToLoginScreen) — это поток смены, здесь не вызывается. Чтение устройства — только read-only методы.
  const cmd = __COMMAND__;
  const S = window.loginStore, A = SteamClient.Auth, Sys = SteamClient.System;
  if (cmd.kind === 'device') {
    const [os, info, host] = await Promise.all([
      Sys?.GetOSType?.().catch(() => null), Sys?.GetSystemInfo?.().catch(() => null),
      A?.GetLocalHostname?.().catch(() => '') ?? '']);
    return { os_type: Number.isInteger(os) ? os : null,
      gaming_device_type: Number.isInteger(info?.eGamingDeviceType) ? info.eGamingDeviceType : null,
      hostname: typeof host === 'string' ? host : '',
      always_show_user_chooser: !!window.settingsStore?.m_ClientSettings?.always_show_user_chooser };
  }
  if (cmd.kind === 'guard') {
    // Сток: m_onGetMachineAuth = SteamClient.Auth.GetSteamGuardData(account) → {eresult, data}.
    const r = await A?.GetSteamGuardData?.(cmd.account).catch(() => null);
    return { data: r && r.eresult === 1 && typeof r.data === 'string' ? r.data : '' };
  }
  if (!S || !A) throw new Error('Account API unavailable');
  const state = S.loginState;
  if (cmd.kind === 'token') {
    if (![0, 1].includes(state)) return { phase: 'restart_required', state };
    const R = window.__deckShellTokenLogin ||= { busy: false };
    if (R.busy) return { phase: 'busy', state };
    R.busy = true;
    try {
      if (state === 0) SteamClient.User.StartLogin?.();
      const reply = await A.SetLoginToken(cmd.refresh, cmd.account);
      if (cmd.guard) await A.SetSteamGuardData(cmd.account, cmd.guard);
      const result = Number.isInteger(reply?.result) ? reply.result : null;
      return { phase: result === 1 ? 'accepted' : result === null ? 'unknown' : 'rejected', result, state: S.loginState };
    } catch (_) {
      return { phase: 'unknown', state: S.loginState };
    } finally { R.busy = false; }
  }
  return { phase: 'unknown', state };
})()
