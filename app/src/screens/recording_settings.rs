//! Recording-only page, traced to local sp.js modules 24457/74373.
//! Preserve the pinned Russian labels and use Steam’s active English bundle when available.
use steam_assets::i18n;
use super::*;
use crate::RecordingSettingsState;

pub(super) fn steam_text(key: &str) -> String {
    let key = key.trim_start_matches('#');
    let fallback = i18n::tr(&format!("recording.steam.{key}"));
    let raw = if i18n::current().code() == "ru" {
        fallback
    } else {
        crate::assets::t_or(key, &fallback)
    };
    // Steam's React localization markup is rendered as plain text here.
    let mut out = String::new();
    let mut tag = false;
    for ch in raw.chars() { match ch { '<' => tag = true, '>' => tag = false, _ if !tag => out.push(ch), _ => {} } }
    out
}
fn loc(suffix: &str) -> String { steam_text(&format!("Settings_GameRecording_{suffix}")) }
pub(super) const SAVE_STATUS: &str = "#DeckShell_RecordingSaveStatus";

fn same_value(a: &Value, b: &Value) -> bool {
    a == b || a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a == b)
}
fn label(y: f32, title: String, desc: String) -> PRow {
    PRow { k: 1, x: 270., y, w: 470., h: 96., lab: title, desc, ..Default::default() }
}
fn select(y: f32, token: &str) -> PRow {
    PRow { k: 4, x: 760., y: y + 4., w: 268., h: 44., tok: token.into(), f: true,
        role: "combobox".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() }
}
pub(super) fn rows() -> Vec<PRow> {
    let mut rows = vec![PRow { k: 14, lab: loc("RecordingMode_Header2"), x: 255., y: 120., w: 770., h: 32., ..Default::default() }];
    for (mode, name, y, desc) in [
        (0, "Never", 160., loc("ModeDescription_Never")),
        (1, "Always", 250., i18n::tr("recording.background_mode_description")),
        (2, "Manual", 374., loc("ModeDescription_Manual").replace("%1$s", &loc("ModeDescription_Manual_Fallback"))),
    ] {
        rows.push(PRow { k: 4, x: 270., y, w: 758., h: if mode == 1 { 116. } else { 82. },
            tok: format!("#DeckShell_RecordingMode{mode}"), lab: loc(&format!("RecordingMode_{name}")), desc,
            f: true, role: "radio".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() });
    }
    rows.push(label(480., i18n::tr_ref("recording_settings.on_the_recording_settings").into(), i18n::tr_ref("recording_settings.select_the_recording_mode_and_adjust_the").into()));
    rows.push(label(584., loc("Gamepad_ToggleRecordingHotkey"), i18n::tr_ref("recording_settings.steam_a_start_or_stop_recording_modification").into()));
    for (y, title, desc, token) in [
        (690., loc("InstantClipDropdown_Label"), String::new(), "#DeckShell_InstantClipDuration"),
        (780., loc("Length_Modal_Title"), loc("BackgroundInstructions"), "#DeckShell_RecordingDuration"),
        (904., loc("RecordingQuality"), String::new(), "#DeckShell_RecordingBitrate"),
        (994., loc("MaxFrameRate"), String::new(), "#DeckShell_RecordingFps"),
        (1084., loc("VideoMaxHeight"), loc("VideoMaxHeight_Description"), "#DeckShell_RecordingHeight"),
    ] { rows.push(label(y, title, desc)); rows.push(select(y, token)); }
    rows.push(PRow { k: 14, lab: loc("AudioRecording"), x: 255., y: 1214., w: 770., h: 32., ..Default::default() });
    for (y, suffix, token) in [(1264., "Mic", "#DeckShell_RecordingMic"), (1378., "MicMono", "#DeckShell_RecordingMicMono"), (1510., "AGC", "#DeckShell_RecordingMicAGC")] {
        let mut row = label(y, loc(&format!("AudioRecording_{suffix}")), loc(&format!("AudioRecording_{suffix}_Description")));
        row.w = 675.; row.tok = token.into(); rows.push(row);
        rows.push(PRow { k: 2, tok: token.into(), x: 984., y: y + 10., w: 38., h: 22., f: true, role: "switch".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() });
    }
    let mic = crate::recording_mic::config();
    rows.push(label(1648., i18n::tr_ref("recording_settings.microphone_mode").into(), i18n::tr_ref("recording_settings.on_a_button_deckshell_controls_the_system").into()));
    rows.last_mut().unwrap().h = 156.;
    let mut mode = select(1648., "recording-mic-mode");
    mode.lab = crate::recording_mic::mode_label(&mic.mode).into(); rows.push(mode);
    rows.push(label(1818., i18n::tr_ref("recording_settings.recording_microphone_button").into(), i18n::tr_ref("recording_settings.select_a_button_including_l4_l5_r4").into()));
    rows.last_mut().unwrap().h = 156.;
    let mut button = select(1818., "recording-mic-button");
    button.lab = crate::recording_mic::button_label(&mic.button); rows.push(button);
    rows.push(PRow { k: 1, x: 270., y: 1988., w: 758., h: 110., tok: SAVE_STATUS.into(), lab: i18n::tr_ref("recording_settings.saving_settings").into(), desc: i18n::tr_args("recording_settings.recording_settings_are_saved_in_steam_the", &[("status_text", &(crate::recording_mic::status_text()).to_string())]), ..Default::default() });
    rows
}

