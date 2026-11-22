//! Input actions shared by keyboard and gamepads.
//!
//! Two sources feed the UI:
//!  * `hidraw` — the Steam Deck's built-in controller read directly (Steam in desktop mode
//!    swallows STEAM / QAM and only exposes a "Steam Virtual Gamepad"). Read-only.
//!  * `gilrs` — any other pad (external controllers). While the hidraw reader is streaming,
//!    gilrs events from Valve devices (vendor 28DE, incl. the virtual pad) are ignored so a
//!    press is never handled twice.
pub mod padgate;
pub mod power;

use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU32, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Duration, Instant};

use crate::screens::hotkeys;
use gilrs::{Axis, Button, EventType, Gilrs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    LB,
    RB,
    /// Triggers: library fast scroll.
    L2,
    R2,
    /// STEAM button: left menu.
    Steam,
    /// "..." button: right Quick Access Menu.
    Qam,
    /// Menu (≡) button: options.
    Menu,
    /// View (⧉) button: search.
    View,
    Search,
    Char(char),
    Backspace,
    /// Виртуальная клавиатура (только пока она открыта): X удерживается/отпущен (Backspace с повтором).
    KbBack(bool),
    /// L2 удерживается/отпущен (Shift).
    KbShift(bool),
    /// L3 отпущен (Caps).
    KbCaps,
    /// Касание тачпада: координаты −32768..32767, Y вверх; on=false — палец поднят.
    KbPad { right: bool, x: i16, y: i16, on: bool },
    /// Клик тачпада (давление, бит нажатия или L2/R2 при касании).
    KbClick { right: bool, down: bool },
    /// Клавиатура открыта: A нажата/отпущена (символ набирается на отпускание, удержание открывает варианты).
    KbA(bool),
}

static OSK_OPEN: AtomicBool = AtomicBool::new(false);
/// Настройка Steam «триггер как клик тачпада» (`TrackpadTypingTriggerAsClick`, по умолчанию да).
static OSK_TRIGGER_CLICK: AtomicBool = AtomicBool::new(true);

pub fn set_osk_trigger_click(on: bool) {
    OSK_TRIGGER_CLICK.store(on, Ordering::Relaxed);
}
/// Клавиатура открыта: кнопки Деки маршрутизируются в неё (X/L2/L3/тачпады), см. `spawn_hidraw`.
pub fn set_osk_open(open: bool) {
    OSK_OPEN.store(open, Ordering::Relaxed);
}
fn osk_open() -> bool {
    OSK_OPEN.load(Ordering::Relaxed)
}

const REPEAT_FIRST: Duration = Duration::from_millis(400);
const REPEAT_NEXT: Duration = Duration::from_millis(50);

static HIDRAW_ACTIVE: AtomicBool = AtomicBool::new(false);
static PAD_HELD: AtomicU32 = AtomicU32::new(0);
static LAST_PAD_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// True while the built-in pad is being pressed (or just was). Steam's "lizard mode" echoes
/// Deck buttons as keyboard keys; those echoes must not be treated as keyboard input.
pub fn pad_busy() -> bool {
    if !HIDRAW_ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    PAD_HELD.load(Ordering::Relaxed) != 0 || now_ms().saturating_sub(LAST_PAD_MS.load(Ordering::Relaxed)) < 250
}

// The UI backend does not expose device identity or distinguish a reconnect
// snapshot from a real press. Require a release baseline per explicit skip key
// and playback generation. No text is retained or logged. First tap after focus
// or playback change arms the key; the next fresh press skips.
#[derive(Default)]
pub(crate) struct AnimationKeyboard {
    generation: u64,
    released: u8,
}
impl AnimationKeyboard {
    pub(crate) fn event(&mut self, generation: u64, key: u8, pressed: bool, repeat: bool, blocked: bool) -> bool {
        if self.generation != generation {
            self.generation = generation;
            self.released = 0;
        }
        if blocked || repeat {
            self.released = 0;
            return false;
        }
        if key == 0 { return false; }
        if !pressed {
            self.released |= key;
            return false;
        }
        let fresh = self.released & key != 0;
        self.released &= !key;
        fresh
    }
}
thread_local! {
    static ANIMATION_KEYBOARD: std::cell::RefCell<AnimationKeyboard> = Default::default();
}
pub fn reset_animation_keyboard() {
    ANIMATION_KEYBOARD.with(|s| *s.borrow_mut() = AnimationKeyboard::default());
}
pub fn animation_key(text: &str, pressed: bool, repeat: bool, modified: bool) -> bool {
    use slint::platform::Key;
    let mut chars = text.chars();
    let c = chars.next();
    let key = if chars.next().is_some() { 0 } else if c == Some(' ') { 1 }
        else if c == Some(Key::Return.into()) || c == Some('\n') || c == Some('\r') { 2 }
        else if c == Some(Key::Escape.into()) { 4 } else { 0 };
    let mode = crate::anim::player::INPUT_MODE.load(Ordering::Relaxed);
    if mode == 0 { reset_animation_keyboard(); return false; }
    let fresh = ANIMATION_KEYBOARD.with(|s| s.borrow_mut().event(
        crate::anim::player::ACTIVE_GEN.load(Ordering::Relaxed), key, pressed, repeat, modified || pad_busy()));
    crate::anim::consume_input("keyboard", fresh)
}

