(async () => {
    const id = {APPID};
    if (!Number.isSafeInteger(id) || id <= 0) throw Error('Invalid app');
    const o = appStore.GetAppOverviewByAppID(id);
    if (!o || typeof o.BIsAppBlocked !== 'function' || o.BIsAppBlocked()) throw Error('App unavailable');
    const w = SteamUIStore.ActiveWindowInstance;
    if (!w || (!w.IsMainGamepadUIWindow() && !w.IsDesktopUIWindow()) || !w.BrowserWindow?.SteamClient?.Window?.BringToFront) throw Error('Steam main window unavailable');
    const m = await SteamClient.Installs.GetInstallManagerInfo();
    // Native snapshots retain the last Complete/Canceled result after the modal closes.
    // Current Deck library.js: None=0, Complete=14, Failed=15, Canceled=16.
    if (!m || !Array.isArray(m.rgApps) || ![0, 14, 16].includes(m.eInstallState) || (m.eInstallState === 0 && m.rgApps.length)) throw Error('Another Steam install wizard is active');
    // No expected-app mutation, listener takeover, Continue, Cancel or EULA acceptance.
    // Direct OpenInstallWizard keeps Steam's normal storage/config decision.
    w.BrowserWindow.SteamClient.Window.BringToFront();
    const r = await SteamClient.Installs.OpenInstallWizard([id], "0");
    if (r !== undefined && r !== true) throw Error('Steam rejected install request: ' + JSON.stringify(r));
    return true;
})()
