//! Per-game logo scale and position, stored locally like the hero crop.
//! Pan -1..1 places the contained image between the frame edges; zoom is
//! uniform and clamped to 0.25..2.
//!
//! Stored pan values are fractions of the frame travel and never change
//! meaning: `logo_layout.json` stays version 1 and is not migrated. The
//! stock position is `pan_y = DEFAULT_PAN_Y` (0.7 of the travel below the
//! centre). Only the editor readout is rescaled: it shows the stock
//! position as +100% (`pan_y_percent`), so the frame bottom reads +143%.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

pub const ZOOM_MIN: f32 = 0.25;
pub const ZOOM_MAX: f32 = 2.0;
pub const ZOOM_STEP: f32 = 0.1;
pub const PAN_STEP: f32 = 0.1;
/// Stock vertical position in stored units (task 361): what the editor showed as +70% before.
pub const DEFAULT_PAN_Y: f32 = 0.7;
/// Pan per repeat of the left stick (it repeats every 50 ms while held, so it moves slower than a button press).
pub const PAN_STEP_STICK: f32 = 0.03;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogoLayout {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for LogoLayout {
    fn default() -> Self {
        LogoLayout { zoom: 1.0, pan_x: -1.0, pan_y: DEFAULT_PAN_Y }
    }
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

impl LogoLayout {
    /// Clamp into the valid range; non-finite values fall back to the default axis value.
    pub fn clamped(self) -> LogoLayout {
        let zoom = if self.zoom.is_finite() { self.zoom.clamp(ZOOM_MIN, ZOOM_MAX) } else { 1.0 };
        let pan = |v: f32, default| if v.is_finite() { v.clamp(-1.0, 1.0) } else { default };
        LogoLayout { zoom: round3(zoom), pan_x: round3(pan(self.pan_x, -1.0)), pan_y: round3(pan(self.pan_y, DEFAULT_PAN_Y)) }
    }

    /// Shift the pan by (dx, dy) steps.
    pub fn panned(self, dx: f32, dy: f32) -> LogoLayout {
        LogoLayout { pan_x: self.pan_x + dx, pan_y: self.pan_y + dy, ..self }.clamped()
    }

    pub fn zoomed(self, dz: f32) -> LogoLayout {
        LogoLayout { zoom: self.zoom + dz, ..self }.clamped()
    }

    /// Shift by editor steps: `dx` in stored units, `dy` in readout units (1.0 = 100% on the Y readout).
    pub fn nudged(self, dx: f32, dy: f32) -> LogoLayout {
        self.panned(dx, dy * DEFAULT_PAN_Y)
    }

    /// Editor readout (X %, Y %). X is the stored pan; Y is scaled so the stock position reads +100.
    pub fn readout(self) -> (i32, i32) {
        ((self.pan_x * 100.0).round() as i32, pan_y_percent(self.pan_y))
    }
}

/// Y readout of a stored pan: stock (`DEFAULT_PAN_Y`) = +100, centre = 0, frame bottom = +143, top = -143.
pub fn pan_y_percent(pan_y: f32) -> i32 {
    (pan_y / DEFAULT_PAN_Y * 100.0).round() as i32
}

/// One entry per app: the current crop and the one step before it (undo swaps the two, like the picture undo).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Entry {
    pub cur: Option<LogoLayout>,
    pub prev: Option<LogoLayout>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Store {
    apps: BTreeMap<u32, Entry>,
}

impl Store {
    pub fn get(&self, appid: u32) -> LogoLayout {
        self.apps.get(&appid).and_then(|e| e.cur).unwrap_or_default()
    }

    pub fn can_undo(&self, appid: u32) -> bool {
        self.apps.get(&appid).is_some_and(|e| e.prev.is_some())
    }

    /// Store `crop` (clamped) for `appid`. Returns false when nothing changed; a change keeps the old crop as the undo step.
    pub fn set(&mut self, appid: u32, crop: LogoLayout) -> bool {
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
                    eprintln!("[deckshell] logo layout: {} unreadable ({e}); using defaults", path.display());
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

fn crop_json(c: LogoLayout) -> serde_json::Value {
    serde_json::json!({ "zoom": c.zoom, "pan_x": c.pan_x, "pan_y": c.pan_y })
}

fn crop_from(v: &serde_json::Value) -> Option<LogoLayout> {
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    Some(LogoLayout { zoom: f("zoom")?, pan_x: f("pan_x")?, pan_y: f("pan_y")? }.clamped())
}

pub fn config_path() -> PathBuf {
    std::env::var_os("DECKSHELL_LOGO_LAYOUT").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("deckshell/logo_layout.json")
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
pub fn get(appid: u32) -> LogoLayout {
    guard().1.get(appid)
}

pub fn can_undo(appid: u32) -> bool {
    guard().1.can_undo(appid)
}

/// Store and save. Ok(false) when the crop did not change (nothing written).
pub fn set(appid: u32, crop: LogoLayout) -> io::Result<bool> {
    let mut g = guard();
    let changed = g.1.set(appid, crop);
    if changed {
        g.1.save(&g.0)?;
    }
    Ok(changed)
}

pub fn reset(appid: u32) -> io::Result<bool> {
    set(appid, LogoLayout::default())
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

    // Mirrors PositionedLogo: uniform contain, then move through remaining space.
    fn placement(c: LogoLayout, ar: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let c = c.clamped();
        let margin = w * 0.02436;
        let max_w = w - 2.0 * margin;
        let iw = max_w.min(max_w * c.zoom).min(h * (144.0 / 633.0) * c.zoom * ar);
        let ih = iw / ar;
        (margin + (max_w - iw) * (c.pan_x + 1.0) / 2.0, (h - ih) * (c.pan_y + 1.0) / 2.0, iw, ih)
    }

    #[test]
    fn defaults_limits_and_placement_preserve_aspect() {
        let d = LogoLayout::default();
        assert_eq!(d, LogoLayout { zoom: 1.0, pan_x: -1.0, pan_y: 0.7 });
        assert_eq!(LogoLayout { zoom: f32::NAN, pan_x: f32::NAN, pan_y: f32::INFINITY }.clamped(), d);
        assert_eq!(d.zoomed(-100.0).zoom, ZOOM_MIN);
        assert_eq!(d.zoomed(100.0).zoom, ZOOM_MAX);
        assert_eq!(d.panned(-10.0, 10.0), LogoLayout { pan_y: 1.0, ..d });
        let (x, y, w, h) = placement(d, 2.0, 1280.0, 633.0);
        assert!((x - 31.18).abs() < 0.1 && (y - 489.0 * 0.85).abs() < 0.1);
        assert!((w / h - 2.0).abs() < 0.001);
        for ar in [0.2, 1.0, 8.0] {
            for z in [ZOOM_MIN, 1.0, ZOOM_MAX] {
                for pan in [-1.0, 1.0] {
                    let (x, y, w, h) = placement(LogoLayout { zoom: z, pan_x: pan, pan_y: pan }, ar, 1280.0, 633.0);
                    assert!(x >= 0.0 && y >= 0.0 && x + w <= 1280.01 && y + h <= 633.01);
                    assert!((w / h - ar).abs() < 0.001);
                }
            }
        }
    }

    /// Task 361: the position the editor called +70 is the stock one and reads +100; nothing else moves.
    #[test]
    fn stock_position_is_old_plus_70_and_reads_plus_100() {
        let d = LogoLayout::default();
        // Same pixels as the old editor value +70 (stored 0.7) in the old build.
        let old_plus_70 = LogoLayout { zoom: 1.0, pan_x: -1.0, pan_y: 0.7 };
        assert_eq!(placement(d, 2.0, 1280.0, 633.0), placement(old_plus_70, 2.0, 1280.0, 633.0));
        assert_eq!(d.readout(), (-100, 100));
        assert_eq!(pan_y_percent(0.0), 0);
        assert_eq!(pan_y_percent(1.0), 143);
        assert_eq!(pan_y_percent(-1.0), -143);
        // One button press = 10 on the readout; the frame bottom stays reachable.
        let ys: Vec<i32> = (0..6).scan(d, |c, _| { *c = c.nudged(0.0, PAN_STEP); Some(c.readout().1) }).collect();
        assert_eq!(ys, [110, 120, 130, 140, 143, 143]);
        assert_eq!(d.nudged(0.0, -PAN_STEP).readout().1, 90);
        assert_eq!(d.nudged(PAN_STEP, 0.0).pan_x, -0.9);
        // The .slint fallbacks agree with Rust (page, editor, preview).
        for source in [include_str!("../../ui/widgets/logo_layout.slint"), include_str!("../../ui/screens/game.slint"), include_str!("../../ui/screens/sgdb.slint")] {
            let n = source.lines().filter(|l| l.contains("<HeroFrame>") && l.contains("layout:")).inspect(|l| assert!(l.contains("{ zoom: 1, pan-x: -1, pan-y: 0.7 }"), "{l}")).count();
            assert!(n > 0);
        }
    }

    /// Saved layouts keep their pixels: the file written by the task-359 build is read as is and never rewritten on load.
    #[test]
    fn saved_layouts_from_the_old_build_keep_their_position() {
        // Shape and values as saved on the Deck by the 359 build (f32 noise included).
        let text = r#"{"apps":{
            "2379780":{"cur":{"pan_x":-1.0,"pan_y":0.800000011920929,"zoom":1.0},"prev":{"pan_x":-1.0,"pan_y":1.0,"zoom":1.0}},
            "2710347437":{"cur":{"pan_x":-1.0,"pan_y":0.8999999761581421,"zoom":1.2999999523162842},"prev":{"pan_x":-1.0,"pan_y":1.0,"zoom":1.0}},
            "2796594733":{"cur":{"pan_x":-1.0,"pan_y":0.699999988079071,"zoom":1.0},"prev":{"pan_x":-1.0,"pan_y":0.800000011920929,"zoom":1.0}},
            "5":{"cur":{"pan_x":-1.0,"pan_y":1.0,"zoom":1.0}}},"version":1}"#;
        let path = std::env::temp_dir().join(format!("deckshell-logo-361-{}.json", std::process::id()));
        std::fs::write(&path, text).unwrap();
        let mut st = Store::load(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "load does not rewrite the file");
        // Old formula of the 359 build (bottom-anchored default did not enter it): identical pixels now.
        let old = |c: LogoLayout| { let (_, y, _, h) = placement(c, 2.0, 1280.0, 633.0); (y, h) };
        for (id, zoom, pan_y, shown) in [(2379780, 1.0, 0.8, 114), (2710347437, 1.3, 0.9, 129), (2796594733, 1.0, 0.7, 100), (5, 1.0, 1.0, 143)] {
            let c = st.get(id);
            assert_eq!(c, LogoLayout { zoom, pan_x: -1.0, pan_y });
            assert_eq!(old(c), old(LogoLayout { zoom, pan_x: -1.0, pan_y }));
            assert_eq!(c.readout().1, shown);
        }
        // An explicitly saved old default (bottom, app 5) is a saved layout too: it stays at the bottom.
        assert_ne!(st.get(5), LogoLayout::default());
        // A game without an entry gets the new stock position; the file version is unchanged.
        assert_eq!(st.get(6), LogoLayout::default());
        assert_eq!(st.to_json()["version"], 1);
        // Undo still returns the stored previous value (the old bottom default for these entries).
        assert!(st.undo(2379780));
        assert_eq!(st.get(2379780).pan_y, 1.0);
        // Saving the stock position for a game with no entry writes nothing.
        assert!(!st.set(6, LogoLayout::default()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stock_default_and_saved_layouts_are_not_migrated() {
        let d = LogoLayout::default();
        assert_eq!((d.pan_y * 100.0).round() as i32, 70);
        // PositionedLogo maps -1..1 to 0..remaining travel; stock is 85% down.
        let (_, y, _, h) = placement(d, 2.0, 1280.0, 633.0);
        assert!((y / (633.0 - h) - 0.85).abs() < 0.0001);
        let json = serde_json::json!({"version": 1, "apps": {
            "7": {"cur": {"zoom": 1.0, "pan_x": -1.0, "pan_y": 1.0},
                  "prev": {"zoom": 1.4, "pan_x": 0.5, "pan_y": -0.5}}
        }});
        let path = std::env::temp_dir().join(format!("deckshell-logo-default-{}.json", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&json).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let mut store = Store::load(&path);
        assert_eq!(store.get(7).pan_y, 1.0, "even the old explicit default stays saved");
        assert_eq!(store.get(8), d);
        assert_eq!(Store::from_json(&store.to_json()), store, "cur and prev retain their f32 values");
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "load/get never rewrite the file");
        assert!(store.undo(7));
        assert_eq!(store.get(7), LogoLayout { zoom: 1.4, pan_x: 0.5, pan_y: -0.5 });
        std::fs::remove_file(path).unwrap();
        // Keep declarative fallback defaults aligned with Rust in page/editor/preview.
        for source in [include_str!("../../ui/widgets/logo_layout.slint"),
                       include_str!("../../ui/screens/game.slint"),
                       include_str!("../../ui/screens/sgdb.slint")] {
            let defaults: Vec<_> = source.lines().filter(|l| l.contains("property <HeroFrame>") && l.contains("layout:")).collect();
            assert!(!defaults.is_empty());
            assert!(defaults.iter().all(|l| l.contains("zoom: 1, pan-x: -1, pan-y: 0.7")));
        }
    }

    #[test]
    fn per_game_roundtrip_reset_and_undo() {
        let path = std::env::temp_dir().join(format!("deckshell-logo-layout-{}.json", std::process::id()));
        let mut store = Store::default();
        let layout = LogoLayout { zoom: 1.4, pan_x: 0.5, pan_y: -0.5 };
        assert!(store.set(7, layout));
        assert_eq!(store.get(8), LogoLayout::default());
        store.save(&path).unwrap();
        let mut loaded = Store::load(&path);
        assert_eq!(loaded.get(7), layout);
        assert!(loaded.set(7, LogoLayout::default()));
        assert!(loaded.undo(7));
        assert_eq!(loaded.get(7), layout);
        assert_eq!(Store::from_json(&serde_json::json!({"apps":{"7":{"cur":{"zoom":9,"pan_x":4,"pan_y":-9}}}})).get(7), LogoLayout { zoom: 2.0, pan_x: 1.0, pan_y: -1.0 });
        let _ = std::fs::remove_file(path);
    }
}
