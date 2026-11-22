//! Пассивная стопка уведомлений: три карточки, новые снизу, уход вправо.
use std::{cell::RefCell, collections::VecDeque, time::Duration};
use slint::{ComponentHandle, Model};
use crate::{AppWindow, RecBadgeWindow, ToastState, ToastData};

pub const HOLD_MS: u64 = 5000;
pub const ANIMATION_MS: u64 = 300;
/// Idle time after the last card before the overlay window is unmapped.
pub const UNMAP_DELAY_MS: u64 = 250;
/// Pause before mapping the overlay is retried after a failure.
pub const RETRY_MS: u64 = 50;

#[derive(Default)]
pub struct Event {
    pub title: String,
    pub body: String,
    pub kind: String,
    pub image: String,
}
impl Event {
    pub fn message(text: &str) -> Self { Self { title: "Уведомление".into(), body: text.into(), ..Default::default() } }
    pub fn capture(text: &str) -> Self {
        Self { title: text.into(), kind: if text.starts_with("Скриншот") { "screenshot" } else { "recording" }.into(), ..Default::default() }
    }
    pub fn resolve_screenshot(&mut self, root: &std::path::Path, user: Option<u32>) {
        if self.kind != "screenshot" { return; }
        if let (Some(relative), Some(user)) = (screenshot_relative(&self.image), user) {
            self.image = root.join("userdata").join(user.to_string()).join("760/remote").join(relative).to_string_lossy().into_owned();
        }
    }
    pub fn from_value(v: &serde_json::Value) -> Self {
        let s = |k| v.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        Self { title: s("title"), body: s("body"), kind: s("kind"), image: s("image") }
    }
}
struct Slot { id: u64, row: usize, leaving: bool }
struct Queue {
    pending: VecDeque<(Event, bool)>, slots: VecDeque<Slot>, serial: u64,
    model: std::rc::Rc<slint::VecModel<ToastData>>, window: Option<RecBadgeWindow>,
    ready: bool, mapped: bool, failures: u32,
    hide: std::rc::Rc<slint::Timer>, retry: std::rc::Rc<slint::Timer>,
}
impl Default for Queue {
    fn default() -> Self {
        Self { pending: VecDeque::new(), slots: VecDeque::new(), serial: 0,
            model: std::rc::Rc::new(slint::VecModel::from(vec![ToastData::default(); 3])), window: None, ready: false,
            mapped: false, failures: 0, hide: Default::default(), retry: Default::default() }
    }
}
thread_local! { static QUEUE: RefCell<Queue> = RefCell::new(Queue::default()); }

