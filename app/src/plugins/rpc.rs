//! Decky Loader backend client (read side of the same protocol Decky's own frontend uses; std only, loopback only).
//! Protocol (decky-loader frontend `WSRouter`): GET http://127.0.0.1:1337/auth/token -> token; WebSocket ws://127.0.0.1:1337/ws?auth=<token>;
//! text frames of JSON. CALL `{type:0,id,route,args}` -> REPLY `{type:1,id,result}` | ERROR `{type:-1,id,error:{name,error,traceback}}`; the client
//! acknowledges with `{type:3,id}`. Plugin methods: route `loader/call_plugin_method` args `[pluginName, method, ...args]`;
//! legacy plugins (CSS Loader) `loader/call_legacy_plugin_method` args `[pluginName, method, {kwargs}]`.
//! Writes: callers must check `crate::plugins::dry_run()` first; this client itself never filters.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const ADDR: &str = "127.0.0.1:1337";

pub struct Rpc {
    s: TcpStream,
    id: u64,
    buf: Vec<u8>,
}

fn connect_tcp() -> std::io::Result<TcpStream> {
    let a: SocketAddr = ADDR.parse().unwrap();
    let s = TcpStream::connect_timeout(&a, Duration::from_millis(400))?;
    s.set_read_timeout(Some(Duration::from_secs(4)))?;
    s.set_write_timeout(Some(Duration::from_secs(2)))?;
    Ok(s)
}

fn http_get(path: &str) -> std::io::Result<String> {
    let mut s = connect_tcp()?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: {ADDR}\r\nConnection: close\r\n\r\n")?;
    let mut v = Vec::new();
    let _ = s.read_to_end(&mut v);
    let t = String::from_utf8_lossy(&v).to_string();
    Ok(t.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

impl Rpc {
    pub fn connect() -> Option<Rpc> {
        let token = http_get("/auth/token").ok()?.trim().to_string();
        if token.len() < 8 {
            return None;
        }
        let mut s = connect_tcp().ok()?;
        write!(
            s,
            "GET /ws?auth={token} HTTP/1.1\r\nHost: {ADDR}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: https://steamloopback.host\r\n\r\n"
        )
        .ok()?;
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if s.read(&mut b).ok()? == 0 || head.len() > 8192 {
                return None;
            }
            head.push(b[0]);
        }
        if !String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101") {
            return None;
        }
        Some(Rpc { s, id: 0, buf: Vec::new() })
    }

    fn send(&mut self, op: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut f = vec![0x80 | op];
        let n = payload.len();
        if n < 126 {
            f.push(0x80 | n as u8);
        } else if n < 65536 {
            f.push(0x80 | 126);
            f.extend((n as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend((n as u64).to_be_bytes());
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        f.extend(mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.s.write_all(&f)
    }

    fn fill(&mut self, n: usize) -> std::io::Result<()> {
        while self.buf.len() < n {
            let mut tmp = [0u8; 8192];
            let k = self.s.read(&mut tmp)?;
            if k == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            self.buf.extend_from_slice(&tmp[..k]);
        }
        Ok(())
    }

    /// next complete text message (control frames handled)
    fn recv(&mut self) -> std::io::Result<String> {
        let mut msg = Vec::new();
        loop {
            self.fill(2)?;
            let (b0, b1) = (self.buf[0], self.buf[1]);
            let mut len = (b1 & 0x7f) as usize;
            let mut off = 2;
            if len == 126 {
                self.fill(4)?;
                len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
                off = 4;
            } else if len == 127 {
                self.fill(10)?;
                len = u64::from_be_bytes(self.buf[2..10].try_into().unwrap()) as usize;
                off = 10;
            }
            self.fill(off + len)?;
            let payload: Vec<u8> = self.buf[off..off + len].to_vec();
            self.buf.drain(..off + len);
            match b0 & 0x0f {
                0x9 => self.send(0xA, &payload)?,
                0x8 => return Err(std::io::ErrorKind::ConnectionAborted.into()),
                0x1 | 0x0 => {
                    msg.extend(payload);
                    if b0 & 0x80 != 0 {
                        return Ok(String::from_utf8_lossy(&msg).to_string());
                    }
                }
                _ => {}
            }
        }
    }

    /// `route` call; Ok(result) or Err(error text)
    pub fn call(&mut self, route: &str, args: Vec<Value>) -> Result<Value, String> {
        self.id += 1;
        let id = self.id;
        let m = json!({"type": 0, "id": id, "route": route, "args": args});
        self.send(0x1, m.to_string().as_bytes()).map_err(|e| e.to_string())?;
        for _ in 0..64 {
            let t = self.recv().map_err(|e| e.to_string())?;
            let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
            if v["id"].as_u64() != Some(id) {
                continue;
            }
            let _ = self.send(0x1, json!({"type": 3, "id": id}).to_string().as_bytes());
            return match v["type"].as_i64() {
                Some(1) => Ok(v["result"].clone()),
                Some(-1) => Err(v["error"].to_string()),
                _ => Err("discarded".into()),
            };
        }
        Err("no reply".into())
    }

    /// `loader/call_plugin_method` (decky >= 3 plugins)
    pub fn plugin(&mut self, plugin: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let mut a = vec![json!(plugin), json!(method)];
        a.extend(args);
        self.call("loader/call_plugin_method", a)
    }

    /// `loader/call_legacy_plugin_method` (kwargs style: CSS Loader, ...)
    pub fn legacy(&mut self, plugin: &str, method: &str, kwargs: Value) -> Result<Value, String> {
        self.call("loader/call_legacy_plugin_method", vec![json!(plugin), json!(method), kwargs])
    }

    /// new-style call, falling back to the legacy kwargs route for api_version 0 plugins; unwraps legacy `{result}` envelopes
    pub fn auto(&mut self, plugin: &str, method: &str, args: Vec<Value>, kwargs: Value) -> Result<Value, String> {
        match self.plugin(plugin, method, args) {
            Err(e) if e.contains("api_version") => self.legacy(plugin, method, kwargs).map(|v| if v.get("result").is_some() && v.as_object().map(|o| o.len() <= 3).unwrap_or(false) { v["result"].clone() } else { v }),
            r => r,
        }
    }
}
