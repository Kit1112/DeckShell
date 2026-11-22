//! Hero (banner) crop per app: zoom and pan of the `hero` image inside the game page banner frame.
//!
//! Steam has no crop/position call for the hero (the Apps custom API list in `context/reference/api/steamclient.md` has only
//! artwork set/clear and logo position), so the crop lives in DeckShell: `~/.config/deckshell/hero_crop.json`
//! (`DECKSHELL_HERO_CROP` overrides the path). It is applied when the hero is drawn (Slint transform, see `hero_img.slint`).
//!
//! Geometry (shared by the Rust tests and the Slint `CroppedImage`), for a frame of width `w` and height `h`:
//! - the image is cover-fitted to the frame and then scaled by `zoom` z in [1, 3]; the scaled size is `sw` x `sh`;
//! - `pan_x`, `pan_y` in [-1, 1]: -1 = the left/top edge of the scaled image sits on the left/top edge of the frame, +1 = right/bottom,
//!   0 = centred. So the image is placed at `x = (w - sw) * (1 + pan_x) / 2` (same for y). At zoom 1 the cover fit overflows the frame
//!   in one axis, so the pan already slides the image there; the frame stays covered at every pan.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub const ZOOM_MIN: f32 = 1.0;
pub const ZOOM_MAX: f32 = 3.0;
pub const ZOOM_STEP: f32 = 0.1;
pub const PAN_STEP: f32 = 0.1;
/// Pan per repeat of the left stick (it repeats every 50 ms while held, so it moves slower than a button press).
pub const PAN_STEP_STICK: f32 = 0.03;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeroCrop {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for HeroCrop {
    fn default() -> Self {
        HeroCrop { zoom: 1.0, pan_x: 0.0, pan_y: 0.0 }
    }
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

impl HeroCrop {
    /// Clamp into the valid range; non-finite values fall back to the default axis value.
    pub fn clamped(self) -> HeroCrop {
        let zoom = if self.zoom.is_finite() { self.zoom.clamp(ZOOM_MIN, ZOOM_MAX) } else { ZOOM_MIN };
        let pan = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        HeroCrop { zoom: round3(zoom), pan_x: round3(pan(self.pan_x)), pan_y: round3(pan(self.pan_y)) }
    }

    /// Reference for the tests; the app only uses the Slint transform (`hero_img.slint`), which these numbers pin down.
    #[cfg(test)]
    pub fn is_default(&self) -> bool {
        *self == HeroCrop::default()
    }

    /// Shift the pan by (dx, dy) steps.
    pub fn panned(self, dx: f32, dy: f32) -> HeroCrop {
        HeroCrop { pan_x: self.pan_x + dx, pan_y: self.pan_y + dy, ..self }.clamped()
    }

    pub fn zoomed(self, dz: f32) -> HeroCrop {
        HeroCrop { zoom: self.zoom + dz, ..self }.clamped()
    }

    /// Where the cover-fitted, zoomed image lands in a `frame_w` x `frame_h` frame: (x, y, w, h) in frame units.
    /// `CroppedImage` in `hero_img.slint` computes the same numbers; the tests tie them to `source_rect`.
    #[cfg(test)]
    pub fn placement(&self, img_w: f32, img_h: f32, frame_w: f32, frame_h: f32) -> (f32, f32, f32, f32) {
        let c = self.clamped();
        let scale = (frame_w / img_w).max(frame_h / img_h) * c.zoom;
        let (w, h) = (img_w * scale, img_h * scale);
        ((frame_w - w) * (1.0 + c.pan_x) / 2.0, (frame_h - h) * (1.0 + c.pan_y) / 2.0, w, h)
    }

    /// The part of an `img_w` x `img_h` image that is visible in a `frame_w` x `frame_h` frame, as (x, y, w, h) in image pixels.
    /// Cover fit first (the frame shows the centre of the largest scaled image), then zoom, then the pan slides the window.
    #[cfg(test)]
    pub fn source_rect(&self, img_w: f32, img_h: f32, frame_w: f32, frame_h: f32) -> (f32, f32, f32, f32) {
        let c = self.clamped();
        let scale = (frame_w / img_w).max(frame_h / img_h);
        let vw = frame_w / (scale * c.zoom);
        let vh = frame_h / (scale * c.zoom);
        let x = (img_w - vw) * (1.0 + c.pan_x) / 2.0;
        let y = (img_h - vh) * (1.0 + c.pan_y) / 2.0;
        (x, y, vw, vh)
    }
}

/// One entry per app: the current crop and the one step before it (undo swaps the two, like the picture undo).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Entry {
    pub cur: Option<HeroCrop>,
    pub prev: Option<HeroCrop>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Store {
    apps: BTreeMap<u32, Entry>,
}

impl Store {
    pub fn get(&self, appid: u32) -> HeroCrop {
        self.apps.get(&appid).and_then(|e| e.cur).unwrap_or_default()
    }

    pub fn can_undo(&self, appid: u32) -> bool {
        self.apps.get(&appid).is_some_and(|e| e.prev.is_some())
    }

    /// Store `crop` (clamped) for `appid`. Returns false when nothing changed; a change keeps the old crop as the undo step.
    pub fn set(&mut self, appid: u32, crop: HeroCrop) -> bool {
        let crop = crop.clamped();
        let before = self.get(appid);
        if before == crop {
            return false;
        }
        let e = self.apps.entry(appid).or_default();
        e.prev = Some(before);
        e.cur = Some(crop);
        true
    }

    /// One level: restore the previous crop; the replaced one becomes the new undo step. False when there is nothing to undo.
    pub fn undo(&mut self, appid: u32) -> bool {
        let Some(e) = self.apps.get_mut(&appid) else { return false };
        let Some(prev) = e.prev else { return false };
        let now = e.cur.unwrap_or_default();
        e.cur = Some(prev);
        e.prev = Some(now);
        true
    }

    pub fn to_json(&self) -> serde_json::Value {
        let apps: serde_json::Map<String, serde_json::Value> = self
            .apps
            .iter()
            .map(|(id, e)| {
                let mut o = serde_json::Map::new();
                if let Some(c) = e.cur {
                    o.insert("cur".into(), crop_json(c));
                }
                if let Some(p) = e.prev {
                    o.insert("prev".into(), crop_json(p));
                }
                (id.to_string(), serde_json::Value::Object(o))
            })
            .collect();
        serde_json::json!({ "version": 1, "apps": apps })
    }

    /// Lenient parse: bad entries are skipped, values are clamped, anything else gives an empty store.
    pub fn from_json(v: &serde_json::Value) -> Store {
        let mut st = Store::default();
        let Some(apps) = v.get("apps").and_then(|a| a.as_object()) else { return st };
        for (k, e) in apps {
            let Ok(id) = k.parse::<u32>() else { continue };
            let cur = e.get("cur").and_then(crop_from);
            let prev = e.get("prev").and_then(crop_from);
            if cur.is_some() || prev.is_some() {
                st.apps.insert(id, Entry { cur, prev });
            }
        }
        st
    }

    pub fn load(path: &Path) -> Store {
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(v) => Store::from_json(&v),
                Err(e) => {
                    eprintln!("[deckshell] hero crop: {} unreadable ({e}); using defaults", path.display());
                    Store::default()
                }
            },
            Err(_) => Store::default(),
        }
    }

    /// Atomic write (temp file + rename) so a power cut never leaves a half-written file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.to_json()).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

fn crop_json(c: HeroCrop) -> serde_json::Value {
    serde_json::json!({ "zoom": c.zoom, "pan_x": c.pan_x, "pan_y": c.pan_y })
}

fn crop_from(v: &serde_json::Value) -> Option<HeroCrop> {
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    Some(HeroCrop { zoom: f("zoom")?, pan_x: f("pan_x")?, pan_y: f("pan_y")? }.clamped())
}

pub fn config_path() -> PathBuf {
    std::env::var_os("DECKSHELL_HERO_CROP").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("deckshell/hero_crop.json")
    })
}