// Окно создаётся заранее (скрытым), но на экран выходит только пока есть карточка: постоянно отображённый
// прозрачный оверлей на весь экран заставлял бы gamescope компоновать каждый кадр игры (task 526).
// Прежняя версия отмечала окно на 10 попыток по 16 мс сразу после старта, при первом отказе навсегда
// писала «поверхность недоступна», и тосты не показывались совсем.
pub fn prepare(main: &AppWindow) {
    let external = crate::gamescope::rec_badge_external();
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        main.global::<ToastState>().set_items(q.model.clone().into());
        if !external { main.global::<ToastState>().set_external(false); q.ready = true; return; }
        if q.window.is_none() {
            crate::level::CREATING_POPUP.set(true);
            let win = RecBadgeWindow::new();
            crate::level::CREATING_POPUP.set(false);
            match win {
                Ok(w) => {
                    w.global::<ToastState>().set_items(q.model.clone().into());
                    w.window().set_size(main.window().size());
                    q.window = Some(w);
                }
                Err(e) => eprintln!("[deckshell] уведомления: окно не создано: {e}"),
            }
        }
        q.ready = q.window.is_some();
        // Without the overlay window the inline stack is the fallback: better under the game than nothing.
        main.global::<ToastState>().set_external(q.window.is_some());
    });
    next(main);
}
/// Maps the overlay window with its compositor role before a card is shown. `false`: not possible yet, retry.
fn ensure_mapped(main: &AppWindow) -> bool {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let Some(w) = q.window.as_ref().and_then(|w| w.as_weak().upgrade()) else { return true };
        q.hide.stop();
        if q.mapped { return true; }
        w.window().set_size(main.window().size());
        match w.show().map_err(|e| e.to_string()).and_then(|_| crate::gamescope::mark_level_popup(w.window())) {
            Ok(()) => {
                q.mapped = true;
                crate::diag::log("notifications.overlay", serde_json::json!({"state": "mapped", "failures": q.failures}));
                q.failures = 0;
                true
            }
            Err(e) => {
                let _ = w.hide();
                if q.failures % 20 == 0 { eprintln!("[deckshell] поверхность уведомлений недоступна: {e}"); }
                q.failures += 1;
                false
            }
        }
    })
}
/// Unmaps the overlay shortly after the last card has left; a new card within the delay keeps it mapped.
fn schedule_unmap() {
    QUEUE.with(|q| {
        let q = q.borrow();
        if q.window.is_none() || !q.mapped { return; }
        q.hide.start(slint::TimerMode::SingleShot, Duration::from_millis(UNMAP_DELAY_MS), || {
            QUEUE.with(|q| {
                let mut q = q.borrow_mut();
                if !q.slots.is_empty() || !q.pending.is_empty() { return; }
                if let Some(w) = &q.window { let _ = w.hide(); }
                q.mapped = false;
                crate::diag::log("notifications.overlay", serde_json::json!({"state": "unmapped"}));
            });
        });
    });
}
pub fn enqueue(main: &AppWindow, event: Event, game: bool) {
    if event.title.is_empty() { return; }
    let initialized = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if q.pending.len() < 128 { q.pending.push_back((event, game)); }
        q.ready
    });
    if !initialized { prepare(main); } else { next(main); }
}
fn update(id: u64, change: impl FnOnce(&mut ToastData)) {
    QUEUE.with(|q| {
        let q = q.borrow();
        if let Some(slot) = q.slots.iter().find(|s| s.id == id) {
            let mut data = q.model.row_data(slot.row).unwrap();
            change(&mut data);
            q.model.set_row_data(slot.row, data);
        }
    });
}
fn next(main: &AppWindow) {
    let full = QUEUE.with(|q| {
        let q = q.borrow();
        if !q.ready || q.pending.is_empty() { return None; }
        q.slots.front().filter(|_| q.slots.len() == 3).map(|s| s.id)
    });
    if let Some(id) = full { leave(main, id); return; }
    let wanted = QUEUE.with(|q| { let q = q.borrow(); q.ready && !q.pending.is_empty() && q.slots.len() < 3 });
    if wanted && !ensure_mapped(main) {
        let weak = main.as_weak();
        QUEUE.with(|q| q.borrow().retry.start(slint::TimerMode::SingleShot, Duration::from_millis(RETRY_MS), move || {
            if let Some(main) = weak.upgrade() { next(&main); }
        }));
        return;
    }
    let entry = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        if !q.ready || q.slots.len() == 3 { return None; }
        let (event, game) = q.pending.pop_front()?;
        let row = (0..3).find(|r| !q.slots.iter().any(|s| s.row == *r)).unwrap();
        q.serial += 1;
        let id = q.serial;
        q.slots.push_back(Slot { id, row, leaving: false });
        let glyph = crate::assets::icon_image(match event.kind.as_str() {
            "screenshot" => "Screenshot", "achievement" => "Achievement", "friend" => "controller", "recording" => "Video", _ => "SteamLogo",
        });
        let logo = if event.kind == "recording" { recording_logo(event.title.contains("завершена")) }
            else if event.kind.is_empty() { crate::assets::icon_image("SteamLogo") } else { Default::default() };
        q.model.set_row_data(row, ToastData {
            title: event.title.clone().into(), body: event.body.clone().into(), glyph, logo,
            friend: event.kind == "friend", screenshot: event.kind == "screenshot",
            mounted: true, entered: false, fading: false, in_game: game, rank: 0,
        });
        ranks(&q);
        Some((id, event.image))
    });
    let Some((id, image)) = entry else { return };
    // Показываем карточку сразу; чтение/сеть/декодирование обновляют только её изображение.
    let weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(16), move || {
        let Some(main) = weak.upgrade() else { return };
        update(id, |d| d.entered = true);
        let weak = main.as_weak();
        slint::Timer::single_shot(Duration::from_millis(HOLD_MS), move || {
            if let Some(main) = weak.upgrade() { leave(&main, id); }
        });
    });
    if !image.is_empty() {
        std::thread::spawn(move || {
            let pixels = load_image(&image);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(pixels) = pixels { update(id, |d| d.logo = slint::Image::from_rgba8(pixels)); }
            });
        });
    }
    next(main);
}
fn ranks(q: &Queue) {
    for (index, slot) in q.slots.iter().enumerate() {
        let mut data = q.model.row_data(slot.row).unwrap();
        data.rank = (q.slots.len() - index - 1) as i32;
        q.model.set_row_data(slot.row, data);
    }
}
fn leave(main: &AppWindow, id: u64) {
    let started = QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let Some(slot) = q.slots.iter_mut().find(|s| s.id == id) else { return false };
        if slot.leaving { return false; }
        slot.leaving = true;
        let row = slot.row;
        let mut data = q.model.row_data(row).unwrap();
        data.fading = true;
        q.model.set_row_data(row, data);
        true
    });
    if !started { return; }
    let weak = main.as_weak();
    slint::Timer::single_shot(Duration::from_millis(ANIMATION_MS), move || {
        let Some(main) = weak.upgrade() else { return };
        QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            if let Some(i) = q.slots.iter().position(|s| s.id == id) {
                let slot = q.slots.remove(i).unwrap();
                q.model.set_row_data(slot.row, ToastData::default());
                ranks(&q);
            }
        });
        next(&main);
        if QUEUE.with(|q| q.borrow().slots.is_empty()) { schedule_unmap(); }
    });
}
// Extracted unchanged geometry: Steam sp/15786 Nm; templates sp/25666 ki/nn.
// See tools/extract-assets/recording-logo.py and docs/notifications-fidelity-2026-11-15.md.
// Embedded in the binary; rasterize before handing it to the GPU renderer.
fn recording_logo(stopped: bool) -> slint::Image {
    let svg: &[u8] = if stopped {
        include_bytes!("../ui/assets/steam/recording-right.svg")
    } else {
        include_bytes!("../ui/assets/steam/recording-left.svg")
    };
    slint::Image::load_from_svg_data(svg).ok().and_then(|image| image.to_rgba8()).map(slint::Image::from_rgba8).unwrap_or_default()
}

