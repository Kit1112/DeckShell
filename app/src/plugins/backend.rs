//! Per-plugin values and actions. Reads come from the same Decky backend the real plugin frontends call (`rpc`) and from the plugins'
//! settings files (read-only). Writes go ONLY through the backend's own methods (never into ~/homebrew) and are skipped under DRYRUN.
use super::rpc::Rpc;
use super::{dry_run, Item, Node};
use serde_json::{json, Value};

#[derive(Default)]
pub struct Update {
    pub idx: usize,
    pub on: Option<bool>,
    pub value: Option<String>,
    pub frac: Option<f32>,
    pub label: Option<String>,
    pub options: Option<Vec<String>>,
}
#[derive(Default)]
pub struct Loaded {
    pub updates: Vec<Update>,
    pub status: String,
    pub data: Value,
}
pub enum Change {
    Toggle(bool),
    Select(String),
    Slider(f32),
    Press,
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_default()
}
fn read_json(rel: &str) -> Value {
    std::fs::read_to_string(home().join("homebrew").join(rel)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}
fn up(idx: usize) -> Update {
    Update { idx, ..Default::default() }
}
fn find(items: &[Item], kind: i32, pred: impl Fn(&Item) -> bool) -> Option<usize> {
    items.iter().position(|i| i.kind == kind && pred(i))
}

/// notch labels of a notched slider are the text of its button node ("0PX 5PX 10PX ...")
fn notches(it: &Item) -> Vec<String> {
    it.nodes.iter().find(|n: &&Node| n.desc.starts_with("button")).map(|n| n.label.split_whitespace().map(str::to_string).collect()).unwrap_or_default()
}

pub fn load(plugin: &str, items: &[Item]) -> Loaded {
    let mut l = Loaded::default();
    let mut rpc = Rpc::connect();
    if rpc.is_none() {
        l.status = "Decky Loader backend offline (showing captured defaults)".into();
    }
    match plugin {
        "CSS Loader" => {
            if let Some(r) = rpc.as_mut() {
                if let Ok(Value::Array(themes)) = r.auto("CSS Loader", "get_themes", vec![], json!({})) {
                    for (i, it) in items.iter().enumerate() {
                        if it.kind == 1 {
                            if let Some(t) = themes.iter().find(|t| t["display_name"].as_str() == Some(it.label.as_str())) {
                                l.updates.push(Update { on: t["enabled"].as_bool(), ..up(i) });
                            }
                        }
                        if it.kind == 3 && it.label.starts_with("Selected Profile") {
                            let profs: Vec<String> = themes.iter().filter(|t| t["flags"].to_string().contains("PRESET")).filter_map(|t| t["display_name"].as_str().map(String::from)).collect();
                            let cur = themes.iter().find(|t| t["flags"].to_string().contains("PRESET") && t["enabled"] == true).and_then(|t| t["display_name"].as_str().map(String::from));
                            l.updates.push(Update { value: cur, options: Some(profs), ..up(i) });
                        }
                    }
                    l.data = Value::Array(themes);
                }
            }
        }
        "Animation Changer" => {
            if let Some(r) = rpc.as_mut() {
                if let Ok(st) = r.auto("Animation Changer", "getState", vec![], json!({})) {
                    let name_of = |id: &str| -> String {
                        st["downloaded_animations"].as_array().and_then(|a| a.iter().find(|x| x["id"].as_str() == Some(id))).and_then(|x| x["name"].as_str().or(x["title"].as_str())).unwrap_or(id).to_string()
                    };
                    let s = &st["settings"];
                    for (i, it) in items.iter().enumerate() {
                        match (it.kind, it.label.as_str()) {
                            (3, "Boot") => l.updates.push(Update { value: Some(name_of(s["boot"].as_str().unwrap_or(""))), ..up(i) }),
                            (3, "Suspend") => l.updates.push(Update { value: Some(name_of(s["suspend"].as_str().unwrap_or(""))), ..up(i) }),
                            (3, "Throbber") => l.updates.push(Update { value: Some(name_of(s["throbber"].as_str().unwrap_or(""))), ..up(i) }),
                            (1, "Shuffle on Boot") => l.updates.push(Update { on: Some(s["randomize"].as_str().map(|r| !r.is_empty()).unwrap_or(false)), ..up(i) }),
                            (1, "Force IPv4") => l.updates.push(Update { on: s["force_ipv4"].as_bool(), ..up(i) }),
                            _ => {}
                        }
                    }
                    l.data = st;
                }
            }
        }
        "SteamGridDB" => {
            if let Some(r) = rpc.as_mut() {
                let keys = ["squares", "uniform_featured"];
                let tg: Vec<usize> = items.iter().enumerate().filter(|(_, i)| i.kind == 1).map(|(i, _)| i).collect();
                for (k, key) in keys.iter().enumerate() {
                    if let (Some(&i), Ok(v)) = (tg.get(k), r.auto("SteamGridDB", "get_setting", vec![json!(key), json!(false)], json!({"key": key, "fallback": false}))) {
                        l.updates.push(Update { on: v.as_bool(), ..up(i) });
                    }
                }
                if let (Some(i), Ok(v)) = (find(items, 2, |_| true), r.auto("SteamGridDB", "get_setting", vec![json!("capsule_glow_amount"), json!(100)], json!({"key": "capsule_glow_amount", "fallback": 100}))) {
                    l.updates.push(Update { frac: v.as_f64().map(|x| (x / 100.0) as f32), ..up(i) });
                }
            }
        }
        "DeckMTP" => {
            if let (Some(r), Some(i)) = (rpc.as_mut(), find(items, 1, |_| true)) {
                if let Ok(v) = r.auto("DeckMTP", "is_running", vec![], json!({})) {
                    l.updates.push(Update { on: v.as_bool(), ..up(i) });
                }
            }
        }
        "Brightness Bar" => {
            let s = read_json("settings/decky-brightness-bar/settings.json");
            for (i, it) in items.iter().enumerate() {
                if it.kind == 1 && it.label == "Container Shadow" {
                    l.updates.push(Update { on: s["containerShadow"].as_bool(), ..up(i) });
                }
                if it.kind == 2 && it.label == "Container Radius" {
                    let n = notches(it);
                    let cur = s["containerRadius"].as_str().unwrap_or("").to_uppercase();
                    if let Some(p) = n.iter().position(|x| *x == cur) {
                        l.updates.push(Update { frac: Some(p as f32 / (n.len().max(2) - 1) as f32), options: Some(n), ..up(i) });
                    }
                }
            }
        }
        "Decky Translator" => {
            let s = read_json("settings/decky-translator/decky-translator-settings.json");
            if let Some(i) = find(items, 1, |_| true) {
                let en = s["enabled"].as_bool().unwrap_or(false);
                l.updates.push(Update { on: Some(en), label: Some(format!("Plugin is {}", if en { "enabled" } else { "disabled" })), ..up(i) });
            }
        }
        "TabMaster" => {
            let s = read_json("settings/TabMaster/settings.json");
            let tabs = s["usersDict"]["1000000001"]["tabs"].clone();
            let id_of = |l: &str| match l {
                "All Games" => "AllGames",
                "Great On Deck" => "GreatOnDeck",
                "Installed" => "Installed",
                "Collections" => "Collections",
                "Non-Steam" => "DesktopApps",
                "Favorites" => "Favorites",
                _ => "",
            };
            for (i, it) in items.iter().enumerate() {
                if it.kind == 3 {
                    if let Some(p) = tabs[id_of(&it.label)]["position"].as_i64() {
                        l.updates.push(Update { value: Some(if p >= 0 { "Hide" } else { "Show" }.into()), ..up(i) });
                    }
                }
            }
        }
        "Decky LSFG-VK" => {
            let t = std::fs::read_to_string(home().join(".config/lsfg-vk/conf.toml")).unwrap_or_default();
            let get = |k: &str| t.lines().find_map(|ln| ln.trim().strip_prefix(k).and_then(|r| r.trim_start().strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string()));
            for (i, it) in items.iter().enumerate() {
                match (it.kind, it.label.as_str()) {
                    (1, "Performance Mode") => l.updates.push(Update { on: get("performance_mode").map(|v| v == "true"), ..up(i) }),
                    (1, "HDR Mode") => l.updates.push(Update { on: get("hdr_mode").map(|v| v == "true"), ..up(i) }),
                    (2, x) if x.starts_with("Flow Scale") => {
                        if let Some(fs) = get("flow_scale").and_then(|v| v.parse::<f32>().ok()) {
                            l.updates.push(Update { frac: Some(((fs - 0.25) / 0.75).clamp(0.0, 1.0)), label: Some(format!("Flow Scale ({:.0}%)", fs * 100.0)), ..up(i) });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    l
}

/// option rows below a section arrow (CSS Loader: the theme's patches)
pub fn expand(plugin: &str, items: &[Item], at: usize, data: &Value) -> Vec<Item> {
    if plugin != "CSS Loader" {
        return vec![];
    }
    // the arrow belongs to the toggle row right above it
    let Some(theme) = items[..at].iter().rev().find(|i| i.kind == 1) else { return vec![] };
    let Some(t) = data.as_array().and_then(|a| a.iter().find(|t| t["display_name"].as_str() == Some(theme.label.as_str()))) else { return vec![] };
    let mut out = vec![];
    for p in t["patches"].as_array().cloned().unwrap_or_default() {
        let name = p["name"].as_str().unwrap_or("").to_string();
        let val = p["value"].as_str().unwrap_or("").to_string();
        let opts: Vec<String> = p["options"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let ty = p["type"].as_str().unwrap_or("");
        let checkbox = ty == "checkbox" && opts.len() == 2;
        let tall = name.chars().count() > 24;
        let (kind, h, cw) = if checkbox { (1, if tall { 60.0 } else { 42.0 }, 38.0) } else { (3, if tall { 60.0 } else { 42.0 }, 100.0) };
        let mut it = Item { kind, x: 48.0, w: 300.0, h, label: name.clone(), focusable: true, options: opts.clone(), value: val.clone(), on: checkbox && val == "Yes", ..Default::default() };
        it.ctl = if checkbox { [242.0, (h - 22.0) / 2.0, 38.0, 22.0] } else { [192.0, (h - 40.0) / 2.0, cw, 40.0] };
        it.row = Some((name.clone(), "row||mouse".into()));
        it.nodes = vec![Node {
            desc: if checkbox { "toggle||A,FOCUS,touch,mouse".into() } else { "button||A,FOCUS,touch,mouse,keyboard".into() },
            label: if checkbox { String::new() } else { val.clone() },
            rect: it.ctl,
        }];
        out.push(it);
    }
    out
}

pub fn apply(plugin: &str, it: &Item, data: &Value, ch: Change) -> String {
    let what = match &ch {
        Change::Toggle(b) => format!("toggle {} = {b}", it.label),
        Change::Select(v) => format!("select {} = {v}", it.label),
        Change::Slider(f) => format!("slider {} = {f:.2}", it.label),
        Change::Press => format!("press {}", it.label),
    };
    // the backend call (if any) for this change
    let call: Option<(&str, &str, Vec<Value>, Value)> = match (plugin, &ch) {
        ("CSS Loader", Change::Toggle(b)) if it.child_of.is_none() => Some(("CSS Loader", "set_theme_state", vec![json!(it.label), json!(b)], json!({"name": it.label, "state": b}))),
        ("CSS Loader", Change::Toggle(_) | Change::Select(_)) if it.child_of.is_some() => {
            let theme = data.as_array().and_then(|a| a.iter().find(|t| t["patches"].as_array().map(|p| p.iter().any(|x| x["name"].as_str() == Some(&it.label))).unwrap_or(false)));
            let v = match &ch {
                Change::Toggle(b) => (if *b { "Yes" } else { "No" }).to_string(),
                Change::Select(v) => v.clone(),
                _ => String::new(),
            };
            theme.and_then(|t| t["name"].as_str()).map(|tn| ("CSS Loader", "set_patch_of_theme", vec![json!(tn), json!(it.label), json!(v)], json!({"themeName": tn, "patchName": it.label, "value": v})))
        }
        ("DeckMTP", Change::Toggle(_)) => Some(("DeckMTP", "toggle_mtp", vec![], json!({}))),
        ("SteamGridDB", Change::Toggle(b)) => {
            let key = it.setting.as_deref().unwrap_or("squares");
            Some(("SteamGridDB", "set_setting", vec![json!(key), json!(b)], json!({"key": key, "value": b})))
        }
        ("Animation Changer", Change::Toggle(b)) if it.label == "Shuffle on Boot" => Some(("Animation Changer", "randomize", vec![json!(b)], json!({"shuffle": b}))),
        _ => None,
    };
    let Some((p, m, args, kw)) = call else {
        return format!("{what}: local only (no backend call)");
    };
    if dry_run() {
        eprintln!("[deckshell] DRYRUN decky {p}.{m} {args:?}");
        return format!("dry-run: {p}.{m} not sent");
    }
    match Rpc::connect().map(|mut r| r.auto(p, m, args, kw)) {
        Some(Ok(_)) => format!("{p}.{m} ok"),
        Some(Err(e)) => format!("{p}.{m} failed: {}", e.chars().take(60).collect::<String>()),
        None => "Decky Loader backend offline".into(),
    }
}
