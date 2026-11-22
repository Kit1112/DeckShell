//! Data access layer: real crates guarded by catch_unwind, with opt-in mock data.
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use deck_system as sys;
use media::Frame;
use steam_control::{Download, SteamPage};
use steam_data::{Art, Game, GameKind, Library};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Restart,
    Shutdown,
    Desktop,
    StockSteam,
}

struct MockSys {
    brightness: f32,
    volume: f32,
    muted: bool,
    wifi: bool,
    bt: bool,
    profile: String,
    tdp: u32,
}

pub struct DataSource {
    env_mock: bool,
    library_unavailable: AtomicBool,
    /// true when the game library is fake (so launching etc. is only logged)
    fake_library: AtomicBool,
    broken: Mutex<HashSet<&'static str>>,
    mock: Mutex<MockSys>,
}

macro_rules! sysc {
    ($s:expr, $name:literal, $real:expr, $mock:expr) => {{
        if $s.env_mock {
            $mock
        } else if $s.use_real($name) {
            match catch_unwind(AssertUnwindSafe(|| $real)) {
                Ok(v) => v,
                Err(_) => {
                    $s.mark_broken($name);
                    Default::default()
                }
            }
        } else {
            Default::default()
        }
    }};
}

impl DataSource {
    pub fn new() -> Self {
        let env_mock = std::env::var("DECKSHELL_MOCK").map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
        DataSource {
            env_mock,
            library_unavailable: AtomicBool::new(false),
            fake_library: AtomicBool::new(env_mock),
            broken: Mutex::new(HashSet::new()),
            mock: Mutex::new(MockSys {
                brightness: 0.6,
                volume: 0.45,
                muted: false,
                wifi: true,
                bt: false,
                profile: "balanced".into(),
                tdp: 12,
            }),
        }
    }

    pub fn is_mock(&self) -> bool {
        self.env_mock
    }

    pub fn library_unavailable(&self) -> bool { self.library_unavailable.load(Ordering::Relaxed) }
    pub fn library_received(&self) { self.library_unavailable.store(false, Ordering::Relaxed); }