fn load_image(source: &str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    use std::io::Read;
    if source.is_empty() { return None; }
    let bytes = if source.starts_with("https://") {
        // Bounded public Steam artwork only, no redirect to an arbitrary service.
        let host = source.strip_prefix("https://")?.split('/').next()?;
        if !matches!(host, "avatars.steamstatic.com" | "avatars.akamai.steamstatic.com" | "avatars.cloudflare.steamstatic.com" | "steamcdn-a.akamaihd.net" | "cdn.akamai.steamstatic.com" | "shared.akamai.steamstatic.com" | "shared.cloudflare.steamstatic.com") { return None; }
        let out = std::process::Command::new("curl").args(["--fail", "--silent", "--max-time", "2", "--max-filesize", "1048576", "--proto", "=https", source]).output().ok()?;
        if !out.status.success() { return None; }
        out.stdout
    } else {
        let path = source.strip_prefix("file://").unwrap_or(source);
        if !path.starts_with('/') { return None; }
        let mut bytes = Vec::new();
        let path = std::path::Path::new(path);
        let thumbnail = path.parent()?.join("thumbnails").join(path.file_name()?);
        let path = if thumbnail.is_file() { thumbnail.as_path() } else { path };
        std::fs::File::open(path).ok()?.take(1_048_577).read_to_end(&mut bytes).ok()?;
        bytes
    };
    decode_image(&bytes)
}

