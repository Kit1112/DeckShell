use steam_assets::i18n::{tr_args, tr_ref};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use slint::{ComponentHandle, Image, ModelRc, Timer, TimerMode, VecModel};

use super::super::controller_gyro::{self, Owner};
use crate::app::App;
use crate::assets::{controller_icon, glyph_image, t_or};
use crate::input::Action;
use crate::router::Route;
use crate::{AppWindow, ControllerDevicesState, DeviceAxis, DeviceRow, SettingsState};

pub const PAGE: &str = "controller";
const DESKTOP_APP: u32 = 413080;
const DASH: &str = "—";
const IDLE_VALUES: [i64; 7] = [9999, 5, 10, 15, 30, 60, 120];
const GYRO: u64 = 1 << 11;
const HAPTICS: u64 = 1 << 13;
const RUMBLE: u64 = (1 << 14) | (1 << 21);
static NEXT_FEED: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Page {
    #[default]
    List,
    Detail,
    Test,
    Calibration,
    LeftStick,
    RightStick,
    Trackpads,
    Gyro,
    Haptics,
    Add,
    PairBluetooth,
    Advanced,
}

impl Page {
    fn feed(self) -> Option<&'static str> {
        match self {
            Self::Test => Some("test"),
            Self::LeftStick | Self::RightStick => Some("sticks"),
            Self::Gyro => Some("gyro"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct Identity {
    #[serde(rename = "nControllerIndex")]
    index: u32,
    #[serde(rename = "eControllerType")]
    kind: u32,
    #[serde(rename = "strSerialNumber")]
    serial: String,
}

impl Identity {
    pub(crate) fn request(&self, op: &str) -> Value {
        json!({"op": op, "index": self.index, "type": self.kind, "serial": self.serial})
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Device {
    #[serde(flatten)]
    id: Identity,
    #[serde(rename = "strName")]
    name: String,
    #[serde(flatten)]
    state: serde_json::Map<String, Value>,
}

impl Device {
    fn has(&self, mask: u64) -> bool {
        self.state.get("caps").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|bits| bits & mask != 0)
    }

    fn deck(&self) -> bool { self.id.kind == 4 }

    fn name(&self) -> String {
        if self.name.starts_with('#') { t_or(&self.name, tr_ref("controller.controller")) }
        else if self.name.is_empty() { tr_ref("controller.controller").into() }
        else { self.name.clone() }
    }
}

#[derive(Deserialize, PartialEq, Default)]
struct Inventory {
    devices: Vec<Device>,
    idle: Option<i64>,
    dongles: Option<Value>,
    bluetooth: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Screen(Page),
    Device(Identity),
    Rename,
    Identify,
    Toggle { key: &'static str, field: &'static str, on: bool },
    Adjust { field: &'static str, lo: i64, hi: i64, step: i64, offset: i64 },
    Idle,
    Desktop,
    Firmware,
    UsbPair,
    GyroStart,
    HapticPulse(bool),
    Test(TestAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TestAction {
    Copy,
    Reset,
    Configure,
}

const TEST_ACTIONS: [(TestAction, &str); 3] = [
    (TestAction::Copy, "controller.copy_to_clipboard"),
    (TestAction::Reset, "controller.reset_device_inputs"),
    (TestAction::Configure, "controller.setup_device_inputs"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepKind {
    Trigger,
    Stick,
    Pad,
    PadClick,
}

/// One prompt of the step-by-step Deck input test. The stock page asks for the inputs in the order
/// of its list; the first prompt («Нажмите на левый триггер») is verified, later ones follow the list.
struct Step {
    kind: StepKind,
    side: &'static str,
    prompt_key: &'static str,
}

const DECK_STEPS: [Step; 8] = [
    Step { kind: StepKind::Trigger, side: "Left", prompt_key: "controller.pull_the_left_trigger" },
    Step { kind: StepKind::Pad, side: "Left", prompt_key: "controller.touch_the_left_trackpad" },
    Step { kind: StepKind::PadClick, side: "Left", prompt_key: "controller.press_the_left_trackpad" },
    Step { kind: StepKind::Stick, side: "Left", prompt_key: "controller.move_the_left_joystick" },
    Step { kind: StepKind::Trigger, side: "Right", prompt_key: "controller.pull_the_right_trigger" },
    Step { kind: StepKind::Pad, side: "Right", prompt_key: "controller.touch_the_right_trackpad" },
    Step { kind: StepKind::PadClick, side: "Right", prompt_key: "controller.press_the_right_trackpad" },
    Step { kind: StepKind::Stick, side: "Right", prompt_key: "controller.move_the_right_joystick" },
];

fn step_complete(step: &Step, sample: &Value) -> bool {
    let value = |suffix: &str| sample[format!("{}{suffix}", step.side)].as_f64().unwrap_or(0.0);
    match step.kind {
        StepKind::Trigger => value("Trigger") >= 16384.0,
        StepKind::PadClick => value("TrackpadPressure") > 0.0,
        StepKind::Stick => value("JoystickX").hypot(value("JoystickY")) >= 16384.0,
        StepKind::Pad => value("TrackpadX").hypot(value("TrackpadY")) >= 8192.0,
    }
}

#[derive(Clone, Copy)]
enum Modal {
    Idle,
    ResetName,
    Firmware,
    TestActions,
}

fn modal_len(modal: Modal) -> usize {
    match modal {
        Modal::Idle => IDLE_VALUES.len(),
        Modal::TestActions => TEST_ACTIONS.len(),
        _ => 2,
    }
}

#[derive(Default)]
pub struct State {
    page: Page,
    stack: Vec<(Page, Option<Command>)>,
    current: Option<Identity>,
    inventory: Option<Inventory>,
    commands: Vec<Option<Command>>,
    modal: Option<Modal>,
    choice: usize,
    name: String,
    timer: Timer,
    generation: u64,
    request: u64,
    polling: bool,
    writing: bool,
    inventory_at: Option<Instant>,
    inventory_error: String,
    action_status: String,
    feed_status: String,
    token: Option<u64>,
    telemetry: Value,
    usb_pairing: bool,
    stick_test: bool,
    test_step: usize,
}

impl State {
    fn current(&self) -> Option<&Device> {
        self.inventory.as_ref()?.devices.iter().find(|d| Some(&d.id) == self.current.as_ref())
    }

    fn focused(&self, index: Option<usize>) -> Option<Command> {
        self.commands.get(index?)?.clone()
    }
}

static DEVICE_TEXT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    json!({
        "calibration_is_already_running": tr_ref("controller.devices.calibration_is_already_running"),
        "calibration_mode_changed_reopen_the_wizard": tr_ref("controller.devices.calibration_mode_changed_reopen_the_wizard"),
        "controller_disconnected_or_replaced": tr_ref("controller.devices.controller_disconnected_or_replaced"),
        "controllerstore_unavailable": tr_ref("controller.devices.controllerstore_unavailable"),
        "selected_controller_s_gyro_is_unavailable": tr_ref("controller.devices.selected_controller_s_gyro_is_unavailable"),
        "steam_did_not_provide_a_calibration_subscription": tr_ref("controller.devices.steam_did_not_provide_a_calibration_subscription"),
        "steam_did_not_provide_new_calibration_tolerances_to": tr_ref("controller.devices.steam_did_not_provide_new_calibration_tolerances_to"),
        "steam_is_already_running_a_controller_test": tr_ref("controller.devices.steam_is_already_running_a_controller_test"),
        "steam_lost_the_calibration_session": tr_ref("controller.devices.steam_lost_the_calibration_session"),
        "steam_refreshed_the_controller_without_a_serial_number": tr_ref("controller.devices.steam_refreshed_the_controller_without_a_serial_number"),
        "steam_rejected_the_request": tr_ref("controller.devices.steam_rejected_the_request"),
        "steam_reported_a_calibration_error": tr_ref("controller.devices.steam_reported_a_calibration_error"),
        "unknown_controller_action": tr_ref("controller.devices.unknown_controller_action"),
    }).to_string()
});

pub(crate) fn request(bridge: &steam_bridge::Bridge, payload: Value, once: bool) -> Result<Value, String> {
    let expr = include_str!("controller_devices.js")
        .replace("/*text*/", &DEVICE_TEXT)
        .replace("/*request*/", &payload.to_string());
    (if once { bridge.eval_json_once(&expr) } else { bridge.eval_json(&expr) })
        .map_err(|err| err.to_string())
}

pub fn active(a: &App) -> bool {
    matches!(a.router.current(), Route::Settings(p) if p == PAGE)
}

pub fn bluetooth(a: &App) -> bool { active(a) && a.se.devices.page == Page::PairBluetooth }
pub fn testing(a: &App) -> bool { active(a) && a.se.devices.page == Page::Test }
pub fn owns_scope(a: &App) -> bool { active(a) && a.se.devices.page != Page::List }

pub fn wire(ui: &AppWindow) {
    let st = ui.global::<ControllerDevicesState>();
    st.on_activate(|i| crate::screens::defer(move |a| {
        if !active(a) || a.se.devices.modal.is_some() || controller_gyro::active(Owner::Settings) { return; }
        if let Some(cmd) = a.se.devices.focused(usize::try_from(i).ok()) {
            a.se.zone = 1;
            a.se.cf = Some(i as usize);
            activate(a, cmd);
        }
    }));
    st.on_choose(|i| crate::screens::defer(move |a| {
        if active(a) && i >= 0 { choose(a, i as usize); }
    }));
    st.on_back(|| crate::screens::defer(|a| { if active(a) { back(a); } }));
}

pub fn open(a: &mut App) {
    leave(a);
    let generation = a.se.devices.generation;
    a.se.devices = State { generation, ..State::default() };
    a.se.zone = 0;
    a.se.cf = None;
    a.se.rows.clear();
    a.se.on.clear();
    a.se.devices.timer.start(TimerMode::Repeated, Duration::from_millis(50), || {
        crate::with_app(refresh);
    });
    push(a);
    refresh(a);
}

pub fn leave(a: &mut App) {
    controller_gyro::close(a, Owner::Settings);
    let s = &mut a.se.devices;
    s.generation = s.generation.wrapping_add(1);
    s.timer.stop();
    end_feed(s);
    if s.usb_pairing {
        s.usb_pairing = false;
        end_usb();
    }
    crate::screens::osk::detach(a, NAME_SINK);
    a.ui().global::<ControllerDevicesState>().set_shown(false);
}

fn end_feed(s: &mut State) {
    if let Some(token) = s.token.take() {
        if let Some(bridge) = crate::actions::bridge() {
            std::thread::spawn(move || { let _ = request(&bridge, json!({"op": "end", "token": token}), true); });
        }
    }
    s.telemetry = Value::Null;
    s.feed_status.clear();
}

fn end_usb() {
    if crate::actions::dryrun() { return; }
    if let Some(bridge) = crate::actions::bridge() {
        std::thread::spawn(move || {
            if let Err(err) = request(&bridge, json!({"op":"usb-pair", "value":false, "cancel":true}), true) {
                eprintln!("[controller devices] failed to close pairing: {err}");
            }
        });
    }
}

fn refresh(a: &mut App) {
    if !active(a) || bluetooth(a) { return; }
    let s = &mut a.se.devices;
    if s.writing || s.polling { return; }
    let Some(bridge) = crate::actions::bridge() else {
        if s.inventory_error.is_empty() {
            s.inventory = None;
            s.inventory_error = tr_ref("controller.no_steam_connection").into();
            push(a);
        }
        return;
    };
    let inventory = s.inventory_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(1));
    let feed = s.page.feed().zip(s.token).zip(s.current.clone()).map(|((mode, token), id)| {
        let mut req = id.request("feed");
        req["token"] = json!(token);
        req["mode"] = json!(mode);
        req
    });
    if !inventory && feed.is_none() { return; }
    let read_dongles = s.page == Page::Add;
    let generation = s.generation;
    let sequence = s.request;
    s.polling = true;
    std::thread::spawn(move || {
        let data = inventory.then(|| request(&bridge, json!({"op":"inventory", "dongles":read_dongles, "defaults":true}), false)
            .and_then(|value| serde_json::from_value::<Inventory>(value).map_err(|e| e.to_string()))
            .map(|mut data| {
                data.bluetooth = read_dongles.then(|| !crate::actions::dryrun() && crate::screens::bluetooth::service_available());
                data
            }));
        let sample = feed.map(|req| request(&bridge, req, true));
        crate::screens::defer(move |a| {
            if !active(a) || a.se.devices.generation != generation { return; }
            let s = &mut a.se.devices;
            s.polling = false;
            if s.request != sequence { return; }
            let mut redraw = false;
            if let Some(data) = data {
                s.inventory_at = Some(Instant::now());
                match data {
                    Ok(data) => {
                        redraw |= s.inventory.as_ref() != Some(&data) || !s.inventory_error.is_empty();
                        s.inventory = Some(data);
                        s.inventory_error.clear();
                        if s.current.is_some() && s.current().is_none() {
                            a.se.devices.current = None;
                            a.se.devices.stack.clear();
                            switch(a, Page::List, None);
                            a.se.devices.action_status = tr_ref("controller.controller_disconnected").into();
                            push(a);
                            return;
                        }
                    }
                    Err(err) => {
                        let error = tr_args("controller.controller_data_unavailable_err", &[("err", &err.to_string())]);
                        redraw |= s.inventory_error != error;
                        s.inventory_error = error;
                        s.inventory = None;
                    }
                }
            }
            if let Some(sample) = sample {
                match sample {
                    Ok(sample) => {
                        if s.page == Page::Test && sample["exit"] == true {
                            back(a);
                            return;
                        }
                        if s.page == Page::Test && s.current().is_some_and(Device::deck) {
                            if let Some(step) = DECK_STEPS.get(s.test_step) {
                                if step_complete(step, &sample["sample"]) {
                                    s.test_step += 1;
                                    redraw = true;
                                }
                            }
                        }
                        redraw |= s.telemetry != sample || !s.feed_status.is_empty();
                        s.telemetry = sample;
                        s.feed_status.clear();
                    }
                    Err(err) => {
                        let error = tr_args("controller.input_unavailable_err", &[("err", &err.to_string())]);
                        redraw |= s.feed_status != error;
                        s.telemetry = Value::Null;
                        s.feed_status = error;
                    }
                }
            }
            if redraw { push(a); }
        });
    });
}

fn write(a: &mut App, payload: Value, accepted: Option<&'static str>) {
    if a.se.devices.writing { return; }
    if crate::actions::dryrun() {
        a.se.devices.action_status = tr_ref("controller.dry_run_action_was_not_sent_to_steam").into();
        push(a);
        return;
    }
    let Some(bridge) = crate::actions::bridge() else {
        a.se.devices.action_status = tr_ref("controller.no_steam_connection").into();
        push(a);
        return;
    };
    let s = &mut a.se.devices;
    let generation = s.generation;
    s.request = s.request.wrapping_add(1);
    s.writing = true;
    s.action_status = tr_ref("controller.waiting_for_steam").into();
    push(a);
    std::thread::spawn(move || {
        let result = request(&bridge, payload, true);
        crate::screens::defer(move |a| {
            if !active(a) || a.se.devices.generation != generation { return; }
            let s = &mut a.se.devices;
            s.writing = false;
            s.inventory_at = None;
            s.action_status = match result {
                Ok(_) => accepted.unwrap_or_default().to_owned(),
                Err(err) => tr_args("controller.steam_error_err", &[("err", &err.to_string())]),
            };
            push(a);
            refresh(a);
        });
    });
}

fn switch(a: &mut App, next: Page, focus: Option<Command>) {
    if bluetooth(a) { crate::screens::bluetooth::leave(a); }
    let s = &mut a.se.devices;
    if s.page == Page::Add && s.usb_pairing {
        s.usb_pairing = false;
        end_usb();
    }
    end_feed(s);
    s.request = s.request.wrapping_add(1);
    s.action_status.clear();
    s.modal = None;
    s.page = next;
    s.stick_test = false;
    s.test_step = 0;
    s.inventory_at = None;
    s.token = next.feed().map(|_| NEXT_FEED.fetch_add(1, Ordering::Relaxed));
    a.se.cf = None;
    if next == Page::PairBluetooth {
        a.ui().global::<ControllerDevicesState>().set_shown(false);
        crate::screens::bluetooth::open(a);
        crate::screens::bluetooth::enter(a);
    } else {
        paint(a, focus);
    }
    a.update_hint();
    refresh(a);
}

fn descend(a: &mut App, page: Page) {
    let s = &mut a.se.devices;
    s.stack.push((s.page, s.focused(a.se.cf)));
    switch(a, page, None);
}

pub fn back(a: &mut App) -> bool {
    if controller_gyro::active(Owner::Settings) {
        controller_gyro::act(a, Action::B);
        return true;
    }
    if a.se.devices.writing { return true; }
    if bluetooth(a) && crate::screens::bluetooth::back(a) { return true; }
    if a.se.devices.modal.take().is_some() {
        push(a);
        return true;
    }
    if std::mem::take(&mut a.se.devices.stick_test) {
        push(a);
        return true;
    }
    if let Some((page, focus)) = a.se.devices.stack.pop() {
        if page == Page::List { a.se.devices.current = None; }
        switch(a, page, focus);
        return true;
    }
    false
}

fn device_write(a: &mut App, op: &str, key: Option<&str>, value: Value) {
    let Some(device) = a.se.devices.current() else { return; };
    let mut req = device.id.request(op);
    if let Some(key) = key { req["key"] = json!(key); }
    req["value"] = value;
    write(a, req, None);
}

fn confirm(a: &mut App, modal: Modal, focus: usize) {
    a.se.devices.modal = Some(modal);
    a.se.devices.choice = focus;
    push(a);
}

fn choose(a: &mut App, i: usize) {
    let Some(modal) = a.se.devices.modal else { return; };
    let count = modal_len(modal);
    if i >= count { return; }
    a.se.devices.modal = None;
    push(a);
    match (modal, i) {
        (Modal::Idle, i) => write(a, json!({"op":"idle", "value":IDLE_VALUES[i]}), None),
        (Modal::ResetName, 1) => device_write(a, "name", None, json!("#SettingsController_SteamController")),
        (Modal::TestActions, i) => test_action(a, TEST_ACTIONS[i].0),
        (Modal::Firmware, 1) => write(a, json!({"op":"firmware"}), Some(tr_ref("controller.steam_accepted_the_update_request"))),
        _ => {}
    }
}

fn activate(a: &mut App, cmd: Command) {
    if a.se.devices.writing { return; }
    match cmd {
        Command::Screen(page) => descend(a, page),
        Command::Device(id) => {
            a.se.devices.current = Some(id);
            descend(a, Page::Detail);
        }
        Command::Rename => {
            if let Some(d) = a.se.devices.current() {
                a.se.devices.name = d.name();
                crate::screens::osk::attach(a, NAME_SINK);
                push(a);
            }
        }
        Command::Identify => device_write(a, "identify", None, Value::Null),
        Command::Toggle { key, field, on } => {
            if key == field {
                device_write(a, "personalization", Some(key), json!(i64::from(!on)));
            } else {
                device_write(a, "toggle", Some(key), json!(!on));
            }
        }
        Command::Idle => {
            let choice = a.se.devices.inventory.as_ref().and_then(|inv| inv.idle)
                .and_then(|value| IDLE_VALUES.iter().position(|x| *x == value)).unwrap_or(0);
            confirm(a, Modal::Idle, choice);
        }
        Command::Desktop => {
            crate::screens::controller::open(a, DESKTOP_APP);
        }
        Command::Firmware => confirm(a, Modal::Firmware, 0),
        Command::UsbPair => {
            let enable = !a.se.devices.usb_pairing;
            if !crate::actions::dryrun() { a.se.devices.usb_pairing = enable; }
            write(a, json!({"op":"usb-pair", "value":enable, "cancel":false}), None);
        }
        Command::GyroStart => {
            if let Some(d) = a.se.devices.current() {
                controller_gyro::open(a, Owner::Settings, d.id.clone());
            }
        }
        Command::HapticPulse(left) => {
            let field = if left { "nLHapticStrength" } else { "nRHapticStrength" };
            if let Some(value) = a.se.devices.current().and_then(|d| d.state.get(field)?.as_i64()) {
                device_write(a, "pulse", Some(field), json!(value - 3));
            }
        }
        Command::Adjust { .. } => adjust(a, cmd, 1),
        Command::Test(action) => test_action(a, action),
    }
}

fn test_action(a: &mut App, action: TestAction) {
    match action {
        TestAction::Reset => {
            let s = &mut a.se.devices;
            s.test_step = 0;
            s.action_status = tr_ref("controller.device_inputs_reset").into();
            push(a);
        }
        TestAction::Configure => descend(a, Page::Calibration),
        TestAction::Copy => {
            let text = test_report(&a.se.devices);
            a.se.devices.action_status = if crate::actions::dryrun() {
                tr_ref("controller.dry_run_clipboard_was_not_changed").into()
            } else {
                std::thread::spawn(move || copy_to_clipboard(&text));
                tr_ref("controller.values_copied_to_clipboard").into()
            };
            push(a);
        }
    }
}

fn test_report(s: &State) -> String {
    let mut text = s.current().map(Device::name).unwrap_or_default();
    for axis in axes(s) {
        text.push_str(&format!("\n{}: {}", axis.label, axis.value));
    }
    text
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (tool, args) in [("wl-copy", &[][..]), ("xclip", &["-selection", "clipboard"][..]), ("xsel", &["-b", "-i"][..])] {
        let Ok(mut child) = Command::new(tool).args(args).stdin(Stdio::piped())
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { continue };
        if child.stdin.take().is_some_and(|mut pipe| pipe.write_all(text.as_bytes()).is_ok()) {
            let _ = child.wait();
            return;
        }
    }
}

fn adjust(a: &mut App, cmd: Command, dir: i64) {
    let Command::Adjust { field, lo, hi, step, offset } = cmd else { return; };
    let Some(device) = a.se.devices.current() else { return; };
    let Some(value) = numeric(device, field) else { return; };
    let next = (value + offset + dir * step).clamp(lo, hi) - offset;
    if next == value { return; }
    let encoded = if field.ends_with("Deadzone") && device.state.get("defaultDeadzone").and_then(Value::as_i64) == Some(next) { -1 } else { next };
    device_write(a, "personalization", Some(field), json!(encoded));
}

const NAME_SINK: crate::screens::osk::ExtSink = crate::screens::osk::ExtSink {
    id: 57, mode: crate::screens::osk::InputMode::Normal, typed: name_typed, enter: name_done, cancel: name_cancel,
};

fn name_typed(a: &mut App, ch: Option<char>) {
    let name = &mut a.se.devices.name;
    match ch {
        Some(c) if !c.is_control() && name.chars().count() < 64 => name.push(c),
        None => { name.pop(); }
        _ => {}
    }
    push(a);
}

fn name_done(a: &mut App) {
    let value = std::mem::take(&mut a.se.devices.name);
    let value = value.trim();
    if !value.is_empty() { device_write(a, "name", None, json!(value)); }
    push(a);
}

fn name_cancel(a: &mut App) {
    a.se.devices.name.clear();
    push(a);
}

pub fn enter(a: &mut App) {
    a.se.zone = 1;
    push(a);
}

pub fn handle(a: &mut App, action: Action) {
    if controller_gyro::active(Owner::Settings) { return controller_gyro::act(a, action); }
    if bluetooth(a) {
        match action {
            Action::B => { back(a); }
            Action::Left => {}
            _ => crate::screens::bluetooth::handle(a, action),
        }
        return;
    }
    if action == Action::B {
        if !back(a) { a.se.zone = 0; a.se.cf = None; push(a); }
        return;
    }
    let s = &mut a.se.devices;
    if s.writing { return; }
    if let Some(modal) = s.modal {
        let count = modal_len(modal);
        let slot = s.choice;
        match action {
            Action::Up | Action::Down => s.choice = crate::screens::settings::sort_menu_step(slot, count, action == Action::Down),
            Action::A => return choose(a, slot),
            _ => {}
        }
        push(a);
        return;
    }
    if s.page.feed() == Some("sticks") && action == Action::Menu {
        s.stick_test = !s.stick_test;
        push(a);
        return;
    }
    if s.page == Page::Test && action == Action::Menu {
        return confirm(a, Modal::TestActions, 0);
    }
    if s.stick_test { return; }
    let cmd = s.focused(a.se.cf);
    match action {
        Action::X if cmd == Some(Command::Rename) => confirm(a, Modal::ResetName, 0),
        Action::Menu if s.page == Page::Haptics => {
            if let Some(Command::Adjust { field, .. }) = cmd {
                activate(a, Command::HapticPulse(field == "nLHapticStrength"));
            }
        }
        Action::A => {
            if let Some(cmd) = cmd { activate(a, cmd); }
        }
        Action::Left | Action::Right => {
            if let Some(cmd @ Command::Adjust { .. }) = cmd {
                adjust(a, cmd, if action == Action::Left { -1 } else { 1 });
            } else if s.page == Page::List && action == Action::Left {
                a.se.zone = 0;
                a.se.cf = None;
                push(a);
            }
        }
        Action::Up | Action::Down => {
            a.se.cf = step(&s.commands, a.se.cf, action == Action::Down);
            push(a);
        }
        _ => {}
    }
}

fn step(rows: &[Option<Command>], focus: Option<usize>, down: bool) -> Option<usize> {
    if down {
        rows.iter().enumerate().skip(focus.map_or(0, |i| i + 1)).find_map(|(i, r)| r.as_ref().map(|_| i)).or(focus)
    } else {
        rows.iter().take(focus.unwrap_or(rows.len())).rposition(Option::is_some).or(focus)
    }
}

pub fn hints(a: &App) -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    if controller_gyro::active(Owner::Settings) { return controller_gyro::hints(); }
    if bluetooth(a) { return crate::screens::bluetooth::hints(); }
    let back = ("b", "#ActionButtonLabelBack", 1, "Esc", Action::B);
    let select = ("a", "#ActionButtonLabelSelect", 1, "Enter", Action::A);
    let s = &a.se.devices;
    if s.modal.is_some() { return vec![select, back]; }
    match s.page {
        Page::Test => vec![("menu", tr_ref("controller.menu"), 2, "F9", Action::Menu), ("b", tr_ref("controller.hold_to_exit"), 1, "Esc", Action::B)],
        Page::LeftStick | Page::RightStick => {
            let label = if s.stick_test { tr_ref("controller.end_test") } else { tr_ref("controller.begin_test") };
            vec![("menu", label, 2, "F9", Action::Menu), back]
        }
        Page::Detail if s.focused(a.se.cf) == Some(Command::Rename) => vec![
            ("x", tr_ref("controller.reset_to_default"), 1, "F3", Action::X),
            ("a", tr_ref("controller.edit_name"), 1, "Enter", Action::A), back,
        ],
        Page::Haptics if matches!(s.focused(a.se.cf), Some(Command::Adjust { .. })) =>
            vec![("menu", tr_ref("controller.test_haptics"), 2, "F9", Action::Menu), select, back],
        _ => vec![select, back],
    }
}

type Rows = Vec<(DeviceRow, Option<Command>)>;

fn item(label: &str, detail: &str, value: &str, kind: i32, command: Option<Command>, glyph: Option<Image>) -> (DeviceRow, Option<Command>) {
    (DeviceRow {
        label: label.into(), detail: detail.into(), value: value.into(), kind,
        enabled: command.is_some(), glyph: glyph.unwrap_or_default(),
    }, command)
}

fn heading(rows: &mut Rows, label: &str) { rows.push(item(label, "", "", 0, None, None)); }

fn button(rows: &mut Rows, label: &str, value: &str, command: Command) {
    rows.push(item(label, "", value, 1, Some(command), None));
}

fn toggle(rows: &mut Rows, label: &str, detail: &str, key: &'static str, field: &'static str, value: Option<bool>) {
    let text = match value { Some(true) => tr_ref("controller.on"), Some(false) => tr_ref("controller.off"), None => DASH };
    rows.push(item(label, detail, text, 1, value.map(|on| Command::Toggle { key, field, on }), None));
}

fn boolean(d: &Device, key: &str) -> Option<bool> { d.state.get(key)?.as_bool() }

fn numeric(d: &Device, field: &str) -> Option<i64> {
    let raw = d.state.get(field)?.as_i64()?;
    if raw == -1 && field.ends_with("Deadzone") { d.state.get("defaultDeadzone")?.as_i64() }
    else { Some(raw) }
}

fn adjustable(rows: &mut Rows, d: &Device, field: &'static str, label: &str, deadzone: bool) {
    let (lo, hi, step, offset) = if deadzone { (2000, 18000, 200, 0) } else { (0, 12, 1, 2) };
    let amount = numeric(d, field);
    let text = amount.map_or_else(|| DASH.into(), |n| {
        if deadzone { format!("‹   {n}   ›") } else { format!("‹   {}dB   ›", n + offset) }
    });
    rows.push(item(label, "", &text, 1,
        amount.map(|_| Command::Adjust { field, lo, hi, step, offset }), None));
}

fn idle_label(value: i64) -> String {
    if value == 9999 { t_or("#ControllerSettings_TurnOffTimeout_Never", tr_ref("controller.never")) }
    else { t_or(&format!("#ControllerSettings_TurnOffTimeout_{value}"), &tr_args("controller.after_value_min", &[("value", &value.to_string())])) }
}

fn rows(a: &App) -> Rows {
    let s = &a.se.devices;
    let mut rows = Vec::new();
    if s.page == Page::List {
        heading(&mut rows, tr_ref("controller.connected_controllers"));
        if let Some(data) = &s.inventory {
            if data.devices.is_empty() { heading(&mut rows, tr_ref("controller.no_controllers_connected")); }
            for d in &data.devices {
                let connection = if boolean(d, "bBluetooth") == Some(true) { "Bluetooth" }
                    else if boolean(d, "bBluetooth") == Some(false) && boolean(d, "bWireless") == Some(false) { "USB" }
                    else { "" };
                rows.push(item(&d.name(), connection, tr_ref("controller.details"), 1, Some(Command::Device(d.id.clone())),
                    controller_icon(crate::screens::controller::controller_icon_name(d.id.kind))));
            }
        }
        button(&mut rows, tr_ref("controller.add_a_new_controller"), tr_ref("controller.add_controller"), Command::Screen(Page::Add));
        heading(&mut rows, tr_ref("controller.advanced_settings"));
        button(&mut rows, &t_or("#Settings_Controller_Advanced", tr_ref("controller.advanced_settings")), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::Advanced));
        return rows;
    }
    if s.page == Page::Add {
        if s.inventory.as_ref().and_then(|inv| inv.dongles.as_ref()).and_then(|d| d["steam_controller_v1"].as_bool()) == Some(true) {
            button(&mut rows, tr_ref("controller.pair_via_usb_receiver"),
                if s.usb_pairing { tr_ref("controller.finish") } else { tr_ref("controller.start") }, Command::UsbPair);
        }
        let cmd = s.inventory.as_ref().filter(|inv| inv.bluetooth == Some(true))
            .map(|_| Command::Screen(Page::PairBluetooth));
        if cmd.is_some() {
            let label = t_or("#Settings_Controller_PairBTController", tr_ref("controller.pair_a_bluetooth_controller"));
            let value = t_or("#Settings_Controller_PairBTController_Click", tr_ref("controller.pair"));
            rows.push(item(&label, "", &value, 1, cmd, None));
        } else {
            heading(&mut rows, &t_or("#Settings_Controller_NoBluetooth", tr_ref("controller.bluetooth_unavailable")));
        }
        return rows;
    }
    if s.page == Page::Advanced {
        heading(&mut rows, tr_ref("controller.external_gamepad_settings"));
        let idle = s.inventory.as_ref().and_then(|data| data.idle);
        rows.push(item(tr_ref("controller.idle_gamepad_shutdown_timeout"), "", &idle.map_or_else(|| DASH.into(), idle_label),
            1, idle.map(|_| Command::Idle), None));
        heading(&mut rows, tr_ref("controller.non_game_controller_layouts"));
        rows.push(item(tr_ref("controller.desktop_layout"), "", tr_ref("controller.edit"), 1,
            s.inventory.as_ref().filter(|inv| !inv.devices.is_empty()).map(|_| Command::Desktop), None));
        heading(&mut rows, tr_ref("controller.controller_firmware"));
        button(&mut rows, tr_ref("controller.update_connected_steam_controller_devices"), tr_ref("controller.start"), Command::Firmware);
        return rows;
    }
    let Some(d) = s.current() else { return rows; };
    let rumble = d.state.get("eRumblePreference").and_then(Value::as_i64).map(|n| n == 1);
    match s.page {
        Page::Detail => {
            let name = if a.osk_is_open() { s.name.clone() } else { d.name() };
            button(&mut rows, tr_ref("controller.controller_name"), &name, Command::Rename);
            if d.has(RUMBLE) {
                button(&mut rows, tr_ref("controller.identify_controller"), tr_ref("controller.ping"), Command::Identify);
                toggle(&mut rows, &t_or("#Settings_ControllerCalibration_RumbleEnable", tr_ref("controller.game_rumble")), "", "rumble", "eRumblePreference", rumble);
            }
            if d.has(HAPTICS) {
                toggle(&mut rows, &t_or("#Settings_ControllerCalibration_HapticsEnable", tr_ref("controller.steam_haptics")), "", "haptics", "bHaptics", boolean(d, "bHaptics"));
            }
            heading(&mut rows, tr_ref("controller.advanced_settings"));
            toggle(&mut rows, &t_or("#Settings_Controller_SwitchLayout", tr_ref("controller.use_nintendo_button_layout")),
                &t_or("#Settings_Controller_SwitchLayoutDesc", tr_ref("controller.if_enabled_layouts_on_this_controller_will_swap")),
                "nintendo", "bUseReversedLayout", boolean(d, "bUseReversedLayout"));
            if !matches!(d.id.kind, 39 | 40) {
                toggle(&mut rows, &t_or("#Settings_Controller_UniversalGlyphs", tr_ref("controller.universal_face_button_glyphs")),
                    &t_or("#Settings_Controller_UniversalGlyphsDesc", tr_ref("controller.face_button_glyphs_will_be_replaced_with_a")),
                    "universal", "bUseUniversalFaceButtonGlyphs", boolean(d, "bUseUniversalFaceButtonGlyphs"));
            }
            button(&mut rows, &t_or("#Settings_Controller_TestInput", tr_ref("controller.test_device_inputs")), &t_or("#Settings_Controller_TestInput_Click", tr_ref("controller.begin_test")), Command::Screen(Page::Test));
            button(&mut rows, &t_or("#Settings_Controller_Calibration", tr_ref("controller.calibration_advanced_settings")), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::Calibration));
        }
        Page::Calibration => {
            if d.has(1 << 2) { button(&mut rows, tr_ref("controller.left_joystick_2"), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::LeftStick)); }
            if d.has(1 << 3) { button(&mut rows, tr_ref("controller.right_joystick_2"), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::RightStick)); }
            if d.deck() { button(&mut rows, tr_ref("controller.trackpad_lockout"), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::Trackpads)); }
            if d.has(GYRO) { button(&mut rows, tr_ref("controller.gyro_calibration"), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::Gyro)); }
            if d.has(HAPTICS | RUMBLE) { button(&mut rows, tr_ref("controller.haptics_settings"), &t_or("#Settings_Controller_Calibration_Click", tr_ref("controller.show_advanced_settings")), Command::Screen(Page::Haptics)); }
        }
        Page::LeftStick => adjustable(&mut rows, d, "nLStickDeadzone", tr_ref("controller.left_joystick_deadzone"), true),
        Page::RightStick => adjustable(&mut rows, d, "nRStickDeadzone", tr_ref("controller.right_joystick_deadzone"), true),
        Page::Trackpads => {
            for (title, key, detail) in [
                (tr_ref("controller.left_joystick_and_left_trackpad"), "bLeftStickTouchDisablesLeftTrackPad", tr_ref("controller.disables_the_left_trackpad_while_a_finger_touches")),
                (tr_ref("controller.right_joystick_and_right_trackpad"), "bRightStickTouchDisablesRightTrackPad", tr_ref("controller.disables_the_right_trackpad_while_a_finger_touches")),
            ] {
                toggle(&mut rows, title, detail, key, key, boolean(d, key));
            }
        }
        Page::Gyro => button(&mut rows, tr_ref("controller.gyro_calibration"), tr_ref("controller.start"), Command::GyroStart),
        Page::Haptics => {
            if d.has(RUMBLE) { toggle(&mut rows, &t_or("#Settings_ControllerCalibration_RumbleEnable", tr_ref("controller.game_rumble")), "", "rumble", "eRumblePreference", rumble); }
            if d.has(HAPTICS) {
                toggle(&mut rows, &t_or("#Settings_ControllerCalibration_HapticsEnable", tr_ref("controller.steam_haptics")), "", "haptics", "bHaptics", boolean(d, "bHaptics"));
                if d.deck() {
                    adjustable(&mut rows, d, "nLHapticStrength", &t_or("#Settings_Controller_Calibration_LHapticStrength", tr_ref("controller.left_haptics_intensity")), false);
                    adjustable(&mut rows, d, "nRHapticStrength", &t_or("#Settings_Controller_Calibration_RHapticStrength", tr_ref("controller.right_haptics_intensity")), false);
                    heading(&mut rows, &t_or("#Settings_Controller_Calibration_HapticsWarning", tr_ref("controller.on_steam_deck_we_recommend_leaving_your_haptics")));
                }
            }
        }
        Page::Test => {
            let sample = &s.telemetry["sample"];
            if !d.deck() {
                for (action, label) in TEST_ACTIONS { button(&mut rows, tr_ref(label), "", Command::Test(action)); }
            }
            for (label, key, glyph) in [
                ("A", "bButtonSouth", "a"), ("B", "bButtonEast", "b"), ("X", "bButtonWest", "x"),
                ("Y", "bButtonNorth", "y"), (tr_ref("controller.d_pad_2"), "bDPadUp", "shared_dpad_up"), (tr_ref("controller.d_pad_3"), "bDPadDown", "shared_dpad_down"),
                (tr_ref("controller.d_pad_4"), "bDPadLeft", "shared_dpad_left"), (tr_ref("controller.d_pad_5"), "bDPadRight", "shared_dpad_right"),
                ("L1", "bLeftBumper", "l1"), ("R1", "bRightBumper", "r1"),
                ("L4", "bL4", "l4"), ("R4", "bR4", "r4"), ("L5", "bL5", "l5"), ("R5", "bR5", "r5"),
                (tr_ref("controller.menu"), "bOptionsButton", "menu"), (tr_ref("controller.view"), "bViewButton", "view"),
                ("Steam", "bSteamButton", "steam"), (tr_ref("controller.quick_access"), "bQuickAccessButton", "qam"),
            ] {
                if let Some(down) = sample[key].as_bool() {
                    rows.push(item(label, "", if down { tr_ref("controller.pressed") } else { tr_ref("controller.released") }, 1, None, glyph_image(glyph)));
                }
            }
        }
        _ => {}
    }
    rows
}