#[cfg(test)]
mod animation_keyboard_tests {
    use super::AnimationKeyboard;
    #[test]
    fn startup_focus_system_modifiers_repeat_held_and_reconnect_do_not_skip() {
        let mut s = AnimationKeyboard::default();
        for key in [0, 1, 2, 4] {
            assert!(!s.event(1, key, true, false, false));
            assert!(!s.event(1, key, true, true, false));
        }
        assert!(!s.event(1, 1, false, false, false));
        assert!(!s.event(1, 1, true, false, true));
        assert!(!s.event(1, 1, true, false, false));
        assert!(!s.event(1, 1, false, false, false));
        assert!(s.event(1, 1, true, false, false));
        assert!(!s.event(1, 1, true, false, false));
        assert!(!s.event(1, 1, false, false, false));
        assert!(!s.event(2, 1, true, false, false));
        s = AnimationKeyboard::default(); // focus/reconnect resets baseline
        assert!(!s.event(2, 1, true, false, false));
        assert!(!s.event(2, 1, false, false, false));
        assert!(s.event(2, 1, true, false, false));
    }
}

/// Sticks pushed off centre or a finger on a trackpad: user activity for the idle timers even when no action results.
fn deck_active(st: &DeckState) -> bool {
    const STICK: i16 = 9000;
    [st.lx, st.ly, st.rx, st.ry].iter().any(|v| v.saturating_abs() > STICK) || st.pads.iter().any(|p| p.touch)
}

fn send(a: Action) {
    send_rep(a, false, false)
}

static ANALOG_DIR: AtomicBool = AtomicBool::new(false);

/// True while the direction being handled came from the left stick, not the D-pad. Screens that give the stick a different job
/// than the D-pad (the hero crop editor: stick pans, D-pad moves focus) read it. Only valid inside that press's handling.
pub fn analog_dir() -> bool {
    ANALOG_DIR.load(Ordering::Relaxed)
}

/// `is_repeat` mirrors Steam's `detail.is_repeat` (nav engine far-jump / fast-scroll thresholds).
/// DECKSHELL_NAV=steam (default): the Steam focus engine (`crate::nav`) gets the press first; it declines while no screen
/// has registered nav nodes, so legacy per-screen handling keeps working until a screen is ported.
fn send_rep(a: Action, is_repeat: bool, analog: bool) {
    let generation = crate::anim::player::ACTIVE_GEN.load(Ordering::Relaxed);
    let _ = slint::invoke_from_event_loop(move || {
        let fresh = !is_repeat && generation == crate::anim::player::ACTIVE_GEN.load(Ordering::Relaxed);
        if crate::anim::consume_input("pad", fresh) { return; }
        // the screensaver is up: this press only wakes it (idle timers, task 518)
        if crate::idle::swallow() { return; }
        // hidraw keeps reporting Deck buttons even while a game or Steam's store owns the
        // screen. Only STEAM/QAM may act on the hidden shell; never activate a tile underneath.
        if !matches!(a, Action::Steam | Action::Qam) && !crate::gamescope::accepts_pad_input() {
            return;
        }
        if !is_repeat && !matches!(a, Action::KbPad { on: true, .. }) {
            crate::diag::log("input.pad",serde_json::json!({"action":format!("{a:?}")}));
        }
        // A menu is sliding out over the game: the page behind it is hidden and must not act.
        if !matches!(a, Action::Steam | Action::Qam) && crate::with_app(|app| app.input_suspended()).unwrap_or(false) {
            return;
        }
        // Клавиатура открыта: ввод принадлежит ей раньше навигации Steam и экранов.
        if osk_open() && crate::with_app(|app| app.osk_handle_repeat(a, is_repeat)).unwrap_or(false) {
            return;
        }
        if crate::nav::intercept(a, is_repeat) {
            crate::with_app(|app| app.set_pad(true));
            return;
        }
        ANALOG_DIR.store(analog, Ordering::Relaxed);
        crate::with_app(|app| app.handle_pad_repeat(a, is_repeat));
        ANALOG_DIR.store(false, Ordering::Relaxed);
    });
}

/// One held direction: what to repeat, when the next repeat is due, and whether it came from the left stick.
#[derive(Clone, Copy)]
struct Held {
    a: Action,
    next: Instant,
    analog: bool,
}

/// Steam-like auto-repeat of one held direction.
#[derive(Default)]
struct Repeat {
    cur: Option<Held>,
}
impl Repeat {
    /// `analog`: `d` is the left stick's direction (false for the D-pad or no direction).
    fn set(&mut self, d: Option<Action>, analog: bool) {
        match (d, self.cur) {
            (Some(d), Some(c)) if d == c.a && analog == c.analog => {}
            (Some(d), _) => {
                send_rep(d, false, analog);
                self.cur = Some(Held { a: d, next: Instant::now() + REPEAT_FIRST, analog });
            }
            (None, _) => self.cur = None,
        }
    }
    fn tick(&mut self) {
        if let Some(h) = self.cur {
            let now = Instant::now();
            if now >= h.next {
                send_rep(h.a, true, h.analog);
                self.cur = Some(Held { next: now + REPEAT_NEXT, ..h });
            }
        }
    }
}

/// D-pad direction wins over the stick, as before. Returns (direction, came from the stick).
fn pick_dir(dpad: Option<Action>, stick: Option<Action>) -> (Option<Action>, bool) {
    match dpad {
        Some(d) => (Some(d), false),
        None => (stick, stick.is_some()),
    }
}

// ------------------------------------------------------------------------ hidraw (Steam Deck)