    fn use_real(&self, name: &'static str) -> bool {
        !self.env_mock && !self.broken.lock().unwrap().contains(name)
    }
    fn mark_broken(&self, name: &'static str) {
        if self.broken.lock().unwrap().insert(name) {
            eprintln!("[deckshell] `{name}` unavailable (panicked)");
        }
    }
    fn fake(&self) -> bool {
        self.fake_library.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------ library
    pub fn load_library(&self) -> Library {
        if !self.env_mock {
            let root = steam_data::default_root();
            let r = catch_unwind(AssertUnwindSafe(|| {
                let root = root.as_ref().ok_or(steam_data::Error::NoSteam)?;
                steam_data::load(root)
            }));
            match r {
                Ok(Ok(lib)) => { self.library_received(); return lib; }
                Ok(Err(e)) => eprintln!("[deckshell] steam-data error: {e} - waiting for Steam library data"),
                Err(_) => eprintln!("[deckshell] steam-data failed - waiting for Steam library data"),
            }
            self.library_unavailable.store(true, Ordering::Relaxed);
            return unavailable_library(root);
        }
        mock_library()
    }

    // ------------------------------------------------------------ actions
    fn log_or<F: FnOnce() -> std::io::Result<()>>(&self, what: &str, f: F) {
        if self.fake() {
            eprintln!("[deckshell] (mock) {what}");
            return;
        }
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(())) => eprintln!("[deckshell] {what}"),
            Ok(Err(e)) => eprintln!("[deckshell] {what} failed: {e}"),
            Err(_) => eprintln!("[deckshell] {what}: not implemented"),
        }
    }
    pub fn launch(&self, g: &Game) {
        self.log_or(&format!("launch {} ({})", g.name, g.app_id), || steam_control::launch(g));
    }
    pub fn install(&self, id: u32) {
        self.log_or(&format!("install {id}"), || steam_control::install(id));
    }
    pub fn open(&self, page: SteamPage) {
        self.log_or(&format!("open {page:?}"), || steam_control::open(page.clone()));
    }
    pub fn power(&self, a: PowerAction) {
        if self.env_mock {
            eprintln!("[deckshell] (mock) power {a:?}");
            return;
        }
        self.log_or(&format!("power {a:?}"), || match a {
            PowerAction::Sleep => sys::suspend(),
            PowerAction::Restart => sys::reboot(),
            PowerAction::Shutdown => sys::poweroff(),
            PowerAction::Desktop => {
                let _ = sys::switch_to_desktop();
                crate::exit_request::request(crate::exit_request::DESKTOP);
                Ok(())
            }
            PowerAction::StockSteam => {
                eprintln!("[deckshell] owner chose: return to stock Steam (exit {})", crate::exit_request::STOCK_STEAM);
                crate::exit_request::request(crate::exit_request::STOCK_STEAM);
                Ok(())
            }
        });
    }
    pub fn downloads(&self, lib: &Library) -> Vec<Download> {
        if !self.env_mock {
            return catch_unwind(AssertUnwindSafe(|| steam_control::downloads(lib)))
                .unwrap_or_default();
        }
        lib.games
            .iter()
            .filter(|g| g.update_pending)
            .take(3)
            .enumerate()
            .map(|(i, g)| Download {
                app_id: g.app_id,
                name: g.name.clone(),
                downloaded: (g.size_on_disk / 5) * (i as u64 + 1),
                total: g.size_on_disk.max(1),
                paused: i == 2,
            })
            .collect()
    }

    fn system_set(&self, name: &'static str, real: impl FnOnce() -> std::io::Result<()>, mock: impl FnOnce()) -> std::io::Result<()> {
        if self.env_mock {
            mock();
            return Ok(());
        }
        if !self.use_real(name) {
            return Err(std::io::Error::other(format!("{name} unavailable")));
        }
        match catch_unwind(AssertUnwindSafe(real)) {
            Ok(result) => result,
            Err(_) => {
                self.mark_broken(name);
                Err(std::io::Error::other(format!("{name} panicked")))
            }
        }
    }

    // ------------------------------------------------------------ system
    pub fn battery(&self) -> Option<sys::Battery> {
        sysc!(self, "battery", sys::battery(), Some(sys::Battery { percent: 76.0, charging: false, online: Some(false), full: false, minutes_left: Some(214) }))
    }
    /// Scale position 0..1 through Steam's display API (task 270); the same writer as `set_brightness`.
    pub fn brightness(&self) -> Option<f32> {
        sysc!(self, "brightness", crate::display::read(), Some(self.mock.lock().unwrap().brightness))
    }
    pub fn set_brightness(&self, v: f32) -> std::io::Result<()> {
        self.system_set("set_brightness", || crate::display::write(v), || self.mock.lock().unwrap().brightness = v)
    }
    pub fn volume(&self) -> Option<(f32, bool)> {
        sysc!(self, "volume", sys::volume(), { let m = self.mock.lock().unwrap(); Some((m.volume, m.muted)) })
    }
    pub fn set_volume(&self, v: f32) -> std::io::Result<()> {
        self.system_set("set_volume", || sys::set_volume(v), || self.mock.lock().unwrap().volume = v)
    }
    pub fn set_muted(&self, v: bool) {
        sysc!(self, "set_muted", { let _ = sys::set_muted(v); }, { self.mock.lock().unwrap().muted = v })
    }
    pub fn wifi(&self) -> Option<bool> {
        sysc!(self, "wifi_enabled", sys::wifi_enabled(), Some(self.mock.lock().unwrap().wifi))
    }
    pub fn set_wifi(&self, v: bool) {
        sysc!(self, "set_wifi_enabled", { let _ = sys::set_wifi_enabled(v); }, { self.mock.lock().unwrap().wifi = v })
    }
    pub fn bluetooth(&self) -> Option<bool> {
        sysc!(self, "bluetooth_enabled", sys::bluetooth_enabled(), Some(self.mock.lock().unwrap().bt))
    }
    pub fn set_bluetooth(&self, v: bool) {
        sysc!(self, "set_bluetooth_enabled", { let _ = sys::set_bluetooth_enabled(v); }, { self.mock.lock().unwrap().bt = v })
    }
    pub fn profiles(&self) -> Vec<String> {
        sysc!(self, "performance_profiles", sys::performance_profiles(), vec!["low-power".into(), "balanced".into(), "performance".into()])
    }
    pub fn profile(&self) -> Option<String> {
        sysc!(self, "performance_profile", sys::performance_profile(), Some(self.mock.lock().unwrap().profile.clone()))
    }
    pub fn set_profile(&self, p: &str) {
        sysc!(self, "set_performance_profile", { let _ = sys::set_performance_profile(p); }, { self.mock.lock().unwrap().profile = p.to_string() })
    }
    pub fn tdp(&self) -> Option<(u32, u32, u32)> {
        sysc!(self, "tdp_limit", sys::tdp_limit(), Some((self.mock.lock().unwrap().tdp, 4, 15)))
    }
    pub fn gpu_clock(&self) -> Option<(bool, Option<u32>, u32, u32)> {
        sysc!(self, "gpu_clock", sys::gpu_clock(), None)
    }

    // ------------------------------------------------------------ media
    /// Decode a still image downscaled to fit `w`x`h`. Blocking: call from a worker.
    pub fn thumbnail(&self, path: &Path, w: u32, h: u32) -> Option<Frame> {
        if self.use_real("load_thumbnail") {
            match catch_unwind(AssertUnwindSafe(|| media::load_thumbnail(path, w, h))) {
                Ok(Ok(f)) => return Some(f),
                Ok(Err(_)) => {}
                Err(_) => self.mark_broken("load_thumbnail"),
            }
        }
        let img = image::open(path).ok()?;
        let img = if img.width() > w || img.height() > h {
            img.resize(w, h, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        Some(Frame { width: rgba.width(), height: rgba.height(), rgba: std::sync::Arc::new(rgba.into_raw()), delay_ms: 0 })
    }

    /// Decode an animated cover. Blocking: call from a worker.
    pub fn animation(&self, path: &Path, w: u32, h: u32) -> Option<Vec<Frame>> {
        if self.use_real("load_animation") {
            match catch_unwind(AssertUnwindSafe(|| media::load_animation(path, w, h))) {
                Ok(Ok(a)) if !a.frames.is_empty() => return Some(a.frames),
                Ok(_) => {}
                Err(_) => self.mark_broken("load_animation"),
            }
        }
        fallback_animation(path, w, h)
    }
}

fn fallback_animation(path: &Path, w: u32, h: u32) -> Option<Vec<Frame>> {
    use image::AnimationDecoder;
    let f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let frames: Vec<image::Frame> = match ext.as_str() {
        "gif" => image::codecs::gif::GifDecoder::new(f).ok()?.into_frames().take(120).collect::<Result<_, _>>().ok()?,
        "webp" => image::codecs::webp::WebPDecoder::new(f).ok()?.into_frames().take(120).collect::<Result<_, _>>().ok()?,
        _ => return None,
    };
    let out: Vec<Frame> = frames
        .into_iter()
        .map(|fr| {
            let (n, d) = fr.delay().numer_denom_ms();
            let delay = (n / d.max(1)).max(20);
            let img = image::DynamicImage::ImageRgba8(fr.into_buffer()).resize(w, h, image::imageops::FilterType::Triangle).to_rgba8();
            Frame { width: img.width(), height: img.height(), rgba: std::sync::Arc::new(img.into_raw()), delay_ms: delay }
        })
        .collect();
    if out.is_empty() { None } else { Some(out) }
}

// ====================================================================== mock library
fn unavailable_library(root: Option<PathBuf>) -> Library {
    Library { steam_root: root.unwrap_or_default(), ..Library::default() }
}

fn mock_library() -> Library {
    let home = std::env::var("HOME").unwrap_or_default();
    let root = PathBuf::from(&home).join(".local/share/Steam/appcache/librarycache");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.join("library_600x900.jpg").exists()).collect())
        .unwrap_or_default();
    dirs.sort();
    let count: usize = std::env::var("DECKSHELL_MOCK_COUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(40);

    const A: [&str; 16] = ["Crimson", "Neon", "Silent", "Lost", "Iron", "Hollow", "Star", "Shadow", "Golden", "Frozen", "Wild", "Last", "Quantum", "Rusted", "Midnight", "Ancient"];
    const B: [&str; 16] = ["Horizon", "Drifter", "Kingdom", "Odyssey", "Protocol", "Legacy", "Frontier", "Tactics", "Harvest", "Descent", "Chronicle", "Uprising", "Circuit", "Sanctum", "Rally", "Gambit"];
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut games = Vec::new();
    for i in 0..count {
        let name = format!("{} {}{}", A[i % 16], B[(i / 16 + i * 7) % 16], if i >= 256 { format!(" {}", i / 256 + 1) } else { String::new() });
        let name = if i % 9 == 0 { format!("{name} (Non-Steam)") } else { name };
        let (art, size) = if dirs.is_empty() {
            (Art::default(), 0)
        } else {
            let d = &dirs[i % dirs.len()];
            let p = |n: &str| Some(d.join(n)).filter(|p| p.exists());
            (Art { capsule: p("library_600x900.jpg"), hero: p("library_hero.jpg"), logo: p("library_logo.png"), header: p("header.jpg"), ..Default::default() }, 0)
        };
        let _ = size;
        let h = (i as u64).wrapping_mul(2654435761) % 1000;
        games.push(Game {
            app_id: 100_000 + i as u32,
            kind: if i % 9 == 0 { GameKind::Shortcut } else { GameKind::Steam },
            sort_name: name.to_lowercase(),
            name,
            installed: i % 3 != 2,
            install_dir: None,
            size_on_disk: (500_000_000 + h * 90_000_000) as u64,
            playtime_minutes: ((h * 37) % 9000) as u32,
            last_played: if i % 4 == 3 { None } else { Some(now.saturating_sub((i as u64) * 7_300 + h * 100)) },
            update_pending: i % 11 == 0,
            collections: match i % 5 { 0 => vec!["Favorites".into()], 1 => vec!["RPG".into()], _ => vec![] },
            art,
        });
    }
    Library { steam_root: PathBuf::new(), users: vec![], active_user: None, games, collections: vec!["Favorites".into(), "RPG".into()] }
}

