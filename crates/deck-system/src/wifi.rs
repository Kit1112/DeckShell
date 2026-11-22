//! Wi-Fi management (Settings → Интернет): scan, connect, forget, autoconnect, details.
//!
//! The pure part (parsers, `nmcli` argument builders, validation, error classification) is tested without a
//! network. All side effects go through [`WifiBackend`]: [`Nmcli`] drives NetworkManager on the Deck, [`MockWifi`]
//! is an in-memory network used by tests, `DECKSHELL_DRYRUN` and `DECKSHELL_MOCK` (a test must never be able to
//! drop the owner's real connection).
use steam_assets::i18n::tr_ref;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Wi-Fi security of an access point / of a typed-in custom network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Security { Open, Wep, Wpa, Wpa2, Wpa3, Enterprise }

impl Security {
    /// Parses the SECURITY column of `nmcli device wifi list` ("WPA2", "WPA1 WPA2", "WPA2 802.1X", "--", "").
    pub fn from_nm(s: &str) -> Security {
        let s = s.trim();
        if s.is_empty() || s == "--" { return Security::Open; }
        if s.contains("802.1X") { return Security::Enterprise; }
        if s.contains("WPA3") || s.contains("SAE") { return Security::Wpa3; }
        if s.contains("WPA2") { return Security::Wpa2; }
        if s.contains("WPA") { return Security::Wpa; }
        if s.contains("WEP") { return Security::Wep; }
        Security::Open
    }
    /// The lock icon in the network row.
    pub fn locked(self) -> bool { self != Security::Open }
    /// A key must be typed before connecting.
    pub fn needs_password(self) -> bool { !matches!(self, Security::Open) }
    /// Stock steamui cannot offer 802.1X in the custom-network form; neither can we.
    pub fn supported(self) -> bool { self != Security::Enterprise }
    pub fn label(self) -> &'static str {
        match self { Security::Open => tr_ref("wifi.security_none"), Security::Wep => "WEP", Security::Wpa => "WPA", Security::Wpa2 => "WPA2", Security::Wpa3 => "WPA3", Security::Enterprise => "WPA Enterprise" }
    }
}

/// Choices of the «Протокол безопасности» drop-down; WPA2 first (the stock default).
pub const CUSTOM_SECURITY: [Security; 5] = [Security::Wpa2, Security::Wpa3, Security::Wpa, Security::Wep, Security::Open];