/// Button bits we track, as (byte, bit) in the 64-byte input report (layout from the Linux
/// `hid-steam.c` Deck input mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckBtn {
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    L4,
    L5,
    R4,
    R5,
    View,
    Menu,
    Steam,
    Qam,
    L3,
    R3,
    LeftPad,
    RightPad,
    LeftPadClick,
    RightPadClick,
    Up,
    Down,
    Left,
    Right,
}
pub const DECK_BTNS: [(DeckBtn, usize, u8); 26] = [
    (DeckBtn::A, 8, 7),
    (DeckBtn::B, 8, 5),
    (DeckBtn::X, 8, 6),
    (DeckBtn::Y, 8, 4),
    (DeckBtn::R2, 8, 0),
    (DeckBtn::L2, 8, 1),
    (DeckBtn::R1, 8, 2),
    (DeckBtn::L1, 8, 3),
    (DeckBtn::Up, 9, 0),
    (DeckBtn::Right, 9, 1),
    (DeckBtn::Left, 9, 2),
    (DeckBtn::Down, 9, 3),
    (DeckBtn::View, 9, 4),
    (DeckBtn::Steam, 9, 5),
    (DeckBtn::Menu, 9, 6),
    (DeckBtn::L5, 9, 7),
    (DeckBtn::R5, 10, 0),
    (DeckBtn::L3, 10, 6),
    (DeckBtn::R3, 11, 2),
    (DeckBtn::LeftPadClick, 10, 1),
    (DeckBtn::RightPadClick, 10, 2),
    (DeckBtn::LeftPad, 10, 3),
    (DeckBtn::RightPad, 10, 4),
    (DeckBtn::L4, 13, 1),
    (DeckBtn::R4, 13, 2),
    (DeckBtn::Qam, 14, 2),
];

#[derive(Debug, Clone, Default)]
pub struct DeckState {
    pub down: Vec<DeckBtn>,
    /// Sticks, -32768..32767; Y positive = up.
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
    pads: [DeckPadReport; 2],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeckPadReport {
    xy: (i16, i16),
    pressure: u16,
    touch: bool,
    click: bool,
}

/// Parse the unnumbered 64-byte Deck state report (version 0x01, type 0x09).
pub fn parse_deck_report(d: &[u8]) -> Option<DeckState> {
    if d.len() != 64 || d[0] != 1 || d[2] != 9 {
        return None;
    }
    let s16 = |o: usize| i16::from_le_bytes([d[o], d[o + 1]]);
    let bit = |i: usize| d[10] & (1 << i) != 0;
    Some(DeckState {
        down: DECK_BTNS.iter().filter(|(_, byte, bit)| d[*byte] & (1 << bit) != 0).map(|x| x.0).collect(),
        lx: s16(48),
        ly: s16(50),
        rx: s16(52),
        ry: s16(54),
        // hid-steam: signed X/Y (Y positive up), unsigned little-endian pad force.
        pads: std::array::from_fn(|i| DeckPadReport {
            xy: (s16(16 + i * 4), s16(18 + i * 4)),
            pressure: u16::from_le_bytes([d[56 + i * 2], d[57 + i * 2]]),
            touch: bit(3 + i),
            click: bit(1 + i),
        }),
    })
}

/// hidraw nodes of the Deck controller (VID 28DE, PID 1205), via sysfs.
pub fn deck_hidraw_nodes() -> Vec<String> {
    let mut v = vec![];
    if let Ok(rd) = fs::read_dir("/sys/class/hidraw") {
        for e in rd.flatten() {
            let uev = fs::read_to_string(e.path().join("device/uevent")).unwrap_or_default();
            if uev.to_ascii_uppercase().contains("HID_ID=0003:000028DE:00001205") {
                v.push(format!("/dev/{}", e.file_name().to_string_lossy()));
            }
        }
    }
    v.sort();
    v
}

/// Open each Deck hidraw node and return (path, reader channel) for the first interface that
/// streams type-0x09 input reports. Reads only; no feature reports are ever sent.
fn open_stream() -> Option<(String, mpsc::Receiver<Vec<u8>>)> {
    // Test mode replaces the physical stream, so real reports cannot interleave
    // with injected press/release pairs. Never enabled without an explicit path.
    if let Some(path) = std::env::var_os("DECKSHELL_TEST_HID_SOCKET") {
        use std::os::unix::net::UnixDatagram;
        use std::os::unix::fs::PermissionsExt;
        let socket = UnixDatagram::bind(&path).map_err(|e| eprintln!("[deckshell] test HID socket: {e}")).ok()?;
        if let Err(e) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            eprintln!("[deckshell] test HID socket permissions: {e}");
            let _ = fs::remove_file(&path);
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let name = path.to_string_lossy().into_owned();
        let _ = std::thread::Builder::new().name("test-hid-read".into()).spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok(n) = socket.recv(&mut buf) {
                if n == 64 && parse_deck_report(&buf).is_some() && tx.send(buf.to_vec()).is_err() { break; }
            }
        });
        return Some((format!("test socket {name}"), rx));
    }
    for path in deck_hidraw_nodes() {
        let Ok(mut f) = fs::File::open(&path) else { continue };
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let (probe_tx, probe_rx) = mpsc::channel::<()>();
        let p2 = path.clone();
        let _ = std::thread::Builder::new().name("hidraw-read".into()).spawn(move || {
            let mut buf = [0u8; 64];
            let mut first = true;
            loop {
                match f.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        if n == 64 && buf[0] == 1 && buf[2] == 9 {
                            if first {
                                first = false;
                                let _ = probe_tx.send(());
                            }
                            if tx.send(buf[..n].to_vec()).is_err() {
                                return;
                            }
                        }
                    }
                    _ => {
                        eprintln!("[deckshell] hidraw {p2} closed");
                        return;
                    }
                }
            }
        });
        if probe_rx.recv_timeout(Duration::from_millis(700)).is_ok() {
            return Some((path, rx));
        }
        // Non-streaming interface (keyboard/mouse): its reader thread stays blocked and is
        // harmless; move on.
    }
    None
}

