//! Hardware/system controls for Steam Deck without root.
//! CONTRACT shared with `app`. Every getter returns None/Err gracefully on non-Deck machines.
//!
//! No external crates: D-Bus is reached through `busctl`, audio through `wpctl`/`pactl`,
//! everything else through sysfs.
use std::fs;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub mod bluetooth;
pub mod guard;
pub mod logind;
pub mod wifi;

#[derive(Debug, Clone, Default)]
pub struct Battery { pub percent: f32, pub charging: bool, pub online: Option<bool>, pub full: bool, pub minutes_left: Option<u32> }
#[derive(Debug, Clone)]
pub struct WifiNetwork { pub ssid: String, pub signal: u8, pub secure: bool, pub connected: bool }
#[derive(Debug, Clone)]
pub struct BtDevice { pub name: String, pub address: String, pub connected: bool, pub paired: bool }

// ---------------------------------------------------------------- helpers

fn unsupported(what: &str) -> Error { Error::new(ErrorKind::Unsupported, format!("{what} is not available")) }
fn failed(msg: impl Into<String>) -> Error { Error::new(ErrorKind::Other, msg.into()) }

fn read_trim(p: impl AsRef<Path>) -> Option<String> { fs::read_to_string(p).ok().map(|s| s.trim().to_string()) }
fn read_num<T: std::str::FromStr>(p: impl AsRef<Path>) -> Option<T> { read_trim(p)?.parse().ok() }

fn have(cmd: &str) -> bool { which(cmd).is_some() }
fn which(cmd: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(cmd)).find(|p| p.is_file())
}

/// Run a command (C locale), return stdout on success.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).env("LC_ALL", "C").env("LANG", "C")
        .stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if out.status.success() { Some(String::from_utf8_lossy(&out.stdout).into_owned()) } else { None }
}
fn run_ok(cmd: &str, args: &[&str]) -> Result<()> {
    if guard::deny(&format!("{cmd} {}", args.join(" "))) { return Ok(()); }
    let st = Command::new(cmd).args(args).env("LC_ALL", "C")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
    if st.success() { Ok(()) } else { Err(failed(format!("{cmd} {} failed: {st}", args.join(" ")))) }
}

// ---------------------------------------------------------------- busctl wrapper

#[derive(Clone, Copy, PartialEq)]
enum Bus { User, System }
impl Bus { fn flag(self) -> &'static str { match self { Bus::User => "--user", Bus::System => "--system" } } }

fn busctl(bus: Bus, args: &[&str]) -> Option<String> {
    let mut a = vec![bus.flag(), "--no-pager"];
    a.extend_from_slice(args);
    run("busctl", &a)
}
fn busctl_ok(bus: Bus, args: &[&str]) -> Result<()> {
    let mut a = vec![bus.flag(), "--no-pager"];
    a.extend_from_slice(args);
    run_ok("busctl", &a)
}

/// Split a busctl value line "s \"abc\"" / "u 15" / "as 2 \"a\" \"b\"" into (type, rest).
fn split_type(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let i = s.find(' ')?;
    Some((&s[..i], s[i + 1..].trim()))
}
/// Extract all double-quoted tokens (handles \" and \\ escapes).
fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut esc = false;
    for c in s.chars() {
        match (&mut cur, c) {
            (None, '"') => cur = Some(String::new()),
            (Some(_), _) if esc => { cur.as_mut().unwrap().push(c); esc = false; }
            (Some(_), '\\') => esc = true,
            (Some(_), '"') => out.push(cur.take().unwrap()),
            (Some(buf), _) => buf.push(c),
            _ => {}
        }
    }
    out
}
fn get_prop(bus: Bus, svc: &str, path: &str, iface: &str, prop: &str) -> Option<String> {
    busctl(bus, &["get-property", svc, path, iface, prop])
}
fn prop_str(bus: Bus, svc: &str, path: &str, iface: &str, prop: &str) -> Option<String> {
    let o = get_prop(bus, svc, path, iface, prop)?;
    let (_, rest) = split_type(&o)?;
    quoted(rest).into_iter().next()
}
fn prop_u32(bus: Bus, svc: &str, path: &str, iface: &str, prop: &str) -> Option<u32> {
    let o = get_prop(bus, svc, path, iface, prop)?;
    split_type(&o)?.1.parse().ok()
}
fn prop_bool(bus: Bus, svc: &str, path: &str, iface: &str, prop: &str) -> Option<bool> {
    let o = get_prop(bus, svc, path, iface, prop)?;
    match split_type(&o)?.1 { "true" => Some(true), "false" => Some(false), _ => None }
}
fn prop_strs(bus: Bus, svc: &str, path: &str, iface: &str, prop: &str) -> Option<Vec<String>> {
    let o = get_prop(bus, svc, path, iface, prop)?;
    Some(quoted(split_type(&o)?.1))
}
/// Object paths below `root` of a service (from `busctl tree`).
fn tree(bus: Bus, svc: &str) -> Vec<String> {
    busctl(bus, &["tree", "--list", svc]).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| l.starts_with('/')).collect()).unwrap_or_default()
}