// ------------------------------------------------------------------ process-wide store (file = config_path())
static STORE: OnceLock<Mutex<(PathBuf, Store)>> = OnceLock::new();

fn guard() -> MutexGuard<'static, (PathBuf, Store)> {
    STORE
        .get_or_init(|| {
            let p = config_path();
            let s = Store::load(&p);
            Mutex::new((p, s))
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Crop shown for `appid` (default when none is stored).
pub fn get(appid: u32) -> HeroCrop {
    guard().1.get(appid)
}

pub fn can_undo(appid: u32) -> bool {
    guard().1.can_undo(appid)
}

/// Store and save. Ok(false) when the crop did not change (nothing written).
pub fn set(appid: u32, crop: HeroCrop) -> io::Result<bool> {
    let mut g = guard();
    let changed = g.1.set(appid, crop);
    if changed {
        g.1.save(&g.0)?;
    }
    Ok(changed)
}

pub fn reset(appid: u32) -> io::Result<bool> {
    set(appid, HeroCrop::default())
}

/// Undo one level and save. Ok(false) when there was nothing to undo.
pub fn undo(appid: u32) -> io::Result<bool> {
    let mut g = guard();
    let done = g.1.undo(appid);
    if done {
        g.1.save(&g.0)?;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(zoom: f32, pan_x: f32, pan_y: f32) -> HeroCrop {
        HeroCrop { zoom, pan_x, pan_y }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn clamp_limits_zoom_pan_and_non_finite_values() {
        assert_eq!(c(0.2, 0.0, 0.0).clamped().zoom, ZOOM_MIN);
        assert_eq!(c(9.0, 0.0, 0.0).clamped().zoom, ZOOM_MAX);
        assert_eq!(c(f32::NAN, f32::INFINITY, -4.0).clamped(), c(1.0, 0.0, -1.0));
        assert!(HeroCrop::default().is_default());
        assert!(!c(1.1, 0.0, 0.0).is_default());
    }

    #[test]
    fn unzoomed_hero_is_the_whole_cover_frame() {
        // 1920x620 hero in a 1280x753 frame: cover scales to height, the width is cropped, pan decides which part shows
        let (x, y, w, h) = HeroCrop::default().source_rect(1920.0, 620.0, 1280.0, 753.0);
        assert!(close(y, 0.0) && close(h, 620.0));
        // visible width = frame aspect * height: 620 * 1280 / 753
        let vis = 620.0 * 1280.0 / 753.0;
        assert!(close(w, vis), "w={w}");
        assert!(close(x, (1920.0 - vis) / 2.0), "x={x}");
    }

    #[test]
    fn zoom_halves_the_visible_window_and_pan_slides_it_to_the_edges() {
        let img = (1920.0, 620.0);
        let frame = (1280.0, 753.0);
        let (_, _, w0, h0) = HeroCrop::default().source_rect(img.0, img.1, frame.0, frame.1);
        let (xc, _, wc, hc) = c(2.0, 0.0, 0.0).source_rect(img.0, img.1, frame.0, frame.1);
        assert!(close(wc, w0 / 2.0) && close(hc, h0 / 2.0));
        assert!(close(xc, (1920.0 - wc) / 2.0));
        let (xl, _, _, _) = c(2.0, -1.0, 0.0).source_rect(img.0, img.1, frame.0, frame.1);
        let (xr, _, _, _) = c(2.0, 1.0, 0.0).source_rect(img.0, img.1, frame.0, frame.1);
        assert!(close(xl, 0.0), "left edge {xl}");
        assert!(close(xr, 1920.0 - wc), "right edge {xr}");
    }

    #[test]
    fn source_window_stays_inside_the_image_for_every_pan() {
        for &z in &[1.0, 1.5, 3.0] {
            for &px in &[-1.0, -0.4, 0.0, 0.7, 1.0] {
                for &py in &[-1.0, 0.0, 1.0] {
                    let (x, y, w, h) = c(z, px, py).source_rect(1000.0, 1000.0, 1280.0, 753.0);
                    assert!(x >= -0.01 && x + w <= 1000.01, "z{z} px{px}: x {x} w {w}");
                    assert!(y >= -0.01 && y + h <= 1000.01, "z{z} py{py}: y {y} h {h}");
                }
            }
        }
    }

    #[test]
    fn pan_moves_the_image_already_at_zoom_1() {
        // 1920x620 hero in a 900x530 frame: cover overflows the width, so the pan slides the image even unzoomed.
        let (img_w, img_h, fw, fh) = (1920.0, 620.0, 900.0, 530.0);
        let (x_left, _, w, _) = c(1.0, -1.0, 0.0).placement(img_w, img_h, fw, fh);
        let (x_right, _, _, _) = c(1.0, 1.0, 0.0).placement(img_w, img_h, fw, fh);
        let (x_mid, _, _, _) = c(1.0, 0.0, 0.0).placement(img_w, img_h, fw, fh);
        assert!(close(x_left, 0.0), "pan -1 puts the left edge on the frame edge: {x_left}");
        assert!(close(x_right, fw - w), "pan +1 puts the right edge on the frame edge: {x_right}");
        assert!(close(x_mid, (fw - w) / 2.0), "pan 0 centres: {x_mid}");
        assert!(x_left - x_mid > 300.0, "a full pan moves the picture by hundreds of px at zoom 1");
    }

    #[test]
    fn placement_covers_the_frame_and_agrees_with_source_rect() {
        let (img_w, img_h, fw, fh) = (1920.0, 620.0, 900.0, 530.0);
        for &z in &[1.0, 1.1, 2.0, 3.0] {
            for &px in &[-1.0, -0.4, 0.0, 0.7, 1.0] {
                for &py in &[-1.0, 0.0, 1.0] {
                    let crop = c(z, px, py);
                    let (x, y, w, h) = crop.placement(img_w, img_h, fw, fh);
                    assert!(x <= 0.01 && x + w >= fw - 0.01, "z{z} px{px}: frame x not covered ({x}, {w})");
                    assert!(y <= 0.01 && y + h >= fh - 0.01, "z{z} py{py}: frame y not covered ({y}, {h})");
                    // the visible source window maps exactly onto the frame: source px p is drawn at x + p * k (k = frame px per image px)
                    let (sx, sy, sw, sh) = crop.source_rect(img_w, img_h, fw, fh);
                    let k = w / img_w;
                    assert!(close(x + sx * k, 0.0) && close(y + sy * k, 0.0), "z{z} px{px} py{py}: window origin");
                    assert!(close(x + (sx + sw) * k, fw) && close(y + (sy + sh) * k, fh), "z{z} px{px} py{py}: window end");
                }
            }
        }
    }

    #[test]
    fn store_set_undo_and_reset_one_level() {
        let mut st = Store::default();
        assert!(!st.set(7, HeroCrop::default()), "default over default is no change");
        assert!(st.set(7, c(1.5, 0.3, 0.0)));
        assert_eq!(st.get(7), c(1.5, 0.3, 0.0));
        assert!(st.can_undo(7));
        assert!(st.set(7, c(2.0, -0.5, 0.5)));
        assert!(st.undo(7));
        assert_eq!(st.get(7), c(1.5, 0.3, 0.0), "undo gives back the previous crop");
        assert!(st.undo(7), "undo of undo swaps back");
        assert_eq!(st.get(7), c(2.0, -0.5, 0.5));
        assert!(st.set(7, HeroCrop::default()));
        assert_eq!(st.get(7), HeroCrop::default());
        assert!(st.undo(7));
        assert_eq!(st.get(7), c(2.0, -0.5, 0.5), "reset is undoable");
        assert!(!Store::default().undo(7));
        assert_eq!(st.get(99), HeroCrop::default(), "other apps untouched");
        assert!(!st.can_undo(99));
    }

    #[test]
    fn store_round_trips_through_a_file_and_survives_bad_input() {
        let dir = std::env::temp_dir().join(format!("deckshell-herocrop-{}-{}", std::process::id(), line!()));
        let path = dir.join("hero_crop.json");
        let mut st = Store::default();
        st.set(730, c(1.7, 0.25, -1.0));
        st.set(730, c(2.4, 0.0, 0.5));
        st.set(4294967295, c(1.2, 0.1, 0.1));
        st.save(&path).unwrap();
        let back = Store::load(&path);
        assert_eq!(back, st);
        assert_eq!(back.get(730), c(2.4, 0.0, 0.5));
        assert!(back.can_undo(730));
        // broken file and wrong shapes fall back, never panic
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Store::load(&path), Store::default());
        let v = serde_json::json!({ "apps": { "abc": {"cur": {"zoom": 2, "pan_x": 0, "pan_y": 0}}, "5": {"cur": {"zoom": 9, "pan_x": 3, "pan_y": "x"}}, "6": {"cur": {"zoom": 2, "pan_x": 0.5, "pan_y": -0.5}} } });
        let s = Store::from_json(&v);
        assert_eq!(s.get(6), c(2.0, 0.5, -0.5));
        assert_eq!(s.get(5), HeroCrop::default(), "missing pan_y skips the entry");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
