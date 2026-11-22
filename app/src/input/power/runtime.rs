//! Exclusive physical Power ownership, only inside our supervised Gaming unit.
use super::{device::Device, ownership::{self, Handoff}, PowerGate, Press};
use std::{io, fs, path::PathBuf, process::Command, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}, mpsc}, time::{Duration, Instant}};
use deck_system::logind::{self, PrepareEvent};

const UNIT: &str = "deckshell-ui.service";
const STOCK: &str = "steamos-powerbuttond.service";
/// Starting the stock owner must never replace a queued job. Its `Requisite=gamescope-session.service`
/// puts a verify-active job on gamescope; in the default replace mode that cancels gamescope's queued stop
/// job during a session teardown, and gamescope outlives its logind session (docs/drm-root-cause-2026-11-15.md).
const START_STOCK: [&str; 3] = ["--job-mode=fail", "start", STOCK];
/// How often the stock owner's state is re-checked while we own Power (see the health block in `run`).
const STOCK_CHECK: Duration = Duration::from_secs(6);

fn ctl(args: &[&str]) -> io::Result<String> {
    if !args.iter().any(|a| matches!(*a, "is-active" | "show" | "is-enabled" | "status" | "is-failed")) && deck_system::guard::deny(&format!("systemctl --user {}", args.join(" "))) { return Ok(String::new()); }
    let o = Command::new("timeout").args(["5s", "systemctl", "--user"]).args(args).output()?;
    if !o.status.success() { return Err(io::Error::other(format!("systemctl {:?}: {}", args, o.status))); }
    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
}

fn ui_call(f: impl FnOnce() + Send + 'static) -> io::Result<()> {
    let (tx, rx) = mpsc::sync_channel(1);
    slint::invoke_from_event_loop(move || { f(); let _ = tx.send(()); }).map_err(io::Error::other)?;
    rx.recv_timeout(Duration::from_secs(3)).map_err(io::Error::other)
}

struct Owner { device: Device, gate: PowerGate, marker: PathBuf }
impl Handoff for Owner {
    fn ready(&mut self) -> io::Result<()> {
        let pid = std::process::id().to_string();
        if ctl(&["show", UNIT, "-p", "MainPID", "--value"])? != pid {
            return Err(io::Error::other("not the supervised UI main process"));
        }
        let recover = std::env::current_exe()?.parent().unwrap().parent().unwrap().join("session/power-recover.sh");
        if !recover.is_file() || !ctl(&["show", UNIT, "-p", "ExecStopPost", "--value"])?.contains(recover.to_str().unwrap_or("!")) {
            return Err(io::Error::other("independent power recovery guard absent"));
        }
        if ctl(&["show", STOCK, "-p", "ActiveState", "--value"])? != "active" {
            return Err(io::Error::other("stock owner is not active before handoff"));
        }
        ui_call(|| {})?;
        // Set before stop: even a SIGKILL inside the handoff has recovery.
        fs::OpenOptions::new().write(true).create_new(true).open(&self.marker)?;
        Ok(())
    }
    fn stop_stock(&mut self) -> io::Result<()> { ctl(&["stop", STOCK]).map(|_| ()) }
    fn stock_stopped(&mut self) -> io::Result<bool> { Ok(ctl(&["show", STOCK, "-p", "ActiveState", "--value"])? == "inactive") }
    fn drain_and_key_up(&mut self) -> io::Result<bool> { self.device.drain_and_key_up() }
    fn arm(&mut self) -> io::Result<()> {
        if self.gate.arm(true, true, true) { Ok(()) } else { Err(io::Error::other("cannot arm Power")) }
    }
    fn disarm(&mut self) { self.gate.disarm(); }
    fn start_stock(&mut self) -> io::Result<()> { ctl(&START_STOCK).map(|_| ()) }
}
impl Drop for Owner {
    fn drop(&mut self) {
        // The independent ExecStopPost also handles SIGKILL/panic/process exit.
        self.gate.disarm();
        if self.marker.exists() { let _ = ctl(&START_STOCK); }
    }
}

#[derive(Default)]
struct ResumeGate { epoch: u64, waiting: bool, short_at: Option<Instant> }
impl ResumeGate {
    fn transition(&mut self, epoch: u64, sleeping: bool, gate: &mut PowerGate) {
        if self.epoch != epoch {
            self.epoch = epoch; self.waiting = !sleeping; self.short_at = None; gate.disarm();
        }
    }
}

