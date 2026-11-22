//! Blocking CDP transport: HTTP target discovery + websocket + request/response matching.
use crate::error::{Error, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use tungstenite::{protocol::WebSocketConfig, Message, WebSocket};

pub const CALL_TIMEOUT: Duration = Duration::from_millis(2000);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const TARGET_TITLE: &str = "SharedJSContext";

fn diag_enabled() -> bool {
    match std::env::var("DECKSHELL_DIAG").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => std::env::var_os("DECKSHELL_SESSION_MODE").is_some(),
    }
}

fn diag(event: &str, fields: Value) {
    if !diag_enabled() { return; }
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
    eprintln!("[bridge-diag] {{\"ts_unix_ms\":{ts},\"pid\":{},\"event\":{},\"fields\":{}}}", std::process::id(), json!(event), fields);
}

fn error_kind(e: &Error) -> &'static str {
    match e {
        Error::Timeout => "timeout",
        Error::Unavailable(s) if s.to_ascii_lowercase().contains("timed out") => "timeout",
        Error::Unavailable(_) => "unavailable",
        Error::Protocol(_) => "protocol",
        Error::Js(_) => "javascript",
    }
}

fn expr_label(expr: &str) -> &'static str {
    if expr.contains("friendStore") && expr.contains("allFriends") { "friends" }
    else if expr.contains("collectionStore") && expr.contains("downloadsStore") { "fingerprint" }
    else if expr.contains("collectionStore") { "library" }
    else if expr.contains("GetAppOverviewByAppID") || expr.contains("APP_DETAILS") { "app_details_or_art" }
    else if expr.contains("System") || expr.contains("settingsStore") { "system_or_settings" }
    else if expr.contains("appStore") || expr.contains("GetAllApps") { "apps_or_library" }
    else { "other" }
}

static LAST_TARGET: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();

pub fn port() -> u16 {
    std::env::var("STEAM_CDP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8080)
}

fn tcp(port: u16) -> Result<TcpStream> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|e| Error::Unavailable(format!("connect 127.0.0.1:{port}: {e}")))?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(CALL_TIMEOUT));
    let _ = s.set_write_timeout(Some(CALL_TIMEOUT));
    Ok(s)
}

/// GET /json and return the websocket path of the SharedJSContext target.
pub fn discover(port: u16) -> Result<String> {
    let started = Instant::now();
    let mut s = match tcp(port) {
        Ok(s) => s,
        Err(e) => {
            diag("discovery_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":error_kind(&e)}));
            return Err(e);
        }
    };
    write!(s, "GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
        .map_err(|e| Error::Unavailable(e.to_string()))?;
    // CEF ignores `Connection: close` and rejects HTTP/1.0, so read by Content-Length.
    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let (body_at, need) = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..p]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .ok_or_else(|| Error::Protocol("/json: no content-length".into()))?;
            break (p + 4, len);
        }
        if Instant::now() > deadline {
            return Err(Error::Timeout);
        }
        match s.read(&mut chunk) {
            Ok(0) => return Err(Error::Unavailable("/json: closed".into())),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(Error::Unavailable(format!("read /json: {e}"))),
        }
    };
    while buf.len() < body_at + need {
        if Instant::now() > deadline {
            return Err(Error::Timeout);
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(Error::Unavailable(format!("read /json: {e}"))),
        }
    }
    let pos = body_at;
    let v: Value = serde_json::from_slice(&buf[pos..]).map_err(|e| Error::Protocol(format!("/json: {e}")))?;
    let Some(target) = find_target(&v) else {
        diag("discovery_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":"target_missing"}));
        return Err(Error::Unavailable("no SharedJSContext target".into()));
    };
    let title = target["title"].as_str().unwrap_or(TARGET_TITLE);
    let Some(url) = target["webSocketDebuggerUrl"].as_str() else {
        diag("discovery_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":"websocket_url_missing","title":title}));
        return Err(Error::Unavailable("SharedJSContext target has no websocket URL".into()));
    };
    let safe_url = url.split('?').next().unwrap_or(url);
    let changed = {
        let mut last = LAST_TARGET.get_or_init(|| std::sync::Mutex::new(None)).lock().unwrap();
        let changed = last.as_deref() != Some(safe_url);
        *last = Some(safe_url.to_owned());
        changed
    };
    if changed {
        diag("target_changed", json!({"title":title,"websocket_url":safe_url,"duration_ms":started.elapsed().as_millis()}));
    }
    if started.elapsed() > Duration::from_millis(250) {
        diag("discovery_slow", json!({"title":title,"duration_ms":started.elapsed().as_millis()}));
    }
    diag("discovery_ok", json!({"title":title,"duration_ms":started.elapsed().as_millis()}));
    Ok(url.to_owned())
}

