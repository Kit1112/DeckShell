//! AnimationChanger state: import of the Decky plugin's settings (read-only), DeckShell's own copy, randomize semantics.
//!
//! Source of truth for the semantics is `~/homebrew/plugins/SDH-AnimationChanger/main.py` (context/archive/docs/animation-changer.md):
//!  * types boot / suspend / throbber; targets boot / suspend / suspend; stock Steam file names below;
//!  * pool of `randomize_all` = local animations + downloads + custom animations of the slot's target, minus `shuffle_exclusions`;
//!  * `randomize_all` replaces the three slots with independent random picks and clears `current_set`;
//!  * `randomize_current_set` picks one *enabled* set and takes all three slots from it;
//! Автоматическое перемешивание DeckShell ограничено слотами boot и suspend.
//! При импорте общее значение плагина преобразуется в два переключателя.
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const VIDEO_TYPES: [&str; 3] = ["boot", "suspend", "throbber"];
pub const VIDEO_TARGETS: [&str; 3] = ["boot", "suspend", "suspend"];
/// File names Steam looks up in `uioverrides/movies` (the plugin symlinks its pick to these).
pub const VIDEO_NAMES: [&str; 3] = ["deck_startup.webm", "steam_os_suspend.webm", "steam_os_suspend_from_throbber.webm"];

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct Dirs {
    /// DeckShell's own state dir (config.json, animations/, downloads/).
    pub own: PathBuf,
    /// Decky plugin settings file (read only).
    pub plugin_config: PathBuf,
    /// Decky plugin runtime dir with `animations/` and `downloads/` (read only).
    pub plugin_data: PathBuf,
    /// Steam override dir (read only fallback for empty slots).
    pub overrides: PathBuf,
    /// Steam stock movies.
    pub stock: PathBuf,
    /// Frame cache.
    pub cache: PathBuf,
    /// steamdeckrepo catalog, thumbnails and previews (task 412).
    pub repo: PathBuf,
    /// Append-only journal of boot and suspend shuffle picks.
    pub picks_log: PathBuf,
}

impl Dirs {
    pub fn detect() -> Dirs {
        let h = home();
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let cache_base = env("XDG_CACHE_HOME").unwrap_or_else(|| h.join(".cache"));
        Dirs {
            own: env("DECKSHELL_ANIM_DIR").unwrap_or_else(|| h.join(".local/share/deckshell/animchanger")),
            plugin_config: env("DECKSHELL_ANIM_PLUGIN_CONFIG").unwrap_or_else(|| h.join("homebrew/settings/SDH-AnimationChanger/config.json")),
            plugin_data: env("DECKSHELL_ANIM_PLUGIN_DATA").unwrap_or_else(|| h.join("homebrew/data/SDH-AnimationChanger")),
            overrides: h.join(".steam/root/config/uioverrides/movies"),
            stock: h.join(".local/share/Steam/steamui/movies"),
            cache: env("DECKSHELL_ANIM_CACHE").unwrap_or_else(|| cache_base.join("deckshell/anim")),
            repo: env("DECKSHELL_ANIMREPO_CACHE").unwrap_or_else(|| cache_base.join("deckshell/animrepo")),
            picks_log: h.join(".local/state/deckshell/anim-picks.log"),
        }
    }

    /// Everything under one root (tests and fixtures).
    #[cfg(test)]
    pub fn under(root: &Path) -> Dirs {
        Dirs {
            own: root.join("own"),
            plugin_config: root.join("plugin/settings/config.json"),
            plugin_data: root.join("plugin/data"),
            overrides: root.join("overrides"),
            stock: root.join("stock"),
            cache: root.join("cache/anim"),
            repo: root.join("cache/animrepo"),
            picks_log: root.join("state/anim-picks.log"),
        }
    }
}

/// One selectable animation (an entry of local / downloaded / custom lists).
#[derive(Clone, Debug)]
pub struct Anim {
    pub id: String,
    pub name: String,
    pub target: String,
    pub source: &'static str,
}

#[derive(Clone, Debug)]
pub struct Extras {
    /// master switch for everything below
    pub enabled: bool,
    pub boot_on_resume: bool,
    /// animation id for shutdown / restart; "" = reuse the suspend slot
    pub shutdown: String,
    pub volume: f32,
    pub audio_latency_ms: i64,
    pub max_width: u32,
    pub max_height: u32,
    pub cache_budget_mb: u64,
    pub inhibit: bool,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { enabled: true, boot_on_resume: false, shutdown: String::new(), volume: 1.0, audio_latency_ms: 20, max_width: 1280, max_height: 800, cache_budget_mb: 3072, inhibit: true }
    }
}

/// Where an animation's file lives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// `~/.local/share/deckshell/animchanger/downloads`
    Own,
    /// only in the plugin's `~/homebrew/data/SDH-AnimationChanger/downloads` (read-only for DeckShell)
    Plugin,
    /// a folder set under `animations/`
    Local,
    /// `custom_animations` entry with its own path
    Custom,
    Missing,
}

#[derive(Default, Debug, PartialEq)]
pub struct Removed {
    pub deleted_file: bool,
    pub plugin_file_kept: bool,
    pub cleared_slots: Vec<&'static str>,
}

#[derive(Default, Debug, PartialEq)]
pub struct ImportReport {
    pub copied: usize,
    pub present: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes: u64,
    pub no_space: bool,
}

pub struct Config {
    pub v: Map<String, Value>,
    pub dirs: Dirs,
    pub local: Vec<Anim>,
    pub local_sets: Vec<Value>,
    pub x: Extras,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShuffleSlot {
    Boot,
    Suspend,
}

impl ShuffleSlot {
    const ALL: [Self; 2] = [Self::Boot, Self::Suspend];

    fn key(self) -> &'static str {
        match self {
            Self::Boot => "randomize_boot",
            Self::Suspend => "randomize_suspend",
        }
    }

    fn target(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Suspend => "suspend",
        }
    }

    fn bag_key(self) -> &'static str {
        match self {
            Self::Boot => "shuffle_bag_boot",
            Self::Suspend => "shuffle_bag_suspend",
        }
    }
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct ShuffleBag {
    remaining: Vec<String>,
    /// Drawn IDs stay here even if temporarily excluded during the round.
    drawn: Vec<String>,
    last: Option<String>,
}

