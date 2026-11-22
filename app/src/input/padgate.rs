//! Keeps the running game from seeing the pad while DeckShell owns the screen.
//!
//! DeckShell reads the Deck controller from hidraw. A game does not: it reads the virtual
//! "Microsoft X-Box 360 pad N" (28de:11ff) that Steam Input creates through uinput, and Steam
//! keeps feeding that pad because it does not know our menus are open. Compositor focus only
//! routes keyboard, mouse and touch, so the same press reached both.
//!
//! While the shell owns input we hold `EVIOCGRAB` on every Steam virtual pad. The kernel then
//! delivers its events to our handle only (evdev and joydev readers get nothing). Before the
//! grab the pad is set to neutral, so a stick or button held at that moment does not stay
//! held in the game. Releasing the grab gives the pad back; a button still held then reaches
//! the game only as a release, which it ignores.
//!
//! Nothing in Steam is changed: no layout, no action set, no Steam Input setting. The grab
//! ends with the process, so a crash cannot leave the game without input.
//!
//! Every device also has an ungrabbed witness handle. Whatever it can read during a hold is
//! what any other reader (the game) could read, so the journal line written on release is
//! evidence of isolation, not an assumption.
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const STEAM_VENDOR: u16 = 0x28de;
const STEAM_VIRTUAL_PAD: u16 = 0x11ff;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const KEY_BYTES: usize = 96; // KEY_MAX 0x2ff
const ABS_BYTES: usize = 8; // ABS_MAX 0x3f
const EVENT_SIZE: usize = 24; // struct input_event on 64-bit

const EVIOCGRAB: libc::c_ulong = 0x4004_4590;
const fn ioc_read(nr: u32, len: usize) -> libc::c_ulong {
    ((2u32 << 30) | ((len as u32) << 16) | (0x45 << 8) | nr) as libc::c_ulong
}

/// How long observed focus must contradict the requested owner before it wins.
const FOCUS_SETTLE: Duration = Duration::from_millis(1500);

/// Who owns the pad, from the shell's request and what gamescope reports.
///
/// The request acts at once (opening a menu must gate before the next press, Resume must
/// hand back before focus moves). Observed input focus overrides it only after it has
/// contradicted the request for `FOCUS_SETTLE`: a game that took focus on its own must not
/// be left without input, and a game that never took focus must not receive shell input.
#[derive(Default)]
pub struct Policy {
    contradiction_since: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// DeckShell page or menu.
    Shell,
    /// The game was asked to take input.
    Game,
    /// Native Steam UI was asked for; Steam gates the game itself.
    Steam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Shell,
    Game,
    Other,
}

impl Policy {
    pub fn requested(&mut self) {
        self.contradiction_since = None;
    }

    /// True when the virtual pads must be withheld from the game.
    pub fn gate(&mut self, owner: Owner, focus: Focus, now: Instant) -> bool {
        let wanted = owner == Owner::Shell;
        let observed = match (owner, focus) {
            (Owner::Steam, _) | (_, Focus::Other) => None,
            (_, Focus::Shell) => Some(true),
            (_, Focus::Game) => Some(false),
        };
        match observed {
            Some(seen) if seen != wanted => {
                let since = *self.contradiction_since.get_or_insert(now);
                if now.duration_since(since) >= FOCUS_SETTLE { seen } else { wanted }
            }
            _ => {
                self.contradiction_since = None;
                wanted
            }
        }
    }
}

struct Pad {
    path: PathBuf,
    grabbed: File,
    witness: Option<File>,
}

#[derive(Default)]
struct Gate {
    pads: Vec<Pad>,
    held_since: Option<Instant>,
    withheld: u64,
    leaked: u64,
    last_scan: Option<Instant>,
}

static GATE: Mutex<Option<Gate>> = Mutex::new(None);

fn enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let off = std::env::var("DECKSHELL_PADGATE").is_ok_and(|v| v == "0");
        !off && !crate::actions::dryrun()
    })
}

/// Hold or release the Steam virtual pads. Idempotent and cheap when nothing changes.
pub fn set(hold: bool) {
    if !enabled() {
        return;
    }
    let mut guard = GATE.lock().unwrap_or_else(|e| e.into_inner());
    let gate = guard.get_or_insert_with(Gate::default);
    if hold {
        gate.hold();
    } else {
        gate.release();
    }
}

