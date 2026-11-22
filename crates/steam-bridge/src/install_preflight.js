(async () => {
    const id = {APPID};
    const folders = await SteamClient.InstallFolder.GetInstallFolders();
    if (!Array.isArray(folders)) throw new Error('native folder list unavailable');
    let require;
    window.webpackChunksteamui.push([[Symbol()], {}, r => { require = r; }]);
    const store = require(32558)?.H;
    let details = store?.GetAppDetails(id);
    if (!details?.lDiskSpaceRequiredBytes) {
        await store?.RequestAppDetails(id);
        details = store?.GetAppDetails(id);
    }
    let eulas;
    try {
        eulas = await SteamClient.Apps.LoadEula(id);
        if (!Array.isArray(eulas)) throw new Error('Invalid EULA response');
    } catch (error) {
        if (error?.result !== 42) throw error;
        eulas = [];
    }
    const bytes = v => Number.isSafeInteger(v) && v >= 0 ? v : null;
    return {schema: 2, requiredBytes: bytes(details?.lDiskSpaceRequiredBytes),
        folders: folders.filter(f => f.bIsMounted === true).map(f => ({
            nFolderIndex: bytes(f.nFolderIndex), strDriveName: f.strDriveName,
            strUserLabel: f.strUserLabel, bIsMounted: true,
            bIsDefaultFolder: typeof f.bIsDefaultFolder === 'boolean' ? f.bIsDefaultFolder : null,
            nFreeSpace: bytes(f.nFreeSpace)
        })), eulas};
})()
