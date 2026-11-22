//! Fail-safe for tests: under a test harness every MUTATING host call (nmcli radio/connect, rfkill,
//! iwd/bluez/SteamOS-manager busctl writes, power-profile, poweroff/reboot/suspend, systemctl, volume)
//! is refused and recorded instead of executed. See docs/test-safety.md.
//!
//! "Test harness" = any of: `cfg(test)` in this crate, `force_test_mode()` was called, env `DECKSHELL_TEST`
//! (non-empty, not "0"), or the current exe lives in a cargo `deps/` directory (every `cargo test` binary does;
//! the real `deckshell` binary does not). `DECKSHELL_TEST=0` is NOT an opt-out; the only way to touch the
//! system from a test binary is `DECKSHELL_ALLOW_HOST_MUTATION=1` (for owner-run hardware checks).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static FORCED: AtomicBool = AtomicBool::new(false);
static BLOCKED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Tests call this (e.g. from a ctor / first line) to be safe even when run from an unusual location.
pub fn force_test_mode() { FORCED.store(true, Ordering::SeqCst); }

fn exe_in_deps() -> bool {
    static V: OnceLock<bool> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.file_name().map(|n| n == "deps").unwrap_or(false))).unwrap_or(false)
    })
}

pub fn active() -> bool {
    if std::env::var("DECKSHELL_ALLOW_HOST_MUTATION").map(|v| v == "1").unwrap_or(false) && !cfg!(test) { return false; }
    cfg!(test)
        || FORCED.load(Ordering::SeqCst)
        || std::env::var("DECKSHELL_TEST").map(|v| !v.is_empty() && v != "0").unwrap_or(false)
        || exe_in_deps()
}

/// `true` = the call must NOT run (it was recorded). Call right before executing a mutating command.
pub fn deny(what: &str) -> bool {
    if !active() { return false; }
    eprintln!("[deckshell] TEST-GUARD blocked host mutation: {what}");
    if let Ok(mut b) = BLOCKED.lock() { b.push(what.to_string()); }
    true
}

/// Everything refused so far in this process.
pub fn blocked_calls() -> Vec<String> { BLOCKED.lock().map(|b| b.clone()).unwrap_or_default() }
pub fn clear_blocked() { if let Ok(mut b) = BLOCKED.lock() { b.clear(); } }