// Raw force units, calibrated 2026-11-18 on a real capture (owner, paired stage beeps): untouched/slide = 0; first (soft) stage plateau 1992-2491 on both pads;
// light first-stage clicks peak 1364-3666; hard click saturates (left 32767 + bit 0x20000). Press below the first stage plateau, release with hysteresis.
const PAD_PRESS_FORCE: u16 = 1600;
const PAD_RELEASE_FORCE: u16 = 900;
const PAD_DEBOUNCE_REPORTS: u8 = 3;

#[derive(Default)]
struct OskPadInput {
    position: Option<(i16, i16)>,
    missed_touch: u8,
    pressure_frames: u8,
    pad_down: bool,
    trigger_down: bool,
    down: bool,
}

impl OskPadInput {
    fn sample(&mut self, pad: DeckPadReport, allow_press: bool) {
        if pad.touch || (self.position.is_some() && pad.pressure > PAD_RELEASE_FORCE) {
            self.missed_touch = 0;
            // If only force survives a lost touch bit, retain the last known point.
            if pad.touch {
                self.position = Some(pad.xy);
            }
        } else {
            self.missed_touch = (self.missed_touch + 1).min(PAD_DEBOUNCE_REPORTS);
            if self.missed_touch == PAD_DEBOUNCE_REPORTS {
                self.position = None;
            }
        }

        if self.position.is_none() {
            self.pad_down = false;
            self.trigger_down = false;
            self.pressure_frames = 0;
            return;
        }

        // The hardware click also works on its own and extends a soft click
        // until both inputs release. Only the pressure edge needs debouncing.
        let next = if self.pad_down {
            pad.click || pad.pressure > PAD_RELEASE_FORCE
        } else {
            allow_press && pad.touch && (pad.click || pad.pressure >= PAD_PRESS_FORCE)
        };
        if next == self.pad_down {
            self.pressure_frames = 0;
        } else {
            self.pressure_frames += 1;
            if (next && pad.click) || self.pressure_frames >= PAD_DEBOUNCE_REPORTS {
                self.pad_down = next;
                self.pressure_frames = 0;
            }
        }
    }

    fn click_edge(&mut self, right: bool) -> Option<Action> {
        let down = self.pad_down || self.trigger_down;
        (std::mem::replace(&mut self.down, down) != down).then_some(Action::KbClick { right, down })
    }
}

pub(crate) fn osk_pad_diag_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| crate::diag::enabled()
        && std::env::var("DECKSHELL_DIAG_OSK_PADS").as_deref() == Ok("1"))
}

