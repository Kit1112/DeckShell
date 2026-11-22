//! Regression gate for the stock Gamepad UI appearing next to DeckShell (docs/steam-ui-parallel-2026-11-20.md).
use std::path::{Path, PathBuf};

fn root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("..") }

fn run_shell(test: &str) {
    let out = std::process::Command::new("bash").arg(root().join("session/tests").join(test)).output().unwrap();
    assert!(out.status.success(), "{test} failed:\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn session_guard_shell_test() { run_shell("steam-ui-guard.sh"); }

#[test]
fn stock_return_shell_test() { run_shell("stock-return.sh"); }

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { if p.file_name().is_some_and(|n| n != "target") { rust_sources(&p, out) } }
        else if p.extension().is_some_and(|x| x == "rs") { out.push(p) }
    }
}

/// Code outside `#[cfg(test)]` and comments; good enough for these literal patterns.
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    let mut v = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim_start().starts_with("#[cfg(test)]") { break; }
        let code = l.split("//").next().unwrap_or("");
        v.push((i + 1, code));
    }
    v
}

/// Every one of these STARTS a Steam client when none runs, and a client started in the gamescope session
/// without -silent is the stock Gamepad UI. Requests to Steam go through steam_control::steam_pipe only.
#[test]
fn no_launcher_type_steam_calls_in_sources() {
    const FORBIDDEN: &[&str] = &["steam.sh\"", "\"xdg-open\"", "arg(\"-shutdown\")", "open/bigpicture", "open/gamepadui", "\"-gamepadui\")"];
    let mut files = Vec::new();
    rust_sources(&root().join("app/src"), &mut files);
    for c in std::fs::read_dir(root().join("crates")).unwrap().flatten() {
        let src = c.path().join("src");
        if src.is_dir() { rust_sources(&src, &mut files) }
    }
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for (n, code) in code_lines(&text) {
            for pat in FORBIDDEN {
                if code.contains(pat) { hits.push(format!("{}:{n}: {pat}", f.display())); }
            }
        }
    }
    assert!(hits.is_empty(), "launcher-type Steam calls:\n{}", hits.join("\n"));
}

/// Exit 42 makes the session shut Steam down (ending any game) and start the stock UI: only the owner's menu choice.
#[test]
fn only_the_owner_menu_requests_stock_exit() {
    let mut files = Vec::new();
    rust_sources(&root().join("app/src"), &mut files);
    let mut uses = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for (n, code) in code_lines(&text) {
            if code.contains("exit_request::request(42") || code.contains("exit_request::request(STOCK_STEAM")
                || code.contains("exit_request::request(crate::exit_request::STOCK_STEAM") {
                uses.push(format!("{}:{n}", f.display()));
            }
        }
    }
    assert_eq!(uses.len(), 1, "stock exit requested from: {uses:?}");
    assert!(uses[0].contains("data.rs"), "stock exit must come from PowerAction::StockSteam only: {uses:?}");
}

// ---------------------------------------------------------------- Steam UI never (docs/steam-ui-never-2026-11-20.md)

#[test]
fn gamepadui_watch_shell_test() { run_shell("steam-ui-never.sh"); }

/// The hard guard must stay wired: the gamescope worker scans for Steam's UI, the bridge switches off
/// "Guide button focuses Steam", and the close request is a close (never an open) over steam.pipe.
#[test]
fn steam_ui_guard_is_wired() {
    let read = |p: &str| std::fs::read_to_string(root().join(p)).unwrap();
    let gs = read("app/src/gamescope.rs");
    assert!(gs.contains("steam_ui_guard::foreign_steam_ui(") && gs.contains("steam_ui_guard::request_close"), "gamescope worker no longer runs the Steam UI guard");
    assert!(read("app/src/backend.rs").contains("steam_ui_guard::on_bridge_connected(&b)"), "bridge no longer applies the Guide setting");
    let guard = read("app/src/steam_ui_guard.rs");
    assert!(guard.contains("pub const CLOSE_URL: &str = \"steam://close/bigpicture\";"));
    assert!(guard.contains("steam_control::open_url(CLOSE_URL)"), "close must go through steam.pipe");
    let session = read("session/deckshell-session.sh");
    assert!(session.contains("start_steam\nstart_gamepadui_watch\n"), "session no longer watches for the Gamepad UI");
}
