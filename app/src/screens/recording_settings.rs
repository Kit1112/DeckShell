//! Recording-only page, traced to local sp.js modules 24457/74373.
//! Russian strings are pinned from Steam's Russian UI bundle; no English fallback.
use super::*;
use crate::RecordingSettingsState;

pub(super) fn ru(key: &str) -> String {
    static TABLE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let text = TABLE.get_or_init(|| serde_json::from_str(include_str!("recording_ru.json")).unwrap());
    let raw = text[key.trim_start_matches('#')].as_str().unwrap_or("Недоступно");
    // Steam's React localization markup is rendered as plain text here.
    let mut out = String::new();
    let mut tag = false;
    for ch in raw.chars() { match ch { '<' => tag = true, '>' => tag = false, _ if !tag => out.push(ch), _ => {} } }
    out
}
fn loc(suffix: &str) -> String { ru(&format!("Settings_GameRecording_{suffix}")) }
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
        (1, "Always", 250., loc("ModeDescription_Always").split("Последние").next().unwrap_or("").into()),
        (2, "Manual", 374., loc("ModeDescription_Manual").replace("%1$s", &loc("ModeDescription_Manual_Fallback"))),
    ] {
        rows.push(PRow { k: 4, x: 270., y, w: 758., h: if mode == 1 { 116. } else { 82. },
            tok: format!("#DeckShell_RecordingMode{mode}"), lab: loc(&format!("RecordingMode_{name}")), desc,
            f: true, role: "radio".into(), pad: "A,FOCUS,touch,mouse".into(), ..Default::default() });
    }
    rows.push(label(480., "О настройках записи".into(), "Выберите режим записи и настройте качество видео и звук.".into()));
    rows.push(label(584., loc("Gamepad_ToggleRecordingHotkey"), "STEAM + A — начать или остановить запись. Изменение сочетания здесь недоступно.".into()));
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
    rows.push(label(1648., "Режим микрофона".into(), "По кнопке DeckShell управляет системным микрофоном через PipeWire. Это также влияет на голосовой чат и другие приложения. Кнопка продолжает работать в игре по её раскладке Steam Input.".into()));
    rows.last_mut().unwrap().h = 156.;
    let mut mode = select(1648., "recording-mic-mode");
    mode.lab = crate::recording_mic::mode_label(&mic.mode).into(); rows.push(mode);
    rows.push(label(1818., "Кнопка микрофона записи".into(), "Выберите кнопку, включая L4/L5/R4/R5, или нажмите кнопку подключённого контроллера. После подключения сначала отпустите кнопку.".into()));
    rows.last_mut().unwrap().h = 156.;
    let mut button = select(1818., "recording-mic-button");
    button.lab = crate::recording_mic::button_label(&mic.button); rows.push(button);
    rows.push(PRow { k: 1, x: 270., y: 1988., w: 758., h: 110., lab: "Сохранение настроек".into(), desc: format!("Настройки записи сохраняются в Steam, режим микрофона и кнопка — в DeckShell. {}", crate::recording_mic::status_text()), ..Default::default() });
    rows
}