pub fn spawn_hidraw() {
    let _ = std::thread::Builder::new().name("hidraw".into()).spawn(|| loop {
        let Some((path, rx)) = open_stream() else {
            std::thread::sleep(Duration::from_secs(3));
            continue;
        };
        eprintln!("[deckshell] Steam Deck controller via {path}");
        let mut prev: Vec<DeckBtn> = vec![];
        let mut first_report = true;
        let mut rep = Repeat::default();
        let (mut stick_l, mut stick_r) = (None::<Action>, None::<Action>);
        let mut hk = crate::screens::hotkeys::Machine::new();
        let mut pad_input: [OskPadInput; 2] = Default::default();
        let mut left_trigger_click = false;
        let diag_pads = osk_pad_diag_enabled();
        let mut last_pad_diag = [Instant::now(); 2];
        let mut prev_pads: [Option<(i16, i16)>; 2] = [None, None];
        let mut last_pad_send = [Instant::now(); 2];
        loop {
            match rx.recv_timeout(Duration::from_millis(15)) {
                Ok(rep_buf) => {
                    let Some(st) = parse_deck_report(&rep_buf) else { continue };
                    HIDRAW_ACTIVE.store(true, Ordering::Relaxed);
                    if crate::recording_mic::report("deck", st.down.iter().map(|b| format!("Deck:{b:?}")).collect(), true) {
                        prev = st.down.clone(); first_report = false; rep.set(None, false); continue;
                    }
                    // A reconnect can report buttons already held. While an animation owns
                    // input, establish a baseline rather than inventing a new press.
                    if std::mem::replace(&mut first_report, false)
                        && crate::anim::player::INPUT_MODE.load(Ordering::Relaxed) != 0 {
                        prev = st.down.clone();
                        stick_l = stick_dir(st.lx, st.ly, None);
                        stick_r = stick_dir(st.rx, st.ry, None);
                        let dpad = [(DeckBtn::Up, Action::Up), (DeckBtn::Down, Action::Down),
                            (DeckBtn::Left, Action::Left), (DeckBtn::Right, Action::Right)]
                            .into_iter().find(|(b, _)| st.down.contains(b)).map(|(_, a)| a);
                        let (dir, analog) = pick_dir(dpad, stick_l.or(stick_r));
                        rep.cur = dir.map(|a| Held { a, next: Instant::now() + REPEAT_FIRST, analog });
                        PAD_HELD.store(st.down.len() as u32 + stick_l.is_some() as u32 + stick_r.is_some() as u32, Ordering::Relaxed);
                        continue;
                    }
                    let pressed = |b: DeckBtn| st.down.contains(&b);
                    // Held count: any non-direction button/direction.
                    PAD_HELD.store(st.down.len() as u32 + stick_l.is_some() as u32 + stick_r.is_some() as u32, Ordering::Relaxed);
                    if st.down != prev {
                        LAST_PAD_MS.store(now_ms(), Ordering::Relaxed);
                    }
                    // idle timers (task 518): buttons, a stick off centre, a finger on a trackpad
                    if st.down != prev || deck_active(&st) { crate::idle::touch(); }
                    let keyboard = osk_open();
                    if keyboard {
                        for (input, raw) in pad_input.iter_mut().zip(st.pads) {
                            input.sample(raw, !pressed(DeckBtn::Steam));
                        }
                    }
                    // STEAM / "..." / chords: pure state machine (screens::hotkeys), STEAM menu opens on release
                    for b in ordered_transitions(&prev, &st.down, false) {
                        if !st.down.contains(&b) {
                            if matches!(b, DeckBtn::Steam | DeckBtn::A) {
                                crate::diag::log("record_input.hid", serde_json::json!({"button":format!("{b:?}"),"edge":"release","steam_held":hk.steam_held()}));
                            }
                            if let Some(hb) = hk_btn(b) {
                                hk_dispatch(hk.event(hotkeys::Ev::Release(hb), now_ms()));
                            }
                            if keyboard {
                                match b {
                                    DeckBtn::X => send(Action::KbBack(false)),
                                    DeckBtn::L2 => {
                                        pad_input[0].trigger_down = false;
                                        if !std::mem::take(&mut left_trigger_click) {
                                            send(Action::KbShift(false));
                                        }
                                    }
                                    DeckBtn::R2 => pad_input[1].trigger_down = false,
                                    DeckBtn::L3 => send(Action::KbCaps),
                                    DeckBtn::A => send(Action::KbA(false)),
                                    _ => {}
                                }
                            }
                        }
                    }
                    for b in ordered_transitions(&prev, &st.down, true) {
                        if prev.contains(&b) {
                            continue;
                        }
                        if matches!(b, DeckBtn::Steam | DeckBtn::A) {
                            crate::diag::log("record_input.hid", serde_json::json!({"button":format!("{b:?}"),"edge":"press","steam_held":hk.steam_held()}));
                        }
                        if let Some(hb) = hk_btn(b) {
                            hk_dispatch(hk.event(hotkeys::Ev::Press(hb), now_ms()));
                        }
                        // while STEAM is held the other buttons belong to the chord layer
                        if hk.steam_held() || matches!(b, DeckBtn::Steam | DeckBtn::Qam) {
                            continue;
                        }
                        if keyboard {
                            match b {
                                DeckBtn::X => { send(Action::KbBack(true)); continue; }
                                DeckBtn::A => { send(Action::KbA(true)); continue; }
                                DeckBtn::L2 => {
                                    left_trigger_click = pad_input[0].position.is_some()
                                        && OSK_TRIGGER_CLICK.load(Ordering::Relaxed);
                                    if left_trigger_click {
                                        pad_input[0].trigger_down = true;
                                    } else {
                                        send(Action::KbShift(true));
                                    }
                                    continue;
                                }
                                DeckBtn::R2 if pad_input[1].position.is_some() && OSK_TRIGGER_CLICK.load(Ordering::Relaxed) => {
                                    pad_input[1].trigger_down = true;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        match b {
                            DeckBtn::A => send(Action::A),
                            DeckBtn::B => send(Action::B),
                            DeckBtn::X => send(Action::X),
                            DeckBtn::Y => send(Action::Y),
                            DeckBtn::L1 => send(Action::LB),
                            DeckBtn::R1 => send(Action::RB),
                            DeckBtn::L2 => send(Action::L2),
                            DeckBtn::R2 => send(Action::R2),
                            DeckBtn::Menu => send(Action::Menu),
                            DeckBtn::View => send(Action::View),
                            _ => {}
                        }
                    }
                    if keyboard {
                        for (i, input) in pad_input.iter_mut().enumerate() {
                            let right = i == 1;
                            let edge = input.click_edge(right);
                            let position = input.position;
                            let had_position = prev_pads[i].is_some();
                            if let Some((x, y)) = position {
                                // Hit-test the current point before delivering a click edge.
                                if position != prev_pads[i]
                                    && (edge.is_some() || prev_pads[i].is_none()
                                        || last_pad_send[i].elapsed() >= Duration::from_millis(8)) {
                                    send(Action::KbPad { right, x, y, on: true });
                                    prev_pads[i] = position;
                                    last_pad_send[i] = Instant::now();
                                }
                            }
                            if let Some(action) = edge {
                                send(action);
                            }
                            if position.is_none() && prev_pads[i].take().is_some() {
                                send(Action::KbPad { right, x: 0, y: 0, on: false });
                            }

                            if diag_pads {
                                let raw = st.pads[i];
                                let (touch, click) = if right {
                                    (DeckBtn::RightPad, DeckBtn::RightPadClick)
                                } else {
                                    (DeckBtn::LeftPad, DeckBtn::LeftPadClick)
                                };
                                if edge.is_some() || position.is_some() != had_position
                                    || raw.touch != prev.contains(&touch)
                                    || raw.click != prev.contains(&click)
                                    || (position.is_some() && last_pad_diag[i].elapsed() >= Duration::from_millis(50)) {
                                    crate::diag::log("input.osk_pad.raw", serde_json::json!({
                                        "right": right, "x": raw.xy.0, "y": raw.xy.1,
                                        "pressure": raw.pressure, "touch_bit": raw.touch, "click_bit": raw.click,
                                        "contact": position.is_some(), "pad_down": input.pad_down,
                                        "down": input.down,
                                    }));
                                    last_pad_diag[i] = Instant::now();
                                }
                            }
                        }
                    } else {
                        prev_pads = [None, None];
                        pad_input = Default::default();
                        left_trigger_click = false;
                    }
                    let dpad = if pressed(DeckBtn::Up) {
                        Some(Action::Up)
                    } else if pressed(DeckBtn::Down) {
                        Some(Action::Down)
                    } else if pressed(DeckBtn::Left) {
                        Some(Action::Left)
                    } else if pressed(DeckBtn::Right) {
                        Some(Action::Right)
                    } else {
                        None
                    };
                    stick_l = stick_dir(st.lx, st.ly, stick_l);
                    stick_r = stick_dir(st.rx, st.ry, stick_r);
                    let ly = match stick_l {
                        Some(Action::Up) => 1,
                        Some(Action::Down) => -1,
                        _ => 0,
                    };
                    if hk.chord_held() {
                        crate::diag::log("bright.stick", serde_json::json!({"raw_x": st.lx, "raw_y": st.ly, "ly": ly, "steam": hk.steam_held()}));
                    }
                    hk_dispatch(hk.event(hotkeys::Ev::LStickY(ly), now_ms()));
                    if hk.chord_held() {
                        // STEAM / "..." + stick is the brightness chord: the stick must not also navigate
                        rep.set(None, false);
                    } else {
                        let (d, analog) = pick_dir(dpad, stick_l.or(stick_r));
                        rep.set(d, analog);
                    }
                    hk_dispatch(hk.tick(now_ms()));
                    prev = st.down;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    rep.tick();
                    hk_dispatch(hk.tick(now_ms()));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            rep.tick();
        }
        crate::recording_mic::report("deck", vec![], false);
        HIDRAW_ACTIVE.store(false, Ordering::Relaxed);
        PAD_HELD.store(0, Ordering::Relaxed);
        std::thread::sleep(Duration::from_secs(2));
    });
}

// A report can contain both edges. Establish the modifier before processing a
// press and release it last, regardless of the wire-format bit ordering.
fn ordered_transitions(prev: &[DeckBtn], current: &[DeckBtn], press: bool) -> Vec<DeckBtn> {
    let mut edges: Vec<_> = if press { current.iter().filter(|b| !prev.contains(b)).copied().collect() }
        else { prev.iter().filter(|b| !current.contains(b)).copied().collect() };
    edges.sort_by_key(|b| if press { *b != DeckBtn::Steam } else { *b == DeckBtn::Steam });
    edges
}

fn hk_btn(b: DeckBtn) -> Option<hotkeys::Btn> {
    use hotkeys::Btn as H;
    Some(match b {
        DeckBtn::A => H::A,
        DeckBtn::B => H::B,
        DeckBtn::X => H::X,
        DeckBtn::Y => H::Y,
        DeckBtn::L1 => H::L1,
        DeckBtn::R1 => H::R1,
        DeckBtn::L2 => H::L2,
        DeckBtn::R2 => H::R2,
        DeckBtn::View => H::View,
        DeckBtn::Menu => H::Menu,
        DeckBtn::Steam => H::Steam,
        DeckBtn::Qam => H::Qam,
        DeckBtn::Up => H::Up,
        DeckBtn::Down => H::Down,
        DeckBtn::Left => H::Left,
        DeckBtn::Right => H::Right,
        _ => return None,
    })
}

fn hk_dispatch(outs: Vec<hotkeys::Out>) {
    for o in outs {
        if matches!(o, hotkeys::Out::Chord(hotkeys::Chord::RecordToggle)) {
            crate::diag::log("record_input.hotkey", serde_json::json!({"output":"RecordToggle","dispatch":"ui_thread"}));
        }
        match o {
            hotkeys::Out::SteamMenu => send(Action::Steam),
            hotkeys::Out::Qam => send(Action::Qam),
            o => {
                let _ = slint::invoke_from_event_loop(move || { crate::with_app(|app| app.hotkey_out(o)); });
            }
        }
    }
}

/// Digital direction from a stick with hysteresis (engage 55%, release 40%).
fn stick_dir(x: i16, y: i16, cur: Option<Action>) -> Option<Action> {
    let (x, y) = (x as i32, y as i32);
    let thr = if cur.is_some() { 13100 } else { 18000 };
    if x.abs() >= y.abs() {
        if x > thr { Some(Action::Right) } else if x < -thr { Some(Action::Left) } else { None }
    } else if y > thr {
        Some(Action::Up)
    } else if y < -thr {
        Some(Action::Down)
    } else {
        None
    }
}

// ------------------------------------------------------------------------ gilrs (external pads)

pub fn spawn_gamepad() {
    spawn_hidraw();
    let _ = std::thread::Builder::new().name("gamepad".into()).spawn(|| {
        let mut gilrs = match Gilrs::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("[deckshell] gamepad unavailable: {e}");
                return;
            }
        };
        for (_, g) in gilrs.gamepads() {
            eprintln!("[deckshell] gamepad: {} ({:04x}:{:04x})", g.name(), g.vendor_id().unwrap_or(0), g.product_id().unwrap_or(0));
        }
        let mut rep = Repeat::default();
        let (mut dp, mut st) = ([false; 4], [0.0f32; 2]); // up down left right; stick x,y
        let mut stick: Option<Action> = None;
        let mut guide = false;
        let mut guide_used = false;
        let mut mic_extra: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        loop {
            for (id, pad) in gilrs.gamepads() {
                if pad.vendor_id() == Some(0x28de) && HIDRAW_ACTIVE.load(Ordering::Relaxed) { continue; }
                let buttons = [Button::South,Button::East,Button::North,Button::West,Button::C,Button::Z,
                    Button::LeftTrigger,Button::LeftTrigger2,Button::RightTrigger,Button::RightTrigger2,
                    Button::Select,Button::Start,Button::Mode,Button::LeftThumb,Button::RightThumb,
                    Button::DPadUp,Button::DPadDown,Button::DPadLeft,Button::DPadRight];
                let source = format!("pad:{id}");
                let mut down = buttons.iter().filter(|b| pad.is_pressed(**b)).map(|b| format!("Pad:{b:?}")).collect::<Vec<_>>();
                down.extend(mic_extra.get(&source).cloned().unwrap_or_default());
                crate::recording_mic::report(&source, down, pad.is_connected());
            }
            if let Some(ev) = gilrs.next_event_blocking(Some(Duration::from_millis(15))) {
                crate::idle::touch();
                let valve = gilrs.gamepad(ev.id).vendor_id() == Some(0x28de);
                if valve && HIDRAW_ACTIVE.load(Ordering::Relaxed) {
                    continue; // handled via hidraw
                }
                let pad = gilrs.gamepad(ev.id);
                let source = format!("pad:{}", ev.id);
                let extra = mic_extra.entry(source.clone()).or_default();
                match ev.event {
                    EventType::ButtonPressed(Button::Unknown, code) => { let key = format!("Pad:Code{}", code.into_u32()); if !extra.contains(&key) { extra.push(key); } },
                    EventType::ButtonReleased(Button::Unknown, code) => extra.retain(|b| b != &format!("Pad:Code{}", code.into_u32())),
                    EventType::Disconnected => extra.clear(),
                    _ => {}
                }
                let known = [Button::South,Button::East,Button::North,Button::West,Button::C,Button::Z,
                    Button::LeftTrigger,Button::LeftTrigger2,Button::RightTrigger,Button::RightTrigger2,
                    Button::Select,Button::Start,Button::Mode,Button::LeftThumb,Button::RightThumb,
                    Button::DPadUp,Button::DPadDown,Button::DPadLeft,Button::DPadRight];
                let mut down = known.iter().filter(|b| pad.is_pressed(**b)).map(|b| format!("Pad:{b:?}")).collect::<Vec<_>>();
                down.extend(extra.clone());
                if crate::recording_mic::report(&source, down, pad.is_connected()) { continue; }
                match ev.event {
                    EventType::ButtonPressed(b, _) => match b {
                        Button::DPadUp => dp[0] = true,
                        Button::DPadDown => dp[1] = true,
                        Button::DPadLeft => dp[2] = true,
                        Button::DPadRight => dp[3] = true,
                        Button::South => {
                            if guide {
                                guide_used = true;
                                send(Action::Qam)
                            } else {
                                send(Action::A)
                            }
                        }
                        Button::East => send(Action::B),
                        Button::West => send(Action::X),
                        Button::North => send(Action::Y),
                        Button::LeftTrigger => send(Action::LB),
                        Button::RightTrigger => send(Action::RB),
                        Button::Start => send(Action::Menu),
                        Button::Select => send(Action::View),
                        Button::Mode => {
                            guide = true;
                            guide_used = false;
                        }
                        _ => {}
                    },
                    EventType::ButtonReleased(b, _) => match b {
                        Button::DPadUp => dp[0] = false,
                        Button::DPadDown => dp[1] = false,
                        Button::DPadLeft => dp[2] = false,
                        Button::DPadRight => dp[3] = false,
                        // Guide alone = Steam menu (on release, so Guide+A can be QAM)
                        Button::Mode => {
                            if guide && !guide_used {
                                send(Action::Steam);
                            }
                            guide = false;
                        }
                        _ => {}
                    },
                    EventType::AxisChanged(ax, v, _) => match ax {
                        Axis::LeftStickX => st[0] = v,
                        Axis::LeftStickY => st[1] = v,
                        Axis::DPadX => {
                            dp[2] = v < -0.5;
                            dp[3] = v > 0.5;
                        }
                        Axis::DPadY => {
                            dp[1] = v < -0.5;
                            dp[0] = v > 0.5;
                        }
                        _ => {}
                    },
                    EventType::Disconnected => {
                        dp = [false; 4];
                        st = [0.0; 2];
                    }
                    _ => {}
                }
                stick = stick_dir((st[0] * 32767.0) as i16, (st[1] * 32767.0) as i16, stick);
                let d = if dp[0] {
                    Some(Action::Up)
                } else if dp[1] {
                    Some(Action::Down)
                } else if dp[2] {
                    Some(Action::Left)
                } else if dp[3] {
                    Some(Action::Right)
                } else {
                    None
                };
                let (d, analog) = pick_dir(d, stick);
                rep.set(d, analog);
            }
            rep.tick();
        }
    });
}

/// Debug: `DECKSHELL_HIDDUMP=<secs>` prints parsed Deck reports (changes only) and exits.
pub fn dump_hidraw(secs: u64) {
    let nodes = deck_hidraw_nodes();
    eprintln!("hidraw nodes: {nodes:?}");
    let Some((path, rx)) = open_stream() else {
        eprintln!("no streaming Deck hidraw interface");
        return;
    };
    eprintln!("streaming interface: {path}");
    let end = Instant::now() + Duration::from_secs(secs);
    let (mut n, mut last) = (0u32, String::new());
    while Instant::now() < end {
        if let Ok(b) = rx.recv_timeout(Duration::from_millis(100)) {
            n += 1;
            if let Some(s) = parse_deck_report(&b) {
                let line = format!("buttons={:?} L=({}, {}) R=({}, {})", s.down, s.lx, s.ly, s.rx, s.ry);
                if line != last {
                    eprintln!("{line}");
                    last = line;
                }
            }
        }
    }
    eprintln!("{n} reports in {secs}s");
}

#[cfg(test)]
mod chord_transition_tests {
    use super::*;
    #[test]
    fn deck_report_pad_touch_and_coordinates() {
        let mut report = [0u8; 64];
        report[0] = 1;
        report[2] = 9;
        assert!(!parse_deck_report(&report).unwrap().pads[0].touch);
        assert!(parse_deck_report(&report[..56]).is_none());
        report[10] = (1 << 3) | (1 << 4) | (1 << 1);
        report[16..18].copy_from_slice(&(-12000i16).to_le_bytes());
        report[18..20].copy_from_slice(&(20000i16).to_le_bytes());
        report[20..22].copy_from_slice(&(300i16).to_le_bytes());
        report[22..24].copy_from_slice(&(-32768i16).to_le_bytes());
        report[56..58].copy_from_slice(&(0x8221u16).to_le_bytes());
        report[58..60].copy_from_slice(&(0x1952u16).to_le_bytes());
        let st = parse_deck_report(&report).unwrap();
        assert_eq!(st.pads[0], DeckPadReport { xy: (-12000, 20000), pressure: 0x8221, touch: true, click: true });
        assert_eq!(st.pads[1], DeckPadReport { xy: (300, -32768), pressure: 0x1952, touch: true, click: false });
        report[10] = (1 << 4) | (1 << 2);
        let st = parse_deck_report(&report).unwrap();
        assert!(!st.pads[0].touch && !st.pads[0].click);
        assert!(st.pads[1].touch && st.pads[1].click);
    }

    #[test]
    fn deck_idle_report_from_hidraw_has_no_pad_contact_or_force() {
        // Passive idle capture from 28de:1205; frame counter and unrelated fields zeroed.
        let mut report = [0u8; 64];
        report[..4].copy_from_slice(&[1, 0, 9, 64]);
        report[48..64].copy_from_slice(&[
            0x24, 0x04, 0xda, 0xff, 0xe3, 0x02, 0xd7, 0xff, 0, 0, 0, 0, 0xfd, 0xff, 0, 0,
        ]);
        let st = parse_deck_report(&report).unwrap();
        assert_eq!(st.pads, [DeckPadReport::default(); 2]);
        assert_eq!((st.lx, st.ly, st.rx, st.ry), (1060, -38, 739, -41));
    }

    #[test]
    fn soft_press_debounces_and_combines_with_hard_click_and_trigger() {
        for right in [false, true] {
            let mut input = OskPadInput::default();
            let mut pad = DeckPadReport { touch: true, xy: (-22000, 5000), ..Default::default() };
            let mut sample = |input: &mut OskPadInput, pressure, click| {
                pad.pressure = pressure;
                pad.click = click;
                input.sample(pad, true);
                input.click_edge(right)
            };
            assert_eq!(sample(&mut input, PAD_PRESS_FORCE, false), None);
            assert_eq!(sample(&mut input, PAD_RELEASE_FORCE, false), None);
            for _ in 1..PAD_DEBOUNCE_REPORTS {
                assert_eq!(sample(&mut input, PAD_PRESS_FORCE, false), None);
            }
            assert_eq!(sample(&mut input, PAD_PRESS_FORCE, false), Some(Action::KbClick { right, down: true }));
            assert_eq!(sample(&mut input, PAD_RELEASE_FORCE + 1, false), None);
            assert_eq!(sample(&mut input, PAD_RELEASE_FORCE, true), None);
            input.trigger_down = true;
            for _ in 0..PAD_DEBOUNCE_REPORTS {
                assert_eq!(sample(&mut input, PAD_RELEASE_FORCE, false), None);
            }
            input.trigger_down = false;
            assert_eq!(input.click_edge(right), Some(Action::KbClick { right, down: false }));
            assert_eq!(sample(&mut input, 0, true), Some(Action::KbClick { right, down: true }));
        }
    }

    #[test]
    fn right_pad_keeps_position_through_touch_bit_dropout() {
        let mut input = OskPadInput::default();
        input.sample(DeckPadReport { touch: true, xy: (-21502, 4956), ..Default::default() }, true);
        for _ in 1..PAD_DEBOUNCE_REPORTS {
            input.sample(DeckPadReport::default(), true);
            assert_eq!(input.position, Some((-21502, 4956)));
            assert_eq!(input.click_edge(true), None);
        }
        input.sample(DeckPadReport { touch: true, xy: (-21000, 6200), ..Default::default() }, true);
        assert_eq!(input.position, Some((-21000, 6200)));
        for _ in 0..PAD_DEBOUNCE_REPORTS {
            input.sample(DeckPadReport::default(), true);
        }
        assert_eq!(input.position, None);
    }

    #[test]
    fn injected_wire_report_contains_deck_chord() {
        let mut report = [0u8; 64];
        report[0] = 1;
        report[2] = 9;
        report[8] = 1 << 7; // A
        report[9] = 1 << 5; // STEAM
        let state = parse_deck_report(&report).unwrap();
        assert!(state.down.contains(&DeckBtn::Steam));
        assert!(state.down.contains(&DeckBtn::A));
        let mut machine = hotkeys::Machine::new();
        let output: Vec<_> = ordered_transitions(&[], &state.down, true).into_iter()
            .flat_map(|b| machine.event(hotkeys::Ev::Press(hk_btn(b).unwrap()), 1)).collect();
        assert_eq!(output, vec![hotkeys::Out::Chord(hotkeys::Chord::RecordToggle)]);
    }

    #[test]
    fn simultaneous_steam_a_report_processes_modifier_first_and_releases_last() {
        let down = [DeckBtn::A, DeckBtn::Steam];
        let mut machine = hotkeys::Machine::new();
        let mut outputs = Vec::new();
        for b in ordered_transitions(&[], &down, true) {
            outputs.extend(machine.event(hotkeys::Ev::Press(hk_btn(b).unwrap()), 1));
        }
        for b in ordered_transitions(&down, &[], false) {
            outputs.extend(machine.event(hotkeys::Ev::Release(hk_btn(b).unwrap()), 100));
        }
        assert_eq!(outputs, vec![hotkeys::Out::Chord(hotkeys::Chord::RecordToggle)]);
    }
}