/// Steam hides mode-dependent groups and microphone sub-options. Reflow removed
/// groups together with their labels, so there are no holes or orphan controls.
pub(super) fn rows_for(snapshot: &Value) -> Vec<PRow> {
    if snapshot["recording_loading"] == true {
        return vec![label(160., i18n::tr_ref("recording_settings.download_the_recording_settings").into(), String::new())];
    }
    let mut rows = rows();
    if ["gamerecording_background_mode", "gamerecording_ic_seconds", "gamerecording_background_max_keep",
        "gamerecording_video_bitrate", "gamerecording_max_fps", "gamerecording_video_maxheight",
        "gamerecording_background_a_m", "gamerecording_force_mic_mono", "gamerecording_automatic_gain_control"]
        .iter().any(|key| snapshot["c"][*key].is_null()) {
        if let Some(row) = rows.last_mut() {
            row.desc = i18n::tr_ref("recording_settings.i_couldn_t_read_some_of_the").into();
            row.lab = i18n::tr_ref("recording_settings.repeat_reading_settings").into();
            row.tok = "recording-retry".into(); row.role = "notice".into(); row.f = true;
        }
    }
    let mode = snapshot["c"]["gamerecording_background_mode"].as_i64();
    let mic = snapshot["c"]["gamerecording_background_a_m"].as_bool();
    let mut removed = Vec::new();
    if mode != Some(0) && crate::recording_mic::config().mode == "always" { removed.push((1818., 1988.)); }
    if mode == Some(0) { removed.push((584., 1988.)); }
    else {
        if mode == Some(2) { removed.push((690., 904.)); }
        if mic == Some(false) { removed.push((1378., 1648.)); }
    }
    rows.retain(|r| !removed.iter().any(|(start,end)| r.y >= *start && r.y < *end));
    for row in &mut rows {
        row.y -= removed.iter().filter(|(_,end)| row.y >= *end).map(|(start,end)| end-start).sum::<f32>();
    }
    rows
}

