//! DeckShell backend: talks to Steam's own UI context (CEF "SharedJSContext" over the DevTools
//! websocket on 127.0.0.1:8080) and reads the same stores / calls the same `SteamClient` methods
//! as the native Steam UI. Blocking, thread-safe (`Bridge` is `Clone`), auto-reconnecting, all
//! calls bounded by 2 s. Every call returns `Err` when Steam is absent so callers can fall back.
pub mod account_restart;
pub mod account_switch;
pub mod account_token;
pub mod art;
pub mod controller_config;
pub mod controller_edit;
pub mod controller_layout;
pub mod controller_list;
pub mod controller_steam_input;
pub mod error;
pub mod js;
pub mod install;
pub mod model;
pub mod recording;
pub mod send_to_phone;
mod transport;

pub use art::{AppArtUrls, ArtKind, ArtResolver, ArtSet};
pub use error::{Error, Result};
pub use model::*;
pub use serde_json::Value;

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use transport::Conn;

fn bridge_diag(event: &str, fields: Value) {
    let enabled = match std::env::var("DECKSHELL_DIAG").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => std::env::var_os("DECKSHELL_SESSION_MODE").is_some(),
    };
    if !enabled { return; }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    eprintln!("[bridge-diag] {{\"ts_unix_ms\":{ts},\"pid\":{},\"event\":{},\"fields\":{}}}", std::process::id(), serde_json::json!(event), fields);
}

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeEvent {
    /// (Re)connected to Steam; consumers should reload everything.
    Connected,
    Disconnected,
    LibraryChanged,
    DownloadsChanged,
    NotificationsChanged,
    /// Currently running app ids (empty = nothing running).
    RunningAppChanged(Vec<u32>),
}

struct Shared {
    port: u16,
    conn: Mutex<Option<Conn>>,
}

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Shared>,
}

impl Bridge {
    /// Connect now; `Err(Unavailable)` if Steam / the CEF port is absent.
    pub fn connect() -> Result<Bridge> {
        let b = Bridge::lazy();
        let c = Conn::open(b.inner.port)?;
        let lock_started = std::time::Instant::now();
        *b.inner.conn.lock().unwrap() = Some(c);
        bridge_diag("bridge_mutex_wait", serde_json::json!({"wait_ms":lock_started.elapsed().as_millis(),"operation":"connect"}));
        Ok(b)
    }

    /// A bridge that connects on first use (and reconnects whenever needed).
    pub fn lazy() -> Bridge {
        Bridge { inner: Arc::new(Shared { port: transport::port(), conn: Mutex::new(None) }) }
    }

    pub fn is_connected(&self) -> bool {
        self.inner.conn.lock().unwrap().is_some()
    }

    /// Evaluate a JS expression in SharedJSContext (returnByValue + awaitPromise).
    /// Retries once over a fresh connection if the old one died (Steam restart).
    pub fn eval_json(&self, expr: &str) -> Result<Value> {
        let lock_started = std::time::Instant::now();
        let mut g = self.inner.conn.lock().unwrap();
        let lock_wait = lock_started.elapsed();
        if lock_wait > Duration::from_millis(25) {
            bridge_diag("bridge_mutex_wait", serde_json::json!({"wait_ms":lock_wait.as_millis(),"operation":"eval"}));
        }
        for attempt in 0..2 {
            if g.is_none() {
                *g = Some(Conn::open(self.inner.port)?);
            }
            match g.as_mut().unwrap().eval(expr) {
                Ok(v) => return Ok(v),
                Err(e @ Error::Js(_)) | Err(e @ Error::Protocol(_)) => return Err(e),
                Err(e) => {
                    *g = None; // drop broken / desynced connection
                    if attempt == 1 || matches!(e, Error::Timeout) {
                        return Err(e);
                    }
                }
            }
        }
        Err(Error::Timeout)
    }

    /// Evaluate once without replaying after an error. A failed call may have executed in Steam;
    /// callers must treat its outcome as unknown. The next call opens a fresh connection.
    pub fn eval_json_once(&self, expr: &str) -> Result<Value> {
        let mut g = self.inner.conn.lock().unwrap();
        if g.is_none() {
            *g = Some(Conn::open(self.inner.port)?);
        }
        let result = g.as_mut().unwrap().eval(expr);
        if result.is_err() {
            *g = None;
        }
        result
    }