// ---------------------------------------------------------------- SteamOS Manager

const SOM: &str = "com.steampowered.SteamOSManager1";
const SOM_PATH: &str = "/com/steampowered/SteamOSManager1";

/// SteamOS Manager sits on the user bus on current SteamOS; older versions use the system bus.
fn som_buses() -> [Bus; 2] { [Bus::User, Bus::System] }
fn som_u32(iface: &str, prop: &str) -> Option<u32> { som_buses().into_iter().find_map(|b| prop_u32(b, SOM, SOM_PATH, &format!("com.steampowered.SteamOSManager1.{iface}"), prop)) }
fn som_str(iface: &str, prop: &str) -> Option<String> { som_buses().into_iter().find_map(|b| prop_str(b, SOM, SOM_PATH, &format!("com.steampowered.SteamOSManager1.{iface}"), prop)) }
fn som_strs(iface: &str, prop: &str) -> Option<Vec<String>> { som_buses().into_iter().find_map(|b| prop_strs(b, SOM, SOM_PATH, &format!("com.steampowered.SteamOSManager1.{iface}"), prop)) }
fn som_set(iface: &str, prop: &str, sig: &str, val: &str) -> Result<()> {
    let iface = format!("com.steampowered.SteamOSManager1.{iface}");
    let mut last = unsupported("SteamOS Manager");
    for b in som_buses() {
        match busctl_ok(b, &["set-property", SOM, SOM_PATH, &iface, prop, sig, val]) { Ok(()) => return Ok(()), Err(e) => last = e }
    }
    Err(last)
}
fn som_call(iface: &str, method: &str) -> Result<()> {
    let iface = format!("com.steampowered.SteamOSManager1.{iface}");
    let mut last = unsupported("SteamOS Manager");
    for b in som_buses() {
        match busctl_ok(b, &["call", SOM, SOM_PATH, &iface, method]) { Ok(()) => return Ok(()), Err(e) => last = e }
    }
    Err(last)
}

// ---------------------------------------------------------------- battery

fn find_battery() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = fs::read_dir("/sys/class/power_supply").ok()?.flatten().map(|e| e.path()).collect();
    cands.sort();
    cands.into_iter().find(|p| read_trim(p.join("type")).as_deref() == Some("Battery") && read_trim(p.join("scope")).as_deref() != Some("Device"))
}