/// Signal 0..=100 → 0..=4 arcs of the icon.
pub fn bars(signal: u8) -> u8 {
    match signal { 0 => 0, 1..=25 => 1, 26..=50 => 2, 51..=75 => 3, _ => 4 }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessPoint { pub ssid: String, pub bssid: String, pub signal: u8, pub security: Security, pub in_use: bool }

/// A saved NetworkManager profile of a Wi-Fi network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile { pub name: String, pub uuid: String, pub autoconnect: bool, pub active: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connectivity { Unknown, Full, Limited, Portal, None }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub mac: String,
    pub dhcp: bool,
    /// (address, netmask) pairs
    pub ipv4: Vec<(String, String)>,
    pub gateway: String,
    pub dns: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StaticIp { pub ip: String, pub netmask: String, pub gateway: String, pub dns1: String, pub dns2: String }

impl StaticIp {
    /// Stock `m()`: every field empty means DHCP.
    pub fn is_empty(&self) -> bool {
        [&self.ip, &self.netmask, &self.gateway, &self.dns1, &self.dns2].iter().all(|s| s.trim().is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpError { Missing(IpField), Invalid(IpField) }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpField { Address, Netmask, Gateway, PrimaryDns, SecondaryDns }

pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut it = s.trim().split('.');
    for o in &mut out {
        let part = it.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) { return None; }
        *o = part.parse().ok()?;
    }
    it.next().is_none().then_some(out)
}

/// Prefix length of a contiguous netmask ("255.255.255.0" → 24); `None` for holes ("255.0.255.0") and non-masks.
pub fn prefix_len(mask: &str) -> Option<u8> {
    let m = u32::from_be_bytes(parse_ipv4(mask)?);
    let ones = m.leading_ones();
    (m.checked_shl(ones).unwrap_or(0) == 0).then_some(ones as u8)
}

/// Manual addressing needs address + mask; gateway and DNS are optional but must be valid when typed.
pub fn validate_static(ip: &StaticIp) -> Result<(), IpError> {
    if ip.ip.trim().is_empty() { return Err(IpError::Missing(IpField::Address)); }
    if parse_ipv4(&ip.ip).is_none() { return Err(IpError::Invalid(IpField::Address)); }
    if ip.netmask.trim().is_empty() { return Err(IpError::Missing(IpField::Netmask)); }
    if prefix_len(&ip.netmask).is_none() { return Err(IpError::Invalid(IpField::Netmask)); }
    for (v, n) in [(&ip.gateway, IpField::Gateway), (&ip.dns1, IpField::PrimaryDns), (&ip.dns2, IpField::SecondaryDns)] {
        if !v.trim().is_empty() && parse_ipv4(v).is_none() { return Err(IpError::Invalid(n)); }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ConnectRequest {
    pub ssid: String,
    pub security: Security,
    pub username: String,
    pub password: String,
    pub hidden: bool,
    /// `None` = DHCP.
    pub ip: Option<StaticIp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError { BadPassword, NotFound, Timeout, Cancelled, Unsupported, Other(String) }

/// What nmcli says when activation fails → the cause (stderr, English: the command runs under `LC_ALL=C`).
pub fn classify_error(stderr: &str) -> ConnectError {
    let s = stderr.to_ascii_lowercase();
    if s.contains("secrets were required") || s.contains("(7)") || s.contains("no secrets") || s.contains("802-11-wireless-security") || s.contains("(1) unknown reason") { return ConnectError::BadPassword; }
    if s.contains("no network with ssid") || s.contains("could not be found") || s.contains("(53)") { return ConnectError::NotFound; }
    if s.contains("timeout") || s.contains("timed out") { return ConnectError::Timeout; }
    ConnectError::Other(stderr.trim().lines().next().unwrap_or("").to_string())
}

// ---------------------------------------------------------------- nmcli text parsing

/// Splits a `nmcli -t -e yes` line on unescaped ':'.
pub fn split_terse(l: &str) -> Vec<String> {
    let mut v = vec![String::new()];
    let mut esc = false;
    for c in l.chars() {
        if esc { v.last_mut().unwrap().push(c); esc = false; }
        else if c == '\\' { esc = true; }
        else if c == ':' { v.push(String::new()); }
        else { v.last_mut().unwrap().push(c); }
    }
    v
}

pub const SCAN_FIELDS: &str = "IN-USE,SSID,BSSID,SIGNAL,SECURITY";

/// `nmcli -t -e yes -f IN-USE,SSID,BSSID,SIGNAL,SECURITY device wifi list`. Hidden (empty SSID) entries are dropped;
/// several BSSIDs of one SSID collapse into the strongest one (the connected one wins), sorted by signal.
pub fn parse_scan(out: &str) -> Vec<AccessPoint> {
    let mut v: Vec<AccessPoint> = Vec::new();
    for l in out.lines() {
        let f = split_terse(l);
        if f.len() < 5 || f[1].is_empty() { continue; }
        let ap = AccessPoint { in_use: f[0] == "*", ssid: f[1].clone(), bssid: f[2].clone(), signal: f[3].parse().unwrap_or(0), security: Security::from_nm(&f[4]) };
        match v.iter_mut().find(|a| a.ssid == ap.ssid) {
            Some(old) => {
                let better = (ap.in_use, ap.signal) > (old.in_use, old.signal);
                let in_use = old.in_use || ap.in_use;
                if better { *old = ap; }
                old.in_use = in_use;
            }
            None => v.push(ap),
        }
    }
    v.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)).then(a.ssid.cmp(&b.ssid)));
    v
}

/// `nmcli -t -e yes -f NAME,UUID,TYPE,AUTOCONNECT,ACTIVE connection show`, Wi-Fi profiles only.
pub fn parse_profiles(out: &str) -> Vec<Profile> {
    out.lines().filter_map(|l| {
        let f = split_terse(l);
        (f.len() >= 5 && f[2] == "802-11-wireless").then(|| Profile { name: f[0].clone(), uuid: f[1].clone(), autoconnect: f[3] == "yes", active: f[4] == "yes" })
    }).collect()
}

/// NetworkManager names a profile after the SSID and adds " 1", " 2" for duplicates.
pub fn profile_for<'a>(profiles: &'a [Profile], ssid: &str) -> Option<&'a Profile> {
    profiles.iter().find(|p| p.name == ssid).or_else(|| profiles.iter().find(|p| p.name.strip_prefix(ssid).is_some_and(|r| r.starts_with(' ') && r[1..].bytes().all(|b| b.is_ascii_digit()) && r.len() > 1)))
}

/// `nmcli -t -e yes -f GENERAL.HWADDR,IP4.ADDRESS,IP4.GATEWAY,IP4.DNS device show <iface>`.
pub fn parse_device_show(out: &str) -> Details {
    let mut d = Details::default();
    for l in out.lines() {
        let Some((key, val)) = l.split_once(':') else { continue };
        let val = val.replace("\\:", ":");
        let key = key.split('[').next().unwrap_or(key);
        match key {
            "GENERAL.HWADDR" => d.mac = val,
            "IP4.ADDRESS" => if let Some((a, p)) = val.split_once('/') {
                let mask = p.parse::<u8>().ok().filter(|p| *p <= 32).map(|p| mask_from_prefix(p)).unwrap_or_default();
                d.ipv4.push((a.to_string(), mask));
            },
            "IP4.GATEWAY" => d.gateway = val,
            "IP4.DNS" => if !val.is_empty() { d.dns.push(val) },
            _ => {}
        }
    }
    d
}

