//! Steam's own UI sounds (docs/engine/sounds.json), preloaded from ~/.local/share/Steam/steamui/sounds and
//! played on a dedicated audio thread (never blocks the UI).
//!
//! Engine rules reproduced from sp.js (GamepadAudio, sp.js:461546):
//!  * one global 50 ms throttle: after any sound plays, everything requested in the next 50 ms is dropped;
//!  * non-immediate requests are deferred ~1 ms and the LOWER enum id wins (so PagedNavigation/ChangeTabs beat the
//!    BasicNav produced by the same focus move); `Typing` is immediate;
//!  * focus moves are `BasicNav` (deck_ui_misc_10), an unhandled D-pad press is `FailedNav` (deck_ui_bumper_end_02).
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Steam's PlayNavSound enum (ids = priority; lower wins when several are requested together).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Snd {
    LaunchGame = 0,
    ToastMessage = 4,
    ToastAchievement = 5,
    ToastMisc = 6,
    VolSound = 10,
    ShowModal = 11,
    HideModal = 12,
    IntoGameDetail = 13,
    OutOfGameDetail = 14,
    PagedNavigation = 15,
    ToggleOn = 16,
    ToggleOff = 17,
    SliderUp = 18,
    SliderDown = 19,
    ChangeTabs = 20,
    DefaultOk = 21,
    OpenSideMenu = 22,
    CloseSideMenu = 23,
    BasicNav = 24,
    FailedNav = 25,
    Typing = 26,
    Screenshot = 28,
}

const ALL: [Snd; 22] = [
    Snd::LaunchGame, Snd::ToastMessage, Snd::ToastAchievement, Snd::ToastMisc, Snd::VolSound, Snd::ShowModal, Snd::HideModal,
    Snd::IntoGameDetail, Snd::OutOfGameDetail, Snd::PagedNavigation, Snd::ToggleOn, Snd::ToggleOff, Snd::SliderUp, Snd::SliderDown,
    Snd::ChangeTabs, Snd::DefaultOk, Snd::OpenSideMenu, Snd::CloseSideMenu, Snd::BasicNav, Snd::FailedNav, Snd::Typing, Snd::Screenshot,
];

fn file(s: Snd) -> &'static str {
    match s {
        Snd::LaunchGame => "deck_ui_launch_game",
        Snd::ToastMessage => "deck_ui_message_toast",
        Snd::ToastAchievement => "deck_ui_achievement_toast",
        Snd::ToastMisc => "deck_ui_toast",
        Snd::VolSound => "deck_ui_volume",
        Snd::ShowModal => "deck_ui_show_modal",
        Snd::HideModal => "deck_ui_hide_modal",
        Snd::IntoGameDetail => "deck_ui_into_game_detail",
        Snd::OutOfGameDetail => "deck_ui_out_of_game_detail",
        Snd::PagedNavigation => "deck_ui_navigation",
        Snd::ToggleOn => "deck_ui_switch_toggle_on",
        Snd::ToggleOff => "deck_ui_switch_toggle_off",
        Snd::SliderUp => "deck_ui_slider_up",
        Snd::SliderDown => "deck_ui_slider_down",
        Snd::ChangeTabs => "deck_ui_tab_transition_01",
        Snd::DefaultOk => "deck_ui_default_activation",
        Snd::OpenSideMenu => "deck_ui_side_menu_fly_in",
        Snd::CloseSideMenu => "deck_ui_side_menu_fly_out",
        Snd::BasicNav => "deck_ui_misc_10",
        Snd::FailedNav => "deck_ui_bumper_end_02",
        Snd::Typing => "deck_ui_typing",
        Snd::Screenshot => "camera1",
    }
}

const THROTTLE: Duration = Duration::from_millis(50);
const COALESCE: Duration = Duration::from_millis(1);

static TX: OnceLock<Sender<Snd>> = OnceLock::new();
static MIXER: OnceLock<rodio::mixer::Mixer> = OnceLock::new();

struct UiPlayback {
    preview: bool,
    sinks: Vec<rodio::Sink>,
}

static UI_PLAYBACK: Mutex<UiPlayback> = Mutex::new(UiPlayback { preview: false, sinks: Vec::new() });