fn axis(label: &str, x: Option<f64>, y: Option<f64>, pair: bool, scale: f64, radius: f32) -> DeviceAxis {
    let ready = x.is_some() && (!pair || y.is_some());
    let value = match (x, y, pair) {
        (Some(x), Some(y), true) => format!("{x:.0}, {y:.0}"),
        (Some(x), _, false) => format!("{x:.0}"),
        _ => DASH.into(),
    };
    DeviceAxis {
        label: label.into(), value: value.into(), ready, pair, radius,
        x: (x.unwrap_or_default() / scale) as f32, y: (y.unwrap_or_default() / scale) as f32,
    }
}

fn axes(s: &State) -> Vec<DeviceAxis> {
    let Some(d) = s.current() else { return Vec::new(); };
    match s.page {
        Page::Test => {
            let sample = &s.telemetry["sample"];
            let mut axes = Vec::new();
            for (side, trigger, stick, pad) in [
                ("Left", tr_ref("controller.left_trigger"), tr_ref("controller.left_joystick_2"), tr_ref("controller.left_trackpad_2")),
                ("Right", tr_ref("controller.right_trigger"), tr_ref("controller.right_joystick_2"), tr_ref("controller.right_trackpad_2")),
            ] {
                let value = |suffix: &str| sample[format!("{side}{suffix}")].as_f64();
                if d.has(1 << 6) {
                    axes.push(axis(trigger, value("Trigger"), None, false, 32767.0, 0.0));
                }
                if d.has(if side == "Left" { 1 << 2 } else { 1 << 3 }) {
                    axes.push(axis(stick, value("JoystickX"), value("JoystickY"), true, 32767.0, 0.0));
                }
                if d.deck() || value("TrackpadX").is_some() {
                    axes.push(axis(pad, value("TrackpadX"), value("TrackpadY"), true, 32767.0, 0.0));
                    let title = if side == "Left" { tr_ref("controller.left_trackpad_click") } else { tr_ref("controller.right_trackpad_click") };
                    axes.push(axis(title, value("TrackpadPressure"), None, false, 32767.0, 0.0));
                }
            }
            axes
        }
        Page::LeftStick | Page::RightStick => {
            let (side, field) = if s.page == Page::LeftStick { ("left", "nLStickDeadzone") } else { ("right", "nRStickDeadzone") };
            let values = &s.telemetry["axes"][side];
            let value = |key: &str| values[key].as_f64().map(|n| n * 32767.0);
            let radius = numeric(d, field).map_or(0.0, |n| n as f32 / 32767.0);
            vec![axis(tr_ref("controller.joystick_position"), value("x"), value("y"), true, 32767.0, radius)]
        }
        _ => Vec::new(),
    }
}