pub fn mask_from_prefix(p: u8) -> String {
    let m = if p == 0 { 0u32 } else { u32::MAX << (32 - p.min(32) as u32) };
    let b = m.to_be_bytes();
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

pub fn parse_connectivity(out: &str) -> Connectivity {
    match out.trim() { "full" => Connectivity::Full, "limited" => Connectivity::Limited, "portal" => Connectivity::Portal, "none" => Connectivity::None, _ => Connectivity::Unknown }
}

// ---------------------------------------------------------------- nmcli argument builders

fn ipv4_args(ip: &Option<StaticIp>) -> Vec<String> {
    match ip {
        None => vec!["ipv4.method".into(), "auto".into(), "ipv4.addresses".into(), "".into(), "ipv4.gateway".into(), "".into(), "ipv4.dns".into(), "".into(), "ipv4.ignore-auto-dns".into(), "no".into()],
        Some(s) => {
            let prefix = prefix_len(&s.netmask).unwrap_or(24);
            let dns: Vec<&str> = [s.dns1.trim(), s.dns2.trim()].into_iter().filter(|d| !d.is_empty()).collect();
            vec!["ipv4.method".into(), "manual".into(), "ipv4.addresses".into(), format!("{}/{}", s.ip.trim(), prefix), "ipv4.gateway".into(), s.gateway.trim().into(), "ipv4.dns".into(), dns.join(" "), "ipv4.ignore-auto-dns".into(), if dns.is_empty() { "no" } else { "yes" }.into()]
        }
    }
}

fn security_args(req: &ConnectRequest) -> Vec<String> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    match req.security {
        Security::Open | Security::Enterprise => vec![],
        Security::Wep => [s(&["wifi-sec.key-mgmt", "none", "wifi-sec.wep-key-type", "passphrase", "wifi-sec.wep-key0"]), vec![req.password.clone()]].concat(),
        Security::Wpa3 => [s(&["wifi-sec.key-mgmt", "sae", "wifi-sec.psk"]), vec![req.password.clone()]].concat(),
        Security::Wpa | Security::Wpa2 => [s(&["wifi-sec.key-mgmt", "wpa-psk", "wifi-sec.psk"]), vec![req.password.clone()]].concat(),
    }
}

/// `nmcli` argument lists (without the program name) that connect: create or update the profile, then bring it up.
/// `existing` is the saved profile of this SSID, if any. The password is only in the argument lists of these calls.
pub fn connect_commands(req: &ConnectRequest, iface: &str, existing: Option<&Profile>) -> Vec<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut cmds = Vec::new();
    match existing {
        Some(p) => cmds.push([s(&["connection", "modify", "uuid"]), vec![p.uuid.clone()], s(&["connection.autoconnect", "yes"]), if req.hidden { s(&["802-11-wireless.hidden", "yes"]) } else { vec![] }, security_args(req), ipv4_args(&req.ip)].concat()),
        None => cmds.push([s(&["connection", "add", "type", "wifi", "ifname"]), vec![iface.to_string()], s(&["con-name"]), vec![req.ssid.clone()], s(&["ssid"]), vec![req.ssid.clone()], if req.hidden { s(&["802-11-wireless.hidden", "yes"]) } else { vec![] }, security_args(req), ipv4_args(&req.ip)].concat()),
    }
    let up = match existing { Some(p) => [s(&["connection", "up", "uuid"]), vec![p.uuid.clone()]].concat(), None => [s(&["connection", "up", "id"]), vec![req.ssid.clone()]].concat() };
    cmds.push([up, s(&["ifname"]), vec![iface.to_string()]].concat());
    cmds
}

/// A shown/logged form of an argument list with every secret replaced.
pub fn redact(args: &[String]) -> String {
    let mut out = Vec::new();
    let mut hide = false;
    for a in args {
        out.push(if hide { "***".to_string() } else { a.clone() });
        hide = matches!(a.as_str(), "wifi-sec.psk" | "wifi-sec.wep-key0" | "password");
    }
    out.join(" ")
}

// ---------------------------------------------------------------- backend

pub trait WifiBackend: Send + Sync {
    fn radio(&self) -> Option<bool>;
    fn set_radio(&self, on: bool) -> Result<(), String>;
    /// `rescan` asks the adapter to scan the air first (slower).
    fn scan(&self, rescan: bool) -> Vec<AccessPoint>;
    fn profiles(&self) -> Vec<Profile>;
    /// Details of the adapter and its active connection (address, mask, gateway, DNS, MAC).
    fn details(&self, ssid: &str) -> Details;
    fn connectivity(&self) -> Connectivity;
    /// Blocking; polls `cancel` and aborts with `Cancelled`.
    fn connect(&self, req: &ConnectRequest, cancel: &AtomicBool) -> Result<(), ConnectError>;
    fn disconnect(&self) -> Result<(), String>;
    fn forget(&self, ssid: &str) -> Result<(), String>;
    fn set_autoconnect(&self, ssid: &str, on: bool) -> Result<(), String>;
    /// `true` for the in-memory network (nothing real can change).
    fn is_mock(&self) -> bool { false }
}