    /// Evaluate once on a connection of its own, with the given reply deadline. For a call that may run for
    /// minutes: it must not hold the shared connection (every other call would wait behind it). Not replayed.
    pub(crate) fn eval_own(&self, expr: &str, timeout: Duration) -> Result<Value> {
        let mut c = transport::Conn::open(self.inner.port)?;
        c.eval_for(expr, timeout)
    }

    // ---- Library -------------------------------------------------------------------------
    pub fn apps(&self) -> Result<Vec<App>> {
        parse_apps(&self.eval_json(js::APPS)?)
    }
    pub fn collections(&self) -> Result<Vec<Collection>> {
        parse_collections(&self.eval_json(js::COLLECTIONS)?)
    }
    /// App ids ordered by recent activity.
    pub fn recent_apps(&self) -> Result<Vec<u32>> {
        parse_ids(&self.eval_json(js::RECENT)?)
    }
    /// Detailed per-app info (null-safe: `Value::Null` if Steam did not answer in 1.2 s).
    pub fn app_details(&self, appid: u32) -> Result<Value> {
        self.eval_json(&js::APP_DETAILS.replace("{APPID}", &appid.to_string()))
    }
    pub fn running_apps(&self) -> Result<Vec<u32>> {
        parse_ids(&self.eval_json(js::RUNNING)?)
    }

    /// Scale position 0..1 of Steam's display callback; `None` until Steam has reported one.
    pub fn brightness(&self) -> Result<Option<f32>> {
        Ok(self.eval_json(js::BRIGHTNESS_READ)?.as_f64().map(|v| v as f32))
    }
    /// Sets the scale position through Steam's own display API (the stock writer).
    pub fn set_brightness(&self, v: f32) -> Result<()> {
        self.eval_json(&js::brightness_set(v)).map(|_| ())
    }

    // ---- Downloads / notifications / system -----------------------------------------------
    pub fn downloads(&self) -> Result<Downloads> {
        parse_downloads(&self.eval_json(js::DOWNLOADS)?)
    }
    pub fn download_settings(&self) -> Result<DownloadSettings> {
        parse_download_settings(&self.eval_json(js::DOWNLOAD_SETTINGS)?)
    }
    pub fn notifications(&self) -> Result<Notifications> {
        parse_notifications(&self.eval_json(js::NOTIFICATIONS)?)
    }
    pub fn system(&self) -> Result<SystemInfo> {
        parse_system(&self.eval_json(js::SYSTEM)?)
    }
    pub fn audio_devices(&self) -> Result<Vec<AudioDevice>> {
        parse_audio(&self.eval_json(js::AUDIO)?)
    }