/// The SharedJSContext entry of the `/json` target list. The live discovery and its tests both use this.
pub fn find_target(v: &Value) -> Option<&Value> {
    v.as_array()?.iter().find(|t| t["title"] == TARGET_TITLE)
}

pub struct Conn {
    ws: WebSocket<TcpStream>,
    next_id: u64,
}

impl Conn {
    pub fn open(port: u16) -> Result<Conn> {
        let started = Instant::now();
        diag("connect_start", json!({"port":port}));
        let url = match discover(port) {
            Ok(url) => url,
            Err(e) => {
                diag("connect_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":error_kind(&e)}));
                return Err(e);
            }
        };
        let stream = match tcp(port) {
            Ok(stream) => stream,
            Err(e) => {
                diag("connect_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":error_kind(&e)}));
                return Err(e);
            }
        };
        let cfg = WebSocketConfig { max_message_size: Some(64 << 20), max_frame_size: Some(64 << 20), ..Default::default() };
        let (ws, _) = match tungstenite::client::client_with_config(url.as_str(), stream, Some(cfg)) {
            Ok(result) => result,
            Err(e) => {
                let err = Error::Unavailable(format!("ws handshake: {e}"));
                diag("connect_error", json!({"duration_ms":started.elapsed().as_millis(),"kind":error_kind(&err)}));
                return Err(err);
            }
        };
        diag("connect_ok", json!({"duration_ms":started.elapsed().as_millis()}));
        Ok(Conn { ws, next_id: 0 })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call_for(method, params, CALL_TIMEOUT)
    }

    /// Like [`Conn::call`], with the reply deadline given by the caller.
    fn call_for(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let deadline = Instant::now() + timeout;
        let msg = json!({"id": id, "method": method, "params": params}).to_string();
        self.ws.send(Message::text(msg)).map_err(io_err)?;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let _ = self.ws.get_ref().set_read_timeout(Some(left));
            match self.ws.read().map_err(io_err)? {
                Message::Text(t) => {
                    let v: Value = serde_json::from_str(t.as_str()).map_err(|e| Error::Protocol(e.to_string()))?;
                    if v["id"].as_u64() == Some(id) {
                        if let Some(e) = v.get("error") {
                            return Err(Error::Protocol(e.to_string()));
                        }
                        return Ok(v["result"].clone());
                    }
                }
                Message::Close(_) => return Err(Error::Unavailable("socket closed".into())),
                _ => {}
            }
        }
    }

    /// Runtime.evaluate with returnByValue + awaitPromise; returns the JS value.
    pub fn eval(&mut self, expr: &str) -> Result<Value> {
        self.eval_for(expr, CALL_TIMEOUT)
    }

    /// [`Conn::eval`] with a caller-given reply deadline. A long script (the phone send) needs more than
    /// [`CALL_TIMEOUT`]; when its reply is lost, the caller must check the page state, not assume failure.
    pub fn eval_for(&mut self, expr: &str, timeout: Duration) -> Result<Value> {
        let started = Instant::now();
        let label = expr_label(expr);
        let result = self.call_for(
            "Runtime.evaluate",
            json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "timeout": 1800}),
            timeout,
        ).and_then(parse_eval_result);
        let elapsed = started.elapsed();
        match &result {
            Err(e) => diag("eval_error", json!({"label":label,"duration_ms":elapsed.as_millis(),"kind":error_kind(e)})),
            Ok(_) if elapsed > Duration::from_millis(250) => diag("eval_slow", json!({"label":label,"duration_ms":elapsed.as_millis()})),
            _ => {}
        }
        result
    }
}

pub fn parse_eval_result(r: Value) -> Result<Value> {
    if let Some(ex) = r.get("exceptionDetails") {
        let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("exception");
        return Err(Error::Js(msg.lines().next().unwrap_or(msg).to_string()));
    }
    Ok(r["result"].get("value").cloned().unwrap_or(Value::Null))
}

fn io_err(e: tungstenite::Error) -> Error {
    use tungstenite::Error as E;
    match e {
        E::Io(ref io) if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Error::Timeout,
        E::ConnectionClosed | E::AlreadyClosed => Error::Unavailable("socket closed".into()),
        other => Error::Unavailable(other.to_string()),
    }
}
