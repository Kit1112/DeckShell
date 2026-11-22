// Read-only call used by Steam's Game Recording settings page. Unknown or ambiguous
// module/service replies must not be mistaken for capability support.
(async () => {
  try {
    let req;
    window.webpackChunksteamui.push([[Symbol()], {}, r => { req = r; }]);
    const ids = Object.keys(req.m).filter(id => req.m[id].toString().includes('GameRecording.GetPlatformCapabilities#1'));
    if (ids.length !== 1) return null;
    const exports = req(ids[0]);
    const services = Object.values(exports).filter(v => v && typeof v.GetPlatformCapabilities === 'function');
    if (services.length !== 1) return null;
    const reply = await services[0].GetPlatformCapabilities({});
    if (reply?.GetEResult?.() !== 1) return null;
    const value = reply.Body()?.per_process_audio_capture?.();
    return typeof value === 'boolean' ? value : null;
  } catch (_) { return null; }
})()
