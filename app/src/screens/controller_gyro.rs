use steam_assets::i18n::{tr_args, tr_ref};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};
use slint::ComponentHandle;

use super::settings::controller_devices::{self, Identity};
use crate::{app::App, assets::t_or, input::Action, AppWindow, ControllerGyroState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Editor,
    Settings,
}

#[derive(Deserialize)]
struct Target {
    #[serde(flatten)]
    id: Identity,
    name: String,
    mode: String,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
enum Stage {
    Loading,
    Prompt,
    Countdown {
        seconds: u64,
    },
    Waiting,
    Stationary {
        progress: Option<f32>,
    },
    Drift {
        progress: Option<f32>,
    },
    Hardware {
        progress: f32,
    },
    Complete {
        savable: bool,
    },
    Unconfirmed,
    Failed {
        detail: String,
        #[serde(default)]
        retryable: bool,
    },
    Saving,
    Saved {
        error: Option<String>,
    },
}

struct Wizard {
    id: u64,
    owner: Owner,
    name: String,
    target: Option<Identity>,
    stage: Stage,
    input: Sender<()>,
}

thread_local! {
    static WIZARD: RefCell<Option<Wizard>> = const { RefCell::new(None) };
}
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) fn wire(ui: &AppWindow) {
    ui.global::<ControllerGyroState>().on_action(|ok| {
        super::defer(move |a| act(a, if ok { Action::A } else { Action::B }));
    });
}

pub(crate) fn active(owner: Owner) -> bool {
    WIZARD.with_borrow(|wizard| wizard.as_ref().is_some_and(|w| w.owner == owner))
}

pub(super) fn close(a: &mut App, owner: Owner) {
    if active(owner) {
        WIZARD.with_borrow_mut(|wizard| *wizard = None);
        sync(a);
    }
}