/// NetworkManager through `nmcli`.
pub struct Nmcli { iface: Mutex<Option<String>> }

impl Default for Nmcli { fn default() -> Self { Nmcli { iface: Mutex::new(None) } } }

impl Nmcli {
    fn cmd(&self, args: &[&str]) -> Result<String, String> {
        let mutating = args.iter().any(|a| matches!(*a, "on" | "off" | "disconnect" | "delete" | "modify" | "up" | "down" | "add" | "connect" | "reload"));
        if mutating && deck_guard_deny(args) { return Ok(String::new()); }
        let out = Command::new("nmcli").args(args).env("LC_ALL", "C").env("LANG", "C").stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
    }
    fn iface(&self) -> String {
        let mut g = self.iface.lock().unwrap();
        if let Some(i) = &*g { return i.clone(); }
        let found = self.cmd(&["-t", "-f", "DEVICE,TYPE", "device"]).ok().and_then(|o| o.lines().find_map(|l| { let f = split_terse(l); (f.len() >= 2 && f[1] == "wifi").then(|| f[0].clone()) }));
        let i = found.unwrap_or_else(|| "wlan0".into());
        *g = Some(i.clone());
        i
    }
}

impl WifiBackend for Nmcli {
    fn radio(&self) -> Option<bool> { crate::wifi_enabled() }
    fn set_radio(&self, on: bool) -> Result<(), String> { crate::set_wifi_enabled(on).map_err(|err| err.to_string()) }
    fn scan(&self, rescan: bool) -> Vec<AccessPoint> {
        let iface = self.iface();
        self.cmd(&["-t", "-e", "yes", "-f", SCAN_FIELDS, "device", "wifi", "list", "ifname", &iface, "--rescan", if rescan { "yes" } else { "no" }]).map(|o| parse_scan(&o)).unwrap_or_default()
    }
    fn profiles(&self) -> Vec<Profile> {
        self.cmd(&["-t", "-e", "yes", "-f", "NAME,UUID,TYPE,AUTOCONNECT,ACTIVE", "connection", "show"]).map(|o| parse_profiles(&o)).unwrap_or_default()
    }
    fn details(&self, ssid: &str) -> Details {
        let iface = self.iface();
        let mut d = self.cmd(&["-t", "-e", "yes", "-f", "GENERAL.HWADDR,IP4.ADDRESS,IP4.GATEWAY,IP4.DNS", "device", "show", &iface]).map(|o| parse_device_show(&o)).unwrap_or_default();
        let profiles = self.profiles();
        d.dhcp = profile_for(&profiles, ssid).and_then(|p| self.cmd(&["-g", "ipv4.method", "connection", "show", "uuid", &p.uuid]).ok()).is_none_or(|m| m.trim() != "manual");
        d
    }
    fn connectivity(&self) -> Connectivity { self.cmd(&["networking", "connectivity", "check"]).map(|o| parse_connectivity(&o)).unwrap_or(Connectivity::Unknown) }
    fn connect(&self, req: &ConnectRequest, cancel: &AtomicBool) -> Result<(), ConnectError> {
        if !req.security.supported() { return Err(ConnectError::Unsupported); }
        let iface = self.iface();
        let existing = profile_for(&self.profiles(), &req.ssid).cloned();
        let created = existing.is_none();
        let cmds = connect_commands(req, &iface, existing.as_ref());
        let deadline = Instant::now() + Duration::from_secs(60);
        for args in &cmds {
            if crate::guard::deny(&format!("nmcli {}", redact(args))) { continue; }
            let mut child = Command::new("nmcli").args(["-w", "45"]).args(args).env("LC_ALL", "C").env("LANG", "C")
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| ConnectError::Other(e.to_string()))?;
            let status = loop {
                if cancel.load(Ordering::SeqCst) || Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = self.cmd(&["device", "disconnect", &iface]);
                    if created { let _ = self.forget(&req.ssid); }
                    return Err(if cancel.load(Ordering::SeqCst) { ConnectError::Cancelled } else { ConnectError::Timeout });
                }
                match child.try_wait() { Ok(Some(s)) => break s, Ok(None) => std::thread::sleep(Duration::from_millis(100)), Err(e) => return Err(ConnectError::Other(e.to_string())) }
            };
            if !status.success() {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() { use std::io::Read; let _ = e.read_to_string(&mut err); }
                // a failed first attempt must not leave a half-configured profile behind (stock forgets it too)
                if created { let _ = self.forget(&req.ssid); }
                return Err(classify_error(&err));
            }
        }
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> { let i = self.iface(); self.cmd(&["device", "disconnect", &i]).map(|_| ()) }
    fn forget(&self, ssid: &str) -> Result<(), String> {
        let profiles = self.profiles();
        let p = profile_for(&profiles, ssid).ok_or_else(|| "profile not found".to_string())?;
        self.cmd(&["connection", "delete", "uuid", &p.uuid]).map(|_| ())
    }
    fn set_autoconnect(&self, ssid: &str, on: bool) -> Result<(), String> {
        let profiles = self.profiles();
        let p = profile_for(&profiles, ssid).ok_or_else(|| "profile not found".to_string())?;
        self.cmd(&["connection", "modify", "uuid", &p.uuid, "connection.autoconnect", if on { "yes" } else { "no" }]).map(|_| ())
    }
}

