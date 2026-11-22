// Read-only snapshot of settingsStore for the settings pages (task 497, checked against the Deck 2026-11-19).
// CDP returnByValue turns a MobX observable into {} (m_ClientSettings arrived with 0 keys, so every power row was grey):
// scalars are copied one by one. window.SystemPowerStore / SystemStoragStore do not exist until the stock UI creates them,
// so the idle dim seconds come from the first RegisterForSettingsChanges message (CMsgSystemSettings fields 1 and 2, float),
// read once and unregistered. GetBatteryPreferences() is an observable: the value is `.Value`, JSON.stringify gives the wrapper.
(() => {
  const S = window.settingsStore;
  if (!S) return null;
  const scalars = (o) => {
    const r = {};
    if (!o) return r;
    try { for (const k of Object.keys(o)) { const v = o[k]; if (v === null || ['string', 'number', 'boolean'].includes(typeof v)) r[k] = v; } } catch (_) {}
    return r;
  };
  const s = {};
  try { for (const [k, v] of Object.entries(S.settings || {})) if (typeof v !== 'object') s[k] = v; } catch (_) {}
  let pref = null;
  try { const p = S.GetBatteryPreferences(); pref = JSON.stringify((p && p.Value !== undefined) ? p.Value : p); } catch (_) {}
  const P = window.SystemPowerStore;
  const power = { battery: P ? !!P.m_bHasBattery : null, dim_battery: null, dim_ac: null, pref };
  const dim = new Promise((resolve) => {
    let un = null, done = false;
    const fin = () => { if (done) return; done = true; try { un && un.unregister && un.unregister(); } catch (_) {} resolve(); };
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
  return dim.then(() => ({ c: scalars(S.m_ClientSettings), f: scalars(S.m_FriendSettings), s,
    w: !!(window.SystemNetworkStore && window.SystemNetworkStore.isWifiEnabled), tz: S.m_strTimeZoneID, power }));
})()