/// Periodic upkeep while held: pick up pads Steam created since, count what was withheld.
pub fn tick() {
    if !enabled() {
        return;
    }
    let mut guard = GATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(gate) = guard.as_mut() {
        if gate.held_since.is_some() {
            gate.hold();
            gate.drain();
        }
    }
}

impl Gate {
    fn hold(&mut self) {
        let now = Instant::now();
        let first = self.held_since.is_none();
        if !first && self.last_scan.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
            return;
        }
        self.last_scan = Some(now);
        let found = steam_virtual_pads(Path::new("/sys/class/input"));
        self.pads.retain(|p| found.contains(&p.path));
        for path in found {
            if self.pads.iter().any(|p| p.path == path) {
                continue;
            }
            match grab(&path) {
                Ok(pad) => {
                    eprintln!("[deckshell] padgate: holding {}", path.display());
                    self.pads.push(pad);
                }
                Err(e) => eprintln!("[deckshell] padgate: cannot hold {}: {e}", path.display()),
            }
        }
        if first {
            self.held_since = Some(now);
            self.withheld = 0;
            self.leaked = 0;
            crate::diag::log("padgate.hold", serde_json::json!({"pads": self.pads.len()}));
        }
    }

    fn drain(&mut self) {
        for pad in &self.pads {
            self.withheld += count_events(&pad.grabbed);
            if let Some(w) = &pad.witness {
                self.leaked += count_events(w);
            }
        }
    }

    fn release(&mut self) {
        let Some(since) = self.held_since.take() else { return };
        self.drain();
        let pads = self.pads.len();
        for pad in self.pads.drain(..) {
            ungrab(&pad);
        }
        self.last_scan = None;
        let ms = since.elapsed().as_millis() as u64;
        // `withheld` is a lower bound: the kernel drops queued events when a buffer overflows.
        eprintln!(
            "[deckshell] padgate: released {pads} pad(s) after {ms} ms: {} events withheld from the game, {} visible to other readers",
            self.withheld, self.leaked
        );
        crate::diag::log("padgate.release", serde_json::json!({"pads": pads, "ms": ms, "withheld": self.withheld, "leaked": self.leaked}));
    }
}

/// Event nodes of Steam Input's virtual gamepads.
fn steam_virtual_pads(sys: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(sys) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") {
            continue;
        }
        let id = |f: &str| {
            fs::read_to_string(e.path().join("device/id").join(f))
                .ok()
                .and_then(|s| u16::from_str_radix(s.trim(), 16).ok())
        };
        if id("vendor") == Some(STEAM_VENDOR) && id("product") == Some(STEAM_VIRTUAL_PAD) {
            out.push(PathBuf::from("/dev/input").join(name));
        }
    }
    out.sort();
    out
}