// ---------------------------------------------------------------- in-memory network

#[derive(Clone, Debug)]
pub struct MockAp { pub ap: AccessPoint, pub password: String, pub reachable: bool }

#[derive(Debug)]
struct MockState {
    radio: bool,
    aps: Vec<MockAp>,
    profiles: Vec<(Profile, Option<StaticIp>, String)>,
    active: Option<String>,
    connectivity: Connectivity,
    /// connect() takes this long (so a progress screen is observable)
    delay: Duration,
    log: Vec<String>,
}

/// Deterministic fake adapter. Mutations are recorded in [`MockWifi::log`].
pub struct MockWifi { s: Mutex<MockState> }

impl MockWifi {
    pub fn new(aps: Vec<MockAp>) -> MockWifi {
        MockWifi { s: Mutex::new(MockState { radio: true, aps, profiles: vec![], active: None, connectivity: Connectivity::Full, delay: Duration::ZERO, log: vec![] }) }
    }
    /// Demo network list for dry runs: generic names, one saved and connected network.
    pub fn demo() -> MockWifi {
        let ap = |ssid: &str, signal: u8, security: Security, pw: &str| MockAp { ap: AccessPoint { ssid: ssid.into(), bssid: format!("02:00:00:00:00:{:02X}", signal), signal, security, in_use: false }, password: pw.into(), reachable: true };
        let m = MockWifi::new(vec![
            ap("HomeNet", 92, Security::Wpa2, "homepass1"), ap("HomeNet-Guest", 78, Security::Wpa2, "guest1234"), ap("Cafe Open", 66, Security::Open, ""),
            ap("Office-5G", 58, Security::Wpa3, "office-secret"), ap("Neighbour", 41, Security::Wpa2, "neighbour1"), ap("Legacy-AP", 22, Security::Wep, "12345"),
        ]);
        {
            let mut s = m.s.lock().unwrap();
            s.profiles.push((Profile { name: "HomeNet".into(), uuid: "demo-uuid-1".into(), autoconnect: true, active: true }, None, "homepass1".into()));
            s.active = Some("HomeNet".into());
        }
        m
    }
    pub fn set_delay(&self, d: Duration) { self.s.lock().unwrap().delay = d; }
    pub fn set_connectivity(&self, c: Connectivity) { self.s.lock().unwrap().connectivity = c; }
    pub fn set_reachable(&self, ssid: &str, on: bool) { for a in &mut self.s.lock().unwrap().aps { if a.ap.ssid == ssid { a.reachable = on; } } }
    pub fn add_ap(&self, ap: MockAp) { self.s.lock().unwrap().aps.push(ap); }
    /// Every mutation made so far, one line each (secrets not included).
    pub fn log(&self) -> Vec<String> { self.s.lock().unwrap().log.clone() }
    pub fn active(&self) -> Option<String> { self.s.lock().unwrap().active.clone() }
}

