// Read-only: the six idle timers of Настройки → Питание (task 518), same shape as settings_snapshot.js (`c`, `power`).
// Client settings are scalar properties of settingsStore.m_ClientSettings (an observable: read key by key, never whole);
// the dim seconds are the first RegisterForSettingsChanges message (CMsgSystemSettings fields 1, 2, float), unregistered at once.
(() => {
  const S = window.settingsStore;
  if (!S) return null;
  const c = {};
  for (const k of ['system_idle_suspend_battery_sec', 'system_idle_suspend_ac_sec', 'system_idle_screensaver_battery_sec', 'system_idle_screensaver_ac_sec']) {
    try { const v = S.m_ClientSettings[k]; if (typeof v === 'number') c[k] = v; } catch (_) {}
  }
  const power = { dim_battery: null, dim_ac: null };
  return new Promise((resolve) => {
    let un = null, done = false;
    const fin = () => { if (done) return; done = true; try { un && un.unregister && un.unregister(); } catch (_) {} resolve({ c, power }); };
    try {
      un = SteamClient.System.RegisterForSettingsChanges((m) => {
        try {
          const b = new Uint8Array(m instanceof ArrayBuffer ? m : m.buffer || m), dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
          let i = 0;
          const vi = () => { let r = 0, sh = 0, x; do { x = b[i++]; r += (x & 127) * 2 ** sh; sh += 7; } while (x & 128); return r; };
          while (i < b.length) {
            const tag = vi(), f = tag >> 3, w = tag & 7;
            if (w === 5) { const v = dv.getFloat32(i, true); i += 4; if (f === 1) power.dim_battery = v; if (f === 2) power.dim_ac = v; }
            else if (w === 0) vi(); else if (w === 1) i += 8; else if (w === 2) i += vi(); else break;
            if (power.dim_battery !== null && power.dim_ac !== null) break;
          }
        } catch (_) {}
        fin();
      });
    } catch (_) { fin(); }
    setTimeout(fin, 700);
  });
})()
