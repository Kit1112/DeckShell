//! logind power hooks (org.freedesktop.login1), without a D-Bus library:
//!  * [`inhibit_delay`]  - a *delay* inhibitor lock (`shutdown:sleep`). While held, logind announces `PrepareForSleep(true)` /
//!    `PrepareForShutdown(true)` and then waits (at most `InhibitDelayMaxUSec`, see [`inhibit_delay_max`]) until the lock is
//!    dropped before it really suspends / powers off. The lock is a `systemd-inhibit ... sleep infinity` child process that
//!    dies with us (PDEATHSIG) and is killed on drop = "release the lock".
//!  * [`watch`] - delivers `PrepareForSleep` / `PrepareForShutdown` signals (via `gdbus monitor`, which does not need
//!    BecomeMonitor privileges like busctl/dbus-monitor do).
//!
//! Inside a distrobox container the system bus is not reachable; commands are then run through `distrobox-host-exec`
//! (override with `DECKSHELL_HOST_EXEC`, set it to an empty string to force local execution).
use std::io::{BufRead, BufReader, Error, ErrorKind, Result};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareEvent {
    /// PrepareForSleep(true): system is about to suspend (false = just resumed).
    Sleep(bool),
    /// PrepareForShutdown(true): system is about to power off / reboot (false = cancelled).
    Shutdown(bool),
}

/// Prefix to run host commands from a container (None = run locally).
fn host_prefix() -> Option<Vec<String>> {
    if let Ok(v) = std::env::var("DECKSHELL_HOST_EXEC") {
        let parts: Vec<String> = v.split_whitespace().map(String::from).collect();
        return if parts.is_empty() { None } else { Some(parts) };
    }
    if Path::new("/run/dbus/system_bus_socket").exists() {
        return None;
    }
    if crate::which("distrobox-host-exec").is_some() { Some(vec!["distrobox-host-exec".into()]) } else { None }
}

fn host_cmd(prog: &str, args: &[&str]) -> Command {
    // Test harness: never reach the host. Read-only callers get a harmless `false`, which they treat as "unavailable".
    if crate::guard::active() { crate::guard::deny(&format!("host_cmd {prog} {}", args.join(" "))); return Command::new("false"); }
    match host_prefix() {
        Some(p) => {
            let mut c = Command::new(&p[0]);
            c.args(&p[1..]).arg(prog).args(args);
            c
        }
        None => {
            let mut c = Command::new(prog);
            c.args(args);
            c
        }
    }
}

/// A held delay lock. Dropping it releases the lock (logind then continues the suspend / power-off).
pub struct Inhibitor {
    child: Option<Child>,
}

impl Inhibitor {
    pub fn is_held(&self) -> bool {
        self.child.is_some()
    }
    pub fn release(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
impl Drop for Inhibitor {
    fn drop(&mut self) {
        self.release();
    }
}

/// Take a delay inhibitor for `what` (e.g. `"shutdown:sleep"`).
pub fn inhibit_delay(what: &str, who: &str, why: &str) -> Result<Inhibitor> {
    let what_arg = format!("--what={what}");
    let who_arg = format!("--who={who}");
    let why_arg = format!("--why={why}");
    let mut cmd = host_cmd("systemd-inhibit", &[&what_arg, &who_arg, &why_arg, "--mode=delay", "sleep", "infinity"]);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            // the lock must not outlive DeckShell (a stale delay lock only costs InhibitDelayMaxSec, but still)
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let child = cmd.spawn()?;
    Ok(Inhibitor { child: Some(child) })
}

/// logind `InhibitDelayMaxUSec` (default 5 s).
pub fn inhibit_delay_max() -> Duration {
    let out = host_cmd("busctl", &["--system", "--no-pager", "get-property", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "InhibitDelayMaxUSec"])
        .env("LC_ALL", "C").stdin(Stdio::null()).stderr(Stdio::null()).output().ok();
    out.filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()))
        .map(Duration::from_micros)
        .unwrap_or(Duration::from_secs(5))
}

/// Parse a `gdbus monitor` line: `/org/freedesktop/login1: org.freedesktop.login1.Manager.PrepareForSleep (true,)`.
pub fn parse_signal_line(line: &str) -> Option<PrepareEvent> {
    let (head, args) = line.split_once(" (")?;
    let member = head.rsplit('.').next()?.trim();
    // Only an actual D-Bus boolean is an event. Unknown or truncated output
    // must not be interpreted as `false` (resume / shutdown cancellation).
    let val = match args.trim() {
        "true,)" => true,
        "false,)" => false,
        _ => return None,
    };
    let ok_iface = head.contains("org.freedesktop.login1.Manager");
    match (ok_iface, member) {
        (true, "PrepareForSleep") => Some(PrepareEvent::Sleep(val)),
        (true, "PrepareForShutdown") => Some(PrepareEvent::Shutdown(val)),
        _ => None,
    }
}

/// Handle of the signal watcher; dropping it stops the monitor process.
pub struct Watcher {
    child: Child,
    stop: Arc<AtomicBool>,
}
impl Watcher {
    pub fn is_alive(&mut self) -> Result<bool> { Ok(self.child.try_wait()?.is_none()) }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Call `cb` (on a background thread) for every PrepareForSleep / PrepareForShutdown signal.
pub fn watch(cb: impl Fn(PrepareEvent) + Send + 'static) -> Result<Watcher> {
    // stdbuf: gdbus prints through stdio, which is block-buffered on a pipe
    let have_stdbuf = crate::which("stdbuf").is_some();
    let mut args: Vec<&str> = vec![];
    if have_stdbuf {
        args.extend(["-oL", "gdbus"]);
    }
    args.extend(["monitor", "--system", "--dest", "org.freedesktop.login1", "--object-path", "/org/freedesktop/login1"]);
    let mut cmd = host_cmd(if have_stdbuf { "stdbuf" } else { "gdbus" }, &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().ok_or_else(|| Error::new(ErrorKind::Other, "no stdout"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let st = stop.clone();
    std::thread::Builder::new().name("logind-watch".into()).spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if st.load(Ordering::Relaxed) {
                break;
            }
            if let Some(ev) = parse_signal_line(&line) {
                cb(ev);
            }
        }
    })?;
    Ok(Watcher { child, stop })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse() {
        assert_eq!(parse_signal_line("/org/freedesktop/login1: org.freedesktop.login1.Manager.PrepareForSleep (true,)"), Some(PrepareEvent::Sleep(true)));
        assert_eq!(parse_signal_line("/org/freedesktop/login1: org.freedesktop.login1.Manager.PrepareForShutdown (false,)"), Some(PrepareEvent::Shutdown(false)));
        for args in ["", "garbage,)", "trueish,)", "false", "false, extra)"] {
            assert_eq!(parse_signal_line(&format!("/org/freedesktop/login1: org.freedesktop.login1.Manager.PrepareForSleep ({args}")), None);
        }
        assert_eq!(parse_signal_line("/org/freedesktop/login1: org.freedesktop.login1.Manager.SessionNew ('1', objectpath '/x')"), None);
    }
}