pub(super) fn available_on(row: &PRow, snapshot: &Value, on: i32) -> i32 {
    let c = &snapshot["c"];
    let active = matches!(c["gamerecording_background_mode"].as_i64(), Some(1 | 2));
    if super::recording_mode(&row.tok).is_some() { return if c["gamerecording_background_mode"].as_i64().is_some() { on } else { -1 }; }
    if row.k == 2 { return if active { on } else { -1 }; }
    if row.tok.starts_with("recording-mic-") { return on; }
    if row.role == "combobox" {
        if !active { return -1; }
        if matches!(row.tok.as_str(), "#DeckShell_InstantClipDuration" | "#DeckShell_RecordingDuration") && c["gamerecording_background_mode"].as_i64() != Some(1) { return -1; }
        if options(&row.tok, snapshot).is_none() { return -1; }
    }
    on
}
fn options(token: &str, snapshot: &Value) -> Option<(&'static str, Vec<(String, Value)>)> {
    let c = &snapshot["c"];
    let ints = |values: &[i64], suffix: &str| values.iter().map(|v| (format!("{v}{suffix}"), Value::from(*v))).collect::<Vec<_>>();
    let (key, mut choices) = match token {
        "#DeckShell_InstantClipDuration" => { c["gamerecording_ic_seconds"].as_f64()?; ("gamerecording_ic_seconds", ints(&[10,20,30,45,60,120,300], i18n::tr_ref("recording_settings.sec"))) },
        "#DeckShell_RecordingDuration" => { c["gamerecording_background_max_keep"].as_str()?; ("gamerecording_background_max_keep", [5,15,30,60,120,240,480].iter().map(|v| (i18n::tr_args("recording_settings.min", &[("value", &(v).to_string())]), Value::String(format!("{v}min")))).collect()) },
        "#DeckShell_RecordingBitrate" => { c["gamerecording_video_bitrate"].as_str()?; ("gamerecording_video_bitrate", [ ("Ultra", "preset_ultra"), ("High", "preset_default"), ("Medium", "preset_medium"), ("Low", "preset_low") ].into_iter().map(|(label, value)| (loc(&format!("BitRate_{label}_Short")), Value::from(value))).collect()) },
        "#DeckShell_RecordingFps" | "#DeckShell_RecordingHeight" => {
            let (key, values) = super::recording_choices(token)?; c[key].as_i64()?;
            let mut choices = ints(values, if key.ends_with("maxheight") { i18n::tr_ref("recording_settings.pix") } else { "" });
            if key.ends_with("maxheight") { choices[0].0 = loc("VideoMaxHeight_Auto"); }
            if !choices.iter().any(|(_, v)| v == &c[key]) { choices.push((c[key].to_string(), c[key].clone())); }
            (key, choices)
        },
        _ => return None,
    };
    if !choices.iter().any(|(_,value)| same_value(value, &c[key])) {
        choices.push((i18n::tr_ref("recording_settings.current_value_other").into(), c[key].clone()));
    }
    Some((key, choices))
}
pub(super) fn render_menu(ui: &AppWindow, state: &State) {
    let start = state.rows.iter().position(|r| r.tok.starts_with("recording-option:"));
    let st = ui.global::<RecordingSettingsState>();
    let power = state.page_id() == super::power_page::PAGE;
    st.set_open(state.recording_menu.is_some());
    st.set_power(power);
    st.set_anchor_y(state.power_menu_y.into());
    st.set_start(start.unwrap_or(0) as i32);
    st.set_focus(state.cf.unwrap_or(0) as i32 - start.unwrap_or(0) as i32);
    let (w, h) = if power { (420., 72.) } else { (520., 48.) };
    st.set_options(ModelRc::new(VecModel::from(start.map(|start| state.rows[start..].iter().enumerate().map(|(i,r)| { let mut row = to_grow(r, 0); row.y = (i as f32 * h).into(); row.x = 0.; row.w = w; row.h = h; row }).collect::<Vec<_>>()).unwrap_or_default())));
}
impl App {
    pub(super) fn close_recording_menu(&mut self) {
        let token = self.se.recording_menu.take().map(|m| m.0);
        self.se.rows.retain(|r| !r.tok.starts_with("recording-option:"));
        self.se.cf = token.and_then(|t| self.se.rows.iter().position(|r| r.tok == t));
        self.push_settings();
    }
    pub(super) fn recording_dropdown(&mut self, row: &PRow) -> bool {
        if let Some(index) = row.tok.strip_prefix("recording-option:").and_then(|s| s.parse::<usize>().ok()) {
            if let Some((token, values)) = self.se.recording_menu.clone().filter(|(t,_)| t.starts_with("recording-mic-")) {
                let value = values.get(index).and_then(|v| v.1.as_str()).map(str::to_owned);
                self.close_recording_menu();
                if let Some(value) = value {
                    if value == "capture" { crate::recording_mic::capture(); self.toast(i18n::tr_ref("recording_settings.press_the_controller_button_for_15_seconds")); }
                    else {
                        let mut c = crate::recording_mic::config();
                        if token == "recording-mic-mode" { c.mode = value; } else { c.button = value; }
                        crate::recording_mic::cancel_capture();
                        if crate::recording_mic::save(c).is_err() { self.toast(i18n::tr_ref("recording_settings.failed_to_save_the_microphone_setting")); }
                        self.push_settings();
                    }
                }
                return true;
            }
            let selected = self.se.recording_menu.as_ref().and_then(|(token, values)| options(token, &self.se.snapshot).map(|(key,_)| (token.clone(), key, values.get(index).map(|v| v.1.clone()))));
            self.close_recording_menu();
            if let Some((_, key, Some(value))) = selected {
                if !same_value(&self.se.snapshot["c"][key], &value) {
                    if key == "gamerecording_background_max_keep" {
                        // Shortening retention can remove old footage. Keep the existing explicit confirmation.
                        let old = self.se.snapshot["c"][key].as_str().and_then(super::recording_duration);
                        let new = value.as_str().and_then(super::recording_duration);
                        if new.zip(old).is_some_and(|(n,o)| n < o) && self.se.pending.as_deref() != Some(&format!("retention:{value}")) {
                            self.se.pending = Some(format!("retention:{value}"));
                            self.toast(i18n::tr_ref("recording_settings.old_background_records_can_be_deleted_select"));
                            return true;
                        }
                    }
                    self.se.pending = None;
                    self.recording_setting(key, value);
                }
            }
            return true;
        }
        if row.tok == "recording-mic-mode" || row.tok == "recording-mic-button" {
            self.header_clear();
            let c = crate::recording_mic::config();
            let values: Vec<(String,Value)> = if row.tok == "recording-mic-mode" {
                ["always","hold","toggle"].iter().map(|m| (crate::recording_mic::mode_label(m).into(), Value::from(*m))).collect()
            } else {
                let mut values = crate::input::DECK_BTNS.iter().map(|(b,_,_)| (crate::recording_mic::button_label(&format!("Deck:{b:?}")), Value::from(format!("Deck:{b:?}")))).collect::<Vec<_>>();
                if !values.iter().any(|(_,v)| v == &Value::from(c.button.clone())) { values.push((crate::recording_mic::button_label(&c.button), Value::from(c.button.clone()))); }
                values.push((i18n::tr_ref("recording_settings.press_the_controller_button").into(), Value::from("capture"))); values
            };
            let current = if row.tok == "recording-mic-mode" { &c.mode } else { &c.button };
            let selected = values.iter().position(|(_,v)| v.as_str() == Some(current)).unwrap_or(0);
            let start = self.se.rows.len();
            for (i,(label,_)) in values.iter().enumerate() { self.se.rows.push(PRow { k:26,lab:label.clone(),tok:format!("recording-option:{i}"),f:true,pad:"A,touch,mouse".into(),..Default::default() }); }
            self.se.recording_menu = Some((row.tok.clone(),values)); self.se.cf = Some(start+selected); self.push_settings(); return true;
        }
        if row.tok == "recording-retry" { self.sync_settings("gamerecording"); return true; }
        if row.role == "notice" { self.toast(&row.desc); return true; }
        if row.role != "combobox" { return false; }
        if available_on(row, &self.se.snapshot, 0) < 0 {
            let reason = i18n::tr_ref("recording_settings.unable_to_read_the_setting_or_not");
            self.toast(reason); return true;
        }
        let Some((key, values)) = options(&row.tok, &self.se.snapshot) else { return true; };
        self.header_clear();
        let start = self.se.rows.len();
        let selected = values.iter().position(|(_, v)| same_value(v, &self.se.snapshot["c"][key])).unwrap_or(0);
        for (i,(label,_)) in values.iter().enumerate() { self.se.rows.push(PRow { k: 26, lab: label.clone(), tok: format!("recording-option:{i}"), f: true, pad: "A,touch,mouse".into(), ..Default::default() }); }
        self.se.recording_menu = Some((row.tok.clone(), values)); self.se.cf = Some(start + selected); self.push_settings(); true
    }
}

#[cfg(test)]
#[path = "recording_settings_tests.rs"]
mod tests;
