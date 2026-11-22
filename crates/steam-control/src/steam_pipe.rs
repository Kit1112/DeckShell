//! The only way DeckShell hands a command line to Steam: the running client's FIFO `~/.steam/steam.pipe`.
//!
//! Why (docs/steam-ui-parallel-2026-11-20.md): `steam.sh <url>`, `xdg-open steam://…` and `steam -shutdown` are
//! LAUNCHERS. When no Steam instance owns the pipe they start a new client, and a Steam client started inside the
//! gamescope session without `-silent` logs `ComputeStartupMode: forcing gamepadui for steamdeck + gamescope` — the
//! stock Gamepad UI next to DeckShell. Seen on the Deck 2026-11-18 14:03:34 (`steam -shutdown` after Steam had
//! already exited) and 2026-11-20 15:53:33 (`steam.sh steam://openurl/…/account/` inside Steam's own 1 s self-restart
//! window; Hollow Knight was then started under that stock UI).
//!
//! Opening a FIFO write-only with `O_NONBLOCK` fails with `ENXIO` when nothing reads it, so this path cannot start
//! Steam at all: no reader, no delivery, an error. The message format is the one Valve's
//! `steam-runtime-steam-remote` (`_srt_steam_command_via_pipe`) writes and the client logs as `ExecCommandLine`:
//! `'~/.steam/root/ubuntu12_32/steam' 'arg' …\n`, each argument quoted like `g_shell_quote`.
use std::fs::{self, OpenOptions};
use std::io::{Error, ErrorKind, Result, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::PathBuf;

/// First word of every forwarded command line (the string `steam-runtime-steam-remote` writes).
pub const PROGRAM: &str = "'~/.steam/root/ubuntu12_32/steam'";
/// Linux PIPE_BUF: a single write of at most this many bytes is atomic, so two writers never interleave.
pub const MAX_MESSAGE: usize = 4096;

/// `~/.steam/steam.pipe`; `DECKSHELL_STEAM_PIPE` overrides it for tests only.
pub fn pipe_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DECKSHELL_STEAM_PIPE") { return PathBuf::from(p); }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".steam/steam.pipe")
}

/// POSIX single-quote quoting, identical to GLib `g_shell_quote`.
pub fn quote(arg: &str) -> String { format!("'{}'", arg.replace('\'', "'\\''")) }

/// The exact bytes written to the pipe for `args`.
pub fn message(args: &[&str]) -> Result<Vec<u8>> {
    if args.is_empty() { return Err(Error::new(ErrorKind::InvalidInput, "empty Steam command line")); }
    if args.iter().any(|a| a.contains('\n') || a.contains('\0')) {
        return Err(Error::new(ErrorKind::InvalidInput, "newline or NUL in a Steam argument"));
    }
    let mut m = String::from(PROGRAM);
    for a in args { m.push(' '); m.push_str(&quote(a)); }
    m.push('\n');
    if m.len() > MAX_MESSAGE { return Err(Error::new(ErrorKind::InvalidInput, "Steam command line longer than PIPE_BUF")); }
    Ok(m.into_bytes())
}

/// Deliver `args` to the RUNNING Steam. Never starts a process.
/// `NotConnected`: no Steam reads the pipe (Steam not running, starting, or restarting) — nothing was sent.
pub fn send(args: &[&str]) -> Result<()> {
    let r = deliver(args);
    match &r {
        Ok(()) => eprintln!("[steam-ipc] pipe <- {args:?}: delivered to the running Steam"),
        Err(e) => eprintln!("[steam-ipc] pipe <- {args:?}: NOT delivered ({e}); no Steam was started"),
    }
    r
}

fn deliver(args: &[&str]) -> Result<()> {
    let msg = message(args)?;
    let path = pipe_path();
    let not_running = |why: &str| Error::new(ErrorKind::NotConnected, format!("Steam is not running: {why}"));
    match fs::metadata(&path) {
        Ok(m) if m.file_type().is_fifo() => {}
        Ok(_) => return Err(Error::new(ErrorKind::InvalidData, format!("{} is not a FIFO", path.display()))),
        Err(e) if e.kind() == ErrorKind::NotFound => return Err(not_running("no steam.pipe")),
        Err(e) => return Err(e),
    }
    let mut f = match OpenOptions::new().append(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW).open(&path) {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => return Err(not_running("no reader on steam.pipe")),
        Err(e) => return Err(e),
    };
    // The path may have been swapped between the two checks.
    if !f.metadata()?.file_type().is_fifo() {
        return Err(Error::new(ErrorKind::InvalidData, format!("{} is not a FIFO", path.display())));
    }
    match f.write(&msg) {
        Ok(n) if n == msg.len() => Ok(()),
        Ok(n) => Err(Error::new(ErrorKind::WriteZero, format!("short write to steam.pipe ({n}/{})", msg.len()))),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Err(Error::new(ErrorKind::WouldBlock, "steam.pipe is full (Steam is not reading)")),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::Mutex;

    /// `DECKSHELL_STEAM_PIPE` is process-wide.
    static ENV: Mutex<()> = Mutex::new(());

    fn mkfifo(p: &std::path::Path) {
        let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    }
    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ds-steam-pipe-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn message_matches_steam_runtime_steam_remote() {
        // Same text the Deck's client logged on 2026-11-20 15:53:41 for a forwarded launch.
        assert_eq!(String::from_utf8(message(&["-steamdeck", "-pipewire", "-steamos3", "-silent"]).unwrap()).unwrap(),
            "'~/.steam/root/ubuntu12_32/steam' '-steamdeck' '-pipewire' '-steamos3' '-silent'\n");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert!(message(&[]).is_err());
        assert!(message(&["a\nb"]).is_err());
        assert!(message(&[&"x".repeat(MAX_MESSAGE)]).is_err());
    }

    #[test]
    fn no_reader_means_not_running_and_nothing_written() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("noreader");
        let p = d.join("steam.pipe");
        mkfifo(&p);
        std::env::set_var("DECKSHELL_STEAM_PIPE", &p);
        let e = send(&["steam://openurl/https://store.steampowered.com/account/"]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotConnected);
        // A missing pipe is the same answer.
        std::env::set_var("DECKSHELL_STEAM_PIPE", d.join("absent"));
        assert_eq!(send(&["-shutdown"]).unwrap_err().kind(), ErrorKind::NotConnected);
        std::env::remove_var("DECKSHELL_STEAM_PIPE");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reader_gets_exactly_one_line() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("reader");
        let p = d.join("steam.pipe");
        mkfifo(&p);
        // Read end opened non-blocking first, as Steam keeps it open.
        let mut r = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&p).unwrap();
        std::env::set_var("DECKSHELL_STEAM_PIPE", &p);
        send(&["steam://rungameid/367520"]).unwrap();
        std::env::remove_var("DECKSHELL_STEAM_PIPE");
        let mut got = String::new();
        r.read_to_string(&mut got).unwrap();
        assert_eq!(got, "'~/.steam/root/ubuntu12_32/steam' 'steam://rungameid/367520'\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn regular_file_is_refused_and_untouched() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = tmp("file");
        let p = d.join("steam.pipe");
        fs::write(&p, "").unwrap();
        std::env::set_var("DECKSHELL_STEAM_PIPE", &p);
        assert_eq!(send(&["-shutdown"]).unwrap_err().kind(), ErrorKind::InvalidData);
        std::env::remove_var("DECKSHELL_STEAM_PIPE");
        assert_eq!(fs::read(&p).unwrap(), b"");
        let _ = fs::remove_dir_all(&d);
    }
}