pub fn battery() -> Option<Battery> {
    let d = find_battery()?;
    let percent = read_num::<f32>(d.join("capacity"))?;
    let status = read_trim(d.join("status")).unwrap_or_default();
    let charging = status == "Charging";
    let discharging = status == "Discharging";
    // (now, full, rate) in either energy/power or charge/current units
    let (now, full, rate) = if let (Some(n), Some(r)) = (read_num::<f64>(d.join("energy_now")), read_num::<f64>(d.join("power_now"))) {
        (n, read_num::<f64>(d.join("energy_full")).unwrap_or(0.0), r)
    } else if let (Some(n), Some(r)) = (read_num::<f64>(d.join("charge_now")), read_num::<f64>(d.join("current_now"))) {
        (n, read_num::<f64>(d.join("charge_full")).unwrap_or(0.0), r)
    } else { (0.0, 0.0, 0.0) };
    let rate = rate.abs();
    let minutes_left = if rate > 0.0 && (charging || discharging) {
        let hours = if charging { (full - now).max(0.0) / rate } else { now / rate };
        let m = (hours * 60.0).round();
        if m.is_finite() && m < 60.0 * 99.0 { Some(m as u32) } else { None }
    } else { None };
    let online = fs::read_dir("/sys/class/power_supply").ok().and_then(|entries| {
        let values: Vec<u8> = entries.flatten().map(|e| e.path()).filter(|p| matches!(read_trim(p.join("type")).as_deref(), Some("Mains" | "USB" | "USB_C" | "USB_PD"))).filter_map(|p| read_num::<u8>(p.join("online"))).collect();
        if values.is_empty() { None } else { Some(values.iter().any(|&n| n != 0)) }
    });
    Some(Battery { percent, charging, online, full: status == "Full", minutes_left })
}

// ---------------------------------------------------------------- brightness
// Brightness is written and read through Steam's display API (app/src/display.rs), which owns the
// curve. This raw backlight reading is for diagnosis only (raw value, max).

fn backlight() -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir("/sys/class/backlight").ok()?.flatten().map(|e| e.path()).collect();
    v.sort();
    let pref = v.iter().position(|p| p.file_name().map(|n| n == "amdgpu_bl0").unwrap_or(false));
    match pref { Some(i) => Some(v.swap_remove(i)), None => v.into_iter().next() }
}

/// (raw, max) of the first backlight, e.g. (1209, 600000); diagnosis only.
pub fn backlight_raw() -> Option<(u32, u32)> {
    let d = backlight()?;
    Some((read_num(d.join("brightness"))?, read_num(d.join("max_brightness"))?))
}

// ---------------------------------------------------------------- volume