/// Останавливает текущие звуки интерфейса до завершения предпросмотра.
pub struct PreviewAudio;

impl PreviewAudio {
    pub fn begin() -> Self {
        let mut ui = UI_PLAYBACK.lock().unwrap();
        ui.preview = true;
        ui.sinks.clear();
        Self
    }
}

impl Drop for PreviewAudio {
    fn drop(&mut self) {
        UI_PLAYBACK.lock().unwrap().preview = false;
    }
}

fn other_output_active(streams: &serde_json::Value) -> bool {
    let pid = std::process::id().to_string();
    streams.as_array().into_iter().flatten().any(|stream| {
        let properties = &stream["properties"];
        stream["corked"].as_bool() == Some(false)
            && stream["mute"].as_bool() == Some(false)
            && properties["application.process.id"].as_str() != Some(&pid)
            && properties["node.name"].as_str() != Some("alsa_playback.deckshell")
            && properties["application.process.binary"].as_str() != Some("sd_dummy")
    })
}

/// Проверяем потоки PulseAudio/PipeWire, не меняя громкость или состояние других приложений.
pub fn preview_output_busy() -> Option<bool> {
    let output = std::process::Command::new("timeout")
        .args(["1", "pactl", "--format=json", "list", "sink-inputs"])
        .output().ok()?;
    if !output.status.success() {
        return None;
    }
    let streams: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    streams.is_array().then(|| other_output_active(&streams))
}

/// Shared output mixer (anim/ plays its single audio stream through it; None = no audio device / DECKSHELL_NOSOUND).
pub fn mixer() -> Option<rodio::mixer::Mixer> {
    MIXER.get().cloned()
}

pub fn init() {
    if std::env::var("DECKSHELL_NOSOUND").is_ok() {
        return;
    }
    let (tx, rx) = channel::<Snd>();
    let _ = TX.set(tx);
    let _ = std::thread::Builder::new().name("audio".into()).spawn(move || {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = std::path::PathBuf::from(home).join(".local/share/Steam/steamui/sounds");
        let mut bank: HashMap<Snd, Arc<[u8]>> = HashMap::new();
        for s in ALL {
            if let Ok(b) = std::fs::read(dir.join(format!("{}.wav", file(s)))) {
                bank.insert(s, b.into());
            }
        }
        let Ok(stream) = rodio::OutputStreamBuilder::open_default_stream() else {
            eprintln!("[deckshell] no audio output");
            return;
        };
        let _ = MIXER.set(stream.mixer().clone());
        let mut blocked_until = Instant::now();
        while let Ok(first) = rx.recv() {
            // coalesce: for ~1 ms the lowest enum value wins (Typing is immediate)
            let mut pick = first;
            if first != Snd::Typing {
                let deadline = Instant::now() + COALESCE;
                while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(n) if n < pick => pick = n,
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            }
            if Instant::now() < blocked_until {
                continue;
            }
            blocked_until = Instant::now() + THROTTLE;
            let Some(bytes) = bank.get(&pick) else { continue };
            if let Ok(dec) = rodio::Decoder::new(Cursor::new(bytes.clone())) {
                let mut ui = UI_PLAYBACK.lock().unwrap();
                ui.sinks.retain(|sink| !sink.empty());
                if !ui.preview {
                    let sink = rodio::Sink::connect_new(stream.mixer());
                    sink.append(dec);
                    ui.sinks.push(sink);
                }
            }
        }
    });
}

pub fn play(s: Snd) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_ignores_its_own_output_and_inactive_streams() {
        let own = json!({"corked": false, "mute": false, "properties": {"node.name": "alsa_playback.deckshell"}});
        let speech_dummy = json!({"corked": false, "mute": false, "properties": {"application.process.binary": "sd_dummy"}});
        let paused = json!({"corked": true, "mute": false});
        let muted = json!({"corked": false, "mute": true});
        assert!(!other_output_active(&json!([own, speech_dummy, paused, muted])));
        assert!(other_output_active(&json!([{"corked": false, "mute": false, "properties": {"media.role": "music"}}])));
    }
}
