//! Event diagnostics: no per-frame logging. Enabled by default in a shell session;
//! DECKSHELL_DIAG=0 disables it, DECKSHELL_DIAG=1 enables it elsewhere.
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("DECKSHELL_DIAG")
        .map(|s| !s.is_empty() && s != "0")
        .unwrap_or_else(|_| std::env::var_os("DECKSHELL_SESSION_MODE").is_some()))
}

pub fn log(event: &str, fields: serde_json::Value) {
    if !enabled() { return; }
    static START: OnceLock<Instant> = OnceLock::new();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let record = serde_json::json!({
        "event": event, "pid": std::process::id(), "seq": SEQ.fetch_add(1, Ordering::Relaxed),
        "ts_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        "elapsed_ms": START.get_or_init(Instant::now).elapsed().as_millis(), "data": fields
    });
    eprintln!("[diag] {record}");
}

/// Independent read-only connection, so observing Steam cannot hold up action/friends requests
/// on the application's bridge mutex. Never creates windows or initializes stores.
pub fn spawn_steam_observer() {
    if !enabled() { return; }
    std::thread::Builder::new().name("steam-observer".into()).spawn(|| {
        let b = steam_bridge::Bridge::lazy();
        let mut previous = None;
        loop {
            let started = Instant::now();
            let value = b.eval_json(r#"(()=>{const f=window.friendStore,s=window.SteamUIStore,w=s?.WindowStore;const rows=f?.allFriends;const d=i=>i?{type:i.WindowType,browser:!!i.BrowserWindow,menu:i.MenuStore?.GetOpenSideMenu?.(),suppression:i.MenuStore?.m_cSuppressRequests}:null;return {friend_store:!!f,friends:Array.isArray(rows)?{count:rows.length,online:rows.filter(x=>x.m_persona?.m_ePersonaState).length,avatar_hashes:rows.filter(x=>!!x.m_persona?.m_strAvatarHash).length}:null,steam_ui_store:!!s,main:d(w?.MainWindowInstance),gamepad:d(w?.GamepadUIMainWindowInstance),main_visible:s?.MainWindowVisible?.Value??s?.MainWindowVisible?.m_currentValue}})()"#);
            match value {
                Ok(value) => {
                    if previous.as_ref() != Some(&value) {
                        log("steam.observe", serde_json::json!({"ms":started.elapsed().as_millis(),"previous":previous,"current":value}));
                        previous = Some(value);
                    }
                }
                Err(e) => log("steam.observe.error", serde_json::json!({"ms":started.elapsed().as_millis(),"error":e.to_string()})),
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
    }).ok();
}