#[cfg(test)]
mod library_availability_tests {
    use super::*;
    #[test]
    fn missing_library_is_not_replaced_with_demo_games_or_a_fake_user() {
        for root in [None, Some(PathBuf::from("test-steam-root"))] {
            let lib = unavailable_library(root.clone());
            assert!(lib.games.is_empty());
            assert!(lib.users.is_empty());
            assert!(lib.active_user.is_none());
            assert_eq!(lib.steam_root, root.unwrap_or_default());
        }
        // Demo remains an explicit, separately selected development mode.
        assert!(!mock_library().games.is_empty());
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    #[test]
    fn system_panic_stays_unavailable_without_using_mock() {
        let mut data = DataSource::new();
        data.env_mock = false;
        let first: Option<i32> = sysc!(data, "test_failure", panic!("test"), Some(76));
        let second: Option<i32> = sysc!(data, "test_failure", Some(1), Some(76));
        assert_eq!(first, None);
        assert_eq!(second, None);
        let list: Vec<String> = sysc!(data, "test_list", panic!("test"), vec!["mock".into()]);
        assert!(list.is_empty());
    }

    #[test]
    fn setter_failure_does_not_report_success() {
        let mut data = DataSource::new();
        data.env_mock = false;
        let failure = data.system_set("test_set_error", || Err(std::io::Error::other("denied")), || panic!("mock used"));
        assert!(failure.is_err());
        let panic = data.system_set("test_set_panic", || panic!("backend panic"), || panic!("mock used"));
        assert!(panic.is_err());
        assert!(data.system_set("test_set_panic", || Ok(()), || {}).is_err());
    }

    #[test]
    fn setter_success_and_mock_update() {
        let mut data = DataSource::new();
        data.env_mock = false;
        let called = std::cell::Cell::new(false);
        assert!(data.system_set("test_set_success", || { called.set(true); Ok(()) }, || panic!("mock used")).is_ok());
        assert!(called.get());
        data.env_mock = true;
        assert!(data.set_brightness(0.3).is_ok());
        assert!(data.set_volume(0.7).is_ok());
        assert_eq!(data.brightness(), Some(0.3));
        assert_eq!(data.volume(), Some((0.7, false)));
    }

    #[test]
    fn mock_system_values_remain_available() {
        let mut data = DataSource::new();
        data.env_mock = true;
        let value: Option<i32> = sysc!(data, "test_mock", panic!("real called"), Some(76));
        assert_eq!(value, Some(76));
    }
}

#[cfg(test)]
mod host_guard {
    use super::*;
    /// Inner half of the sentinel: runs only in the child process started by `no_host_tool_runs_from_tests`.
    #[test]
    #[ignore]
    fn inner_mutations() {
        let ds = DataSource::new();
        ds.set_wifi(false);
        ds.set_wifi(true);
        ds.set_bluetooth(false);
        ds.set_profile("low-power");
        let _ = ds.set_volume(0.2);
        ds.set_muted(true);
        let _ = deck_system::set_wifi_enabled(false);
    }