pub fn volume() -> Option<(f32, bool)> {
    if have("wpctl") {
        let o = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?; // "Volume: 0.50 [MUTED]"
        let v: f32 = o.split_whitespace().nth(1)?.parse().ok()?;
        return Some((v.clamp(0.0, 1.5), o.contains("MUTED")));
    }
    if have("pactl") {
        let o = run("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?;
        let pct: f32 = o.split_whitespace().find_map(|t| t.strip_suffix('%')?.parse().ok())?;
        let m = run("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?;
        return Some((pct / 100.0, m.contains("yes")));
    }
    None
}
pub fn set_volume(v: f32) -> Result<()> {
    let v = v.clamp(0.0, 1.5);
    if have("wpctl") { return run_ok("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{v:.3}")]); }
    if have("pactl") { return run_ok("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &format!("{}%", (v * 100.0).round() as u32)]); }
    Err(unsupported("audio control (wpctl/pactl)"))
}
pub fn set_muted(m: bool) -> Result<()> {
    let n = if m { "1" } else { "0" };
    if have("wpctl") { return run_ok("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", n]); }
    if have("pactl") { return run_ok("pactl", &["set-sink-mute", "@DEFAULT_SINK@", n]); }
    Err(unsupported("audio control (wpctl/pactl)"))
}

// ---------------------------------------------------------------- rfkill

/// (soft_blocked, hard_blocked, path) for the first rfkill device of a type ("wlan" / "bluetooth").
fn rfkill(kind: &str) -> Option<(bool, bool, PathBuf)> {
    for e in fs::read_dir("/sys/class/rfkill").ok()?.flatten() {
        let p = e.path();
        if read_trim(p.join("type")).as_deref() == Some(kind) {
            return Some((read_trim(p.join("soft"))? == "1", read_trim(p.join("hard")).as_deref() == Some("1"), p));
        }
    }
    None
}
fn set_rfkill(kind: &str, on: bool) -> Result<()> {
    if have("rfkill") { if run_ok("rfkill", &[if on { "unblock" } else { "block" }, kind]).is_ok() { return Ok(()); } }
    let (_, _, p) = rfkill(kind).ok_or_else(|| unsupported("rfkill"))?;
    fs::write(p.join("soft"), if on { "0" } else { "1" })
}

// ---------------------------------------------------------------- Wi-Fi (NetworkManager or iwd)

const IWD: &str = "net.connman.iwd";

fn have_nm() -> bool { have("nmcli") && run("nmcli", &["-t", "-f", "RUNNING", "general"]).map(|s| s.trim() == "running").unwrap_or(false) }
fn iwd_paths(iface: &str) -> Vec<String> {
    // objects exporting the interface, via introspection of each node
    tree(Bus::System, IWD).into_iter()
        .filter(|p| busctl(Bus::System, &["introspect", IWD, p, iface]).map(|s| s.contains(iface)).unwrap_or(false))
        .collect()
}
fn iwd_devices() -> Vec<String> { iwd_paths("net.connman.iwd.Device") }

fn parse_wifi_radio(s: &str) -> Option<bool> {
    match s.trim() { "enabled" => Some(true), "disabled" => Some(false), _ => None }
}

pub fn wifi_enabled() -> Option<bool> {
    if have_nm() {
        return parse_wifi_radio(&run("nmcli", &["radio", "wifi"])?);
    }
    if let Some(d) = iwd_devices().first() {
        if let Some(p) = prop_bool(Bus::System, IWD, d, "net.connman.iwd.Device", "Powered") { return Some(p); }
    }
    let (soft, hard, _) = rfkill("wlan")?;
    Some(!soft && !hard)
}
pub fn set_wifi_enabled(on: bool) -> Result<()> {
    if have_nm() { return run_ok("nmcli", &["radio", "wifi", if on { "on" } else { "off" }]); }
    if on { let _ = set_rfkill("wlan", true); }
    if let Some(d) = iwd_devices().first() {
        return busctl_ok(Bus::System, &["set-property", IWD, d, "net.connman.iwd.Device", "Powered", "b", if on { "true" } else { "false" }]);
    }
    set_rfkill("wlan", on)
}
pub fn wifi_networks() -> Vec<WifiNetwork> {
    if have_nm() {
        let o = run("nmcli", &["-t", "-e", "yes", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]).unwrap_or_default();
        let mut v: Vec<WifiNetwork> = o.lines().filter_map(|l| {
            let f = split_escaped(l);
            if f.len() < 4 || f[1].is_empty() { return None; }
            Some(WifiNetwork { ssid: f[1].clone(), signal: f[2].parse().unwrap_or(0), secure: !f[3].is_empty() && f[3] != "--", connected: f[0] == "*" })
        }).collect();
        v.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.signal.cmp(&a.signal)));
        return v;
    }
    // iwd: Station.GetOrderedNetworks -> a(on) (path, signal in 1/100 dBm)
    let mut out = Vec::new();
    for st in iwd_paths("net.connman.iwd.Station") {
        let Some(o) = busctl(Bus::System, &["call", IWD, &st, "net.connman.iwd.Station", "GetOrderedNetworks"]) else { continue };
        // "a(on) 3 \"/net/...\" -4500 \"/net/...\" -6000"
        let toks: Vec<&str> = o.split_whitespace().skip(2).collect();
        for pair in toks.chunks(2) {
            if pair.len() < 2 { continue; }
            let path = pair[0].trim_matches('"');
            let dbm100: i32 = pair[1].parse().unwrap_or(-10000);
            let net = "net.connman.iwd.Network";
            let Some(ssid) = prop_str(Bus::System, IWD, path, net, "Name") else { continue };
            let ty = prop_str(Bus::System, IWD, path, net, "Type").unwrap_or_default();
            let connected = prop_bool(Bus::System, IWD, path, net, "Connected").unwrap_or(false);
            // map -100..-50 dBm to 0..100 %
            let dbm = dbm100 as f32 / 100.0;
            let signal = (((dbm + 100.0) * 2.0).clamp(0.0, 100.0)) as u8;
            out.push(WifiNetwork { ssid, signal, secure: ty != "open", connected });
        }
    }
    out
}
/// Split a `nmcli -t -e yes` line on unescaped ':'.
fn split_escaped(l: &str) -> Vec<String> {
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

// ---------------------------------------------------------------- Bluetooth (BlueZ)

const BLUEZ: &str = "org.bluez";

fn bt_adapter() -> Option<String> {
    tree(Bus::System, BLUEZ).into_iter().find(|p| p.starts_with("/org/bluez/hci") && p.matches('/').count() == 3)
}
pub fn bluetooth_enabled() -> Option<bool> {
    if let Some(a) = bt_adapter() {
        if let Some(p) = prop_bool(Bus::System, BLUEZ, &a, "org.bluez.Adapter1", "Powered") { return Some(p); }
    }
    let (soft, hard, _) = rfkill("bluetooth")?;
    Some(!soft && !hard)
}
pub fn set_bluetooth_enabled(on: bool) -> Result<()> {
    if on { let _ = set_rfkill("bluetooth", true); }
    if let Some(a) = bt_adapter() {
        return busctl_ok(Bus::System, &["set-property", BLUEZ, &a, "org.bluez.Adapter1", "Powered", "b", if on { "true" } else { "false" }]);
    }
    set_rfkill("bluetooth", on)
}
pub fn bluetooth_devices() -> Vec<BtDevice> {
    let dev = "org.bluez.Device1";
    tree(Bus::System, BLUEZ).into_iter().filter(|p| p.contains("/dev_")).filter_map(|p| {
        let address = prop_str(Bus::System, BLUEZ, &p, dev, "Address")?;
        let name = prop_str(Bus::System, BLUEZ, &p, dev, "Alias").or_else(|| prop_str(Bus::System, BLUEZ, &p, dev, "Name")).unwrap_or_else(|| address.clone());
        Some(BtDevice {
            name, address,
            connected: prop_bool(Bus::System, BLUEZ, &p, dev, "Connected").unwrap_or(false),
            paired: prop_bool(Bus::System, BLUEZ, &p, dev, "Paired").unwrap_or(false),
        })
    }).collect()
}

// ---------------------------------------------------------------- performance (SteamOS Manager)
//
// Interfaces seen on SteamOS 3.8: GpuPerformanceLevel1 (auto/low/high/manual/profile_peak),
// GpuPowerProfile1 (CAPPED/UNCAPPED...), TdpLimit1 (TdpLimit/Min/Max), CpuScaling1, CpuBoost1,
// BatteryChargeLimit1, FanControl1. Older builds had Manager.PerformanceProfile; we try that first.

pub fn performance_profiles() -> Vec<String> {
    if let Some(v) = som_strs("PerformanceProfile1", "AvailablePerformanceProfiles") { return v; }
    som_strs("GpuPerformanceLevel1", "AvailableGpuPerformanceLevels").unwrap_or_default()
}
pub fn performance_profile() -> Option<String> {
    som_str("PerformanceProfile1", "PerformanceProfile").or_else(|| som_str("GpuPerformanceLevel1", "GpuPerformanceLevel"))
}
pub fn set_performance_profile(p: &str) -> Result<()> {
    if som_strs("PerformanceProfile1", "AvailablePerformanceProfiles").is_some() {
        return som_set("PerformanceProfile1", "PerformanceProfile", "s", p);
    }
    if !performance_profiles().iter().any(|x| x == p) { return Err(Error::new(ErrorKind::InvalidInput, format!("unknown profile {p}"))); }
    som_set("GpuPerformanceLevel1", "GpuPerformanceLevel", "s", p)
}
// SteamOS Manager's cached property alone is not hardware readback. Locate the
// AMD GPU's hwmon cap by name (the hwmon number varies across boots).
fn tdp_cap_at(root: &Path) -> Option<u32> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if read_trim(path.join("name")).as_deref() != Some("amdgpu") { continue; }
        let microwatts: u64 = read_num(path.join("power1_cap"))?;
        if microwatts % 1_000_000 != 0 { return None; }
        return u32::try_from(microwatts / 1_000_000).ok();
    }
    None
}
pub fn tdp_limit() -> Option<(u32, u32, u32)> {
    let min = som_u32("TdpLimit1", "TdpLimitMin")?;
    let max = som_u32("TdpLimit1", "TdpLimitMax")?;
    let current = tdp_cap_at(Path::new("/sys/class/hwmon"))?;
    (min < max && (min..=max).contains(&current)).then_some((current, min, max))
}
pub fn set_tdp_limit(w: u32) -> Result<()> {
    let (_, min, max) = tdp_limit().ok_or_else(|| unsupported("TDP readback"))?;
    if !(min..=max).contains(&w) { return Err(Error::new(ErrorKind::InvalidInput, "TDP outside hardware range")); }
    som_set("TdpLimit1", "TdpLimit", "u", &w.to_string())?;
    for _ in 0..10 {
        if tdp_cap_at(Path::new("/sys/class/hwmon")) == Some(w) { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(failed("TDP sysfs readback did not match request"))
}
/// Manual GPU clock read from amdgpu's kernel state, with manager-provided limits.
fn gpu_sysfs_at(root: &Path) -> Option<(bool, Option<u32>)> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if read_trim(path.join("name")).as_deref() != Some("amdgpu") { continue; }
        let device = path.join("device");
        let mode = read_trim(device.join("power_dpm_force_performance_level"))?;
        if mode == "auto" { return Some((false, None)); }
        if mode != "manual" { return None; }
        let clock = (|| {
            let od = read_trim(device.join("pp_od_clk_voltage"))?;
            let mut lines = od.lines();
            if lines.next()? != "OD_SCLK:" { return None; }
            let parse = |line: &str, prefix: &str| -> Option<u32> {
                line.trim().strip_prefix(prefix)?.trim().strip_suffix("Mhz")?.trim().parse().ok()
            };
            let min = parse(lines.next()?, "0:")?;
            let max = parse(lines.next()?, "1:")?;
            (min == max).then_some(max)
        })();
        // Even an unpinned/unreadable clock must leave the manual switch usable
        // so the user can return the GPU to auto.
        return Some((true, clock));
    }
    None
}
pub fn gpu_clock() -> Option<(bool, Option<u32>, u32, u32)> {
    let min = som_u32("GpuPerformanceLevel1", "ManualGpuClockMin")?;
    let max = som_u32("GpuPerformanceLevel1", "ManualGpuClockMax")?;
    let (on, clock) = gpu_sysfs_at(Path::new("/sys/class/hwmon"))?;
    (min < max && clock.map_or(true, |n| (min..=max).contains(&n))).then_some((on, clock, min, max))
}
pub fn set_gpu_clock(on: bool, mhz: u32) -> Result<()> {
    let min = som_u32("GpuPerformanceLevel1", "ManualGpuClockMin").ok_or_else(|| unsupported("GPU clock limits"))?;
    let max = som_u32("GpuPerformanceLevel1", "ManualGpuClockMax").ok_or_else(|| unsupported("GPU clock limits"))?;
    if !(min..=max).contains(&mhz) { return Err(Error::new(ErrorKind::InvalidInput, "GPU clock outside hardware range")); }
    som_set("GpuPerformanceLevel1", "GpuPerformanceLevel", "s", if on { "manual" } else { "auto" })?;
    if on { som_set("GpuPerformanceLevel1", "ManualGpuClock", "u", &mhz.to_string())?; }
    std::thread::sleep(std::time::Duration::from_millis(300));
    for _ in 0..10 {
        if gpu_sysfs_at(Path::new("/sys/class/hwmon")) == Some((on, on.then_some(mhz))) { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(failed("GPU sysfs readback did not match request"))
}
/// Extra: battery charge limit percent (SteamOS Manager BatteryChargeLimit1).
pub fn battery_charge_limit() -> Option<u32> { som_u32("BatteryChargeLimit1", "MaxChargeLevel") }

// ---------------------------------------------------------------- power / session

const LOGIN1: &str = "org.freedesktop.login1";
fn logind(method: &str) -> Result<()> {
    busctl_ok(Bus::System, &["call", LOGIN1, "/org/freedesktop/login1", "org.freedesktop.login1.Manager", method, "b", "false"])
}
fn power_action(method: &str, systemctl: &str) -> Result<()> {
    logind(method).or_else(|_| run_ok("systemctl", &[systemctl]))
}
pub fn suspend() -> Result<()> { power_action("Suspend", "suspend") }
pub fn reboot() -> Result<()> { power_action("Reboot", "reboot") }
pub fn poweroff() -> Result<()> { power_action("PowerOff", "poweroff") }
pub fn switch_to_desktop() -> Result<()> {
    if have("steamos-session-select") { return run_ok("steamos-session-select", &["plasma"]); }
    som_call("SessionManagement1", "SwitchToDesktopMode")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_busctl() {
        assert_eq!(split_type("u 15"), Some(("u", "15")));
        assert_eq!(quoted("2 \"performance\" \"power\\\"save\""), vec!["performance", "power\"save"]);
    }
    #[test]
    fn nm_split() { assert_eq!(split_escaped("*:My\\:Net:80:WPA2"), vec!["*", "My:Net", "80", "WPA2"]); }
    #[test]
    fn getters_do_not_panic() {
        let _ = battery(); let _ = backlight_raw(); let _ = volume(); let _ = tdp_limit();
        let _ = performance_profiles();
    }
}

#[cfg(test)]
mod tdp_readback_tests {
    use super::*;
    #[test]
    fn reads_amd_cap_and_rejects_missing_or_fractional_watts() {
        let root = std::env::temp_dir().join(format!("deckshell-tdp-test-{}", std::process::id()));
        let gpu = root.join("hwmon42");
        fs::create_dir_all(&gpu).unwrap();
        fs::write(gpu.join("name"), "amdgpu\n").unwrap();
        fs::write(gpu.join("power1_cap"), "14000000\n").unwrap();
        assert_eq!(tdp_cap_at(&root), Some(14));
        fs::write(gpu.join("power1_cap"), "14500000\n").unwrap();
        assert_eq!(tdp_cap_at(&root), None);
        fs::remove_file(gpu.join("power1_cap")).unwrap();
        assert_eq!(tdp_cap_at(&root), None);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod gpu_readback_tests {
    use super::*;
    #[test]
    fn manual_mode_survives_unpinned_or_unreadable_clock() {
        let root = std::env::temp_dir().join(format!("deckshell-gpu-test-{}", std::process::id()));
        let gpu = root.join("hwmon42");
        let dev = gpu.join("device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(gpu.join("name"), "amdgpu\n").unwrap();
        fs::write(dev.join("power_dpm_force_performance_level"), "auto\n").unwrap();
        assert_eq!(gpu_sysfs_at(&root), Some((false, None)));
        fs::write(dev.join("power_dpm_force_performance_level"), "manual\n").unwrap();
        fs::write(dev.join("pp_od_clk_voltage"), "OD_SCLK:\n0: 300Mhz\n1: 300Mhz\n").unwrap();
        assert_eq!(gpu_sysfs_at(&root), Some((true, Some(300))));
        fs::write(dev.join("pp_od_clk_voltage"), "OD_SCLK:\n0: 200Mhz\n1: 300Mhz\n").unwrap();
        assert_eq!(gpu_sysfs_at(&root), Some((true, None)));
        fs::write(dev.join("pp_od_clk_voltage"), "unreadable").unwrap();
        assert_eq!(gpu_sysfs_at(&root), Some((true, None)));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod wifi_radio_tests {
    use super::parse_wifi_radio;

    #[test]
    fn unknown_radio_output_is_not_disabled() {
        assert_eq!(parse_wifi_radio("enabled\n"), Some(true));
        assert_eq!(parse_wifi_radio("disabled\n"), Some(false));
        assert_eq!(parse_wifi_radio(""), None);
        assert_eq!(parse_wifi_radio("unexpected"), None);
    }
}
