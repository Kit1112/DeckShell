(async () => {
    const id = {APPID};
    let overview;
    try { overview = window.appStore?.GetAppOverviewByAppID(id); } catch {}
    const category = overview?.steam_deck_compat_category;
    const result = {
        resolved_category: [0, 1, 2, 3].includes(category) ? category : null,
        resolved_items: null,
        steam_deck_blog_url: null
    };
    if (category === 3) return result;

    // Use the same transient AppDetails subscription as the stock install modal.
    const store = window.appDetailsStore;
    let subscription;
    let timer;
    try {
        const details = store?.GetAppDetails?.(id) ?? await new Promise(resolve => {
            if (!store?.RegisterForAppData) { resolve(null); return; }
            timer = setTimeout(() => resolve(null), 1800);
            subscription = store.RegisterForAppData(id, data => { if (data) resolve(data); });
        });
        if (Array.isArray(details?.vecDeckCompatTestResults)) {
            result.resolved_items = details.vecDeckCompatTestResults.map(item => ({
                display_type: item?.test_result,
                loc_token: item?.test_loc_token
            }));
        }
        if (details?.strSteamDeckBlogURL) {
            const url = new URL(details.strSteamDeckBlogURL);
            if (url.protocol === 'https:' && !url.username && !url.password) {
                result.steam_deck_blog_url = url.href;
            }
        }
    } catch {
        // Partial replies still carry the confirmed category, if Steam has it.
    } finally {
        clearTimeout(timer);
        subscription?.unregister();
    }
    return result;
})()
