use crate::screens::expected_text;
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
    assert!(loading[0].lab.contains(expected_text("Загрузка", "Loading")));
    assert!(!loading[0].desc.contains(expected_text("недоступ", "unavailable")));
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
    st.set_is_recording(true); st.set_page_title(steam_text("Settings_Page_GameRecording").into());
    st.set_rail(ModelRc::new(VecModel::from(vec![ListEntry { label: "Запись игр".into(), ..Default::default() }])));
    st.set_zone(1);
    let ds = std::sync::Arc::new(crate::data::DataSource::new());
    let loader = crate::loader::Loader::start(ds.clone(), 0);
    let mut a = App::new(ui.as_weak(), ds, std::sync::Arc::new(steam_data::Library::default()), loader);
    a.se.pages = vec![Page { id: "gamerecording".into(), label: steam_text("Settings_Page_GameRecording") }];
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
