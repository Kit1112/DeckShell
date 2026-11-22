//! Runtime access to the user's own Steam assets (icons, glyphs, localization).
//!
//! Nothing here is committed to the repo: icons live in `~/.cache/deckshell/assets`
//! (produced by `tools/extract-assets/extract.mjs`), glyphs are read in place from
//! `~/.local/share/Steam/controller_base/images/api`, strings from `steamui/localization`.
//! Every lookup degrades gracefully (None / token text) when Steam files are absent.

use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
/// Steam install root (`DECKSHELL_STEAM` overrides).
pub fn steam_root() -> PathBuf {
    std::env::var_os("DECKSHELL_STEAM").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share/Steam"))
}
/// Asset cache dir (`DECKSHELL_ASSETS` overrides).
pub fn cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("DECKSHELL_ASSETS") {
        return p.into();
    }
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"));
    base.join("deckshell/assets")
}

// ---------------------------------------------------------------- localization

pub mod i18n;

/// Steam bundle name for the effective DeckShell locale.
pub fn language() -> String {
    i18n::current().steam_name().into()
}

fn registry_language(paths: &[PathBuf]) -> Option<String> {
    paths.iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        text.lines().find_map(|line| {
            let mut fields = line.split('"');
            fields.next()?;
            let key = fields.next()?;
            fields.next()?;
            let value = fields.next()?.trim();
            (key.eq_ignore_ascii_case("language") && !value.is_empty()).then(|| value.to_owned())
        })
    })
}

/// Decode the JS string passed to JSON.parse without executing the bundle.
fn localization_json(source: &str) -> Option<String> {
    let start = source.find("JSON.parse(")? + "JSON.parse(".len();
    let input = source[start..].trim_start();
    let quote = input.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') { return None; }
    let mut chars = input[quote.len_utf8()..].chars().peekable();
    let mut units = Vec::<u16>::new();
    while let Some(c) = chars.next() {
        if c == quote {
            if chars.find(|c| !c.is_whitespace()) != Some(')') { return None; }
            return String::from_utf16(&units).ok();
        }
        if quote == '`' && c == '$' && chars.peek() == Some(&'{') { return None; }
        if c != '\\' {
            if c == '\n' || c == '\r' { return None; }
            units.extend(c.encode_utf16(&mut [0; 2]).iter().copied());
            continue;
        }
        let escaped = chars.next()?;
        let decoded = match escaped {
            '\n' => continue,
            '\r' => { if chars.peek() == Some(&'\n') { chars.next(); } continue; }
            'b' => 8, 'f' => 12, 'n' => 10, 'r' => 13, 't' => 9, 'v' => 11,
            '0' => { if chars.peek().is_some_and(|c| c.is_ascii_digit()) { return None; } 0 },
            'x' | 'u' => {
                let count = if escaped == 'x' { 2 } else { 4 };
                let mut value = 0u16;
                for _ in 0..count { value = value.checked_mul(16)?.checked_add(chars.next()?.to_digit(16)? as u16)?; }
                value
            }
            c if c.is_ascii_digit() => return None,
            c => { units.extend(c.encode_utf16(&mut [0; 2]).iter().copied()); continue; }
        };
        units.push(decoded);
    }
    None
}

/// Parse Steam's JSON.parse string literal; never evaluate JavaScript.
fn parse_loc_file(path: &Path) -> HashMap<String, String> {
    let Some(json) = std::fs::read_to_string(path).ok().and_then(|s| localization_json(&s)) else { return HashMap::new() };
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&json) else { return HashMap::new() };
    m.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned()))).collect()
}

fn load_language(lang: &str) -> HashMap<String, String> {
    load_language_in(&steam_root().join("steamui/localization"), lang)
}

fn load_language_in(dir: &Path, lang: &str) -> HashMap<String, String> {
    let mut all = HashMap::new();
    // later files do not override earlier ones: shared, steamui, friendsui
    for kind in ["steamui", "shared", "friendsui"] {
        for (k, v) in parse_loc_file(&dir.join(format!("{kind}_{lang}-json.js"))) {
            all.entry(k).or_insert(v);
        }
    }
    all
}

