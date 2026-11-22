mod account_flow;
mod notifications;
mod exit_request;
mod actions;
mod anim;
mod clips;
mod app;
mod art_cache;
mod assets;
mod backend;
mod control;
mod dumptree;
mod feed;
mod footer;
mod gamescope;
mod gamescope_atoms;
mod mango_config;
mod framestats;
mod data;
mod display;
mod diag;
mod idle;
mod input;
mod recording_mic;
mod install_flow;
mod uninstall_flow;
mod launch_cat;
mod launch_splash;
mod level;
mod loader;
mod model;
mod nav;
mod plugins;
mod router;
mod screens;
mod setters;
mod sgdb;
mod sound;
mod steam_ui_guard;

use std::cell::RefCell;
use slint::ComponentHandle;
use std::sync::Arc;

slint::include_modules!();

thread_local! {
    static APP: RefCell<Option<app::App>> = const { RefCell::new(None) };
}

/// Run `f` against the UI-thread application state (None if unavailable / re-entrant).
pub fn with_app<R>(f: impl FnOnce(&mut app::App) -> R) -> Option<R> {
    APP.with(|c| match c.try_borrow_mut() {
        Ok(mut b) => b.as_mut().map(f),
        Err(_) => None,
    })
}

/// Register Motiva Sans (extracted from Steam's clientui.uifont by tools/gen-tokens/extract-fonts.mjs into
/// ~/.local/share/deckshell/fonts, never into the repo) with Slint's shared fontique collection.
/// Cyrillic is not covered by Motiva -> DejaVu Sans from the system via fontconfig fallback, like Steam itself.
fn load_fonts() {
    use slint::fontique_011::{fontique, shared_collection};
    let dirs: Vec<std::path::PathBuf> = std::env::var_os("DECKSHELL_FONT_DIR")
        .map(|d| vec![d.into()])
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
            vec![home.join(".local/share/deckshell/fonts"), home.join(".local/share/Steam/clientui/fonts")]
        });
    let mut coll = shared_collection();
    let mut n = 0;
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // only the Latin UI font; skip the 15 MB GoNoto fallbacks (system fonts cover fallback)
            if !(name.starts_with("MotivaSans") && name.ends_with(".ttf")) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&p) {
                let fonts = coll.register_fonts(fontique::Blob::new(Arc::new(bytes)), None);
                n += fonts.len();
            }
        }
    }
    let mut fams: Vec<String> = coll.family_names().map(|s| s.to_string()).filter(|s| s.contains("Motiva")).collect();
    fams.sort();
    eprintln!("[deckshell] fonts: {n} Motiva faces registered {fams:?}{}", if n == 0 { " (run tools/gen-tokens/extract-fonts.mjs; using fallback font)" } else { "" });
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--recording-mic-guardian") {
        if let Err(e) = recording_mic::guardian(&args[2..]) { eprintln!("Сторож микрофона: {e}"); std::process::exit(1); }
        return;
    }
    if args.get(1).is_some_and(|s| s == "--restore-steam-guide-focus") {
        std::process::exit(steam_ui_guard::restore_cli());
    }
    // gamescope (session, nested test, or a Steam shortcut inside gamescope) is driven over X11 on its Xwayland:
    // native Wayland needs `gamescope --expose-wayland` (absent in the real session) and has no appid/atoms.
    if env_flag("DECKSHELL_GAMESCOPE") {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
    }
    // todo!()/unimplemented!() in contract crates are caught; keep the log to one line.
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        eprintln!("[deckshell] caught panic at {loc}");
    }));

    // Read-only power input readiness probe. It never takes ownership or emits actions.
    if env_flag("DECKSHELL_POWER_PROBE") {
        match input::power::device::Device::open().and_then(|mut d| d.drain_and_key_up()) {
            Ok(true) => eprintln!("power input: verified, key up"),
            Ok(false) => { eprintln!("power input: key held"); std::process::exit(2); }
            Err(e) => { eprintln!("power input unavailable: {e}"); std::process::exit(1); }
        }
        return;
    }
    if let Some(secs) = std::env::var("DECKSHELL_HIDDUMP").ok().and_then(|v| v.parse().ok()) {
        input::dump_hidraw(secs);
        return;
    }
    // few arenas: image workers would otherwise each keep their own freed decode buffers resident
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    diag::log("startup", serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "session": std::env::var("DECKSHELL_SESSION_MODE").ok(), "dryrun": actions::dryrun()}));
    sound::init();
    let ds = Arc::new(data::DataSource::new());
    let lib = Arc::new(ds.load_library());
    eprintln!("[deckshell] {} games loaded{}", lib.games.len(), if ds.is_mock() { " (mock)" } else { "" });

    level::select_overlay_backend().expect("select overlay backend");
    let ui = AppWindow::new().expect("create window");
    ui.set_dry_run(actions::dryrun());
    load_fonts();
    ui.global::<Shell>().set_kenburns(!env_flag("DECKSHELL_NOKENBURNS"));
    if std::env::var("DECKSHELL_FULLSCREEN").map(|v| v != "0" && !v.is_empty()).unwrap_or(false) {
        ui.window().set_fullscreen(true);
    }

    anim::init(&ui);
    launch_splash::init(&ui);
    framestats::init(&ui);
    let loader = loader::Loader::start(ds.clone(), 3);
    let app = app::App::new(ui.as_weak(), ds.clone(), lib, loader);
    APP.with(|c| *c.borrow_mut() = Some(app));

    ui.global::<Shell>().on_animation_key(|text, pressed, repeat, modified| input::animation_key(text.as_str(), pressed, repeat, modified));
    ui.global::<Shell>().on_animation_key_reset(input::reset_animation_keyboard);
    ui.global::<Shell>().on_key(|text, ctrl, repeat| with_app(|a| a.on_key(text.as_str(), ctrl, repeat)).unwrap_or(false));
    footer::wire(&ui);
    screens::wire_all(&ui);
    sgdb::wire(&ui);

    screens::osk::prewarm();
    assets::init_icons(&ui);
    with_app(|a| a.start(ui.global::<LibraryState>().get_cols()));
    // steam-bridge on a worker thread: connects, loads everything, follows change events (steam-data stays the fallback)
    backend::spawn_battery();
    account_flow::recover_on_start();
    backend::spawn();
    with_app(|a| {
        notifications::prepare(&a.ui());
        a.schedule_native_recording_poll();
        if !crate::actions::dryrun() { a.poll_capture_events(); }
    });
    diag::spawn_steam_observer();
    if let Ok(p) = std::env::var("DECKSHELL_CONTROL") {
        control::spawn(p);
    }
    dumptree::init_oneshot();
    // DECKSHELL_INPUT=none: no gamepad/hidraw at all (test runs must not react to virtual pads).
    if std::env::var("DECKSHELL_INPUT").map(|v| v == "none").unwrap_or(false) {
        eprintln!("[deckshell] input: none (gamepad/hidraw disabled)");
    } else {
        input::spawn_gamepad();
        recording_mic::start();
    }
    gamescope::init(&ui);
    idle::init(&ui);
    input::power::runtime::start();
    if anim::player::defer_initial_show() {
        // Boot playback will show the window on the UI thread once its first
        // frame is installed; run_event_loop() keeps the unmapped app alive.
        slint::run_event_loop().expect("run");
    } else {
        ui.run().expect("run");
    }
    APP.with(|c| { c.borrow_mut().take(); });
    drop(ui);
    if let Some(code) = exit_request::take() { std::process::exit(code); }
}

#[cfg(test)]
mod fixture_gallery;
#[cfg(test)]
mod toggle_tests;