impl ShuffleBag {
    fn draw(&mut self, pool: &[String], initial_previous: Option<&str>) -> Option<String> {
        let mut shuffle = self.remaining.is_empty();
        if !shuffle {
            // Keep the current round: drop unavailable clips and mix in newcomers.
            self.remaining.retain(|id| pool.binary_search(id).is_ok());
            for id in pool {
                if !self.drawn.contains(id) && !self.remaining.contains(id) {
                    self.remaining.push(id.clone());
                    shuffle = true;
                }
            }
        }
        if self.remaining.is_empty() {
            self.remaining.extend_from_slice(pool);
            self.drawn.clear();
            shuffle = true;
        }
        if shuffle {
            for i in (1..self.remaining.len()).rev() {
                self.remaining.swap(i, rand_below(i + 1));
            }
        }
        let previous = self.last.as_deref().or(initial_previous);
        if self.remaining.len() > 1 && self.remaining.last().map(String::as_str) == previous {
            let top = self.remaining.len() - 1;
            self.remaining.swap(top, rand_below(top));
        }
        let pick = self.remaining.pop()?;
        self.drawn.push(pick.clone());
        self.last = Some(pick.clone());
        Some(pick)
    }
}

fn defaults() -> Map<String, Value> {
    let v = json!({"boot": "", "suspend": "", "throbber": "", "current_set": "", "downloads": [], "custom_animations": [],
                   "custom_sets": [], "shuffle_exclusions": [], "force_ipv4": false});
    v.as_object().cloned().unwrap()
}

// ---------------------------------------------------------------- tiny RNG (no rand crate in the app)
fn rand_below(n: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static S: AtomicU64 = AtomicU64::new(0);
    let mut x = S.load(Ordering::Relaxed);
    if x == 0 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        x = t ^ ((std::process::id() as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
        if x == 0 {
            x = 1;
        }
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    S.store(x, Ordering::Relaxed);
    ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize) % n.max(1)
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, p)
}

impl Config {
    pub fn config_path(dirs: &Dirs) -> PathBuf {
        dirs.own.join("config.json")
    }

    /// Our config if present, else an import of the plugin's settings (saved immediately).
    pub fn load() -> Config {
        Self::load_from(Dirs::detect())
    }