impl WifiBackend for MockWifi {
    fn is_mock(&self) -> bool { true }
    fn radio(&self) -> Option<bool> { Some(self.s.lock().unwrap().radio) }
    fn set_radio(&self, on: bool) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        s.radio = on;
        s.log.push(format!("radio {}", if on { "on" } else { "off" }));
        if !on { s.active = None; }
        Ok(())
    }
    fn scan(&self, _rescan: bool) -> Vec<AccessPoint> {
        let s = self.s.lock().unwrap();
        if !s.radio { return vec![]; }
        let mut v: Vec<AccessPoint> = s.aps.iter().filter(|a| a.reachable).map(|a| AccessPoint { in_use: s.active.as_deref() == Some(a.ap.ssid.as_str()), ..a.ap.clone() }).collect();
        v.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
        v
    }
    fn profiles(&self) -> Vec<Profile> {
        let s = self.s.lock().unwrap();
        s.profiles.iter().map(|(p, _, _)| Profile { active: s.active.as_deref() == Some(p.name.as_str()), ..p.clone() }).collect()
    }
    fn details(&self, ssid: &str) -> Details {
        let s = self.s.lock().unwrap();
        let mut d = Details { mac: "02:AA:BB:CC:DD:EE".into(), dhcp: true, ..Default::default() };
        if s.active.as_deref() == Some(ssid) {
            match s.profiles.iter().find(|(p, _, _)| p.name == ssid).and_then(|(_, ip, _)| ip.clone()) {
                Some(ip) => { d.dhcp = false; d.ipv4 = vec![(ip.ip, ip.netmask)]; d.gateway = ip.gateway; d.dns = [ip.dns1, ip.dns2].into_iter().filter(|x| !x.is_empty()).collect(); }
                None => { d.ipv4 = vec![("192.168.1.50".into(), "255.255.255.0".into())]; d.gateway = "192.168.1.1".into(); d.dns = vec!["192.168.1.1".into()]; }
            }
        }
        d
    }
    fn connectivity(&self) -> Connectivity { let s = self.s.lock().unwrap(); if s.active.is_some() { s.connectivity } else { Connectivity::None } }
    fn connect(&self, req: &ConnectRequest, cancel: &AtomicBool) -> Result<(), ConnectError> {
        let (delay, outcome) = {
            let s = self.s.lock().unwrap();
            let outcome = match s.aps.iter().find(|a| a.ap.ssid == req.ssid && a.reachable) {
                None => Err(ConnectError::NotFound),
                Some(_) if !req.security.supported() => Err(ConnectError::Unsupported),
                Some(a) => {
                    let saved = s.profiles.iter().any(|(p, _, pw)| p.name == req.ssid && *pw == a.password);
                    if !a.ap.security.needs_password() || req.password == a.password || (req.password.is_empty() && saved) { Ok(()) } else { Err(ConnectError::BadPassword) }
                }
            };
            (s.delay, outcome)
        };
        let t0 = Instant::now();
        while t0.elapsed() < delay {
            if cancel.load(Ordering::SeqCst) { return Err(ConnectError::Cancelled); }
            std::thread::sleep(Duration::from_millis(5));
        }
        if cancel.load(Ordering::SeqCst) { return Err(ConnectError::Cancelled); }
        let mut s = self.s.lock().unwrap();
        s.log.push(format!("connect {} ({:?}, ip {})", req.ssid, req.security, if req.ip.is_some() { "manual" } else { "dhcp" }));
        outcome?;
        let pw = req.password.clone();
        match s.profiles.iter_mut().find(|(p, _, _)| p.name == req.ssid) {
            Some(e) => { e.1 = req.ip.clone(); if !pw.is_empty() { e.2 = pw; } }
            None => { let n = s.profiles.len(); s.profiles.push((Profile { name: req.ssid.clone(), uuid: format!("mock-uuid-{n}"), autoconnect: true, active: false }, req.ip.clone(), pw)); }
        }
        s.active = Some(req.ssid.clone());
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> { let mut s = self.s.lock().unwrap(); s.log.push("disconnect".into()); s.active = None; Ok(()) }
    fn forget(&self, ssid: &str) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        let n = s.profiles.len();
        s.profiles.retain(|(p, _, _)| p.name != ssid);
        if s.profiles.len() == n { return Err("profile not found".into()); }
        s.log.push(format!("forget {ssid}"));
        if s.active.as_deref() == Some(ssid) { s.active = None; }
        Ok(())
    }
    fn set_autoconnect(&self, ssid: &str, on: bool) -> Result<(), String> {
        let mut s = self.s.lock().unwrap();
        let p = s.profiles.iter_mut().find(|(p, _, _)| p.name == ssid).ok_or_else(|| "profile not found".to_string())?;
        p.0.autoconnect = on;
        s.log.push(format!("autoconnect {ssid} {on}"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_column() {
        assert_eq!(Security::from_nm("WPA2"), Security::Wpa2);
        assert_eq!(Security::from_nm("WPA1 WPA2"), Security::Wpa2);
        assert_eq!(Security::from_nm("WPA2 WPA3"), Security::Wpa3);
        assert_eq!(Security::from_nm("WPA2 802.1X"), Security::Enterprise);
        assert_eq!(Security::from_nm("WEP"), Security::Wep);
        assert_eq!(Security::from_nm("--"), Security::Open);
        assert_eq!(Security::from_nm(""), Security::Open);
        assert!(!Security::Open.locked() && Security::Wpa2.locked());
    }

    #[test]
    fn signal_to_arcs() {
        assert_eq!([0, 1, 25, 26, 50, 51, 75, 76, 100].map(bars), [0, 1, 1, 2, 2, 3, 3, 4, 4]);
    }

    const SCAN: &str = "*:Home\\:Net:AA\\:BB\\:CC\\:DD\\:EE\\:01:81:WPA2\n :Home\\:Net:AA\\:BB\\:CC\\:DD\\:EE\\:02:90:WPA2\n :Cafe:11\\:22\\:33\\:44\\:55\\:66:40:--\n ::00\\:00\\:00\\:00\\:00\\:01:99:WPA2\n :Weak:77\\:77\\:77\\:77\\:77\\:77:12:WPA1 WPA2\n";

    #[test]
    fn scan_collapses_bssids_keeps_the_connected_flag_and_sorts() {
        let v = parse_scan(SCAN);
        let names: Vec<_> = v.iter().map(|a| a.ssid.as_str()).collect();
        assert_eq!(names, ["Home:Net", "Cafe", "Weak"], "hidden SSIDs dropped, colon unescaped, connected first");
        assert!(v[0].in_use, "in-use survives the stronger duplicate");
        assert_eq!(v[0].signal, 81, "the connected BSSID is the one shown");
        assert_eq!(v[1].security, Security::Open);
    }

    #[test]
    fn profiles_only_wifi_and_numbered_duplicates() {
        let out = "Home:u1:802-11-wireless:yes:yes\nWired:u2:802-3-ethernet:yes:no\nCafe 1:u3:802-11-wireless:no:no\n";
        let p = parse_profiles(out);
        assert_eq!(p.len(), 2);
        assert!(p[0].autoconnect && p[0].active && !p[1].autoconnect);
        assert_eq!(profile_for(&p, "Cafe").unwrap().uuid, "u3");
        assert_eq!(profile_for(&p, "Home").unwrap().uuid, "u1");
        assert!(profile_for(&p, "Hom").is_none() && profile_for(&p, "Cafe 1x").is_none());
    }

    #[test]
    fn device_show() {
        let out = "GENERAL.HWADDR:02\\:00\\:00\\:00\\:00\\:01\nIP4.ADDRESS[1]:192.168.1.50/24\nIP4.GATEWAY:192.168.1.1\nIP4.DNS[1]:1.1.1.1\nIP4.DNS[2]:8.8.8.8\n";
        let d = parse_device_show(out);
        assert_eq!(d.mac, "02:00:00:00:00:01");
        assert_eq!(d.ipv4, [("192.168.1.50".to_string(), "255.255.255.0".to_string())]);
        assert_eq!((d.gateway.as_str(), d.dns.len()), ("192.168.1.1", 2));
    }

    #[test]
    fn netmasks() {
        assert_eq!(mask_from_prefix(24), "255.255.255.0");
        assert_eq!(mask_from_prefix(0), "0.0.0.0");
        assert_eq!(mask_from_prefix(32), "255.255.255.255");
        assert_eq!(prefix_len("255.255.254.0"), Some(23));
        assert_eq!(prefix_len("255.0.255.0"), None);
        assert_eq!(prefix_len("0.0.0.0"), Some(0));
        assert_eq!(prefix_len("abc"), None);
    }

    #[test]
    fn ipv4_parsing_is_strict() {
        assert_eq!(parse_ipv4(" 10.0.0.1 "), Some([10, 0, 0, 1]));
        for bad in ["", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1..2.3", "a.b.c.d", "1.2.3.-4", "1.2.3.4444"] { assert_eq!(parse_ipv4(bad), None, "{bad}"); }
    }

    #[test]
    fn static_ip_validation() {
        let ok = StaticIp { ip: "192.168.0.5".into(), netmask: "255.255.255.0".into(), gateway: "192.168.0.1".into(), dns1: "9.9.9.9".into(), dns2: "".into() };
        assert_eq!(validate_static(&ok), Ok(()));
        assert!(StaticIp::default().is_empty());
        assert_eq!(validate_static(&StaticIp { ip: "".into(), ..ok.clone() }), Err(IpError::Missing(IpField::Address)));
        assert_eq!(validate_static(&StaticIp { netmask: "255.0.255.0".into(), ..ok.clone() }), Err(IpError::Invalid(IpField::Netmask)));
        assert_eq!(validate_static(&StaticIp { dns2: "x".into(), ..ok.clone() }), Err(IpError::Invalid(IpField::SecondaryDns)));
        assert_eq!(validate_static(&StaticIp { gateway: "".into(), ..ok }), Ok(()), "gateway is optional");
    }

    #[test]
    fn error_causes() {
        assert_eq!(classify_error("Error: Connection activation failed: (7) Secrets were required, but not provided."), ConnectError::BadPassword);
        assert_eq!(classify_error("Error: No network with SSID 'x' found."), ConnectError::NotFound);
        assert_eq!(classify_error("Error: Timeout expired (45 seconds)"), ConnectError::Timeout);
        assert_eq!(classify_error("Error: something odd\nmore"), ConnectError::Other("Error: something odd".into()));
    }

    fn req(security: Security, pw: &str) -> ConnectRequest { ConnectRequest { ssid: "My Net".into(), security, username: String::new(), password: pw.into(), hidden: false, ip: None } }

    #[test]
    fn new_wpa2_profile_commands() {
        let c = connect_commands(&req(Security::Wpa2, "pw"), "wlan0", None);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0][..5], ["connection", "add", "type", "wifi", "ifname"]);
        let joined = c[0].join(" ");
        assert!(joined.contains("con-name My Net ssid My Net") && joined.contains("wifi-sec.key-mgmt wpa-psk wifi-sec.psk pw") && joined.contains("ipv4.method auto"), "{joined}");
        assert_eq!(c[1].join(" "), "connection up id My Net ifname wlan0");
        assert!(!c[0].iter().any(|a| a == "802-11-wireless.hidden"));
    }

    #[test]
    fn hidden_open_static_and_existing() {
        let mut r = req(Security::Open, "");
        r.hidden = true;
        r.ip = Some(StaticIp { ip: "10.0.0.2".into(), netmask: "255.255.255.0".into(), gateway: "10.0.0.1".into(), dns1: "1.1.1.1".into(), dns2: "8.8.8.8".into() });
        let c = connect_commands(&r, "wlan0", None)[0].join(" ");
        assert!(c.contains("802-11-wireless.hidden yes") && !c.contains("wifi-sec"), "{c}");
        assert!(c.contains("ipv4.method manual ipv4.addresses 10.0.0.2/24 ipv4.gateway 10.0.0.1 ipv4.dns 1.1.1.1 8.8.8.8 ipv4.ignore-auto-dns yes"), "{c}");
        let p = Profile { name: "My Net".into(), uuid: "U".into(), autoconnect: false, active: false };
        let c = connect_commands(&req(Security::Wep, "12345"), "wlan0", Some(&p));
        assert_eq!(c[0][..4], ["connection", "modify", "uuid", "U"]);
        assert!(c[0].join(" ").contains("wifi-sec.key-mgmt none wifi-sec.wep-key-type passphrase wifi-sec.wep-key0 12345"));
        assert_eq!(c[1].join(" "), "connection up uuid U ifname wlan0");
    }

    #[test]
    fn wpa3_uses_sae() {
        assert!(connect_commands(&req(Security::Wpa3, "pw"), "wlan0", None)[0].join(" ").contains("wifi-sec.key-mgmt sae wifi-sec.psk pw"));
    }

    #[test]
    fn secrets_are_redacted_for_logs() {
        for sec in [Security::Wpa2, Security::Wep, Security::Wpa3] {
            let c = connect_commands(&req(sec, "hunter2"), "wlan0", None);
            for args in &c { assert!(!redact(args).contains("hunter2"), "{sec:?}"); }
            assert!(redact(&c[0]).contains("***"));
        }
    }

    #[test]
    fn mock_connects_with_the_right_password_only() {
        let m = MockWifi::demo();
        let never = AtomicBool::new(false);
        let mut r = ConnectRequest { ssid: "Neighbour".into(), security: Security::Wpa2, username: String::new(), password: "wrong".into(), hidden: false, ip: None };
        assert_eq!(m.connect(&r, &never), Err(ConnectError::BadPassword));
        assert_eq!(m.active().as_deref(), Some("HomeNet"), "a failed attempt keeps the old connection");
        r.password = "neighbour1".into();
        assert_eq!(m.connect(&r, &never), Ok(()));
        assert_eq!(m.active().as_deref(), Some("Neighbour"));
        assert!(m.scan(false)[0].in_use && m.scan(false)[0].ssid == "Neighbour");
        assert!(profile_for(&m.profiles(), "Neighbour").is_some());
        assert_eq!(m.forget("Neighbour"), Ok(()));
        assert!(m.active().is_none() && m.forget("Neighbour").is_err());
    }

    #[test]
    fn mock_open_hidden_cancel_and_radio() {
        let m = MockWifi::demo();
        let never = AtomicBool::new(false);
        let open = ConnectRequest { ssid: "Cafe Open".into(), security: Security::Open, username: String::new(), password: String::new(), hidden: false, ip: None };
        assert_eq!(m.connect(&open, &never), Ok(()));
        let missing = ConnectRequest { ssid: "Nope".into(), hidden: true, ..open.clone() };
        assert_eq!(m.connect(&missing, &never), Err(ConnectError::NotFound));
        m.set_delay(Duration::from_millis(200));
        let cancel = AtomicBool::new(true);
        assert_eq!(m.connect(&ConnectRequest { ssid: "HomeNet".into(), security: Security::Wpa2, password: "homepass1".into(), ..open.clone() }, &cancel), Err(ConnectError::Cancelled));
        assert_eq!(m.active().as_deref(), Some("Cafe Open"));
        m.set_radio(false).unwrap();
        assert!(m.scan(true).is_empty() && m.active().is_none() && m.radio() == Some(false));
    }

    #[test]
    fn mock_details_report_the_manual_address() {
        let m = MockWifi::demo();
        let never = AtomicBool::new(false);
        let ip = StaticIp { ip: "192.168.7.9".into(), netmask: "255.255.255.0".into(), gateway: "192.168.7.1".into(), dns1: "1.1.1.1".into(), dns2: "".into() };
        let r = ConnectRequest { ssid: "Neighbour".into(), security: Security::Wpa2, username: String::new(), password: "neighbour1".into(), hidden: false, ip: Some(ip) };
        m.connect(&r, &never).unwrap();
        let d = m.details("Neighbour");
        assert!(!d.dhcp && d.ipv4[0].0 == "192.168.7.9" && d.dns == ["1.1.1.1"]);
        assert!(m.details("HomeNet").ipv4.is_empty(), "an inactive network has no address");
    }
}

fn deck_guard_deny(args: &[&str]) -> bool { crate::guard::deny(&format!("nmcli {}", args.join(" "))) }