/// Steam hides mode-dependent groups and microphone sub-options. Reflow removed
/// groups together with their labels, so there are no holes or orphan controls.
pub(super) fn rows_for(snapshot: &Value) -> Vec<PRow> {
    if snapshot["recording_loading"] == true {
        return vec![label(160., "Загрузка настроек записи…".into(), String::new())];
    }
    let mut rows = rows();
    if ["gamerecording_background_mode", "gamerecording_ic_seconds", "gamerecording_background_max_keep",
        "gamerecording_video_bitrate", "gamerecording_max_fps", "gamerecording_video_maxheight",
        "gamerecording_background_a_m", "gamerecording_force_mic_mono", "gamerecording_automatic_gain_control"]
        .iter().any(|key| snapshot["c"][*key].is_null()) {
        if let Some(row) = rows.last_mut() {
            row.desc = "Не удалось прочитать часть настроек Steam. Нажмите, чтобы повторить чтение.".into();
            row.lab = "Повторить чтение настроек".into();
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
        "#DeckShell_InstantClipDuration" => { c["gamerecording_ic_seconds"].as_f64()?; ("gamerecording_ic_seconds", ints(&[10,20,30,45,60,120,300], " сек.")) },
        "#DeckShell_RecordingDuration" => { c["gamerecording_background_max_keep"].as_str()?; ("gamerecording_background_max_keep", [5,15,30,60,120,240,480].iter().map(|v| (format!("{v} мин."), Value::String(format!("{v}min")))).collect()) },
        "#DeckShell_RecordingBitrate" => { c["gamerecording_video_bitrate"].as_str()?; ("gamerecording_video_bitrate", [ ("Ultra", "preset_ultra"), ("High", "preset_default"), ("Medium", "preset_medium"), ("Low", "preset_low") ].into_iter().map(|(label, value)| (loc(&format!("BitRate_{label}_Short")), Value::from(value))).collect()) },
        "#DeckShell_RecordingFps" | "#DeckShell_RecordingHeight" => {
            let (key, values) = super::recording_choices(token)?; c[key].as_i64()?;
            let mut choices = ints(values, if key.ends_with("maxheight") { " пикс." } else { "" });
            if key.ends_with("maxheight") { choices[0].0 = loc("VideoMaxHeight_Auto"); }
            if !choices.iter().any(|(_, v)| v == &c[key]) { choices.push((c[key].to_string(), c[key].clone())); }
            (key, choices)
        },
        _ => return None,
    };
    if !choices.iter().any(|(_,value)| same_value(value, &c[key])) {
        choices.push(("Текущее значение (другое)".into(), c[key].clone()));
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
                    if value == "capture" { crate::recording_mic::capture(); self.toast("Нажмите кнопку контроллера в течение 15 секунд"); }
                    else {
                        let mut c = crate::recording_mic::config();
                        if token == "recording-mic-mode" { c.mode = value; } else { c.button = value; }
                        crate::recording_mic::cancel_capture();
                        if crate::recording_mic::save(c).is_err() { self.toast("Не удалось сохранить настройку микрофона"); }
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
                            self.toast("Старые фоновые записи могут быть удалены. Выберите длительность ещё раз для подтверждения");
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
                values.push(("Нажать кнопку контроллера…".into(), Value::from("capture"))); values
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
            let reason = "Не удалось прочитать настройку или она не используется в текущем режиме записи";
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
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn dependent_groups_and_options_follow_native_settings() {
        let mut snapshot = json!({"c":{"gamerecording_background_mode":2,"gamerecording_background_a_m":false,"gamerecording_max_fps":48}});
        let rows = rows_for(&snapshot);
        assert!(!rows.iter().any(|r| r.tok == "#DeckShell_InstantClipDuration"));
        assert!(!rows.iter().any(|r| r.tok == "#DeckShell_RecordingMicMono"));
        assert!(rows.iter().any(|r| r.tok == "#DeckShell_RecordingMic"));
        let (_, choices) = options("#DeckShell_RecordingFps", &snapshot).unwrap();
        assert!(choices.iter().any(|(_,v)| v == &json!(48)));
        let custom = json!({"c":{"gamerecording_video_bitrate":"custom_12000"}});
        let (_, choices) = options("#DeckShell_RecordingBitrate", &custom).unwrap();
        assert_eq!(choices[0].1, json!("preset_ultra"));
        assert_eq!(choices.last().unwrap().1, json!("custom_12000"));
        snapshot["c"]["gamerecording_background_mode"] = json!(0);
        assert!(!rows_for(&snapshot).iter().any(|r| r.role == "combobox" || r.k == 2));
        snapshot["c"]["gamerecording_background_mode"] = json!(1);
        snapshot["recording_per_process_audio"] = json!(false);
        assert!(!rows_for(&snapshot).iter().any(|r| r.tok == "#DeckShell_RecordingAudio"));
    }

    #[test]
    fn hidden_source_has_no_layout_focus_or_missing_value_dependency() {
        let snapshot = json!({"c":{"gamerecording_background_mode":1,
            "gamerecording_background_a_m":true, "gamerecording_force_mic_mono":false,
            "gamerecording_automatic_gain_control":true, "gamerecording_background_max_keep":"120min",
            "gamerecording_ic_seconds":30, "gamerecording_video_bitrate":"preset_default",
            "gamerecording_max_fps":60, "gamerecording_video_maxheight":0}});
        let rows = rows_for(&snapshot);
        assert!(!rows.iter().any(|r| r.tok == "recording-retry"));
        let agc = rows.iter().find(|r| r.k == 2 && r.tok == "#DeckShell_RecordingMicAGC").unwrap();
        let mode = rows.iter().find(|r| r.tok == "recording-mic-mode").unwrap();
        assert_eq!(mode.y - agc.y, 132.); // next group follows AGC without the deleted 172px source block
        assert!(options("#DeckShell_RecordingAudio", &snapshot).is_none());
        for recording_mode in [0, 1, 2] {
            for mic in [false, true] {
                let mut snapshot = snapshot.clone();
                snapshot["c"]["gamerecording_background_mode"] = json!(recording_mode);
                snapshot["c"]["gamerecording_background_a_m"] = json!(mic);
                let rows = rows_for(&snapshot);
                assert!(!rows.iter().any(|r| r.tok.contains("RecordingAudio") || r.role == "checkbox"));
                if recording_mode != 0 {
                    let mode = rows.iter().find(|r| r.tok == "recording-mic-mode").unwrap();
                    let expected = 1652. - if recording_mode == 2 { 214. } else { 0. } - if mic { 0. } else { 270. };
                    assert_eq!(mode.y, expected);
                }
            }
        }
    }

    #[test]
    fn clip_float_selects_existing_integer_option() {
        let snapshot = json!({"c":{"gamerecording_ic_seconds":60.0}});
        let (_, choices) = options("#DeckShell_InstantClipDuration", &snapshot).unwrap();
        assert_eq!(choices.len(), 7);
        assert_eq!(choices.iter().position(|(_, v)| same_value(v, &snapshot["c"]["gamerecording_ic_seconds"])), Some(4));
        assert!(rows_for(&Value::Null).iter().any(|r| r.tok == "recording-retry"));
        let loading = rows_for(&json!({"recording_loading":true}));
        assert_eq!(loading.len(), 1);
        assert!(loading[0].lab.contains("Загрузка"));
        assert!(!loading[0].desc.contains("недоступ"));
    }

    #[test]
    fn settings_long_text_stays_inside_rows_and_above_footer() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        ))).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.global::<crate::Shell>().set_page(4);
        let st = ui.global::<SettingsState>();
        let dir = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = &dir { std::fs::create_dir_all(dir).unwrap(); }
        let save = |name: &str, image: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            if let Some(dir) = &dir { image::save_buffer(dir.join(format!("settings-{name}.png")), image.as_bytes(), image.width(), image.height(), image::ColorType::Rgba8).unwrap(); }
        };
        for height in [720u32, 800, 900] {
            ui.window().set_size(slint::PhysicalSize::new(1280, height));
            for recording in [false, true] {
                st.set_is_recording(recording);
                st.set_page_title(if recording { "Запись игр" } else { "Настройки" }.into());
                st.set_content(ModelRc::default()); st.set_content_h(2400.); st.set_vy(0.);
                let _ = ui.window().take_snapshot().unwrap();
                i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
                slint::platform::update_timers_and_animations();
                let baseline = ui.window().take_snapshot().unwrap();
                let row_y = height as f32 - 150.;
                st.set_content(ModelRc::new(VecModel::from(vec![GPageRow {
                    kind: 1, x: 400., y: row_y, w: 600., h: 64.,
                    label: "Длинная подпись настройки с переносом текста ".repeat(8).into(),
                    desc: "Длинное описание настройки должно оставаться внутри строки и области содержимого. ".repeat(100).into(),
                    ..Default::default()
                }])));
                let rendered = ui.window().take_snapshot().unwrap();
                // No glyph may escape the row into the next row or the translucent footer.
                let first_clear_y = (row_y + 66.) as usize;
                for y in first_clear_y..height as usize {
                    let range = (y * 1280 + 380) * 4..(y + 1) * 1280 * 4;
                    assert_eq!(&rendered.as_bytes()[range.clone()], &baseline.as_bytes()[range], "text escaped row: recording={recording}, height={height}, y={y}");
                }
                assert!(rendered.as_bytes() != baseline.as_bytes(), "fixture must actually render");
                save(&format!("long-{recording}-{height}"), &rendered);
                // A row spanning the footer must be clipped by the shared content viewport.
                use slint::Model;
                let mut row = st.get_content().row_data(0).unwrap(); row.h = 400.;
                st.set_content(ModelRc::new(VecModel::from(vec![row])));
                let rendered = ui.window().take_snapshot().unwrap();
                let footer_y = (height as f32 - ui.global::<crate::FooterState>().get_bar_height()).ceil() as usize;
                for y in footer_y..height as usize {
                    let range = (y * 1280 + 380) * 4..(y + 1) * 1280 * 4;
                    assert_eq!(&rendered.as_bytes()[range.clone()], &baseline.as_bytes()[range], "text crossed footer: recording={recording}, height={height}, y={y}");
                }
                save(&format!("footer-{recording}-{height}"), &rendered);
                assert!((st.get_viewport_h() - (height as f32 - 62.73 - 47.94)).abs() < 0.1);
            }
        }
    }

    #[test]
    fn recording_page_gallery() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        ))).unwrap();
        let ui = AppWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        ui.global::<crate::Shell>().set_page(4);
        let st = ui.global::<SettingsState>();
        st.set_is_recording(true); st.set_page_title(ru("Settings_Page_GameRecording").into());
        st.set_rail(ModelRc::new(VecModel::from(vec![ListEntry { label: "Запись игр".into(), ..Default::default() }])));
        st.set_zone(1);
        let ds = std::sync::Arc::new(crate::data::DataSource::new());
        let loader = crate::loader::Loader::start(ds.clone(), 0);
        let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
        a.se.pages = vec![Page { id: "gamerecording".into(), label: ru("Settings_Page_GameRecording") }];
        a.se.zone = 1;
        a.router.replace(crate::router::Route::Settings("gamerecording".into()));
        a.se.snapshot = json!({"c":{"gamerecording_background_mode":1,"gamerecording_background_a_m":true,
            "gamerecording_force_mic_mono":false,"gamerecording_automatic_gain_control":true,
            "gamerecording_background_max_keep":"120min","gamerecording_ic_seconds":30,
            "gamerecording_video_bitrate":"preset_default","gamerecording_max_fps":60,
            "gamerecording_video_maxheight":0,"gamerecording_background_audio":1},"recording_per_process_audio":true});
        a.push_settings();
        let dir = std::env::var_os("DECKSHELL_GALLERY_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = &dir { std::fs::create_dir_all(dir).unwrap(); }
        let save = |name: &str| {
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(2));
            slint::platform::update_timers_and_animations();
            let image = ui.window().take_snapshot().unwrap();
            assert_eq!((image.width(),image.height()),(1280,800));
            if let Some(dir) = &dir { image::save_buffer(dir.join(format!("recording-{name}.png")), image.as_bytes(), image.width(),image.height(),image::ColorType::Rgba8).unwrap(); }
        };
        use slint::Model;
        let rendered = st.get_content();
        for (i,row) in a.se.rows.iter().enumerate().filter(|(_,r)| r.role == "combobox") {
            assert!(rendered.row_data(i).unwrap().on >= 0, "known supported value must enable pointer input: {}", row.tok);
        }
        for (name, scroll) in [("modes",0.),("video",680.),("audio",1200.)] {
            a.se.vy = scroll; a.push_settings(); save(name);
        }
        let agc = a.se.rows.iter().position(|r| r.k == 2 && r.tok == "#DeckShell_RecordingMicAGC").unwrap();
        a.se.cf = Some(agc); a.handle_settings(Action::Down);
        assert_eq!(a.se.rows[a.se.cf.unwrap()].tok, "recording-mic-mode");
        // Exercise the production pointer/A entry, directional handler, B and
        // selected-value path. Merely opening/canceling must never write.
        let bitrate = a.se.rows.iter().position(|r| r.tok == "#DeckShell_RecordingBitrate").unwrap();
        a.se.cf = Some(bitrate);
        a.settings_activate(bitrate);
        assert!(a.se.recording_menu.is_some());
        let start = a.se.rows.iter().position(|r| r.tok.starts_with("recording-option:")).unwrap();
        assert_eq!(a.se.cf, Some(start + 1));
        save("menu");
        a.handle_pad(Action::Up); assert_eq!(a.se.cf, Some(start));
        a.handle_pad(Action::Up); assert_eq!(a.se.cf, Some(start));
        assert!(crate::screens::chrome::hf() < 0, "Граница списка не должна уводить фокус в шапку");
        assert!(a.settings_back());
        assert!(a.se.recording_menu.is_none()); assert_eq!(a.se.cf, Some(bitrate));
        assert_eq!(a.se.snapshot["c"]["gamerecording_video_bitrate"], json!("preset_default"));
        a.settings_activate(bitrate); a.handle_settings(Action::A);
        assert!(a.se.recording_write.is_none()); assert!(a.se.recording_menu.is_none());
        // Microphone bindings are identified from the full row, independently of
        // the localized display text and other setting groups.
        let mic = a.se.rows.iter().position(|r| r.k == 2 && r.tok == "#DeckShell_RecordingMic").unwrap();
        assert_eq!(a.toggle_binding(mic).unwrap().0, "c.gamerecording_background_a_m");
        assert_eq!(a.se.on[mic], 1);
        a.se.snapshot["c"]["gamerecording_background_a_m"] = json!(false); a.push_settings();
        assert!(!a.se.rows.iter().any(|r| r.tok == "#DeckShell_RecordingMicMono"));
        // Shortening retention offers an explicit warning before any setter.
        let duration = a.se.rows.iter().position(|r| r.tok == "#DeckShell_RecordingDuration").unwrap();
        a.settings_activate(duration);
        let option = a.se.rows.iter().position(|r| r.tok == "recording-option:0").unwrap();
        a.settings_activate(option);
        assert!(a.se.pending.is_some()); assert!(a.se.recording_write.is_none());
        assert_eq!(a.se.snapshot["c"]["gamerecording_background_max_keep"], json!("120min"));
        a.settings_activate(duration); a.handle_settings(Action::Up);
        assert!(a.se.pending.is_some(), "menu movement must not erase retention confirmation");
        a.settings_back(); assert!(a.se.pending.is_none());
        // No source/process rows or orphan focus stops for any capability.
        a.se.snapshot["recording_per_process_audio"] = json!(false);
        a.push_settings();
        let mic = a.se.rows.iter().position(|r| r.k == 2 && r.tok == "#DeckShell_RecordingMic").unwrap();
        assert_eq!(a.se.on[mic], 0);
        a.se.cf = Some(mic);
        a.handle_settings(Action::Down);
        assert_eq!(a.se.rows[a.se.cf.unwrap()].tok, "recording-mic-mode");
        save("mic-mode");
        assert!(!a.se.rows.iter().any(|r| r.tok == "recording-mic-button"));
        let mode_row = a.se.cf.unwrap(); a.settings_activate(mode_row);
        let option = a.se.rows.iter().position(|r| r.tok == "recording-option:1").unwrap();
        a.settings_activate(option);
        assert_eq!(crate::recording_mic::config().mode, "hold");
        a.se.cf = a.se.rows.iter().position(|r| r.tok == "recording-mic-mode");
        a.handle_settings(Action::Down);
        assert_eq!(a.se.rows[a.se.cf.unwrap()].tok, "recording-mic-button");
        save("mic-button");
        let button = a.se.cf.unwrap(); a.settings_activate(button);
        assert!(a.se.recording_write.is_none()); assert!(a.se.recording_menu.is_some());
        a.settings_back();
        let button = a.se.rows.iter().position(|r| r.tok == "recording-mic-button").unwrap();
        a.settings_activate(button);
        let r5 = a.se.recording_menu.as_ref().unwrap().1.iter().position(|(_,v)| v == &json!("Deck:R5")).unwrap();
        let option = a.se.rows.iter().position(|r| r.tok == format!("recording-option:{r5}")).unwrap();
        a.settings_activate(option);
        assert_eq!(crate::recording_mic::config().button, "Deck:R5");
        for (index, mode, visible) in [(2,"toggle",true),(0,"always",false),(1,"hold",true)] {
            let row = a.se.rows.iter().position(|r| r.tok == "recording-mic-mode").unwrap();
            a.settings_activate(row);
            let option = a.se.rows.iter().position(|r| r.tok == format!("recording-option:{index}")).unwrap();
            a.settings_activate(option);
            assert_eq!(crate::recording_mic::config().mode,mode);
            assert_eq!(crate::recording_mic::config().button,"Deck:R5");
            assert_eq!(a.se.rows.iter().any(|r| r.tok == "recording-mic-button"),visible);
            assert!(a.se.recording_write.is_none());
        }
        crate::recording_mic::save(crate::recording_mic::Config::default()).unwrap();
        a.push_settings();
        assert!(!a.se.rows.iter().any(|r| r.tok == "recording-mic-button"));
        for capability in [Value::Null, json!(false), json!(true)] {
            for source in [0, 1, 2] {
                a.se.snapshot["recording_per_process_audio"] = capability.clone();
                a.se.snapshot["c"]["gamerecording_background_audio"] = json!(source);
                a.push_settings();
                assert!(!a.se.rows.iter().any(|r| r.tok.contains("RecordingAudio") || r.tok.starts_with("recording-session:") || r.tok == "recording-audio-explanation"));
            }
        }
        // Lost backend: every control gets its real unavailable presentation.
        a.se.snapshot = Value::Null; a.se.vy = 0.; a.push_settings(); save("unavailable");
        a.settings_activate(bitrate); assert!(a.se.recording_menu.is_none());
    }
}
