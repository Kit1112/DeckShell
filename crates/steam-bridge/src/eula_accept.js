(async () => {
    await SteamClient.Apps.MarkEulaAccepted({APPID}, {ID}, {VERSION});
    return true;
})()