    fn host_trap_script(tool: &str, log: &str) -> String {
        let pat = match tool {
            "busctl" => " set-property | call ",
            "nmcli" => " on| off| connect| disconnect| delete| modify| up| down| add",
            "systemctl" => " start| stop| restart| kill| enable| disable| mask| poweroff| reboot| suspend",
            "wpctl" | "pactl" => " set-",
            _ => "",
        };
        let cond = if pat.is_empty() { "true".to_string() } else { format!("case \" $* \" in *{}*) true;; *) false;; esac", pat.replace('|', "*|*")) };
        format!("#!/bin/sh\nif {cond}; then echo \"{tool} $*\" >> '{log}'; fi\nexit 1\n")
    }

    /// Fails the suite if a mutating host tool (nmcli/rfkill/busctl/systemctl/...) is reached from app code under test.
    #[test]
    fn no_host_tool_runs_from_tests() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("deckshell-apptrap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        for t in ["nmcli", "rfkill", "bluetoothctl", "busctl", "systemctl", "systemd-run", "wpctl", "pactl", "steamos-session-select", "iwctl", "powerprofilesctl", "xdg-open"] {
            let p = dir.join(t);
            std::fs::write(&p, host_trap_script(t, &log.display().to_string())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let st = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "data::host_guard::inner_mutations", "--test-threads=1"])
            .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
            .env_remove("DECKSHELL_DRYRUN").env_remove("DECKSHELL_TEST")
            .output().unwrap();
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(st.status.success(), "inner test failed: {}", String::from_utf8_lossy(&st.stdout));
        assert!(calls.is_empty(), "a mutating host tool was invoked from a test:\n{calls}");
    }
}