fn grab(path: &Path) -> std::io::Result<Pad> {
    let nonblock = |o: &mut OpenOptions| {
        o.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
    };
    // Writable when possible: only a writable handle can set the pad to neutral.
    let mut rw = OpenOptions::new();
    rw.read(true).write(true);
    nonblock(&mut rw);
    let mut ro = OpenOptions::new();
    ro.read(true);
    nonblock(&mut ro);
    let grabbed = rw.open(path).or_else(|_| ro.open(path))?;
    let witness = ro.open(path).ok();
    neutralize(&grabbed);
    if unsafe { libc::ioctl(grabbed.as_raw_fd(), EVIOCGRAB, 1 as libc::c_int) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The neutral events above are meant for the game; only later ones would be a leak.
    if let Some(w) = &witness {
        count_events(w);
    }
    count_events(&grabbed);
    Ok(Pad { path: path.to_path_buf(), grabbed, witness })
}

fn ungrab(pad: &Pad) {
    unsafe { libc::ioctl(pad.grabbed.as_raw_fd(), EVIOCGRAB, 0 as libc::c_int) };
}

/// Release every held key and centre every axis, as the game sees it.
fn neutralize(f: &File) {
    let fd = f.as_raw_fd();
    let mut events: Vec<u8> = vec![];
    fn push(events: &mut Vec<u8>, ty: u16, code: u16, value: i32) {
        events.extend_from_slice(&[0u8; 16]);
        events.extend_from_slice(&ty.to_ne_bytes());
        events.extend_from_slice(&code.to_ne_bytes());
        events.extend_from_slice(&value.to_ne_bytes());
    }
    let mut keys = [0u8; KEY_BYTES];
    if unsafe { libc::ioctl(fd, ioc_read(0x18, KEY_BYTES), keys.as_mut_ptr()) } >= 0 {
        for code in 0..KEY_BYTES * 8 {
            if keys[code / 8] & (1 << (code % 8)) != 0 {
                push(&mut events, EV_KEY, code as u16, 0);
            }
        }
    }
    let mut axes = [0u8; ABS_BYTES];
    if unsafe { libc::ioctl(fd, ioc_read(0x20 + u32::from(EV_ABS), ABS_BYTES), axes.as_mut_ptr()) } >= 0 {
        for code in 0..ABS_BYTES * 8 {
            if axes[code / 8] & (1 << (code % 8)) == 0 {
                continue;
            }
            let mut info = [0i32; 6]; // value, minimum, maximum, fuzz, flat, resolution
            if unsafe { libc::ioctl(fd, ioc_read(0x40 + code as u32, 24), info.as_mut_ptr()) } < 0 {
                continue;
            }
            let rest = neutral_axis(info[1], info[2]);
            if info[0] != rest {
                push(&mut events, EV_ABS, code as u16, rest);
            }
        }
    }
    if events.is_empty() {
        return;
    }
    push(&mut events, EV_SYN, 0, 0);
    let _ = unsafe { libc::write(fd, events.as_ptr().cast(), events.len()) };
}

/// Sticks and hats rest at 0; a trigger that cannot report 0 rests at its minimum.
fn neutral_axis(min: i32, max: i32) -> i32 {
    if min <= max { 0.clamp(min, max) } else { 0 }
}

/// Read and discard everything queued on a non-blocking evdev handle.
fn count_events(f: &File) -> u64 {
    let mut buf = [0u8; EVENT_SIZE * 64];
    let mut n = 0u64;
    loop {
        let got = unsafe { libc::read(f.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if got <= 0 {
            return n;
        }
        for ev in buf[..got as usize].chunks_exact(EVENT_SIZE) {
            // SYN frames are not presses.
            if u16::from_ne_bytes([ev[16], ev[17]]) != EV_SYN {
                n += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(ms: u64) -> Instant {
        static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *T0.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    #[test]
    fn opening_a_menu_gates_before_focus_moves() {
        let mut p = Policy::default();
        p.requested();
        // The game still has focus for the first polls after STEAM or QAM.
        assert!(p.gate(Owner::Shell, Focus::Game, t(0)));
        assert!(p.gate(Owner::Shell, Focus::Game, t(400)));
        assert!(p.gate(Owner::Shell, Focus::Shell, t(600)));
    }

    #[test]
    fn resume_hands_the_pad_back_before_focus_moves() {
        let mut p = Policy::default();
        p.requested();
        assert!(!p.gate(Owner::Game, Focus::Shell, t(0)));
        assert!(!p.gate(Owner::Game, Focus::Game, t(300)));
        assert!(!p.gate(Owner::Game, Focus::Game, t(60_000)));
    }

    #[test]
    fn a_game_that_took_focus_on_its_own_gets_its_pad() {
        let mut p = Policy::default();
        assert!(p.gate(Owner::Shell, Focus::Shell, t(0)));
        assert!(p.gate(Owner::Shell, Focus::Game, t(200)));
        assert!(p.gate(Owner::Shell, Focus::Game, t(1600)));
        assert!(!p.gate(Owner::Shell, Focus::Game, t(1700)));
        // Back on the shell: gated again at once.
        assert!(p.gate(Owner::Shell, Focus::Shell, t(1900)));
    }

    #[test]
    fn a_launch_that_never_takes_focus_does_not_leak_to_a_background_game() {
        let mut p = Policy::default();
        p.requested();
        assert!(!p.gate(Owner::Game, Focus::Shell, t(0)));
        assert!(!p.gate(Owner::Game, Focus::Shell, t(1400)));
        assert!(p.gate(Owner::Game, Focus::Shell, t(1500)));
        // A new request starts the wait again.
        p.requested();
        assert!(!p.gate(Owner::Game, Focus::Shell, t(1600)));
    }

    #[test]
    fn native_steam_ui_is_never_gated_by_us() {
        let mut p = Policy::default();
        for focus in [Focus::Shell, Focus::Game, Focus::Other] {
            assert!(!p.gate(Owner::Steam, focus, t(0)));
            assert!(!p.gate(Owner::Steam, focus, t(5000)));
        }
    }

    #[test]
    fn a_brief_focus_flicker_does_not_flip_the_gate() {
        let mut p = Policy::default();
        assert!(p.gate(Owner::Shell, Focus::Game, t(0)));
        assert!(p.gate(Owner::Shell, Focus::Other, t(1000)));
        assert!(p.gate(Owner::Shell, Focus::Game, t(1600)));
        assert!(p.gate(Owner::Shell, Focus::Game, t(3000)));
        assert!(!p.gate(Owner::Shell, Focus::Game, t(3100)));
    }

    #[test]
    fn only_steam_virtual_pads_are_selected() {
        let dir = std::env::temp_dir().join(format!("deckshell-padgate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mk = |name: &str, vendor: &str, product: &str| {
            let id = dir.join(name).join("device/id");
            fs::create_dir_all(&id).unwrap();
            fs::write(id.join("vendor"), format!("{vendor}\n")).unwrap();
            fs::write(id.join("product"), format!("{product}\n")).unwrap();
        };
        mk("event10", "28de", "11ff");
        mk("event18", "28de", "11ff");
        mk("event4", "28de", "1205"); // the Deck's own controller
        mk("event20", "045e", "028e"); // a real Xbox pad
        mk("js0", "28de", "11ff");
        fs::create_dir_all(dir.join("event99")).unwrap(); // no id files
        assert_eq!(
            steam_virtual_pads(&dir),
            vec![PathBuf::from("/dev/input/event10"), PathBuf::from("/dev/input/event18")]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The kernel contract the whole gate rests on, against a real uinput pad:
    /// another reader sees the pad go neutral, then nothing while held, then input again.
    /// Skipped where /dev/uinput or the new event node is not accessible.
    #[test]
    fn a_held_pad_is_neutral_and_silent_for_another_reader() {
        use std::io::Write;
        const BTN_A: u16 = 0x130;
        const ABS_X: u16 = 0;
        let Ok(mut ui) = OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open("/dev/uinput") else {
            eprintln!("skipped: /dev/uinput not accessible");
            return;
        };
        let fd = ui.as_raw_fd();
        let io = |req: libc::c_ulong, arg: libc::c_ulong| unsafe { libc::ioctl(fd, req, arg) };
        assert_eq!(io(0x4004_5564, EV_KEY.into()), 0); // UI_SET_EVBIT
        assert_eq!(io(0x4004_5564, EV_ABS.into()), 0);
        assert_eq!(io(0x4004_5565, BTN_A.into()), 0); // UI_SET_KEYBIT
        assert_eq!(io(0x4004_5567, ABS_X.into()), 0); // UI_SET_ABSBIT
        let mut abs = [0u8; 28]; // uinput_abs_setup: code, pad, absinfo{value,min,max,...}
        abs[8..12].copy_from_slice(&(-32768i32).to_ne_bytes());
        abs[12..16].copy_from_slice(&32767i32.to_ne_bytes());
        assert_eq!(unsafe { libc::ioctl(fd, 0x401c_5504, abs.as_ptr()) }, 0); // UI_ABS_SETUP
        let mut setup = [0u8; 92]; // uinput_setup: bus, vendor, product, version, name[80], ff
        setup[0..2].copy_from_slice(&3u16.to_ne_bytes());
        setup[2..4].copy_from_slice(&0x1209u16.to_ne_bytes()); // a test id, not Steam's
        setup[4..6].copy_from_slice(&0xdec5u16.to_ne_bytes());
        setup[8..8 + 21].copy_from_slice(b"deckshell padgate tst");
        assert_eq!(unsafe { libc::ioctl(fd, 0x405c_5503, setup.as_ptr()) }, 0); // UI_DEV_SETUP
        assert_eq!(io(0x5501, 0), 0); // UI_DEV_CREATE
        struct Destroy(i32);
        impl Drop for Destroy {
            fn drop(&mut self) { unsafe { libc::ioctl(self.0, 0x5502, 0) }; } // UI_DEV_DESTROY
        }
        let _destroy = Destroy(fd);
        let mut sysname = [0u8; 64];
        assert!(unsafe { libc::ioctl(fd, 0x8040_552c, sysname.as_mut_ptr()) } >= 0); // UI_GET_SYSNAME(64)
        let sysname = String::from_utf8_lossy(&sysname).trim_end_matches('\0').to_string();
        let node = fs::read_dir(format!("/sys/devices/virtual/input/{sysname}")).unwrap().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("event")).map(|n| PathBuf::from("/dev/input").join(n)).unwrap();
        let open_reader = || OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&node);
        let t0 = Instant::now();
        let game = loop {
            match open_reader() {
                Ok(f) => break f,
                Err(_) if t0.elapsed() < Duration::from_secs(3) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    eprintln!("skipped: {} not accessible: {e}", node.display());
                    return;
                }
            }
        };
        let mut emit = |ty: u16, code: u16, value: i32| {
            let mut ev = [0u8; EVENT_SIZE * 2];
            ev[16..18].copy_from_slice(&ty.to_ne_bytes());
            ev[18..20].copy_from_slice(&code.to_ne_bytes());
            ev[20..24].copy_from_slice(&value.to_ne_bytes()); // second event: SYN_REPORT (zeros)
            ui.write_all(&ev).unwrap();
        };
        // What the reader saw last per (type, code), from everything queued.
        let read_last = |f: &File| {
            let mut seen = std::collections::HashMap::new();
            let mut buf = [0u8; EVENT_SIZE * 64];
            loop {
                let got = unsafe { libc::read(f.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if got <= 0 { break; }
                for ev in buf[..got as usize].chunks_exact(EVENT_SIZE) {
                    let ty = u16::from_ne_bytes([ev[16], ev[17]]);
                    if ty != EV_SYN {
                        seen.insert((ty, u16::from_ne_bytes([ev[18], ev[19]])), i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]));
                    }
                }
            }
            seen
        };
        // The game is holding A and the stick when a menu opens.
        emit(EV_KEY, BTN_A, 1);
        emit(EV_ABS, ABS_X, 20000);
        let before = read_last(&game);
        assert_eq!(before.get(&(EV_KEY, BTN_A)), Some(&1));
        assert_eq!(before.get(&(EV_ABS, ABS_X)), Some(&20000));

        let pad = grab(&node).expect("grab");
        let at_open = read_last(&game);
        assert_eq!(at_open.get(&(EV_KEY, BTN_A)), Some(&0), "held button must be released for the game");
        assert_eq!(at_open.get(&(EV_ABS, ABS_X)), Some(&0), "held stick must be centred for the game");

        // Menu navigation: Steam keeps writing the pad.
        emit(EV_ABS, ABS_X, -30000);
        emit(EV_KEY, BTN_A, 1);
        emit(EV_KEY, BTN_A, 0);
        emit(EV_KEY, BTN_A, 1); // still held when the menu closes
        assert!(read_last(&game).is_empty(), "the game must see nothing while the shell holds the pad");
        assert_eq!(pad.witness.as_ref().map(count_events), Some(0));
        assert!(count_events(&pad.grabbed) >= 4);

        ungrab(&pad);
        emit(EV_KEY, BTN_A, 0);
        let after = read_last(&game);
        // Only the release of the press that closed the menu arrives; then normal input.
        assert_eq!(after.get(&(EV_KEY, BTN_A)), Some(&0));
        emit(EV_KEY, BTN_A, 1);
        assert_eq!(read_last(&game).get(&(EV_KEY, BTN_A)), Some(&1), "input must return after release");
    }

    #[test]
    fn neutral_is_centre_for_sticks_and_floor_for_one_sided_axes() {
        assert_eq!(neutral_axis(-32768, 32767), 0);
        assert_eq!(neutral_axis(0, 255), 0);
        assert_eq!(neutral_axis(-1, 1), 0);
        assert_eq!(neutral_axis(10, 255), 10);
    }

    #[test]
    fn ioctl_numbers_match_linux_input_h() {
        assert_eq!(ioc_read(0x18, 96), 0x8060_4518); // EVIOCGKEY(96)
        assert_eq!(ioc_read(0x20 + 3, 8), 0x8008_4523); // EVIOCGBIT(EV_ABS, 8)
        assert_eq!(ioc_read(0x40, 24), 0x8018_4540); // EVIOCGABS(ABS_X)
    }
}