pub(super) fn open(a: &mut App, owner: Owner, identity: Identity) {
    if crate::actions::dryrun() {
        a.toast(tr_ref("controller.dry_run_calibration_is_disabled"));
        return;
    }
    if WIZARD.with_borrow(Option::is_some) {
        return;
    }
    let Some(bridge) = crate::actions::bridge() else {
        a.toast(tr_ref("controller.steam_unavailable"));
        return;
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let token = format!("{}-{stamp}-{id}", std::process::id());
    let (input, actions) = mpsc::channel();
    WIZARD.with_borrow_mut(|wizard| {
        *wizard = Some(Wizard {
            id,
            owner,
            name: String::new(),
            target: None,
            stage: Stage::Loading,
            input,
        });
    });
    sync(a);

    std::thread::spawn(move || {
        let mut selector = identity.request("gyro-target");
        selector["token"] = json!(token);
        let target = controller_devices::request(&bridge, selector, false)
            .and_then(|value| serde_json::from_value::<Target>(value).map_err(|e| e.to_string()));
        let result = target.and_then(|target| run(&bridge, id, &token, target, &actions));
        if let Err(detail) = result {
            publish(
                id,
                Stage::Failed {
                    detail,
                    retryable: false,
                },
            );
        }
    });
}

fn publish(id: u64, stage: Stage) {
    super::defer(move |a| {
        let changed = WIZARD.with_borrow_mut(|wizard| {
            let Some(w) = wizard.as_mut().filter(|w| w.id == id) else {
                return false;
            };
            if w.stage == stage
                || matches!((&w.stage, &stage), (Stage::Saving, Stage::Complete { .. }))
            {
                return false;
            }
            w.stage = stage;
            true
        });
        if changed {
            sync(a);
        }
    });
}

fn run(
    bridge: &steam_bridge::Bridge,
    id: u64,
    token: &str,
    target: Target,
    actions: &Receiver<()>,
) -> Result<(), String> {
    let identity = target.id.clone();
    super::defer(move |a| {
        let changed = WIZARD.with_borrow_mut(|wizard| {
            let Some(w) = wizard.as_mut().filter(|w| w.id == id) else {
                return false;
            };
            w.name = if target.name.is_empty() {
                tr_ref("controller.controller").into()
            } else if target.name.starts_with('#') {
                t_or(&target.name, tr_ref("controller.controller"))
            } else {
                target.name
            };
            w.target = Some(identity);
            w.stage = Stage::Prompt;
            true
        });
        if changed {
            sync(a);
        }
    });
    if actions.recv().is_err() {
        return Ok(());
    }
    for seconds in (0..=4).rev() {
        publish(id, Stage::Countdown { seconds });
        if actions.recv_timeout(Duration::from_secs(1)) != Err(RecvTimeoutError::Timeout) {
            return Ok(());
        }
    }

    let call = |op: &str| {
        let mut req = target.id.request(op);
        req["token"] = json!(token);
        req["mode"] = json!(target.mode);
        controller_devices::request(bridge, req, true)
    };
    publish(id, Stage::Waiting);
    let result = call("gyro-start").and_then(|_| track(&call, id, actions));
    // The start can reach Steam even if CDP loses the reply. End only this token.
    let cleanup = call("gyro-end");
    match (result, cleanup) {
        (Ok(Some(stage)), Ok(_)) => publish(id, stage),
        (Err(err), Ok(_)) => return Err(err),
        (result, Err(err)) => {
            let detail = match result {
                Ok(Some(Stage::Saved { error: None })) => {
                    tr_ref("controller.steam_confirmed_calibration_save_request_sent").into()
                }
                Ok(Some(Stage::Saved { error: Some(error) })) => tr_args("controller.steam_confirmed_calibration_save_unconfirmed_error", &[("error", &error)]),
                Ok(Some(Stage::Complete { .. })) => tr_ref("controller.steam_confirmed_calibration").into(),
                Ok(Some(Stage::Failed { detail, .. })) | Err(detail) => detail,
                _ => tr_ref("controller.calibration_session_finished_in_deckshell").into(),
            };
            return Err(tr_args("controller.detail_could_not_end_the_steam_session_err", &[("detail", &detail), ("err", &err.to_string())]));
        }
        (Ok(None), Ok(_)) => {}
    }
    Ok(())
}

fn track(
    call: &impl Fn(&str) -> Result<Value, String>,
    id: u64,
    actions: &Receiver<()>,
) -> Result<Option<Stage>, String> {
    let start = Instant::now();
    let mut savable = false;
    loop {
        match actions.recv_timeout(Duration::from_millis(100)) {
            Err(RecvTimeoutError::Disconnected) => return Ok(None),
            Ok(()) if savable => {
                let error = call("gyro-save").err();
                return Ok(Some(Stage::Saved { error }));
            }
            _ => {}
        }
        let stage =
            serde_json::from_value::<Stage>(call("gyro-poll")?).map_err(|e| e.to_string())?;
        match stage {
            Stage::Complete { savable: true } => savable = true,
            Stage::Complete { savable: false } | Stage::Unconfirmed | Stage::Failed { .. } => {
                return Ok(Some(stage));
            }
            _ if start.elapsed() >= Duration::from_secs(30) => {
                return Err(tr_ref("controller.steam_did_not_confirm_calibration_within_30_seconds").into());
            }
            _ => {}
        }
        publish(id, stage);
    }
}

pub(crate) fn act(a: &mut App, action: Action) {
    let mut retry = None;
    let close = WIZARD.with_borrow_mut(|wizard| {
        let Some(w) = wizard.as_mut() else {
            return false;
        };
        match (&w.stage, action) {
            (Stage::Saving, _) => false,
            (Stage::Prompt, Action::A) => {
                w.stage = Stage::Countdown { seconds: 4 };
                let _ = w.input.send(());
                false
            }
            (Stage::Complete { savable: true }, Action::A) => {
                w.stage = Stage::Saving;
                let _ = w.input.send(());
                false
            }
            (
                Stage::Failed {
                    retryable: true, ..
                },
                Action::A,
            ) => {
                retry = w.target.clone().map(|target| (w.owner, target));
                true
            }
            (
                Stage::Complete { savable: false }
                | Stage::Unconfirmed
                | Stage::Failed { .. }
                | Stage::Saved { .. },
                Action::A,
            )
            | (_, Action::B) => true,
            _ => false,
        }
    });
    if close {
        WIZARD.with_borrow_mut(|wizard| *wizard = None);
    }
    if let Some((owner, target)) = retry {
        open(a, owner, target);
    }
    sync(a);
}

pub(crate) fn hints() -> Vec<(&'static str, &'static str, i32, &'static str, Action)> {
    WIZARD.with_borrow(|wizard| {
        let Some(w) = wizard.as_ref() else {
            return Vec::new();
        };
        let (_, _, _, ok, back) = presentation(&w.stage);
        [(ok, "a", "Enter", Action::A), (back, "b", "Esc", Action::B)]
            .into_iter()
            .filter_map(|(label, glyph, key, act)| {
                (!label.is_empty()).then_some((glyph, label, 1, key, act))
            })
            .collect()
    })
}

fn presentation(
    stage: &Stage,
) -> (
    &'static str,
    String,
    Option<f32>,
    &'static str,
    &'static str,
) {
    let title = tr_ref("controller.gyro_calibration");
    let still = tr_ref("controller.place_the_controller_on_a_flat_surface_and");
    match stage {
        Stage::Loading => (
            title,
            tr_ref("controller.reading_selected_controller_data").into(),
            None,
            "",
            tr_ref("controller.cancel"),
        ),
        Stage::Prompt => (title, still.into(), None, tr_ref("controller.continue"), tr_ref("controller.cancel")),
        Stage::Countdown { seconds } => (
            title,
            tr_args("controller.still_starting_in_seconds_s", &[("still", still), ("seconds", &seconds.to_string())]),
            None,
            "",
            tr_ref("controller.cancel"),
        ),
        Stage::Waiting => (title, tr_ref("controller.waiting_for_steam").into(), None, "", tr_ref("controller.close")),
        Stage::Stationary { progress } => (
            title,
            tr_ref("controller.measuring_sensor_noise_at_rest").into(),
            *progress,
            "",
            tr_ref("controller.cancel"),
        ),
        Stage::Drift { progress } => (
            title,
            tr_ref("controller.compensating_for_gyro_drift").into(),
            *progress,
            "",
            tr_ref("controller.cancel"),
        ),
        Stage::Hardware { progress } => (
            title,
            tr_ref("controller.hardware_calibration_timeout").into(),
            Some(*progress),
            "",
            tr_ref("controller.close"),
        ),
        Stage::Complete { savable: true } => (
            tr_ref("controller.calibration_complete"),
            tr_ref("controller.steam_confirmed_the_result_save_the_new_tolerances").into(),
            None,
            tr_ref("controller.save"),
            tr_ref("controller.close"),
        ),
        Stage::Complete { savable: false } => (
            tr_ref("controller.calibration_complete"),
            tr_ref("controller.steam_confirmed_the_result_new_tolerances_are_unavailable").into(),
            None,
            tr_ref("controller.ok"),
            "",
        ),
        Stage::Unconfirmed => (
            title,
            tr_ref("controller.timed_out_steam_did_not_report_a_hardware").into(),
            None,
            tr_ref("controller.ok"),
            "",
        ),
        Stage::Failed {
            detail,
            retryable: true,
        } => (
            tr_ref("controller.calibration_error"),
            detail.clone(),
            None,
            tr_ref("controller.retry"),
            tr_ref("controller.cancel"),
        ),
        Stage::Failed {
            detail,
            retryable: false,
        } => (tr_ref("controller.calibration_wizard_message"), detail.clone(), None, tr_ref("controller.ok"), ""),
        Stage::Saving => (
            title,
            tr_ref("controller.waiting_for_steam_to_save").into(),
            None,
            "",
            "",
        ),
        Stage::Saved { error } => (
            tr_ref("controller.calibration_complete"),
            error.as_ref().map_or_else(
                || tr_ref("controller.steam_confirmed_calibration_save_request_sent").into(),
                |error| tr_args("controller.steam_confirmed_calibration_save_unconfirmed_error", &[("error", &error)]),
            ),
            None,
            tr_ref("controller.ok"),
            "",
        ),
    }
}

fn sync(a: &mut App) {
    let ui = a.ui();
    let view = ui.global::<ControllerGyroState>();
    WIZARD.with_borrow(|wizard| {
        view.set_shown(wizard.is_some());
        if let Some(w) = wizard.as_ref() {
            let (title, detail, fraction, ok, back) = presentation(&w.stage);
            view.set_title(title.into());
            view.set_device(w.name.as_str().into());
            view.set_detail(detail.into());
            view.set_progress(fraction.unwrap_or(-1.0));
            view.set_primary(ok.into());
            view.set_secondary(back.into());
        }
    });
    a.update_hint();
}