struct Loc {
    lang: HashMap<String, String>,
    english: HashMap<String, String>,
}
fn loc_tables() -> &'static Loc {
    static L: OnceLock<Loc> = OnceLock::new();
    L.get_or_init(|| {
        let l = language();
        Loc { lang: load_language(&l), english: if l == "english" { HashMap::new() } else { load_language("english") } }
    })
}

/// Localize a token (`"#ActionButtonLabelBack"` or `"ActionButtonLabelBack"`); user language, then English,
/// then the bare token. `{0}`-style args are untouched; see [`loc_fmt`].
pub fn loc(token: &str) -> String {
    let k = token.trim_start_matches('#');
    let t = loc_tables();
    t.lang.get(k).or_else(|| t.english.get(k)).cloned().unwrap_or_else(|| k.to_string())
}
/// True if the token exists in the user's language or English.
pub fn has_loc(token: &str) -> bool {
    let k = token.trim_start_matches('#');
    let t = loc_tables();
    t.lang.contains_key(k) || t.english.contains_key(k)
}
/// Russian localization without a fallback to English or the raw token.
pub fn loc_russian(token: &str) -> Option<String> {
    static RUSSIAN: OnceLock<HashMap<String, String>> = OnceLock::new();
    RUSSIAN.get_or_init(|| load_language("russian"))
        .get(token.trim_start_matches('#')).cloned()
}

/// `loc` + substitute `%1$s`/`{0}`-style placeholders with `args` in order.
pub fn loc_fmt(token: &str, args: &[&str]) -> String {
    let mut s = loc(token);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("%{}$s", i + 1), a).replace(&format!("{{{i}}}"), a);
    }
    s
}

// ---------------------------------------------------------------- icons

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn cache_complete(dir: &Path) -> bool {
    let Some(index) = read_json(&dir.join("index.json")) else { return false };
    let Some(icons) = index["icons"].as_object() else { return false };
    !icons.is_empty() && icons.values().all(|entry| {
        let Some(file) = entry["file"].as_str() else { return false };
        // Extractor emits only flat filenames; never follow an index path outside icons.
        !file.is_empty() && Path::new(file).components().count() == 1
            && matches!(Path::new(file).components().next(), Some(std::path::Component::Normal(_)))
            && dir.join("icons").join(file).is_file()
    })
}

fn ensure_with(dir: &Path, tried: &Mutex<bool>, required: &[&str], extract: impl FnOnce() -> bool) -> bool {
    let complete = || cache_complete(dir) && required.iter().all(|name| icon_in(dir, name).is_some());
    let mut attempted = tried.lock().unwrap();
    if complete() { return true; }
    if *attempted { return false; }
    *attempted = true;
    extract() && complete()
}

/// Validate the index and its files; repair an incomplete cache once per process.
/// Call from a worker: extraction invokes Node and can be slow.
pub fn ensure_extracted() -> bool {
    static TRIED: Mutex<bool> = Mutex::new(false);
    let dir = cache_dir();
    ensure_with(&dir, &TRIED, &["ControllerType.Deck", "ControllerType.Xbox"], || {
        let script = std::env::var_os("DECKSHELL_EXTRACT")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/extract-assets/extract.mjs"));
        let mut command = Command::new("node");
        if std::env::var_os("STEAMUI").is_none() {
            command.env("STEAMUI", steam_root().join("steamui"));
        }
        script.exists() && command.arg(script).status().is_ok_and(|status| status.success())
    })
}

fn icon_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let idx = read_json(&dir.join("index.json"))?;
    let real = idx["aliases"][name].as_str().unwrap_or(name);
    let file = idx["icons"][real]["file"].as_str()?;
    if Path::new(file).components().count() != 1 || !matches!(Path::new(file).components().next(), Some(std::path::Component::Normal(_))) { return None; }
    let p = dir.join("icons").join(file);
    p.is_file().then_some(p)
}

/// Look up an already extracted icon. Does not start a blocking extraction.
pub fn icon(name: &str) -> Option<PathBuf> {
    icon_in(&cache_dir(), name)
}
/// All extracted icon names (empty if no cache).
pub fn icon_names() -> Vec<String> {
    read_json(&cache_dir().join("index.json"))
        .and_then(|v| v["icons"].as_object().map(|o| o.keys().cloned().collect()))
        .unwrap_or_default()
}

// ---------------------------------------------------------------- controller glyphs