fn run() -> io::Result<()> {
    let sleeping = Arc::new(AtomicBool::new(false));
    let epoch = Arc::new(AtomicU64::new(0));
    let (s, e) = (sleeping.clone(), epoch.clone());
    // This worker lives as long as the reader; PDEATHSIG cannot lose its parent
    // immediately after setup, unlike the former temporary setup threads.
    let mut monitor = logind::watch(move |event| {
        if let PrepareEvent::Sleep(value) = event {
            s.store(value, Ordering::SeqCst); e.fetch_add(1, Ordering::SeqCst);
        }
    })?;
    let mut owner = Owner {
        device: Device::open()?, gate: PowerGate::default(),
        marker: PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| io::Error::other("runtime dir absent"))?).join("deckshell-power-claimed"),
    };
    ownership::acquire(&mut owner)?;
    eprintln!("[power] armed: verified device, stock stopped, independent recovery ready");
    let mut resume = ResumeGate::default();
    let mut health = Instant::now();
    let mut stock_checked = Instant::now();
    loop {
        let is_sleeping = sleeping.load(Ordering::SeqCst);
        resume.transition(epoch.load(Ordering::SeqCst), is_sleeping, &mut owner.gate);
        if !is_sleeping && resume.waiting && owner.device.drain_and_key_up()? {
            owner.gate.arm(true, true, true); resume.waiting = false;
            eprintln!("[power] rearmed after resume: queue drained, key up");
        }
        if resume.short_at.is_some_and(|t| t.elapsed() > Duration::from_secs(20)) {
            return Err(io::Error::other("sleep request did not reach logind"));
        }
        let event = owner.device.next(50)?;
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) } != 0 { return Err(io::Error::last_os_error()); }
        let now = Duration::new(now.tv_sec as u64, now.tv_nsec as u32);
        let press = if is_sleeping || resume.waiting { None }
            else { event.and_then(|v| owner.gate.event(v.event_type, v.code, v.value, v.at)).or_else(|| owner.gate.tick(now)) };
        if event.is_some_and(|v| v.event_type == 0 && v.code == 3) { return Err(io::Error::other("power events lost")); }
        match press {
            Some(Press::Short) => {
                owner.gate.disarm(); resume.short_at = Some(Instant::now());
                eprintln!("[power] short: requesting OS sleep once");
                ui_call(|| { crate::with_app(|app| app.act(crate::actions::Act::Power(crate::data::PowerAction::Sleep))); })?;
            }
            Some(Press::Long) => {
                eprintln!("[power] long: opening own power menu");
                ui_call(|| { crate::with_app(|app| app.hk_power_menu()); })?;
            }
            None => {}
        }
        if health.elapsed() > Duration::from_secs(2) {
            if !monitor.is_alive()? { return Err(io::Error::other("logind monitor lost")); }
            ui_call(|| {})?;
            // Two processes (timeout + systemctl) per check: the stock owner is checked every 6 s, liveness every 2 s.
            if stock_checked.elapsed() > STOCK_CHECK {
                if !owner.stock_stopped()? { return Err(io::Error::other("stock owner unexpectedly active")); }
                stock_checked = Instant::now();
            }
            health = Instant::now();
        }
    }
}

pub fn start() {
    if std::env::var("DECKSHELL_POWER_MANAGED").as_deref() != Ok("1") || crate::actions::dryrun()
        || std::env::var("DECKSHELL_MOCK").is_ok_and(|v| v != "0" && !v.is_empty()) { return; }
    let _ = slint::invoke_from_event_loop(|| {
        std::thread::spawn(|| {
            let result = std::panic::catch_unwind(run);
            match result {
                Ok(Err(e)) => eprintln!("[power] owner failed: {e}; falling back to stock"),
                Err(_) => eprintln!("[power] reader panicked; falling back to stock"),
                Ok(Ok(())) => {}
            }
            // systemd runs recovery (ExecStopPost restores the stock Power daemon) even if UI delivery is
            // unavailable. 44 = restart the UI; the wrapper counts it toward its crash-loop limit. It used to be
            // 42, the owner's explicit "return to stock Steam": a lost Power reader then shut Steam down (with
            // any running game) and brought up the stock Gamepad UI (Deck 2026-11-20 02:09,
            // docs/steam-ui-parallel-2026-11-20.md).
            crate::exit_request::request(crate::exit_request::RESTART_UI);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stock_daemon_start_never_replaces_a_queued_job() {
        // A plain start cancels gamescope's queued stop job through Requisite= (docs/drm-root-cause-2026-11-15.md).
        assert_eq!(START_STOCK, ["--job-mode=fail", "start", "steamos-powerbuttond.service"]);
    }
    #[test]
    fn full_sleep_cycle_between_polls_still_requires_a_drain_before_arm() {
        let mut gate = PowerGate::default(); gate.arm(true, true, true);
        let mut r = ResumeGate::default();
        r.transition(2, false, &mut gate);
        assert!(r.waiting);
        assert_eq!(gate.event(1, 116, 1, Duration::from_secs(5)), None);
        assert_eq!(gate.event(1, 116, 0, Duration::from_secs(6)), None);
        assert!(gate.arm(true, true, true));
        assert_eq!(gate.event(1, 116, 1, Duration::from_secs(7)), None);
        assert_eq!(gate.event(1, 116, 0, Duration::from_millis(7100)), Some(Press::Short));
    }
}