fn decode_image(bytes: &[u8]) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    if bytes.len() > 1_048_576 { return None; }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096); limits.max_image_height = Some(4096); limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if image.width() > 256 || image.height() > 256 { image.thumbnail(256, 256) } else { image };
    let image = image.to_rgba8();
    Some(slint::SharedPixelBuffer::clone_from_slice(image.as_raw(), image.width(), image.height()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestState(std::rc::Rc<slint::VecModel<ToastData>>);
    impl TestState {
        fn change(&self, f: impl FnOnce(&mut ToastData)) {
            let mut d = self.0.row_data(0).unwrap(); f(&mut d); self.0.set_row_data(0, d);
        }
        fn set_title(&self, v: slint::SharedString) { self.change(|d| d.title = v); }
        fn set_body(&self, v: slint::SharedString) { self.change(|d| d.body = v); }
        fn set_mounted(&self, v: bool) { self.change(|d| d.mounted = v); }
        fn set_entered(&self, v: bool) { self.change(|d| d.entered = v); }
        fn set_in_game(&self, v: bool) { self.change(|d| d.in_game = v); }
        fn set_fading(&self, v: bool) { self.change(|d| d.fading = v); }
    }

    #[test]
    fn native_screenshot_url_maps_to_the_active_users_local_file() {
        let mut e = Event::from_value(&serde_json::json!({"kind":"screenshot", "title":"Скриншот сделан", "image":"screenshots/292030/screenshots/test.jpg"}));
        e.resolve_screenshot(std::path::Path::new("/steam"), Some(123));
        assert_eq!(e.image, "/steam/userdata/123/760/remote/292030/screenshots/test.jpg");
        assert_eq!(screenshot_relative("https://steamloopback.host/screenshots/22/screenshots/test.jpg"), Some("22/screenshots/test.jpg"));
        for bad in ["screenshots/../screenshots/test.jpg", "screenshots/22/screenshots/../test.jpg", "screenshots/22/screenshots/.hidden", "https://other/screenshots/22/screenshots/test.jpg"] { assert!(screenshot_relative(bad).is_none(), "{bad}"); }
    }

    #[test]
    fn notification_images_preserve_aspect_and_reject_oversized_data() {
        let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([200, 30, 20, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let pixels = decode_image(bytes.get_ref()).unwrap();
        assert_eq!(pixels.width(), 200);
        assert_eq!(pixels.height(), 100);
        assert!(decode_image(&vec![0; 1_048_577]).is_none());
        assert!(decode_image(b"not an image").is_none());
    }

    #[test]
    fn overlay_window_is_mapped_only_while_a_card_is_shown() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        ))).unwrap();
        crate::load_fonts();
        let main = AppWindow::new().unwrap();
        main.window().set_size(slint::PhysicalSize::new(1280, 800));
        let win = RecBadgeWindow::new().unwrap();
        QUEUE.with(|q| { let mut q = q.borrow_mut(); q.window = win.as_weak().upgrade(); q.ready = true; });
        assert!(!win.window().is_visible(), "no permanent full-screen layer over the game");
        enqueue(&main, Event::message("Игра"), true);
        assert!(win.window().is_visible(), "a card needs the overlay window");
        QUEUE.with(|q| assert!(q.borrow().mapped));
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(17));
        slint::platform::update_timers_and_animations();
        // a second card while mapped keeps it; the window goes away only after the last card left
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(HOLD_MS + 100));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ANIMATION_MS + 1));
        slint::platform::update_timers_and_animations();
        QUEUE.with(|q| assert!(q.borrow().slots.is_empty()));
        assert!(win.window().is_visible(), "unmap waits a moment for a follow-up card");
        enqueue(&main, Event::message("Ещё"), true);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(17));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(UNMAP_DELAY_MS + 50));
        slint::platform::update_timers_and_animations();
        assert!(win.window().is_visible(), "the new card cancelled the pending unmap");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(HOLD_MS + 100));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ANIMATION_MS + 1));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(UNMAP_DELAY_MS + 1));
        slint::platform::update_timers_and_animations();
        assert!(!win.window().is_visible(), "idle again: the layer is unmapped");
        QUEUE.with(|q| assert!(!q.borrow().mapped));
    }

    #[test]
    fn overlay_role_script_detects_the_unmarked_window() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let out = std::process::Command::new("bash").arg(root.join("tools/gamescope/tests/notification-overlay-check.sh")).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn notification_render_geometry_motion_and_game_input_intent() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        ))).unwrap();
        crate::load_fonts();
        let before = crate::gamescope::popup_focus_snapshot();
        let ui = RecBadgeWindow::new().unwrap();
        ui.window().set_size(slint::PhysicalSize::new(1280, 800));
        let model = std::rc::Rc::new(slint::VecModel::from(vec![ToastData::default(); 3]));
        ui.global::<ToastState>().set_items(model.clone().into());
        let state = TestState(model.clone());
        state.set_title("Скриншот сделан".into());
        state.set_body("Название игры".into());
        ui.show().unwrap();
        let _ = ui.window().take_snapshot().unwrap();
        state.set_mounted(true);
        state.set_entered(true);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(150));
        slint::platform::update_timers_and_animations();
        let entering = ui.window().take_snapshot().unwrap();
        let at = ((666 * 1280 + 831) * 4) as usize;
        assert_ne!(&entering.as_bytes()[at..at+3], &[14, 20, 27], "halfway through entry the card has not reached its left edge");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(200));
        slint::platform::update_timers_and_animations();
        let home = ui.window().take_snapshot().unwrap();
        let px = |buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, x, y| {
            let n = ((y * 1280 + x) * 4) as usize;
            buf.as_bytes()[n..n+4].to_vec()
        };
        assert_eq!(&px(&home, 1278, 666)[..3], &[14, 20, 27], "right edge / square top corner");
        assert_eq!(&px(&home, 831, 666)[..3], &[14, 20, 27], "450px card width");
        state.set_body("Проверка длинного описания достижения в двух строках".into());
        let long = ui.window().take_snapshot().unwrap();
        let second_line = (703..720).flat_map(|y| (909..1250).map(move |x| (x,y))).filter(|&(x,y)| {
            let c = px(&long, x, y); c[0] > 100 && c[1] > 100 && c[2] > 100 && c[3] > 200
        }).count();
        assert!(second_line > 30, "long body must paint a second line, got {second_line} text pixels");
        if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
            image::save_buffer(std::path::Path::new(&dir).join("notification-long.png"), long.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        state.set_body("Название игры".into());
        state.set_in_game(true);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(350));
        slint::platform::update_timers_and_animations();
        let game = ui.window().take_snapshot().unwrap();
        assert_eq!(&px(&game, 831, 726)[..3], &[14, 20, 27], "game card is 60px lower");
        state.set_fading(true);
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(100));
        slint::platform::update_timers_and_animations();
        let leaving = ui.window().take_snapshot().unwrap();
        assert_ne!(&px(&leaving, 831, 726)[..3], &[14,20,27], "карточка уезжает вправо");
        assert_eq!(&px(&leaving, 1278, 726)[..3], &[14,20,27], "при уходе карточка остаётся непрозрачной");
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        slint::platform::update_timers_and_animations();
        let hidden = ui.window().take_snapshot().unwrap();
        assert_eq!(px(&hidden, 1000, 750)[3], 0, "exit finishes transparent");
        assert_eq!(crate::gamescope::popup_focus_snapshot(), before, "rendering never claims game input");
        if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            for (name, shot) in [("notification-home", home), ("notification-game", game), ("notification-faded", hidden)] {
                image::save_buffer(std::path::Path::new(&dir).join(format!("{name}.png")), shot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
        }
        let main = AppWindow::new().unwrap();
        main.window().set_size(slint::PhysicalSize::new(1280, 800));
        prepare(&main);
        for title in ["Первое", "Второе", "Третье"] { enqueue(&main, Event::message(title), false); }
        let first = QUEUE.with(|q| {
            let q = q.borrow();
            assert_eq!(q.slots.len(), 3);
            for (i, slot) in q.slots.iter().enumerate() { assert_eq!(q.model.row_data(slot.row).unwrap().rank, (2-i) as i32); }
            q.slots[0].id
        });
        enqueue(&main, Event::message("Четвёртое"), false);
        QUEUE.with(|q| {
            let q = q.borrow();
            assert_eq!(q.slots.len(), 3);
            assert!(q.slots[0].leaving);
            assert_eq!(q.pending.len(), 1);
        });
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(301));
        slint::platform::update_timers_and_animations();
        QUEUE.with(|q| {
            let q = q.borrow();
            assert_eq!(q.slots.len(), 3);
            assert!(q.pending.is_empty());
            assert_eq!(q.model.row_data(q.slots[0].row).unwrap().body, "Второе");
            assert_eq!(q.model.row_data(q.slots[2].row).unwrap().body, "Четвёртое");
        });
        update(first, |d| d.title = "Устаревшая загрузка".into());
        QUEUE.with(|q| assert_eq!(q.borrow().model.row_data(0).unwrap().body, "Четвёртое"));
        let mut logos = Vec::new();
        for stopped in [false, true] {
            state.change(|d| {
                d.fading = false; d.entered = true; d.logo = recording_logo(stopped);
                d.title = if stopped { "Запись завершена" } else { "Запись началась" }.into();
                d.body = "".into(); d.glyph = crate::assets::icon_image("Video");
            });
            assert_eq!(model.row_data(0).unwrap().logo.size().width, 40);
            assert_eq!(model.row_data(0).unwrap().logo.size().height, 40);
            let _ = ui.window().take_snapshot().unwrap();
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(350));
            slint::platform::update_timers_and_animations();
            let shot = ui.window().take_snapshot().unwrap();
            let gold = (730..775).flat_map(|y| (831..888).map(move |x| (x,y))).filter(|&(x,y)| {
                let c = px(&shot,x,y); c[0] > 140 && c[1] > 120 && c[2] < 140
            }).count();
            assert!(gold > 50, "значок записи должен содержать золотое изображение");
            if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
                image::save_buffer(std::path::Path::new(&dir).join(if stopped { "recording-stop.png" } else { "recording-start.png" }), shot.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
            }
            logos.push(shot);
        }
        assert_ne!(logos[0].as_bytes(), logos[1].as_bytes(), "начало и конец записи различаются");
        for i in 0..3 {
            let mut d = model.row_data(0).unwrap();
            d.rank = i as i32; d.title = format!("Уведомление {}",3-i).into();
            model.set_row_data(i,d);
        }
        let _ = ui.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(350));
        slint::platform::update_timers_and_animations();
        let stack = ui.window().take_snapshot().unwrap();
        for y in [516,621,726] { assert_eq!(&px(&stack,831,y)[..3], &[14,20,27], "три отдельные карточки в стопке"); }
        if let Some(dir) = std::env::var_os("DECKSHELL_GALLERY_DIR") {
            image::save_buffer(std::path::Path::new(&dir).join("notification-stack.png"), stack.as_bytes(), 1280, 800, image::ColorType::Rgba8).unwrap();
        }
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(17));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(HOLD_MS + ANIMATION_MS + 50));
        slint::platform::update_timers_and_animations();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(ANIMATION_MS + 1));
        slint::platform::update_timers_and_animations();
        QUEUE.with(|q| assert!(q.borrow().slots.is_empty()));

        // Exercise the real capture -> queue -> ToastData wiring, not just the fixture logo.
        for title in ["Запись началась", "Запись завершена"] {
            enqueue(&main, Event::capture(title), false);
        }
        QUEUE.with(|q| {
            let q = q.borrow();
            assert_eq!(q.slots.len(), 2);
            let pixels: Vec<_> = q.slots.iter().map(|slot| {
                let data = q.model.row_data(slot.row).unwrap();
                assert_eq!((data.logo.size().width, data.logo.size().height), (40, 40));
                data.logo.to_rgba8().unwrap()
            }).collect();
            assert_ne!(pixels[0].as_bytes(), pixels[1].as_bytes(), "capture events select opposite Steam directions");
            assert_eq!(pixels[0].as_bytes(), recording_logo(false).to_rgba8().unwrap().as_bytes());
            assert_eq!(pixels[1].as_bytes(), recording_logo(true).to_rgba8().unwrap().as_bytes());
        });

    }
}

// Steam 78926/fu prefixes this path with steamloopback.host for CEF. Rust reads
// the same user's local screenshot, never sends this private path to a CDN.
fn screenshot_relative(source: &str) -> Option<&str> {
    let path = source.strip_prefix("https://steamloopback.host/").unwrap_or(source).strip_prefix("screenshots/")?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 3 || parts[1] != "screenshots" || parts[0].is_empty() || !parts[0].bytes().all(|b| b.is_ascii_digit()) { return None; }
    let name = parts[2];
    if name.is_empty() || name.starts_with('.') || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') { return None; }
    Some(path)
}