/// Directory with Steam's controller glyphs (`dark`, `light`, `knockout` subfolders).
pub fn glyph_root() -> PathBuf {
    steam_root().join("controller_base/images/api")
}
/// Original glyph file, no copying. `name` like `sd_button_menu`, `shared_button_a`, `sd_l4`;
/// `variant` is `knockout` | `dark` | `light`, optionally with a raster size: `"knockout.md"`
/// (`sm`/`md`/`lg` -> PNG), otherwise the SVG. Falls back to PNG if no SVG exists.
pub fn glyph(name: &str, variant: &str) -> Option<PathBuf> {
    let (v, size) = variant.split_once('.').map_or((variant, None), |(a, b)| (a, Some(b)));
    let v = if v.is_empty() { "knockout" } else { v };
    let dir = glyph_root().join(v);
    let mut cands = Vec::new();
    match size {
        Some(sz) => cands.push(format!("{name}_{sz}.png")),
        None => {
            cands.push(format!("{name}.svg"));
            cands.push(format!("{name}_md.png"));
            cands.push(format!("{name}_lg.png"));
        }
    }
    cands.into_iter().map(|c| dir.join(c)).find(|p| p.exists())
}
/// Convenience for the footer/action hints: `hint_glyph("menu")` -> Deck MENU glyph (knockout SVG).
/// Keys: a b x y menu(≡) view(⧉) steam qam(aux) l1 r1 l2 r2 l4 l5 r4 r5 dpad.
pub fn hint_glyph(key: &str) -> Option<PathBuf> {
    glyph(&hint_glyph_name(key), "knockout")
}