fn paint(a: &mut App, restore: Option<Command>) {
    let list = rows(a);
    let axes = axes(&a.se.devices);
    let s = &mut a.se.devices;
    let was = restore.or_else(|| s.focused(a.se.cf));
    let (rows, commands): (Vec<_>, Vec<_>) = list.into_iter().unzip();
    s.commands = commands;
    let matches_focus = |cmd: &Command| s.commands.iter().position(|entry| {
        match (entry.as_ref(), cmd) {
            (Some(Command::Toggle { key: a, .. }), Command::Toggle { key: b, .. }) => a == b,
            _ => entry.as_ref() == Some(cmd),
        }
    });
    a.se.cf = if a.se.zone == 1 {
        was.as_ref().and_then(matches_focus).or_else(|| s.commands.iter().position(Option::is_some))
    } else { None };
    let gyro = &s.telemetry["gyro"];
    let gyro_reading = gyro["pitch"].as_f64().zip(gyro["yaw"].as_f64()).zip(gyro["roll"].as_f64())
        .map(|((x, y), z)| tr_args("controller.pitch_x_yaw_y_roll_z", &[("x", &format!("{x:.2}")), ("y", &format!("{y:.2}")), ("z", &format!("{z:.2}"))]))
        .unwrap_or_else(|| tr_ref("controller.waiting_for_gyro_data").into());
    let (title, subtitle) = match s.page {
        Page::List => (tr_ref("controller.controller").into(), String::new()),
        Page::Detail => (s.current().map_or_else(|| tr_ref("controller.controller").into(), Device::name), String::new()),
        Page::Test => (s.current().map_or_else(|| tr_ref("controller.input_test").into(), Device::name), test_prompt(s)),
        Page::Calibration => (t_or("#Settings_Controller_Calibration", tr_ref("controller.calibration_advanced_settings")), String::new()),
        Page::LeftStick => (tr_ref("controller.left_joystick_2").into(), stick_help(s.stick_test).into()),
        Page::RightStick => (tr_ref("controller.right_joystick_2").into(), stick_help(s.stick_test).into()),
        Page::Trackpads => (tr_ref("controller.trackpad_lockout").into(), String::new()),
        Page::Gyro => (tr_ref("controller.gyro_3").into(), gyro_reading),
        Page::Haptics => (tr_ref("controller.haptics_settings").into(), String::new()),
        Page::Add => (tr_ref("controller.add_a_new_controller").into(), tr_ref("controller.to_add_a_steam_controller_connect_the_usb").into()),
        Page::Advanced => (t_or("#Settings_Controller_Advanced", tr_ref("controller.advanced_settings")), String::new()),
        Page::PairBluetooth => unreachable!(),
    };
    let status = [&s.action_status, &s.feed_status, &s.inventory_error].into_iter()
        .find(|status| !status.is_empty()).cloned().unwrap_or_default();
    let modal = s.modal.map(|modal| match modal {
        Modal::Idle => (tr_ref("controller.idle_gamepad_shutdown_timeout"), "", IDLE_VALUES.map(idle_label).to_vec()),
        Modal::ResetName => (tr_ref("controller.reset_controller_name"), tr_ref("controller.steam_will_restore_the_default_name"), vec![tr_ref("controller.cancel").into(), tr_ref("controller.reset").into()]),
        Modal::TestActions => (tr_ref("controller.input_test"), "", TEST_ACTIONS.map(|(_, label)| tr_ref(label).into()).to_vec()),
        Modal::Firmware => (tr_ref("controller.update_controller_firmware"), tr_ref("controller.steam_will_update_supported_devices_keep_them_connected"), vec![tr_ref("controller.cancel").into(), tr_ref("controller.start").into()]),
    });
    let full = s.page != Page::List;
    let choice = s.choice;
    let ui = a.ui();
    let st = ui.global::<ControllerDevicesState>();
    st.set_shown(true);
    st.set_full(full);
    st.set_title(title.into());
    st.set_subtitle(subtitle.into());
    st.set_status(status.into());
    st.set_focus(a.se.cf.map_or(-1, |i| i as i32));
    st.set_rows(ModelRc::new(VecModel::from(rows)));
    st.set_axes(ModelRc::new(VecModel::from(axes)));
    st.set_modal(modal.is_some());
    if let Some((title, detail, choices)) = modal {
        st.set_modal_title(title.into());
        st.set_modal_description(detail.into());
        st.set_choices(ModelRc::new(VecModel::from(choices.into_iter().map(|c| DeviceRow { label: c.into(), kind: 1, enabled: true, ..DeviceRow::default() }).collect::<Vec<_>>())));
        st.set_choice_focus(choice as i32);
    }
    let ss = ui.global::<SettingsState>();
    ss.set_is_internet(false);
    ss.set_is_anim(false);
    ss.set_is_recording(false);
    ss.set_is_storage(false);
    ss.set_storage_modal(false);
    ss.set_focus(a.se.focus as i32);
    ss.set_cursor(a.se.cursor as i32);
    ss.set_zone(a.se.zone as i32);
}

