//! Test guard: PATH holds trap scripts for every mutating host tool; the test fails if any was run.
//! Single test in its own process, so changing PATH here cannot race other tests. See docs/test-safety.md.
use std::fs;
use std::os::unix::fs::PermissionsExt;

pub const TRAPPED: [&str; 12] = ["nmcli", "rfkill", "bluetoothctl", "busctl", "systemctl", "systemd-run", "wpctl", "pactl", "steamos-session-select", "iwctl", "powerprofilesctl", "xdg-open"];

#[test]
fn no_mutating_host_tool_is_ever_run() {
    let dir = std::env::temp_dir().join(format!("deckshell-trap-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let log = dir.join("calls.log");
    for t in TRAPPED {
        let p = dir.join(t);
        fs::write(&p, trap_script(t, &log.display().to_string())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PATH", format!("{}:/usr/bin:/bin", dir.display()));
    std::env::remove_var("DECKSHELL_DRYRUN");

    let _ = deck_system::set_wifi_enabled(false);
    let _ = deck_system::set_wifi_enabled(true);
    let _ = deck_system::set_bluetooth_enabled(false);
    let _ = deck_system::set_volume(0.3);
    let _ = deck_system::set_muted(true);
    let _ = deck_system::set_performance_profile("low-power");
    let _ = deck_system::set_tdp_limit(8);
    let _ = deck_system::set_gpu_clock(true, 1000);
    let _ = deck_system::suspend();
    let _ = deck_system::reboot();
    let _ = deck_system::poweroff();
    let blocked = deck_system::guard::blocked_calls();
    assert!(!blocked.is_empty(), "guard recorded nothing: it is not active under cargo test");
    assert!(blocked.iter().any(|b| b.contains("radio wifi off") || b.contains("block wlan")), "wifi off must have been blocked: {blocked:?}");
    let calls = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    assert!(calls.is_empty(), "a mutating host tool was invoked from a test:\n{calls}");
}

/// Read-only calls (get-property, `nmcli radio wifi`, systemctl is-active...) are logged nowhere; only mutating ones count.
pub fn trap_script(tool: &str, log: &str) -> String {
    let pat = match tool {
        "busctl" => " set-property | call ",
        "nmcli" => " on| off| connect| disconnect| delete| modify| up| down| add",
        "systemctl" => " start| stop| restart| kill| enable| disable| mask| poweroff| reboot| suspend",
        "wpctl" => " set-",
        "pactl" => " set-",
        _ => "",
    };
    let cond = if pat.is_empty() { "true".to_string() } else { format!("case \" $* \" in *{}*) true;; *) false;; esac", pat.replace('|', "*|*")) };
    format!("#!/bin/sh\nif {cond}; then echo \"{tool} $*\" >> '{log}'; fi\nexit 1\n")
}
