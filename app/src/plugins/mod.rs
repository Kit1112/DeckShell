//! Decky tab of the Quick Access Menu: plugin list + a native replica of each installed plugin's panel.
//! * layout: `ui/generated/decky/panels.json` (tools/compile-ui/decky/compile-decky.mjs, from the live Decky capture) rendered by
//!   `ui/screens/decky/decky_tab.slint` (`DeckyTab`, state in the `DeckyState` global);
//! * values/actions: `backend.rs` talks to the SAME Decky Loader backend the real plugins use (`rpc.rs`, ws://127.0.0.1:1337) and reads the
//!   plugins' settings files (read-only; this module never writes under ~/homebrew). State-changing backend calls are skipped under
//!   `DECKSHELL_DRYRUN=1` (logged instead).
//! Integration (qam owner): call `plugins::init(&ui)` once, `plugins::open()` when the Decky rail tab is selected, `plugins::handle(Action)`
//! for pad input while `plugins::active()`, `plugins::route_state()` for the parity tree route.
pub mod backend;
pub mod rpc;
mod ui;

pub use ui::{active, handle, init, open, open_plugin, set_active};

use serde_json::Value;

pub const SCALE: f32 = 1.2;

pub fn dry_run() -> bool {
    std::env::var("DECKSHELL_DRYRUN").map(|v| v != "0" && !v.is_empty()).unwrap_or(false) || deck_system::guard::active()
}

/// One row/control of a panel (css px geometry, relative to the panel; converted to device px by `ui`).
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub kind: i32, // 0 button 1 toggle 2 slider 3 select 4 collapse 5 chip 6 text 7 header
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub label: String,
    pub desc: String,
    pub value: String,
    pub on: bool,
    pub frac: f32,
    pub focusable: bool,
    pub busy: bool,
    pub row: Option<(String, String)>,
    pub nodes: Vec<Node>,
    pub ctl: [f32; 4], // main control rect relative to the item
    /// select options / slider notch labels (backend filled)
    pub options: Vec<String>,
    /// children inserted below a collapse item while expanded
    pub child_of: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub desc: String,
    pub label: String,
    pub rect: [f32; 4],
}

pub struct PanelDef {
    pub name: String,
    pub owner: String,
    pub root_label: String,
    pub root_desc: String,
    pub header: Vec<Node>,
    pub items: Vec<Item>,
    pub is_list: bool,
}

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}
fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn parse_panel(p: &Value, is_list: bool) -> PanelDef {
    let mut items = Vec::new();
    for it in p["items"].as_array().cloned().unwrap_or_default() {
        let kind = match it["kind"].as_str().unwrap_or("") {
            "button" => 0,
            "toggle" => 1,
            "slider" => 2,
            "select" => 3,
            "collapse" => 4,
            "chip" => 5,
            "text" => 6,
            _ => 7,
        };
        let (x, y, w, h) = (f(&it["x"]), f(&it["y"]), f(&it["w"]), f(&it["h"]));
        let nodes: Vec<Node> = it["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|n| Node { desc: s(&n["desc"]), label: s(&n["label"]), rect: [f(&n["x"]) - x, f(&n["y"]) - y, f(&n["w"]), f(&n["h"])] })
            .collect();
        let main = nodes
            .iter()
            .find(|n| match kind {
                1 => n.desc.starts_with("toggle"),
                2 => n.desc.starts_with("slider"),
                _ => n.desc.starts_with("button"),
            })
            .cloned();
        let ctl = main.map(|n| n.rect).unwrap_or([0.0, 0.0, w, h]);
        let row = if it["row"].is_object() { Some((s(&it["row"]["label"]), s(&it["row"]["desc"]))) } else { None };
        items.push(Item {
            kind,
            x,
            y,
            w,
            h,
            label: s(&it["label"]),
            desc: s(&it["desc"]),
            value: s(&it["value"]),
            frac: if kind == 2 { 0.5 } else { 0.0 },
            focusable: matches!(kind, 0..=5),
            row,
            nodes,
            ctl,
            ..Default::default()
        });
    }
    // section fragments above the first item
    if let Some(fr) = p["frag"].as_array().and_then(|a| a.first()).and_then(|v| v.as_str()) {
        let top = items.iter().map(|i| i.y).fold(f32::MAX, f32::min);
        items.insert(0, Item { kind: 7, x: 48.0, y: (top - 26.0).max(52.0), w: 300.0, h: 24.0, label: fr.to_string(), ..Default::default() });
    }
    PanelDef {
        name: s(&p["name"]),
        owner: s(&p["owner"]),
        root_label: s(&p["rootText"]),
        root_desc: s(&p["rootDesc"]),
        header: p["header"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|n| Node { desc: s(&n["desc"]), label: String::new(), rect: [f(&n["x"]), f(&n["y"]), f(&n["w"]), f(&n["h"])] })
            .collect(),
        items,
        is_list,
    }
}

/// (list, plugins) from the generated spec
pub fn load_defs() -> (PanelDef, Vec<PanelDef>) {
    let v: Value = serde_json::from_str(include_str!("../../ui/generated/decky/panels.json")).unwrap_or(Value::Null);
    let list = parse_panel(&v["list"], true);
    let plugins = v["plugins"].as_array().cloned().unwrap_or_default().iter().map(|p| parse_panel(p, false)).collect();
    (list, plugins)
}
