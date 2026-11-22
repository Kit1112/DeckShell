// Stock sp.js 24457/74373 reads clientSettings[key]. Copy each scalar explicitly:
// The hidden audio-source scalar is retained solely for the existing generic
// Steam setter/readback contract; it is not a page control or a required UI value.
// CDP returnByValue must not serialize MobX objects or touch unrelated stores.
(() => {
  const store = window.settingsStore;
  if (!store) return null;
  const sources = [];
  // A failing accessor must not discard the other nine settings.
  for (const field of ['clientSettings', 'm_ClientSettings']) {
    try { const source = store[field]; if (source && !sources.includes(source)) sources.push(source); } catch (_) {}
  }
  if (!sources.length) return null;
  const c = {};
  for (const key of ['gamerecording_background_mode', 'gamerecording_background_max_keep',
    'gamerecording_ic_seconds', 'gamerecording_video_bitrate', 'gamerecording_max_fps',
    'gamerecording_video_maxheight', 'gamerecording_background_a_m',
    'gamerecording_force_mic_mono', 'gamerecording_automatic_gain_control',
    'gamerecording_background_audio']) {
    c[key] = null;
    for (const source of sources) {
      try {
        const value = source[key];
        if (['boolean', 'number', 'string'].includes(typeof value)) { c[key] = value; break; }
      } catch (_) {}
    }
  }
  return {c};
})()