    // ---- Actions (change Steam state) -------------------------------------------------------
    fn act(&self, tpl: &str, subs: &[(&str, String)]) -> Result<bool> {
        let mut e = tpl.to_string();
        for (k, v) in subs {
            e = e.replace(k, v);
        }
        Ok(self.eval_json(&e)?.as_bool().unwrap_or(true))
    }
    fn act_app(&self, tpl: &str, appid: u32) -> Result<bool> {
        self.act(tpl, &[("{APPID}", appid.to_string())])
    }
    /// Play (native Play button). Returns false if Steam considers the app already launching.
    pub fn launch(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_LAUNCH, appid)
    }
    pub fn stop(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_STOP, appid)
    }
    pub fn cancel_launch(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_CANCEL_LAUNCH, appid)
    }
    /// Hand off to Steam's complete wizard. Never continue/cancel/accept from DeckShell.
    /// True means dispatch only. A lost reply must never replay the open request.
    pub fn install_stock(&self, appid: u32) -> Result<bool> {
        Ok(self.eval_json_once(&js::ACTION_INSTALL_STOCK.replace("{APPID}", &appid.to_string()))? == Value::Bool(true))
    }
    pub fn install(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_INSTALL, appid)
    }
    pub fn install_continue(&self) -> Result<bool> {
        self.act(js::ACTION_INSTALL_CONTINUE, &[])
    }
    pub fn install_cancel(&self) -> Result<bool> {
        self.act(js::ACTION_INSTALL_CANCEL, &[])
    }
    /// Caller must obtain explicit user confirmation for this app first.
    /// True acknowledges dispatch only; never replay after an ambiguous reply.
    pub fn uninstall(&self, appid: u32) -> Result<bool> {
        // Strict result: generic act() treats a missing/non-boolean result as true.
        Ok(self.eval_json_once(&js::action_uninstall(appid))? == Value::Bool(true))
    }
    pub fn uninstall_installed(&self, appid: u32) -> Result<Option<bool>> {
        Ok(self.eval_json(&js::UNINSTALL_STATE.replace("{APPID}", &appid.to_string()))?.as_bool())
    }
    pub fn pause_downloads(&self) -> Result<bool> {
        self.act(js::ACTION_PAUSE_ALL, &[])
    }
    pub fn resume_downloads(&self) -> Result<bool> {
        self.act(js::ACTION_RESUME_ALL, &[])
    }
    pub fn pause_download(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_PAUSE_APP, appid)
    }
    pub fn resume_download(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_RESUME_APP, appid)
    }
    pub fn queue_download(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_QUEUE_APP, appid)
    }
    pub fn remove_download(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_REMOVE_DOWNLOAD, appid)
    }
    pub fn open_store(&self, appid: u32) -> Result<bool> {
        self.act_app(js::ACTION_OPEN_STORE, appid)
    }
    /// Set output volume (0..1) of an audio device (see `audio_devices`).
    pub fn set_volume(&self, device_id: i64, volume: f64) -> Result<bool> {
        self.act(js::ACTION_SET_VOLUME, &[("{DEV}", device_id.to_string()), ("{VOL}", volume.clamp(0.0, 1.0).to_string())])
    }
    pub fn set_wifi(&self, on: bool) -> Result<bool> {
        self.act(js::ACTION_SET_WIFI, &[("{ON}", on.to_string())])
    }
    pub fn notifications_viewed(&self) -> Result<bool> {
        self.act(js::ACTION_NOTIFICATIONS_VIEWED, &[])
    }
    pub fn shutdown(&self) -> Result<bool> {
        self.act(js::ACTION_SHUTDOWN, &[])
    }
    pub fn restart(&self) -> Result<bool> {
        self.act(js::ACTION_RESTART, &[])
    }
    pub fn suspend(&self) -> Result<bool> {
        self.act(js::ACTION_SUSPEND, &[])
    }

    // ---- QAM: Performance / Quick settings / chat / notifications ------------------------------------
    fn tpl(&self, tpl: &str, subs: &[(&str, String)]) -> Result<bool> {
        let mut e = tpl.replace("@W@", js::WEBPACK_W);
        for (k, v) in subs {
            e = e.replace(k, v);
        }
        Ok(self.eval_json(&e)?.as_bool().unwrap_or(true))
    }
    /// SystemPerfStore method (`SetFPSLimit`, `SetDisplayRefreshRateManualHz`, ...) with a JS argument list. Ok(false) = no Perf channel
    /// (not the gamescope session). `method` must be a plain identifier.
    pub fn perf_call(&self, method: &str, args: &str) -> Result<bool> {
        if method.is_empty() || !method.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Protocol(format!("bad perf method {method}")));
        }
        self.tpl(js::PERF_CALL, &[("@METHOD@", method.to_string()), ("@ARGS@", args.to_string())])
    }
    /// Whole Performance + Quick settings state (see `js::PERF_READ`).
    pub fn perf_state(&self) -> Result<Value> {
        self.eval_json(&js::PERF_READ.replace("@W@", js::WEBPACK_W))
    }
    /// SteamOSManager charge-limit range and CEC wake support (see `js::POWER_LIMITS`); `Value::Null` when unavailable.
    pub fn power_limits(&self) -> Result<Value> {
        self.eval_json(&js::POWER_LIMITS.replace("@W@", js::WEBPACK_W))
    }
    /// settingsStore client setting (`steamos_tdp_limit`, `steamos_manual_gpu_clock_hz`, ...). `value_json` is a JSON literal.
    pub fn set_client_setting(&self, name: &str, value_json: &str) -> Result<bool> {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Protocol(format!("bad setting {name}")));
        }
        self.tpl(js::SET_CLIENT_SETTING, &[("@NAME@", format!("\"{name}\"")), ("@VALUE@", value_json.to_string())])
    }
    /// Persist both GPU fields in one native settings update, avoiding a transient
    /// manual mode at the previously stored frequency.
    pub fn set_manual_gpu_clock(&self, on: bool, mhz: u32) -> Result<bool> {
        self.tpl(js::SET_MANUAL_GPU_CLOCK, &[("@ON@", on.to_string()), ("@MHZ@", mhz.to_string())])
    }
    pub fn set_airplane(&self, on: bool) -> Result<bool> {
        self.tpl(js::SET_AIRPLANE, &[("{ON}", on.to_string())])
    }
    pub fn set_night_mode(&self, on: bool) -> Result<bool> {
        self.tpl(js::SET_NIGHT_MODE, &[("{ON}", on.to_string())])
    }
    pub fn night_mode_settings(&self) -> Result<Value> {
        self.eval_json(&js::NIGHT_MODE_SETTINGS.replace("@W@", js::WEBPACK_W))
    }
    pub fn set_ui_auto_scale(&self, on: bool) -> Result<bool> {
        self.tpl(js::SET_UI_AUTO_SCALE, &[("{ON}", on.to_string())])
    }
    pub fn set_ui_scale_factor(&self, f: f64) -> Result<bool> {
        self.tpl(js::SET_UI_SCALE_FACTOR, &[("{VAL}", f.to_string())])
    }
    pub fn friend_open_chat(&self, accountid: u32) -> Result<bool> {
        self.tpl(js::FRIEND_OPEN_CHAT, &[("{ACC}", accountid.to_string())])
    }
    pub fn open_chat_group(&self, group_id: &str) -> Result<bool> {
        self.tpl(js::OPEN_CHAT_GROUP, &[("{GID}", serde_json::to_string(group_id).unwrap_or_else(|_| "\"\"".into()))])
    }
    pub fn friends_navigate_to_invites(&self) -> Result<bool> {
        self.tpl(js::FRIENDS_NAVIGATE_TO_INVITES, &[])
    }
    pub fn friends_create_group(&self) -> Result<bool> {
        self.tpl(js::FRIENDS_CREATE_GROUP, &[])
    }
    pub fn friend_profile(&self, accountid: u32) -> Result<bool> {
        self.tpl(js::FRIEND_PROFILE, &[("{ACC}", accountid.to_string())])
    }
    pub fn notification_dismiss(&self, notification_id: u64) -> Result<bool> {
        self.tpl(js::NOTIFICATION_DISMISS, &[("{NID}", notification_id.to_string())])
    }
    pub fn open_account(&self) -> Result<bool> {
        self.tpl(js::OPEN_ACCOUNT, &[])
    }
    pub fn open_store_search(&self, term: &str) -> Result<bool> {
        let t = serde_json::to_string(term).unwrap_or_else(|_| "\"\"".into());
        self.tpl(js::OPEN_STORE_SEARCH, &[("{TERM}", t)])
    }

    // ---- Events ------------------------------------------------------------------------------
    /// Spawn a poller thread (own connection, ~500 ms cadence) and return its event channel.
    /// Emits `Connected` first and after recoverable reconnects. After `Disconnected` it ends;
    /// callers that need to wait for Steam to return must create a new subscription.
    pub fn subscribe(&self) -> mpsc::Receiver<BridgeEvent> {
        self.subscribe_every(Duration::from_millis(500))
    }

    pub fn subscribe_every(&self, period: Duration) -> mpsc::Receiver<BridgeEvent> {
        let (tx, rx) = mpsc::channel();
        // A subscription owns its connection but retains this bridge's endpoint.
        let poller = Bridge { inner: Arc::new(Shared { port: self.inner.port, conn: Mutex::new(None) }) };
        std::thread::Builder::new()
            .name("steam-bridge-poll".into())
            .spawn(move || {
                let mut last: Option<Value> = None;
                // a single slow/timed-out poll (Steam busy, e.g. launching the store) is NOT a disconnect: only report
                // Disconnected after several consecutive failures, otherwise the app tears down to the steam-data
                // fallback and reloads everything (test 3: frequent "bridge disconnected", empty friends).
                let mut fails = 0u32;
                loop {
                    match poller.eval_json(js::FINGERPRINT) {
                        Ok(fp) => {
                            fails = 0;
                            let evs = match &last {
                                None => vec![BridgeEvent::Connected],
                                Some(old) => diff_fingerprint(old, &fp),
                            };
                            last = Some(fp);
                            for e in evs {
                                if e == BridgeEvent::Connected {
                                    bridge_diag("poll_connected", serde_json::json!({}));
                                }
                                if tx.send(e).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(_) => {
                            fails += 1;
                            if fails >= 4 {
                                bridge_diag("poll_disconnected", serde_json::json!({"consecutive_failures":fails}));
                                let _ = tx.send(BridgeEvent::Disconnected);
                                return;
                            }
                        }
                    }
                    std::thread::sleep(period);
                }
            })
            .expect("spawn poller");
        rx
    }
}

/// Compare two fingerprints and produce events.
pub fn diff_fingerprint(old: &Value, new: &Value) -> Vec<BridgeEvent> {
    // A different identity invalidates every account-scoped cache, even for equal libraries.
    if old["account"] != new["account"] { return vec![BridgeEvent::Connected]; }
    let mut v = Vec::new();
    if old["lib"] != new["lib"] {
        v.push(BridgeEvent::LibraryChanged);
    }
    if old["dl"] != new["dl"] {
        v.push(BridgeEvent::DownloadsChanged);
    }
    if old["n"] != new["n"] {
        v.push(BridgeEvent::NotificationsChanged);
    }
    if old["run"] != new["run"] {
        let ids = new["run"].as_str().unwrap_or("").split(',').filter_map(|s| s.parse().ok()).collect();
        v.push(BridgeEvent::RunningAppChanged(ids));
    }
    v
}

#[cfg(test)]
fn expected_text<'a>(russian: &'a str, english: &'a str) -> &'a str {
    if steam_assets::i18n::current().code() == "ru" { russian } else { english }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Each session serves discovery and a websocket, recording the expression before either
    // dropping the reply or answering. The listener deadline prevents a failed test from hanging.
    fn mock_cdp(drop_first: bool) -> (Bridge, std::thread::JoinHandle<Vec<String>>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut expressions = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            for session in 0..2 {
                let accept = || loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    }
                };
                let mut discovery = accept();
                discovery.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                // TCP may split even this small request across reads. Drain the full
                // headers before closing, otherwise unread bytes can reset the reply.
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = discovery.read(&mut chunk).unwrap();
                    assert!(n > 0, "discovery request closed before headers completed");
                    request.extend_from_slice(&chunk[..n]);
                    assert!(request.len() <= 8192, "discovery headers too large");
                }
                assert!(request.starts_with(b"GET /json "));
                let body = json!([{"title":"SharedJSContext","webSocketDebuggerUrl":format!("ws://127.0.0.1:{port}/devtools/page/1")}]).to_string();
                write!(discovery, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                drop(discovery);
                let stream = accept();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "Runtime.evaluate");
                expressions.push(request["params"]["expression"].as_str().unwrap().to_owned());
                if session != 0 || !drop_first {
                    ws.send(tungstenite::Message::text(json!({"id":request["id"],"result":{"result":{"value":42}}}).to_string())).unwrap();
                }
            }
            expressions
        });
        (Bridge { inner: Arc::new(Shared { port, conn: Mutex::new(None) }) }, server)
    }

    #[test]
    fn once_does_not_replay_after_lost_reply_and_next_call_reconnects() {
        let (bridge, server) = mock_cdp(true);
        assert!(bridge.eval_json_once("first_action").is_err());
        assert!(!bridge.is_connected());
        assert_eq!(bridge.eval_json_once("distinct_action").unwrap(), json!(42));
        assert_eq!(server.join().unwrap(), vec!["first_action", "distinct_action"]);
    }

    // One session per entry, each serving one Runtime.evaluate: `Some(v)` answers with `v`, `None` drops the socket
    // (the reply is lost). Returns the expressions in arrival order.
    fn mock_cdp_script(replies: Vec<Option<Value>>) -> (Bridge, std::thread::JoinHandle<Vec<String>>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut expressions = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            for reply in replies {
                let accept = || loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    }
                };
                let mut discovery = accept();
                discovery.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = discovery.read(&mut chunk).unwrap();
                    assert!(n > 0, "discovery request closed before headers completed");
                    request.extend_from_slice(&chunk[..n]);
                }
                let body = json!([{"title":"SharedJSContext","webSocketDebuggerUrl":format!("ws://127.0.0.1:{port}/devtools/page/1")}]).to_string();
                write!(discovery, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                drop(discovery);
                let stream = accept();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let request: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                expressions.push(request["params"]["expression"].as_str().unwrap().to_owned());
                if let Some(v) = reply {
                    ws.send(tungstenite::Message::text(json!({"id":request["id"],"result":{"result":{"value":v}}}).to_string())).unwrap();
                }
            }
            expressions
        });
        (Bridge { inner: Arc::new(Shared { port, conn: Mutex::new(None) }) }, server)
    }

    fn send_request() -> crate::send_to_phone::SendRequest {
        crate::send_to_phone::SendRequest { kind: crate::send_to_phone::MediaKind::Screenshot, clip_id: None,
            app: 1000, duration_ms: None, shot_path: Some("/x/userdata/1/760/remote/2/screenshots/a.jpg".into()), live: true }
    }

    fn fast_timing() -> crate::send_to_phone::SendTiming {
        crate::send_to_phone::SendTiming { call: Duration::from_millis(300), window: Duration::from_secs(4),
            start: Duration::from_millis(200), step: Duration::from_millis(10) }
    }

    #[test]
    fn lost_send_reply_is_settled_as_sent_from_page_state_without_resend() {
        // The send's reply is lost (the upload finished in Steam). The page state of this run says sent.
        let (bridge, server) = mock_cdp_script(vec![None, Some(json!({"id": "run-a", "step": "sent"}))]);
        let reply = bridge.phone_send_timed(&send_request(), "run-a", fast_timing()).unwrap();
        assert_eq!(reply, crate::send_to_phone::SendReply::Sent);
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2, "one send and one state read, never a second send");
        assert!(seen[0].contains("const __kind=\"shot\""));
        assert!(!seen[1].contains("const __kind"), "the second call only reads the state");
        assert!(seen[1].contains("\"run-a\""));
    }

    #[test]
    fn lost_send_reply_with_no_readable_state_stays_unknown() {
        // Nothing answers the state read: the outcome is genuinely unknown, and the lost-reply error is returned.
        let (bridge, server) = mock_cdp_script(vec![None]);
        let err = bridge.phone_send_timed(&send_request(), "run-b", fast_timing());
        assert!(matches!(err, Err(Error::Timeout) | Err(Error::Unavailable(_))), "{err:?}");
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn native_install_lost_replies_never_replay_start_or_accept() {
        for accept in [false, true] {
            let (bridge, server) = mock_cdp_script(vec![None]);
            if accept { assert!(bridge.accept_eula(42, "license", 2).is_err()); }
            else { assert!(bridge.install_start_headless(42, 7).is_err()); }
            let seen = server.join().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(!seen[0].contains("BringToFront"));
            assert!(seen[0].contains(if accept { "MarkEulaAccepted" } else { "SetInstallFolderIndexAndStartInstall" }));
        }
    }

    #[test]
    fn stock_install_lost_reply_never_replays() {
        let (bridge, server) = mock_cdp(true);
        assert!(bridge.install_stock(204360).is_err());
        assert_eq!(bridge.eval_json_once("read_after_lost_install").unwrap(), json!(42));
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].contains("OpenInstallWizard([id], \"0\")"));
        assert_eq!(seen[1], "read_after_lost_install");
    }

    #[test]
    fn uninstall_lost_reply_never_replays() {
        let (bridge, server) = mock_cdp(true);
        assert!(bridge.uninstall(42).is_err());
        assert_eq!(bridge.eval_json_once("read_after_lost_uninstall").unwrap(), json!(42));
        let seen = server.join().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].contains("OpenUninstallWizard([id],\"0\",true)"));
        assert_eq!(seen[1], "read_after_lost_uninstall");
    }

    #[test]
    fn uninstall_requires_explicit_dispatch_ack() {
        for value in [json!(false), json!(null), json!({"error": 2}), json!(true)] {
            let (bridge, server) = mock_cdp_script(vec![Some(value.clone())]);
            assert_eq!(bridge.uninstall(42).unwrap(), value == json!(true));
            assert_eq!(server.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn uninstall_fingerprint_causes_library_refresh() {
        let before = json!({"lib":"1/1/0/0/11/","dl":"","n":"","run":""});
        let after = json!({"lib":"1/0/0/0/0/","dl":"","n":"","run":""});
        assert_eq!(diff_fingerprint(&before, &after), vec![BridgeEvent::LibraryChanged]);
    }

    #[test]
    fn read_eval_still_retries_after_lost_reply() {
        let (bridge, server) = mock_cdp(true);
        assert_eq!(bridge.eval_json("read_expression").unwrap(), json!(42));
        assert_eq!(server.join().unwrap(), vec!["read_expression", "read_expression"]);
    }

    #[test]
    fn parses_apps() {
        let v = json!([[251570,"7 Days to Die","7 days to die",0,"0",17,0,1715615439,1,9,0,0,"251570",1,227,86,1721925232,1715615439],
            [3078970514u64,"Burnout 3","burnout 3",1,"482056602",5,2,1794000000,1073741824,11,100,1,"13224077663011864576",1,0,0,0,0]]);
        let a = parse_apps(&v).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].display_name, "7 Days to Die");
        assert!(!a[0].installed && a[0].is_game());
        assert_eq!(a[1].size_on_disk, 482056602);
        assert!(a[1].installed && a[1].is_shortcut);
        assert_eq!(a[1].game_id, "13224077663011864576");
        assert_eq!(a[1].appid, 3078970514);
        assert!(parse_apps(&json!([[1, 2]])).is_err());
    }

    #[test]
    fn parses_collections() {
        let c = parse_collections(&json!([["favorite", "Избранное", [1, 2]], ["x", "X", []]])).unwrap();
        assert_eq!(c[0].apps, vec![1, 2]);
        assert_eq!(c[1].name, "X");
    }

    #[test]
    fn parses_downloads() {
        let d = parse_downloads(&json!({"overview":{"state":"None","appid":0,"net":0,"disc":0,"upload":false},
            "items":[{"appid":2379780,"active":false,"paused":true,"completed":false,"queue_index":-1,"percent":33,"eta":-1,"bytes":10,"total":30,"launch_on_completion":false}]})).unwrap();
        assert_eq!(d.items[0].appid, 2379780);
        assert!(d.items[0].paused);
        assert_eq!(d.items[0].percent, 33.0);
        assert_eq!(d.overview.state, "None");
        // An older snapshot without the extended fields: unknown stays unknown.
        assert_eq!(d.overview.paused, None);
        assert_eq!(d.items[0].update_result, None);
        assert_eq!(d.items[0].name, None);
        assert_eq!(d.items[0].dl_total, None);
    }

    #[test]
    fn parses_extended_downloads_and_settings() {
        let d = parse_downloads(&json!({"overview":{"state":"Downloading","appid":10,"net":1000,"disc":0,"upload":false,"paused":false,
            "throttling_suspended":true,"lan_peer_hostname":"","percent":41,"eta":120,"dl_bytes":41,"dl_total":100},
            "items":[{"appid":10,"active":true,"paused":false,"completed":false,"queue_index":0,"percent":41,"eta":120,"bytes":41,"total":100,
            "launch_on_completion":true,"name":"Game","deferred_time":0,"completed_time":0,"update_result":0,"update_error":null,
            "auto_update":null,"disabled_by_app":false,"dl_bytes":41,"dl_total":100}]})).unwrap();
        assert_eq!(d.overview.paused, Some(false));
        assert_eq!(d.overview.throttling_suspended, Some(true));
        assert_eq!(d.overview.dl_total, Some(100));
        let i = &d.items[0];
        assert_eq!((i.name.as_deref(), i.update_result, i.auto_update, i.disabled_by_app), (Some("Game"), Some(0), None, Some(false)));
        let s = parse_download_settings(&json!({"throttle_rate":8000,"rate_bits":false,"restrict":null,"restrict_start":1,"restrict_end":5,
            "while_running":true,"throttle_streaming":false,"update_behavior":3,"peer_content":1,"region":38,
            "hours":[[0,"0:00"],[1,"1:00"]],"regions":[[38,"Poland"]]})).unwrap();
        assert_eq!(s.throttle_rate, Some(8000));
        assert_eq!(s.restrict_auto_updates, None);
        assert_eq!(s.hours[1], (1, "1:00".to_string()));
        assert_eq!(s.regions, vec![(38, "Poland".to_string())]);
        assert!(parse_download_settings(&json!(null)).is_err());
    }

    #[test]
    fn download_writes_follow_the_stock_argument_order() {
        assert_eq!(js::enable_all_downloads(false), r#"(SteamClient.Downloads.EnableAllDownloads(false,"0"),true)"#);
        assert_eq!(js::resume_app_update(7), r#"(SteamClient.Downloads.ResumeAppUpdate(7,"0"),true)"#);
        assert_eq!(js::pause_app_update(7), r#"(SteamClient.Downloads.PauseAppUpdate(7,"0"),true)"#);
        // appid, index, client: not (appid, client, index).
        assert_eq!(js::set_queue_index(7, 2), r#"(SteamClient.Downloads.SetQueueIndex(7,2,"0"),true)"#);
        assert_eq!(js::remove_from_download_list(7, false), r#"(SteamClient.Downloads.RemoveFromDownloadList(7,"0"),true)"#);
        assert!(js::remove_from_download_list(7, true).contains(r#"EnableAllDownloads(true,"0")"#));
        assert_eq!(js::set_launch_on_update_complete(0), "(SteamClient.Downloads.SetLaunchOnUpdateComplete(0),true)");
        assert_eq!(js::suspend_download_throttling(true), r#"(SteamClient.Downloads.SuspendDownloadThrottling(true,"0"),true)"#);
    }

    #[test]
    fn parses_notifications_and_system() {
        let n = parse_notifications(&json!({"unviewed":2,"items":[{"id":5,"type":1,"source":2,"is_new":true,"read":false,"count":1,"data":"{}"}]})).unwrap();
        assert_eq!(n.unviewed, 2);
        assert!(n.items[0].is_new);
        let s = parse_system(&json!({"network":{"connected":true,"internet":true,"steam":true,"wifi_enabled":true,"wifi_strength":3,"ssid":"Home"},
            "battery":{"present":true,"level":0.28,"ac_state":1,"state":1,"seconds_remaining":5229},
            "user":{"persona_name":"p","account_name":"a","avatar_url":"/avatarcache/1.png","steamid":"1"}})).unwrap();
        assert_eq!(s.network.ssid.as_deref(), Some("Home"));
        assert_eq!(s.network.wifi_strength, Some(3));
        assert!((s.battery.level - 0.28).abs() < 1e-9);
        assert_eq!(s.user.persona_name.as_deref(), Some("p"));
    }

    #[test]
    fn picks_target_and_eval_errors() {
        let v = json!([{"title":"Library Supernav","webSocketDebuggerUrl":"ws://a"},{"title":"SharedJSContext","webSocketDebuggerUrl":"ws://b"}]);
        assert_eq!(transport::find_target(&v).and_then(|t| t["webSocketDebuggerUrl"].as_str()), Some("ws://b"));
        assert!(transport::find_target(&json!([])).is_none());
        let ok = transport::parse_eval_result(json!({"result":{"type":"number","value":3}})).unwrap();
        assert_eq!(ok, json!(3));
        let err = transport::parse_eval_result(json!({"result":{},"exceptionDetails":{"text":"Uncaught","exception":{"description":"TypeError: x\n at y"}}}));
        assert!(matches!(err, Err(Error::Js(m)) if m == "TypeError: x"));
    }

    #[test]
    fn fingerprint_diff() {
        let a = json!({"lib":"1","dl":"a","n":"0|","run":""});
        let b = json!({"lib":"1","dl":"b","n":"0|","run":"5,6"});
        assert_eq!(diff_fingerprint(&a, &b), vec![BridgeEvent::DownloadsChanged, BridgeEvent::RunningAppChanged(vec![5, 6])]);
        assert!(diff_fingerprint(&a, &a).is_empty());
    }

    #[test]
    fn subscription_ends_after_initial_connection_failures() {
        // No successful fingerprint is needed before reporting offline. Previously last=None
        // left this subscription polling forever and backend waiting for its first Connected.
        let b = Bridge { inner: Arc::new(Shared { port: 1, conn: Mutex::new(None) }) };
        let rx = b.subscribe_every(Duration::from_millis(1));
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), BridgeEvent::Disconnected);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(1)), Err(mpsc::RecvTimeoutError::Disconnected)));
    }

    #[test]
    fn unavailable_port_errors_fast() {
        let t = std::time::Instant::now();
        assert!(matches!(Conn::open(1), Err(Error::Unavailable(_))));
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