/// Map a footer hint key to the stock glyph name for either asset source.
pub fn hint_glyph_name(key: &str) -> String {
    let key = key.to_ascii_lowercase();
    match key.as_str() {
        "a" => "shared_button_a", "b" => "shared_button_b", "x" => "shared_button_x", "y" => "shared_button_y",
        "menu" | "start" => "sd_button_menu", "view" | "select" | "back" => "sd_button_view",
        "steam" | "guide" => "sd_button_steam", "qam" | "aux" | "..." => "sd_button_aux",
        "dpad" => "shared_dpad",
        k @ ("l1" | "r1" | "l2" | "r2" | "l4" | "l5" | "r4" | "r5") => return format!("sd_{k}"),
        other => other,
    }.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loc_footer_hints() {
        for t in ["#ActionButtonLabelMenu", "#ActionButtonLabelSelect", "#ActionButtonLabelBack"] {
            assert!(has_loc(t), "{t}");
            println!("{t} = {} (lang {})", loc(t), language());
        }
        assert_eq!(loc("#NoSuchToken_xyz"), "NoSuchToken_xyz");
    }
    #[test]
    fn glyphs() {
        for k in ["a", "b", "x", "y", "menu", "view", "steam", "qam", "l1", "r2", "l4", "r5", "dpad"] {
            let p = hint_glyph(k);
            println!("glyph {k}: {p:?}");
            assert!(p.is_some(), "{k}");
        }
        assert!(glyph("sd_button_menu", "light.md").unwrap().to_string_lossy().ends_with("sd_button_menu_md.png"));
        assert!(glyph("nope", "dark").is_none());
    }
    #[test]
    fn icons() {
        // Lookup is deliberately nonblocking; callers initialize the cache explicitly.
        assert!(ensure_extracted(), "Steam assets unavailable: run tools/extract-assets/extract.mjs");
        let h = icon("home").expect("home icon (run tools/extract-assets/extract.mjs)");
        assert!(std::fs::read_to_string(h).unwrap().contains("<svg"));
        assert!(icon("Battery.50").is_some());
        let controller_svg = |name| std::fs::read_to_string(icon(name).unwrap()).unwrap();
        let deck = controller_svg("ControllerType.Deck");
        let xbox = controller_svg("ControllerType.Xbox");
        assert!(deck.contains(r#"viewBox="0 0 36 36""#) && xbox.contains(r#"viewBox="0 0 36 36""#));
        assert_ne!(deck, xbox);
        assert!(icon("definitely-not-an-icon").is_none());
        println!("{} icons", icon_names().len());
    }
}

#[cfg(test)]
mod localization_tests {
    use super::*;

    #[test]
    fn decodes_js_literals_before_json() {
        for source in [
            r#"JSON.parse('{"Greeting":"Привет","Quote":"it\'s","Backslash":"a\\\\b"}')"#,
            r#"JSON.parse(`{"Greeting":"Привет","Quote":"it's","Backslash":"a\\\\b"}`)"#,
        ] {
            let json = localization_json(source).unwrap();
            let value: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["Greeting"], "Привет");
            assert_eq!(value["Quote"], "it's");
            assert_eq!(value["Backslash"], "a\\b");
        }
        let json = localization_json(r#"JSON.parse('{"value":"\x41\uD83D\uDE00"}')"#).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap()["value"], "A😀");
    }

    #[test]
    fn rejects_incomplete_and_dynamic_literals() {
        for source in ["JSON.parse('unterminated", r#"JSON.parse(`{"x":"${evil}"}`)"#, r#"JSON.parse('\xQ0')"#, r#"JSON.parse('\uD800')"#, r#"JSON.parse('{}' + other)"#] {
            assert!(localization_json(source).is_none(), "{source}");
        }
    }

    #[test]
    fn loads_selected_language_and_english_fallback() {
        let dir = std::env::temp_dir().join(format!("deckshell-loc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("steamui_russian-json.js"), r#"JSON.parse('{"Title":"Друзья","Common":"основной"}')"#).unwrap();
        std::fs::write(dir.join("shared_russian-json.js"), r#"JSON.parse('{"Common":"общий"}')"#).unwrap();
        std::fs::write(dir.join("steamui_english-json.js"), r#"JSON.parse('{"Title":"Friends","Fallback":"Back"}')"#).unwrap();
        let chosen = load_language_in(&dir, "russian");
        let english = load_language_in(&dir, "english");
        assert_eq!(chosen.get("Title").map(String::as_str), Some("Друзья"));
        assert_eq!(chosen.get("Common").map(String::as_str), Some("основной"));
        assert_eq!(chosen.get("Fallback").or_else(|| english.get("Fallback")).map(String::as_str), Some("Back"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod icon_recovery_tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("deckshell-icons-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap(); Self(path)
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn index(dir: &Path) {
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join("index.json"), r#"{"icons":{"Settings":{"file":"Settings.svg"}},"aliases":{"settings":"Settings"}}"#).unwrap();
    }
    fn icon_file(dir: &Path) { std::fs::write(dir.join("icons/Settings.svg"), "<svg/>").unwrap(); }

    #[test]
    fn missing_file_repairs_and_failed_repair_does_not_claim_success() {
        let t=Temp::new(); index(&t.0);
        assert!(!cache_complete(&t.0)); assert!(icon_in(&t.0,"settings").is_none());
        let tried=Mutex::new(false);
        assert!(ensure_with(&t.0,&tried,&[],|| { icon_file(&t.0); true }));
        assert!(icon_in(&t.0,"settings").is_some());
        std::fs::remove_file(t.0.join("icons/Settings.svg")).unwrap();
        assert!(!ensure_with(&t.0,&tried,&[],|| panic!("must not retry")));
        assert!(icon_in(&t.0,"settings").is_none());
    }

    #[test]
    fn lookup_recovers_after_delayed_extraction_and_replaced_index() {
        let t=Temp::new(); index(&t.0);
        let dir=t.0.clone(); let tried=Arc::new(Mutex::new(false)); let worker_tried=tried.clone();
        let (start_tx,start_rx)=mpsc::channel(); let (done_tx,done_rx)=mpsc::channel();
        let worker=std::thread::spawn(move || ensure_with(&dir,&worker_tried,&[],|| {
            start_tx.send(()).unwrap(); done_rx.recv().unwrap();
            icon_file(&dir); true
        }));
        start_rx.recv().unwrap();
        assert!(icon_in(&t.0,"settings").is_none());
        done_tx.send(()).unwrap(); assert!(worker.join().unwrap());
        assert!(icon_in(&t.0,"settings").is_some());
        std::fs::write(t.0.join("index.json"), r#"{"icons":{"Power":{"file":"Power.svg"}},"aliases":{"power":"Power"}}"#).unwrap();
        std::fs::write(t.0.join("icons/Power.svg"), "<svg/>").unwrap();
        assert!(icon_in(&t.0,"settings").is_none());
        assert!(icon_in(&t.0,"power").is_some());
    }
}
