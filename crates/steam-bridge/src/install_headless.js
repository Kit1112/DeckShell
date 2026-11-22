(async () => {
    const id = {APPID}, folder = {FOLDER};
    const api = SteamClient.Installs;
    const before = await api.GetInstallManagerInfo();
    if (!before || ![0, 14, 16].includes(before.eInstallState)) throw new Error('Install manager busy or unavailable');
    let require;
    window.webpackChunksteamui.push([[Symbol()], {}, r => { require = r; }]);
    const store = require(65691)?.I;
    // Fail before opening if this client no longer exposes the researched contract.
    if (!store?.SetInstallFolderIndexAndStartInstall) throw new Error('Install action store unavailable');
    store.m_unAppIDExpectedInstall = id;
    await api.OpenInstallWizard([id], '0');
    const deadline = Date.now() + {TIMEOUT};
    while (Date.now() < deadline) {
        const info = await api.GetInstallManagerInfo();
        if ([7, 9, 14].includes(info?.eInstallState) &&
            (!Array.isArray(info.rgApps) || info.rgApps.length !== 1 || info.rgApps[0].nAppID !== id)) throw new Error('Foreign install request');
        if (info?.eInstallState === 7) {
            await store.SetInstallFolderIndexAndStartInstall(folder);
            return true;
        }
        if ([9, 14].includes(info?.eInstallState)) {
            if (info.iInstallFolder !== folder) throw new Error('Automatic install target differs; inspect Downloads');
            return true;
        }
        if (info?.eInstallState === 16) throw new Error('Install cancelled');
        await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error('Install timeout; inspect Downloads before retry');
})()
