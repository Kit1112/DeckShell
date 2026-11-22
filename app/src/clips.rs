//! Animated still images that Steam shows in a plain <img>: the hero banner and the game logo. Both are built once into an
//! on-disk clip (media::open_clip, loader `Job::Clip`) and then played one frame at a time by the UI clock (app.rs `clip_tick`).
//! Each slot plays one clip at a time and only while its image is on screen; leaving the page drops the clip, so nothing
//! is decoded or redrawn for it off screen.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// One clip playing for one game: the frame on screen and when it was put there.
pub struct ClipPlay {
    pub id: u32,
    clip: Arc<media::Clip>,
    frame: usize,
    shown: Instant,
}

impl ClipPlay {
    pub fn new(id: u32, clip: Arc<media::Clip>, now: Instant) -> Self {
        ClipPlay { id, clip, frame: 0, shown: now }
    }

    pub fn frame_index(&self) -> usize {
        self.frame
    }

    /// True when the frame on screen has been shown for its delay. A few ms early still counts: the UI clock ticks at 16 ms.
    pub fn due(&self, now: Instant) -> bool {
        now.duration_since(self.shown).as_millis() as u32 + 3 >= self.clip.delay_ms(self.frame)
    }

    /// Moves to the next frame once the current one is due and returns its pixels. None when not due or when the frame
    /// cannot be read (the old frame stays on screen).
    pub fn step(&mut self, now: Instant) -> Option<Image> {
        if !self.due(now) {
            return None;
        }
        let next = (self.frame + 1) % self.clip.len();
        let f = self.clip.frame(next).ok()?;
        self.frame = next;
        self.shown = now;
        Some(Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&f.rgba, f.width, f.height)))
    }
}

/// State of one clip kind (hero or logo).
#[derive(Default)]
pub struct ClipSlot {
    pub play: Option<ClipPlay>,
    /// Id whose clip job is queued or running.
    pub req: Option<u32>,
    /// Per id: whether its image is animated (sniffed once from the file head). Missing = not checked yet.
    pub animated: HashMap<u32, bool>,
}

impl ClipSlot {
    /// The art source of `id` changed: forget what was known about its file.
    pub fn forget(&mut self, id: u32) {
        self.animated.remove(&id);
        if self.req == Some(id) {
            self.req = None;
        }
        if self.play.as_ref().is_some_and(|p| p.id == id) {
            self.play = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Writes a 3-frame animated GIF (100 ms per frame) into a fresh directory and returns its path.
    pub(crate) fn write_gif(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deckshell-clips-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.gif");
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = image::codecs::gif::GifEncoder::new(file);
        let frames = (0..3u8).map(|i| {
            let img = image::RgbaImage::from_pixel(8, 4, image::Rgba([i * 80, 0, 0, 255]));
            image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(100, 1))
        });
        enc.encode_frames(frames).unwrap();
        std::env::set_var("DECKSHELL_CACHE", dir.join("cache"));
        path
    }

    fn gif_clip(tag: &str) -> Arc<media::Clip> {
        Arc::new(media::open_clip(&write_gif(tag), 480, 210, 16).expect("gif clip"))
    }

    /// The game logo plays only while its game page is the route: requested on entry, shown, stopped on leaving.
    #[test]
    fn logo_clip_follows_the_game_page() {
        use crate::loader::Loader;
        use crate::router::Route;
        use slint::ComponentHandle;
        use steam_data::{Art, Game, GameKind, Library};
        let _ = slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions { renderer_name: Some("software".into()), mock_time: true, ..Default::default() },
        )));
        let ui = crate::AppWindow::new().unwrap();
        let gif = write_gif("page");
        let game = Game {
            app_id: 77, kind: GameKind::Steam, name: "logo game".into(), sort_name: "logo game".into(),
            installed: true, install_dir: None, size_on_disk: 0, playtime_minutes: 0, last_played: None,
            update_pending: false, collections: vec![], art: Art { logo: Some(gif.clone()), ..Default::default() },
        };
        let lib = Arc::new(Library { games: vec![game], ..Default::default() });
        let ds = Arc::new(crate::data::DataSource::new());
        let loader = Loader::start(ds.clone(), 0);
        let mut app = crate::app::App::new(ui.as_weak(), ds, lib, loader);

        app.router.push(Route::App(77));
        app.logo_sync();
        assert_eq!(app.logo_clips.req, Some(77), "entering the page requests the animated logo");
        let clip = Arc::new(media::open_clip(&gif, 480, 210, 16).expect("gif clip"));
        app.on_clip(77, crate::loader::Kind::Logo, Some(clip));
        assert!(app.logo_clips.play.is_some(), "a clip that arrives while the page is shown plays");

        app.router.back();
        app.logo_sync();
        assert!(app.logo_clips.play.is_none(), "leaving the page stops the logo clip");
        assert!(app.logo_clips.animated.get(&77) != Some(&false), "a real animation is not marked as still");
    }

    #[test]
    fn logo_clip_advances_on_its_delay_and_loops() {
        let clip = gif_clip("advance");
        assert!(clip.len() > 1);
        let t0 = Instant::now();
        let mut play = ClipPlay::new(7, clip, t0);
        assert!(play.step(t0 + Duration::from_millis(40)).is_none(), "frame must stay for its delay");
        assert_eq!(play.frame_index(), 0);
        assert!(play.step(t0 + Duration::from_millis(100)).is_some());
        assert_eq!(play.frame_index(), 1);
        let mut t = t0 + Duration::from_millis(100);
        for _ in 0..10 {
            t += Duration::from_millis(100);
            play.step(t);
        }
        assert!(play.frame_index() < 3, "playback loops over the clip's frames");
    }

    #[test]
    fn forget_drops_play_and_request_of_the_changed_id_only() {
        let clip = gif_clip("forget");
        let mut slot = ClipSlot::default();
        slot.play = Some(ClipPlay::new(1, clip.clone(), Instant::now()));
        slot.req = Some(2);
        slot.animated.insert(1, true);
        slot.animated.insert(2, false);
        slot.forget(1);
        assert!(slot.play.is_none());
        assert_eq!(slot.req, Some(2));
        assert!(!slot.animated.contains_key(&1));
        assert_eq!(slot.animated.get(&2), Some(&false));
        slot.forget(2);
        assert_eq!(slot.req, None);
    }
}