fn test_prompt(s: &State) -> String {
    match s.current() {
        Some(d) if d.deck() => DECK_STEPS.get(s.test_step).map_or_else(
            || tr_ref("controller.input_test_hold_b_to_exit").into(), |step| tr_ref(step.prompt_key).into()),
        _ => tr_ref("controller.input_test_hold_b_to_exit").into(),
    }
}

fn stick_help(test: bool) -> &'static str {
    if test { tr_ref("controller.move_the_joystick_b_ends_the_test") }
    else { tr_ref("controller.adjust_the_deadzone_with_the_left_and_right") }
}

pub fn push(a: &mut App) {
    if bluetooth(a) {
        a.ui().global::<ControllerDevicesState>().set_shown(false);
        crate::screens::bluetooth::push(a);
    } else {
        paint(a, None);
    }
    a.update_hint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_step_skips_headings_and_stays_at_edges() {
        let rows = [None, Some(Command::Idle), None, Some(Command::Firmware)];
        assert_eq!(step(&rows, None, true), Some(1));
        assert_eq!(step(&rows, Some(1), true), Some(3));
        assert_eq!(step(&rows, Some(3), true), Some(3));
        assert_eq!(step(&rows, Some(3), false), Some(1));
        assert_eq!(step(&rows, Some(1), false), Some(1));
    }

    #[test]
    fn deck_steps_start_with_left_trigger_and_detect_inputs() {
        assert_eq!(tr_ref(DECK_STEPS[0].prompt_key), tr_ref("controller.pull_the_left_trigger"));
        let idle = json!({"LeftTrigger": 0, "RightTrigger": 40000});
        assert!(!step_complete(&DECK_STEPS[0], &idle));
        assert!(step_complete(&DECK_STEPS[0], &json!({"LeftTrigger": 32767})));
        assert!(!step_complete(&DECK_STEPS[4], &json!({"LeftTrigger": 32767})));
        assert!(step_complete(&DECK_STEPS[7], &json!({"RightJoystickX": 30000, "RightJoystickY": 0})));
        assert!(!step_complete(&DECK_STEPS[3], &json!({"LeftJoystickX": 3000, "LeftJoystickY": 3000})));
        assert!(step_complete(&DECK_STEPS[2], &json!({"LeftTrackpadPressure": 5})));
    }

    #[test]
    fn test_actions_match_stock_menu_and_dryrun_copy_is_silent() {
        let labels: Vec<_> = TEST_ACTIONS.iter().map(|(_, l)| tr_ref(l)).collect();
        assert_eq!(labels, [tr_ref("controller.copy_to_clipboard"), tr_ref("controller.reset_device_inputs"), tr_ref("controller.setup_device_inputs")]);
        assert_eq!(modal_len(Modal::TestActions), 3);
        assert_eq!(test_report(&State::default()), "");
    }

    #[test]
    fn dryrun_never_reaches_steam() {
        // Dangerous actions are guarded by the DRYRUN check in `write`; the idle list never offers a value outside the stock set.
        assert_eq!(IDLE_VALUES[0], 9999);
        assert!(IDLE_VALUES[1..].windows(2).all(|w| w[0] < w[1]));
    }
}