    pub fn load_from(dirs: Dirs) -> Config {
        let mut c = Config { v: defaults(), dirs: dirs.clone(), local: vec![], local_sets: vec![], x: Extras::default() };
        let own = Self::config_path(&dirs);
        match std::fs::read(&own).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            Some(Value::Object(m)) => {
                let migrate_shuffle = m.contains_key("randomize");
                c.v.extend(m);
                c.fix_types();
                if migrate_shuffle {
                    c.save();
                }
            }
            _ => {
                let n = c.import_plugin();
                eprintln!("[anim] first run: imported {n} downloads from {}", dirs.plugin_config.display());
                c.save();
            }
        }
        c.read_extras();
        c.scan_local();
        c
    }

    fn fix_types(&mut self) {
        let legacy = self.v.remove("randomize");
        let enabled = legacy.as_ref().is_some_and(|v| {
            v.as_bool() == Some(true) || matches!(v.as_str(), Some("all" | "set"))
        });
        for slot in ShuffleSlot::ALL {
            if self.v.get(slot.key()).and_then(Value::as_bool).is_none() {
                self.set_shuffle(slot, enabled);
            }
        }
        for k in ["boot", "suspend", "throbber", "current_set"] {
            if !self.v.get(k).map(|x| x.is_string()).unwrap_or(false) {
                self.v.insert(k.into(), json!(""));
            }
        }
        for k in ["downloads", "custom_animations", "custom_sets", "shuffle_exclusions"] {
            if !self.v.get(k).map(|x| x.is_array()).unwrap_or(false) {
                self.v.insert(k.into(), json!([]));
            }
        }
    }

    fn read_extras(&mut self) {
        let d = self.v.get("deckshell").cloned().unwrap_or(Value::Null);
        let mut x = Extras::default();
        let b = |k: &str, def: bool| d.get(k).and_then(|v| v.as_bool()).unwrap_or(def);
        x.enabled = b("enabled", x.enabled);
        x.boot_on_resume = b("boot_on_resume", x.boot_on_resume);
        x.inhibit = b("inhibit", x.inhibit);
        x.shutdown = d.get("shutdown").and_then(|v| v.as_str()).unwrap_or("").to_string();
        x.volume = d.get("volume").and_then(|v| v.as_f64()).map(|v| v as f32).unwrap_or(x.volume).clamp(0.0, 2.0);
        x.audio_latency_ms = d.get("audio_latency_ms").and_then(|v| v.as_i64()).unwrap_or(x.audio_latency_ms);
        x.max_width = d.get("max_width").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(x.max_width).max(64);
        x.max_height = d.get("max_height").and_then(|v| v.as_u64()).map(|v| v as u32).unwrap_or(x.max_height).max(64);
        x.cache_budget_mb = d.get("cache_budget_mb").and_then(|v| v.as_u64()).unwrap_or(x.cache_budget_mb);
        self.x = x;
        self.write_extras();
    }

    /// Write the knobs back so the user can find them in the file; other keys of the object (favorites, hidden) stay.
    fn write_extras(&mut self) {
        let e = &self.x;
        let known = json!({"enabled": e.enabled, "boot_on_resume": e.boot_on_resume, "shutdown": e.shutdown, "volume": e.volume,
                   "audio_latency_ms": e.audio_latency_ms, "max_width": e.max_width, "max_height": e.max_height,
                   "cache_budget_mb": e.cache_budget_mb, "inhibit": e.inhibit});
        let mut obj = self.v.get("deckshell").and_then(|d| d.as_object()).cloned().unwrap_or_default();
        obj.extend(known.as_object().cloned().unwrap_or_default());
        self.v.insert("deckshell".into(), Value::Object(obj));
    }

    pub fn save(&self) {
        let bytes = serde_json::to_vec_pretty(&Value::Object(self.v.clone())).unwrap_or_default();
        if let Err(e) = write_atomic(&Self::config_path(&self.dirs), &bytes) {
            eprintln!("[anim] cannot save config: {e}");
        }
    }

    /// (Re-)import the plugin's config.json: `config.update(json)` like the plugin's load_config, plus downloads found on
    /// disk but missing from its list. DeckShell-only knobs are kept. Returns the number of downloads.
    pub fn import_plugin(&mut self) -> usize {
        let extras = self.v.get("deckshell").cloned();
        let mut m = defaults();
        if let Some(Value::Object(p)) = std::fs::read(&self.dirs.plugin_config).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
            m.extend(p);
        } else {
            eprintln!("[anim] no plugin config at {}", self.dirs.plugin_config.display());
        }
        for slot in ShuffleSlot::ALL {
            let key = slot.bag_key();
            m.remove(key);
            if let Some(bag) = self.v.remove(key) {
                m.insert(key.into(), bag);
            }
        }
        self.v = m;
        if let Some(e) = extras {
            self.v.insert("deckshell".into(), e);
        }
        self.fix_types();
        self.sync_downloads();
        self.downloads().len()
    }

    /// Add downloads present on disk but unknown to the list (no metadata -> boot target, id as name); drop nothing.
    /// Files the user removed from the list (`deckshell.hidden`, plugin-owned files DeckShell may not delete) stay out.
    pub fn sync_downloads(&mut self) {
        let mut known: Vec<String> = self.downloads().iter().map(|d| str_of(d, "id")).collect();
        known.extend(self.id_list("hidden"));
        let mut add = vec![];
        for dir in self.download_dirs() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect();
            names.sort();
            for n in names {
                if let Some(id) = n.strip_suffix(".webm") {
                    if !known.iter().any(|k| k == id) && !add.iter().any(|a: &Value| str_of(a, "id") == id) {
                        add.push(json!({"id": id, "name": id, "author": "", "target": "boot", "version": "", "manifest_version": 1, "source": "disk"}));
                    }
                }
            }
        }
        if !add.is_empty() {
            eprintln!("[anim] {} downloaded files not in the list added", add.len());
            if let Some(Value::Array(a)) = self.v.get_mut("downloads") {
                a.extend(add);
            }
        }
    }

    pub fn download_dirs(&self) -> Vec<PathBuf> {
        vec![self.dirs.own.join("downloads"), self.dirs.plugin_data.join("downloads")]
    }
    fn anim_dirs(&self) -> Vec<PathBuf> {
        vec![self.dirs.own.join("animations"), self.dirs.plugin_data.join("animations")]
    }

    pub fn downloads(&self) -> Vec<Value> {
        self.v.get("downloads").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    }
    fn custom_animations(&self) -> Vec<Value> {
        self.v.get("custom_animations").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    }
    fn custom_sets(&self) -> Vec<Value> {
        self.v.get("custom_sets").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    }
    pub fn exclusions(&self) -> Vec<String> {
        self.v.get("shuffle_exclusions").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    }
    pub fn force_ipv4(&self) -> bool {
        self.v.get("force_ipv4").and_then(|x| x.as_bool()).unwrap_or(false)
    }
    pub fn get(&self, k: &str) -> String {
        self.v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    }
    pub fn set(&mut self, k: &str, val: &str) {
        self.v.insert(k.into(), json!(val));
    }

    /// Port of `load_local_animations`.
    pub fn scan_local(&mut self) {
        let mut anims: Vec<Anim> = vec![];
        let mut sets: Vec<Value> = vec![];
        for base in self.anim_dirs() {
            let Ok(rd) = std::fs::read_dir(&base) else { continue };
            let mut dirs: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(String::from)).collect();
            dirs.sort();
            for dir in dirs {
                if sets.iter().any(|s| str_of(s, "id") == dir) {
                    continue; // own dir shadows the plugin's
                }
                let cfg_path = base.join(&dir).join("config.json");
                let mut anim_config = Value::Object(Map::new());
                let mut is_set = false;
                if cfg_path.exists() {
                    match std::fs::read(&cfg_path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                        Some(v) => {
                            anim_config = v;
                            is_set = true;
                        }
                        None => eprintln!("[anim] cannot parse {}", cfg_path.display()),
                    }
                } else {
                    is_set = VIDEO_NAMES.iter().any(|n| base.join(&dir).join(n).exists());
                }
                if !is_set {
                    continue;
                }
                let mut set = json!({"id": dir, "enabled": anim_config.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true), "local": true});
                for i in 0..3 {
                    let t = VIDEO_TYPES[i];
                    let has = anim_config.get(t).is_some();
                    let mut filename = if has { str_of(&anim_config, t) } else { VIDEO_NAMES[i].to_string() };
                    if !has && !base.join(&dir).join(&filename).exists() {
                        filename.clear();
                    }
                    set[t] = json!(filename);
                    if !filename.is_empty() {
                        let name = if t == "boot" { dir.clone() } else { format!("{dir} - {}", cap(t)) };
                        anims.push(Anim { id: format!("{dir}/{filename}"), name, target: VIDEO_TARGETS[i].into(), source: "local" });
                    }
                }
                sets.push(set);
            }
        }
        self.local = anims;
        self.local_sets = sets;
    }

    /// All animations the user can pick from, in the plugin's pool order (local, downloads, custom).
    pub fn all(&self) -> Vec<Anim> {
        let mut v = self.local.clone();
        for d in self.downloads() {
            v.push(Anim { id: str_of(&d, "id"), name: str_of(&d, "name"), target: str_of(&d, "target"), source: "download" });
        }
        for d in self.custom_animations() {
            v.push(Anim { id: str_of(&d, "id"), name: str_of(&d, "name"), target: str_of(&d, "target"), source: "custom" });
        }
        v
    }

    /// Port of `apply_animation`'s lookup: downloads -> custom -> local. None = empty id / unknown id / missing file.
    pub fn resolve(&self, id: &str) -> Option<PathBuf> {
        if id.is_empty() {
            return None;
        }
        if self.downloads().iter().any(|d| str_of(d, "id") == id) {
            return self.download_dirs().into_iter().map(|d| d.join(format!("{id}.webm"))).find(|p| p.is_file());
        }
        if let Some(c) = self.custom_animations().iter().find(|d| str_of(d, "id") == id) {
            let p = PathBuf::from(str_of(c, "path"));
            return p.is_file().then_some(p);
        }
        if self.local.iter().any(|a| a.id == id) {
            return self.anim_dirs().into_iter().map(|d| d.join(id)).find(|p| p.is_file());
        }
        None
    }

    pub fn name_of(&self, id: &str) -> String {
        self.all().into_iter().find(|a| a.id == id).map(|a| a.name).unwrap_or_default()
    }

    pub fn shuffle_enabled(&self, slot: ShuffleSlot) -> bool {
        self.v.get(slot.key()).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn set_shuffle(&mut self, slot: ShuffleSlot, enabled: bool) {
        self.v.insert(slot.key().into(), json!(enabled));
    }

    /// Advances the bag in memory. Save it together with any changes to the selected slots.
    pub fn shuffled(&mut self, slot: ShuffleSlot) -> Option<String> {
        self.shuffle_enabled(slot).then(|| self.draw_from_bag(slot)).flatten()
    }

    fn shuffle_pool(&self, target: &str) -> Vec<String> {
        let exclusions = self.exclusions();
        let mut pool: Vec<_> = self.all().into_iter()
            .filter(|a| a.target == target && !exclusions.contains(&a.id) && self.resolve(&a.id).is_some())
            .map(|a| a.id)
            .collect();
        pool.sort_unstable();
        pool.dedup();
        pool
    }

    fn draw_from_bag(&mut self, slot: ShuffleSlot) -> Option<String> {
        let key = slot.bag_key();
        let mut bag: ShuffleBag = self.v.get(key).cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        let pool = self.shuffle_pool(slot.target());
        // On migration, the configured boot clip already plays during this launch.
        let initial_previous = match slot {
            ShuffleSlot::Boot => self.v.get("boot").and_then(Value::as_str),
            ShuffleSlot::Suspend => None,
        };
        let pick = bag.draw(&pool, initial_previous);
        self.log_pick(slot, pick.as_deref(), pool.len(), bag.remaining.len());
        self.v.insert(key.into(), json!(bag));
        pick
    }

    fn log_pick(&self, slot: ShuffleSlot, id: Option<&str>, bag_size: usize, remaining: usize) {
        let small_pool = slot == ShuffleSlot::Suspend && bag_size < 2;
        if id.is_none() && !small_pool {
            return;
        }
        let mut entry = json!({
            "time": chrono::Utc::now().to_rfc3339(),
            "slot": slot.target(),
            "id": id,
            "bag_size": bag_size,
            "remaining": remaining,
        });
        if small_pool {
            entry["warning"] = json!("пул слишком мал");
        }
        let path = &self.dirs.picks_log;
        let result = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
            .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(path))
            .and_then(|mut file| file.write_all(format!("{entry}\n").as_bytes()));
        if let Err(e) = result {
            eprintln!("[anim] cannot append to {}: {e}", path.display());
        }
    }

    /// Ручная команда перемешивания из управляющего сокета.
    pub fn randomize_all(&mut self) {
        for slot in ShuffleSlot::ALL {
            if let Some(pick) = self.draw_from_bag(slot) {
                self.set(slot.target(), &pick);
            }
        }
        let throbber_pool = self.shuffle_pool("suspend");
        if let Some(pick) = throbber_pool.get(rand_below(throbber_pool.len())) {
            self.set("throbber", pick);
        }
        self.set("current_set", "");
    }

    /// Port of `randomize_current_set` (the plugin's version raises a TypeError for sets, see docs): a random *enabled* set
    /// provides all three slots. Custom sets hold animation objects `{id,..}` per type, local sets hold file names.
    pub fn randomize_set(&mut self) {
        let mut sets: Vec<Value> = self.local_sets.clone();
        sets.extend(self.custom_sets());
        let active: Vec<&Value> = sets.iter().filter(|s| s.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true)).collect();
        if active.is_empty() {
            return;
        }
        let pick = active[rand_below(active.len())].clone();
        self.set("current_set", &str_of(&pick, "id"));
        let is_local = pick.get("local").and_then(|l| l.as_bool()).unwrap_or(false);
        for t in VIDEO_TYPES {
            let id = match pick.get(t) {
                Some(Value::Object(o)) => o.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                Some(Value::String(s)) if is_local && !s.is_empty() => format!("{}/{s}", str_of(&pick, "id")),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            self.set(t, &id);
        }
    }

    // ------------------------------------------------------------ page operations (task 412)

    /// String list stored under `deckshell.<key>` (favorites, hidden).
    pub fn id_list(&self, key: &str) -> Vec<String> {
        self.v.get("deckshell").and_then(|d| d.get(key)).and_then(|l| l.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    fn set_id_list(&mut self, key: &str, list: Vec<String>) {
        if !self.v.get("deckshell").map(|d| d.is_object()).unwrap_or(false) {
            self.v.insert("deckshell".into(), json!({}));
        }
        if let Some(d) = self.v.get_mut("deckshell").and_then(|d| d.as_object_mut()) {
            d.insert(key.into(), json!(list));
        }
    }

    fn toggle_in(list: &mut Vec<String>, id: &str) -> bool {
        if let Some(i) = list.iter().position(|x| x == id) {
            list.remove(i);
            false
        } else {
            list.push(id.to_string());
            true
        }
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.id_list("favorites").iter().any(|x| x == id)
    }

    /// Returns the new state.
    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        let mut l = self.id_list("favorites");
        let on = Self::toggle_in(&mut l, id);
        self.set_id_list("favorites", l);
        on
    }

    pub fn is_excluded(&self, id: &str) -> bool {
        self.exclusions().iter().any(|x| x == id)
    }

    /// The plugin's `shuffle_exclusions`; returns true when `id` is now excluded.
    pub fn toggle_exclusion(&mut self, id: &str) -> bool {
        let mut l = self.exclusions();
        let on = Self::toggle_in(&mut l, id);
        self.v.insert("shuffle_exclusions".into(), json!(l));
        on
    }

    pub fn set_extra(&mut self, f: impl FnOnce(&mut Extras)) {
        f(&mut self.x);
        self.write_extras();
    }

    /// Metadata entry of a downloaded or custom animation.
    pub fn entry(&self, id: &str) -> Option<Value> {
        self.downloads().into_iter().chain(self.custom_animations()).find(|d| str_of(d, "id") == id)
    }

    pub fn own_download(&self, id: &str) -> PathBuf {
        self.dirs.own.join("downloads").join(format!("{id}.webm"))
    }

    pub fn plugin_download(&self, id: &str) -> PathBuf {
        self.dirs.plugin_data.join("downloads").join(format!("{id}.webm"))
    }

    pub fn origin(&self, id: &str) -> Origin {
        if self.downloads().iter().any(|d| str_of(d, "id") == id) {
            if self.own_download(id).is_file() {
                Origin::Own
            } else if self.plugin_download(id).is_file() {
                Origin::Plugin
            } else {
                Origin::Missing
            }
        } else if self.custom_animations().iter().any(|d| str_of(d, "id") == id) {
            Origin::Custom
        } else if self.local.iter().any(|a| a.id == id) {
            Origin::Local
        } else {
            Origin::Missing
        }
    }

    /// Record a finished download (replaces an entry with the same id, un-hides it).
    pub fn add_download(&mut self, entry: Value) {
        let id = str_of(&entry, "id");
        let mut list: Vec<Value> = self.downloads().into_iter().filter(|d| str_of(d, "id") != id).collect();
        list.push(entry);
        self.v.insert("downloads".into(), Value::Array(list));
        let hidden: Vec<String> = self.id_list("hidden").into_iter().filter(|h| *h != id).collect();
        self.set_id_list("hidden", hidden);
    }

    /// Remove a downloaded or custom animation from the lists. DeckShell's own file is deleted; the plugin's file in
    /// `~/homebrew` is never touched (it is hidden instead so the rescan does not bring it back). Slots that used it fall back to
    /// the default (the plugin left them dangling) and it leaves favorites and exclusions.
    pub fn remove(&mut self, id: &str) -> Result<Removed, String> {
        let origin = self.origin(id);
        if origin == Origin::Local {
            return Err("Анимация из папки набора: удалите папку вручную".into());
        }
        let mut r = Removed::default();
        let own = self.own_download(id);
        if own.is_file() {
            std::fs::remove_file(&own).map_err(|e| format!("Не удалось удалить файл: {e}"))?;
            r.deleted_file = true;
        }
        if self.plugin_download(id).is_file() {
            r.plugin_file_kept = true;
            let mut hidden = self.id_list("hidden");
            if !hidden.iter().any(|h| h == id) {
                hidden.push(id.to_string());
            }
            self.set_id_list("hidden", hidden);
        }
        let downloads: Vec<Value> = self.downloads().into_iter().filter(|d| str_of(d, "id") != id).collect();
        self.v.insert("downloads".into(), Value::Array(downloads));
        let custom: Vec<Value> = self.custom_animations().into_iter().filter(|d| str_of(d, "id") != id).collect();
        self.v.insert("custom_animations".into(), Value::Array(custom));
        for slot in VIDEO_TYPES {
            if self.get(slot) == id {
                self.set(slot, "");
                r.cleared_slots.push(slot);
            }
        }
        if self.x.shutdown == id {
            self.set_extra(|x| x.shutdown.clear());
            r.cleared_slots.push("shutdown");
        }
        let fav: Vec<String> = self.id_list("favorites").into_iter().filter(|f| f != id).collect();
        self.set_id_list("favorites", fav);
        let ex: Vec<String> = self.exclusions().into_iter().filter(|f| f != id).collect();
        self.v.insert("shuffle_exclusions".into(), json!(ex));
        Ok(r)
    }

    /// Copy the plugin's downloaded files into DeckShell's own folder and take over their catalog metadata, so they stay
    /// when the plugin is removed. Existing own files are kept; nothing in `~/homebrew` is changed. Settings are not touched.
    pub fn import_files(&mut self, free_space: &dyn Fn(&Path) -> Option<u64>) -> ImportReport {
        let mut rep = ImportReport::default();
        let plugin_meta: Vec<Value> = std::fs::read(&self.dirs.plugin_config)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("downloads").and_then(|d| d.as_array()).cloned())
            .unwrap_or_default();
        let dir = self.dirs.plugin_data.join("downloads");
        let mut names: Vec<String> = std::fs::read_dir(&dir).map(|rd| rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect()).unwrap_or_default();
        names.sort();
        let dest_dir = self.dirs.own.join("downloads");
        let hidden = self.id_list("hidden");
        for n in names {
            let Some(id) = n.strip_suffix(".webm").map(String::from) else { continue };
            if hidden.contains(&id) {
                rep.skipped += 1;
                continue;
            }
            let src = dir.join(&n);
            let dest = self.own_download(&id);
            if !dest.is_file() {
                let len = src.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = std::fs::create_dir_all(&dest_dir);
                if free_space(&dest_dir).is_some_and(|f| f < len + (64 << 20)) {
                    rep.no_space = true;
                    break;
                }
                let tmp = dest.with_extension("webm.part");
                match std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dest)) {
                    Ok(()) => {
                        rep.copied += 1;
                        rep.bytes += len;
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp);
                        eprintln!("[anim] import {id}: {e}");
                        rep.failed += 1;
                        continue;
                    }
                }
            } else {
                rep.present += 1;
            }
            let current = self.entry(&id);
            let stub = current.as_ref().map(|c| str_of(c, "author").is_empty() && str_of(c, "name") == id).unwrap_or(true);
            if stub {
                let meta = plugin_meta.iter().find(|m| str_of(m, "id") == id).cloned().unwrap_or_else(|| {
                    json!({"id": id, "name": id, "author": "", "target": "boot", "version": "", "manifest_version": 1, "source": "disk"})
                });
                self.add_download(meta);
            }
        }
        rep
    }

    /// Take over the plugin's choices (slots, shuffle mode, exclusions, IPv4); lists and DeckShell knobs stay as they are.
    pub fn import_plugin_settings(&mut self) -> bool {
        let Some(Value::Object(p)) = std::fs::read(&self.dirs.plugin_config).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return false };
        if p.contains_key("randomize") {
            for slot in ShuffleSlot::ALL {
                self.v.remove(slot.key());
            }
        }
        for k in ["boot", "suspend", "throbber", "randomize", "randomize_boot", "randomize_suspend", "current_set", "shuffle_exclusions", "force_ipv4"] {
            if let Some(v) = p.get(k) {
                self.v.insert(k.into(), v.clone());
            }
        }
        self.fix_types();
        true
    }

    /// Slot id for a playback kind ("boot" | "suspend" | "throbber" | "shutdown").
    pub fn slot(&self, kind: &str) -> String {
        match kind {
            "shutdown" => {
                if self.x.shutdown.is_empty() {
                    self.get("suspend")
                } else {
                    self.x.shutdown.clone()
                }
            }
            k => self.get(k),
        }
    }

    pub fn is_oled() -> bool {
        std::fs::read_to_string("/sys/class/dmi/id/product_name").map(|s| s.trim() == "Galileo").unwrap_or(false)
    }

    /// Steam's behaviour for an empty slot: `uioverrides/movies/<name>` first, then the stock movie of this device
    /// (sp.js `tt()`: Deck LCD `deck-suspend-animation*`, OLED `oled-suspend-animation*`, boot `deck_startup` / `oled_startup`).
    pub fn fallback(&self, kind: &str) -> Option<PathBuf> {
        let i = match kind {
            "boot" => 0,
            "throbber" => 2,
            _ => 1,
        };
        let o = self.dirs.overrides.join(VIDEO_NAMES[i]);
        if o.is_file() {
            return Some(o);
        }
        let oled = Self::is_oled();
        let stock = match (i, oled) {
            (0, true) => "oled_startup.webm",
            (0, false) => "deck_startup.webm",
            (1, true) => "oled-suspend-animation.webm",
            (1, false) => "deck-suspend-animation.webm",
            (_, true) => "oled-suspend-animation-from-throbber.webm",
            (_, false) => "deck-suspend-animation-from-throbber.webm",
        };
        let p = self.dirs.stock.join(stock);
        p.is_file().then_some(p)
    }

    /// Final file for a kind: slot -> fallback.
    pub fn path_for(&self, kind: &str) -> Option<PathBuf> {
        self.resolve(&self.slot(kind)).or_else(|| self.fallback(kind))
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config(name: &str) -> Config {
        let root = std::env::temp_dir().join(format!("anim-bag-{name}-{}", std::process::id()));
        Config { v: defaults(), dirs: Dirs::under(&root), local: vec![], local_sets: vec![], x: Extras::default() }
    }

    fn add_clips(c: &mut Config, target: &str, ids: &[&str]) {
        std::fs::create_dir_all(c.dirs.own.join("downloads")).unwrap();
        for id in ids {
            std::fs::write(c.own_download(id), b"x").unwrap();
            c.add_download(json!({"id": id, "target": target}));
        }
    }

    #[test]
    fn randomize_all_pools_by_target() {
        let mut c = empty_config("manual");
        add_clips(&mut c, "boot", &["a", "b"]);
        add_clips(&mut c, "suspend", &["s"]);
        c.set("current_set", "x");
        c.v.insert("shuffle_exclusions".into(), json!(["b"]));
        for _ in 0..20 {
            c.randomize_all();
            assert_eq!(c.get("boot"), "a"); // b excluded
            assert_eq!(c.get("suspend"), "s");
            assert_eq!(c.get("throbber"), "s"); // throbber draws from the suspend pool
            assert_eq!(c.get("current_set"), "");
        }
        std::fs::remove_dir_all(c.dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn empty_pool_keeps_slot() {
        let mut c = empty_config("empty");
        c.set("boot", "keep");
        c.randomize_all();
        assert_eq!(c.get("boot"), "keep");
        std::fs::remove_dir_all(c.dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn set_slots() {
        let mut c = empty_config("set");
        c.v.insert("custom_sets".into(), json!([{"id":"s1","enabled":true,"boot":{"id":"B"},"suspend":{"id":"S"},"throbber":{"id":"T"}},{"id":"off","enabled":false,"boot":{"id":"X"}}]));
        c.randomize_set();
        assert_eq!((c.get("boot"), c.get("suspend"), c.get("throbber"), c.get("current_set")), ("B".into(), "S".into(), "T".into(), "s1".into()));
    }

    /// A plugin layout like the owner's Deck: config with two downloads + a file without metadata.
    pub fn plugin_fixture(name: &str) -> Dirs {
        let root = std::env::temp_dir().join(format!("anim-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let d = Dirs::under(&root);
        std::fs::create_dir_all(d.plugin_data.join("downloads")).unwrap();
        std::fs::create_dir_all(d.plugin_config.parent().unwrap()).unwrap();
        for (id, len) in [("PDkKY", 3000), ("ndbjY", 1000), ("Extra", 500)] {
            let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
            v.resize(len, 1);
            std::fs::write(d.plugin_data.join("downloads").join(format!("{id}.webm")), v).unwrap();
        }
        std::fs::write(&d.plugin_config, serde_json::to_vec(&json!({
            "boot": "PDkKY", "suspend": "ndbjY", "throbber": "ndbjY", "randomize": "all", "current_set": "", "force_ipv4": true,
            "custom_animations": [], "custom_sets": [], "shuffle_exclusions": [],
            "downloads": [
                {"id": "PDkKY", "name": "Playstation 2", "author": "Parimak", "target": "boot", "likes": 262, "downloads": 60373, "last_changed": "2022-10-11T21:25:37.000000Z"},
                {"id": "ndbjY", "name": "minimal glitch logo suspend", "author": "x", "target": "suspend", "likes": 1, "downloads": 2, "last_changed": "2023-01-01T00:00:00.000000Z"}
            ]})).unwrap()).unwrap();
        d
    }

    #[test]
    fn legacy_shuffle_migrates_and_explicit_switches_survive_reload() {
        let dirs = plugin_fixture("shuffle-migration");
        let path = Config::config_path(&dirs);
        for (legacy, enabled) in [(json!(true), true), (json!(false), false), (json!("all"), true), (json!("set"), true), (json!(""), false)] {
            write_atomic(&path, &serde_json::to_vec(&json!({
                "randomize": legacy, "shuffle_exclusions": ["ndbjY"]
            })).unwrap()).unwrap();
            let mut c = Config::load_from(dirs.clone());
            assert_eq!(c.shuffle_enabled(ShuffleSlot::Boot), enabled);
            assert_eq!(c.shuffle_enabled(ShuffleSlot::Suspend), enabled);
            assert!(c.is_excluded("ndbjY"));
            c.set_shuffle(ShuffleSlot::Boot, false);
            c.set_shuffle(ShuffleSlot::Suspend, true);
            c.save();
            let reloaded = Config::load_from(dirs.clone());
            assert!(!reloaded.shuffle_enabled(ShuffleSlot::Boot));
            assert!(reloaded.shuffle_enabled(ShuffleSlot::Suspend));
            assert!(!reloaded.v.contains_key("randomize"));
            assert!(reloaded.is_excluded("ndbjY"));
        }
        write_atomic(&path, br#"{"randomize":"all","randomize_boot":false}"#).unwrap();
        let mut c = Config::load_from(dirs.clone());
        assert!(!c.shuffle_enabled(ShuffleSlot::Boot));
        assert!(c.shuffle_enabled(ShuffleSlot::Suspend));
        let plugin = br#"{"randomize":"set","shuffle_exclusions":["PDkKY"]}"#;
        write_atomic(&dirs.plugin_config, plugin).unwrap();
        assert!(c.import_plugin_settings());
        assert!(c.shuffle_enabled(ShuffleSlot::Boot) && c.shuffle_enabled(ShuffleSlot::Suspend));
        assert!(c.is_excluded("PDkKY"));
        assert_eq!(std::fs::read(&dirs.plugin_config).unwrap(), plugin);
        std::fs::remove_dir_all(dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn automatic_shuffle_only_picks_for_its_enabled_slot() {
        use super::super::{sleep_pick, Kind};
        let dirs = plugin_fixture("shuffle-selection");
        let mut c = Config::load_from(dirs.clone());
        c.set("boot", "ручной запуск");
        c.set("suspend", "ручной сон");
        c.set("throbber", "сон из загрузки");
        c.toggle_exclusion("Extra");
        c.set_shuffle(ShuffleSlot::Suspend, false);
        assert_eq!(c.shuffled(ShuffleSlot::Boot).as_deref(), Some("PDkKY"));
        assert_eq!(sleep_pick(&mut c, Kind::Suspend), None);
        c.set_shuffle(ShuffleSlot::Boot, false);
        c.set_shuffle(ShuffleSlot::Suspend, true);
        assert_eq!(c.shuffled(ShuffleSlot::Boot), None);
        assert_eq!(sleep_pick(&mut c, Kind::Suspend).as_deref(), Some("ndbjY"));
        assert_eq!(sleep_pick(&mut c, Kind::Throbber), None);
        assert_eq!(sleep_pick(&mut c, Kind::Shutdown), None);
        assert_eq!(c.slot("shutdown"), "ручной сон");
        assert_eq!(c.get("throbber"), "сон из загрузки");
        assert_eq!(c.get("boot"), "ручной запуск");
        c.toggle_exclusion("ndbjY");
        assert_eq!(sleep_pick(&mut c, Kind::Suspend), None);
        assert_eq!(c.get("suspend"), "ручной сон");

        let entries: Vec<Value> = std::fs::read_to_string(&dirs.picks_log).unwrap()
            .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].get("warning").is_none());
        for (entry, id, size) in [(&entries[1], json!("ndbjY"), 1), (&entries[2], Value::Null, 0)] {
            assert_eq!(entry["slot"], "suspend");
            assert_eq!(entry["id"], id);
            assert_eq!(entry["bag_size"], size);
            assert_eq!(entry["warning"], "пул слишком мал");
            assert!(chrono::DateTime::parse_from_rfc3339(entry["time"].as_str().unwrap()).is_ok());
        }
        std::fs::remove_dir_all(dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn shuffle_bags_balance_draws_and_avoid_repeats_across_restarts() {
        use super::super::{sleep_pick, Kind};
        use std::collections::{HashMap, HashSet};

        let mut c = empty_config("distribution");
        let boot = ["a", "b", "c", "d", "e"];
        let suspend = ["x", "y"];
        add_clips(&mut c, "boot", &boot);
        add_clips(&mut c, "suspend", &suspend);
        c.set_shuffle(ShuffleSlot::Boot, true);
        c.set_shuffle(ShuffleSlot::Suspend, true);
        c.save();

        let mut picks = [Vec::new(), Vec::new()];
        for _ in 0..53 {
            let mut c = Config::load_from(c.dirs.clone());
            let next_boot = c.shuffled(ShuffleSlot::Boot).unwrap();
            c.set("boot", &next_boot);
            c.save();
            picks[0].push(next_boot);
            picks[1].push(sleep_pick(&mut c, Kind::Suspend).unwrap());
        }
        for (picks, pool) in picks.iter().zip([boot.as_slice(), suspend.as_slice()]) {
            assert!(picks.windows(2).all(|pair| pair[0] != pair[1]));
            let mut counts = HashMap::new();
            for cycle in picks.chunks(pool.len()) {
                assert_eq!(cycle.iter().collect::<HashSet<_>>().len(), cycle.len());
                for id in cycle {
                    assert!(pool.contains(&id.as_str()));
                    *counts.entry(id).or_insert(0) += 1;
                }
            }
            assert_eq!(counts.len(), pool.len());
            assert!(counts.values().max().unwrap() - counts.values().min().unwrap() <= 1);
        }
        std::fs::remove_dir_all(c.dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn config_without_bags_initializes_on_first_draw() {
        let mut c = empty_config("bag-migration");
        add_clips(&mut c, "boot", &["current", "next"]);
        c.set("boot", "current");
        c.set_shuffle(ShuffleSlot::Boot, true);
        c.save();
        let dirs = c.dirs.clone();
        let mut c = Config::load_from(dirs.clone());
        assert!(!c.v.contains_key("shuffle_bag_boot"));
        assert_eq!(c.shuffled(ShuffleSlot::Boot).as_deref(), Some("next"));
        c.save();
        let mut c = Config::load_from(dirs.clone());
        assert_eq!(c.get("boot"), "current");
        assert!(!c.v.contains_key("shuffle_bag_suspend"));
        assert_eq!(c.shuffled(ShuffleSlot::Boot).as_deref(), Some("current"));
        std::fs::remove_dir_all(dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn shuffle_bag_reconciles_added_missing_and_excluded_clips() {
        let mut c = empty_config("pool-changes");
        let slot = ShuffleSlot::Boot;
        add_clips(&mut c, "boot", &["a", "b", "c", "d"]);
        c.set_shuffle(slot, true);
        let first = c.shuffled(slot).unwrap();
        let bag: ShuffleBag = serde_json::from_value(c.v[slot.bag_key()].clone()).unwrap();
        let [deleted, excluded, kept] = bag.remaining.as_slice() else { panic!("expected three pending clips") };
        std::fs::remove_file(c.own_download(deleted)).unwrap();
        c.toggle_exclusion(excluded);
        c.toggle_exclusion(&first);
        add_clips(&mut c, "boot", &["new"]);
        c.save();

        let mut c = Config::load_from(c.dirs);
        let second = c.shuffled(slot).unwrap();
        c.toggle_exclusion(&first);
        let third = c.shuffled(slot).unwrap();
        assert_ne!(second, third);
        assert!(second == *kept || second == "new");
        assert!(third == *kept || third == "new");
        c.toggle_exclusion(excluded);
        let mut next_cycle: Vec<_> = (0..4).map(|_| c.shuffled(slot).unwrap()).collect();
        assert_ne!(next_cycle[0], third);
        next_cycle.sort();
        let mut expected = vec![first, excluded.clone(), kept.clone(), "new".into()];
        expected.sort();
        assert_eq!(next_cycle, expected);
        std::fs::remove_dir_all(c.dirs.own.parent().unwrap()).unwrap();
    }

    #[test]
    fn first_run_imports_and_keeps_page_lists_across_reload() {
        let d = plugin_fixture("reload");
        let mut c = Config::load_from(d.clone());
        assert_eq!(c.downloads().len(), 3, "two with metadata + one found on disk");
        assert_eq!(c.get("boot"), "PDkKY");
        assert!(c.force_ipv4());
        assert!(c.shuffle_enabled(ShuffleSlot::Boot));
        assert!(c.shuffle_enabled(ShuffleSlot::Suspend));
        assert!(c.toggle_favorite("PDkKY"));
        assert!(c.toggle_exclusion("Extra"));
        c.set_extra(|x| { x.boot_on_resume = true; x.volume = 0.5; });
        c.save();
        let c2 = Config::load_from(d.clone());
        assert!(c2.is_favorite("PDkKY") && c2.is_excluded("Extra"));
        assert!(c2.x.boot_on_resume && (c2.x.volume - 0.5).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(d.own.parent().unwrap());
    }

    #[test]
    fn import_copies_plugin_files_once_and_never_touches_homebrew() {
        let d = plugin_fixture("import");
        let mut c = Config::load_from(d.clone());
        assert_eq!(c.origin("PDkKY"), Origin::Plugin);
        let r = c.import_files(&|_| Some(1 << 40));
        assert_eq!((r.copied, r.present, r.failed, r.no_space), (3, 0, 0, false));
        assert_eq!(r.bytes, 4500);
        assert_eq!(c.origin("PDkKY"), Origin::Own);
        assert!(c.plugin_download("PDkKY").is_file(), "the plugin's file stays");
        assert_eq!(c.entry("PDkKY").unwrap()["author"], json!("Parimak"));
        let again = c.import_files(&|_| Some(1 << 40));
        assert_eq!((again.copied, again.present), (0, 3));
        // not enough room: stops, nothing half-copied
        let d2 = plugin_fixture("import-full");
        let mut c2 = Config::load_from(d2.clone());
        let full = c2.import_files(&|_| Some(1000));
        assert!(full.no_space);
        assert_eq!(full.copied, 0);
        assert!(std::fs::read_dir(d2.own.join("downloads")).map(|r| r.count()).unwrap_or(0) == 0);
        for x in [d, d2] {
            let _ = std::fs::remove_dir_all(x.own.parent().unwrap());
        }
    }

    #[test]
    fn remove_clears_slots_and_hides_plugin_files() {
        let d = plugin_fixture("remove");
        let mut c = Config::load_from(d.clone());
        c.toggle_favorite("ndbjY");
        c.set_extra(|x| x.shutdown = "ndbjY".into());
        // plugin-only file: entry goes, file stays, rescans do not bring it back
        let r = c.remove("ndbjY").unwrap();
        assert_eq!(r, Removed { deleted_file: false, plugin_file_kept: true, cleared_slots: vec!["suspend", "throbber", "shutdown"] });
        assert!(c.plugin_download("ndbjY").is_file());
        assert!(c.entry("ndbjY").is_none() && !c.is_favorite("ndbjY"));
        c.sync_downloads();
        assert!(c.entry("ndbjY").is_none(), "hidden");
        // own copy: really deleted
        c.import_files(&|_| Some(1 << 40));
        assert_eq!(c.origin("PDkKY"), Origin::Own);
        let r = c.remove("PDkKY").unwrap();
        assert!(r.deleted_file && r.plugin_file_kept);
        assert_eq!(c.get("boot"), "");
        assert!(!c.own_download("PDkKY").exists());
        // downloading again un-hides
        c.add_download(json!({"id": "ndbjY", "name": "again", "target": "suspend"}));
        assert!(!c.id_list("hidden").contains(&"ndbjY".to_string()));
        assert_eq!(c.path_for("suspend"), None.or(c.fallback("suspend")), "slot stays default until chosen");
        let _ = std::fs::remove_dir_all(d.own.parent().unwrap());
    }
}
