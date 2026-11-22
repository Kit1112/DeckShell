//! Live Slint element tree -> context/archive/docs/parity/deckshell-tree-format.md (v1).
//! Uses slint's private_unstable_api (ItemRc walk): no .slint edits needed. Roles/handlers are derived from the
//! item kind (Text -> label, Image -> image, TouchArea -> button touch+mouse, Flickable -> scroll-region,
//! FocusScope -> keyboard+FOCUS) or from `accessible-role`/`accessible-label` where a widget sets them.
//! Extra (non-spec) keys: `slint_id` (element id from SLINT_EMIT_DEBUG_INFO=1 builds), `kind`.
use crate::AppWindow;
use serde_json::{json, Value};
use slint::private_unstable_api::re_exports as rx;
use slint::ComponentHandle;
use std::cell::RefCell;

thread_local! {
    /// all screens seen so far this run, keyed by "route|state" (replaced on re-dump)
    static ACC: RefCell<Vec<(String, Vec<Value>)>> = const { RefCell::new(Vec::new()) };
    static ONESHOT: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

fn role_name(r: rx::AccessibleRole) -> Option<&'static str> {
    use rx::AccessibleRole as R;
    Some(match r {
        R::Button => "button",
        R::Tab => "tab",
        R::Checkbox | R::Switch => "toggle",
        R::Slider => "slider",
        R::TextInput => "text-field",
        R::Text => "label",
        R::Groupbox | R::TabPanel | R::TabList => "row",
        R::List | R::ListItem => "list-item",
        R::Image => "image",
        _ => return None,
    })
}

/// Metadata convention (set in .slint on interactive widgets / screen elements):
///   accessible-label       = visible text exactly as rendered
///   accessible-description = "role|loc_token|handlers"  (any part may be empty), e.g. "tab|#LibraryHome_Recent|A,B,FOCUS,BLUR,touch,mouse"
///     role      one of the ledger roles (button tab tile list-item row toggle slider text-field header footer-hint menu-item dialog ...)
///     loc_token Steam token; when empty the token is looked up from the text via the `t()` registry (assets::token_for)
///     handlers  comma list: gamepad names (A B X Y L1 R1 START SELECT DPAD_* FOCUS BLUR STEAM QAM ANY) and flags
///               touch mouse scroll long_press keyboard
/// Elements under an interactive (described) ancestor do not produce label/touch nodes of their own.
#[derive(Default)]
struct Meta {
    role: String,
    token: String,
    gamepad: Vec<String>,
    flags: Vec<String>,
}

fn parse_meta(desc: &str) -> Option<Meta> {
    let mut it = desc.splitn(3, '|');
    let role = it.next()?.trim().to_string();
    let token = it.next().unwrap_or("").trim().to_string();
    let hs = it.next().unwrap_or("");
    if role.is_empty() && token.is_empty() && hs.is_empty() {
        return None;
    }
    let mut m = Meta { role, token, ..Default::default() };
    for h in hs.split(',').map(str::trim).filter(|h| !h.is_empty()) {
        if h.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            m.gamepad.push(h.to_string());
        } else {
            m.flags.push(h.to_string());
        }
    }
    Some(m)
}

struct Ctx<'a> {
    route: &'a str,
    state: &'a str,
    counts: std::collections::HashMap<String, usize>,
    out: Vec<Value>,
}

/// Flat node list of everything on screen right now. `route`/`state` fill `id_hint`.
pub fn collect(ui: &AppWindow, route: &str, state: &str) -> Vec<Value> {
    let win = rx::WindowInner::from_pub(ui.window());
    let comp = win.component();
    let root = rx::ItemRc::new_root(comp);
    let mut cx = Ctx { route, state, counts: Default::default(), out: Vec::new() };
    walk(&root, &mut cx, false, false);
    footer_hints(cx.out)
}

/// The generated footer (ui/generated/footer.slint, `GKeyHint`) cannot carry accessible metadata: recognise its chips as
/// touch targets in the footer band (a TouchArea with a label inside), merge the label into the chip and tag it `footer-hint`.
/// Tapping a chip runs the same action as its gamepad button (src/footer.rs), so it reacts to that button (reported as A, the
/// ledger's activation button for hints).
fn footer_hints(mut nodes: Vec<Value>) -> Vec<Value> {
    let rect = |n: &Value| -> [f64; 4] {
        let r = &n["rect"];
        [r[0].as_f64().unwrap_or(0.0), r[1].as_f64().unwrap_or(0.0), r[2].as_f64().unwrap_or(0.0), r[3].as_f64().unwrap_or(0.0)]
    };
    let chips: Vec<usize> = (0..nodes.len())
        .filter(|&i| nodes[i]["kind"] == "TouchArea" && {
            let r = rect(&nodes[i]);
            r[1] >= 740.0 && r[3] <= 60.0 && r[2] > 40.0
        })
        .collect();
    let mut drop = std::collections::HashSet::new();
    for &i in &chips {
        let r = rect(&nodes[i]);
        let lab = (0..nodes.len()).find(|&j| {
            nodes[j]["id_hint"]["role"] == "label" && {
                let q = rect(&nodes[j]);
                q[0] >= r[0] - 1.0 && q[0] + q[2] <= r[0] + r[2] + 1.0 && q[1] >= r[1] - 1.0 && q[1] + q[3] <= r[1] + r[3] + 1.0
                    && nodes[j]["id_hint"]["text"].as_str().map(|t| t.chars().count() > 5 || t.chars().all(|c| c.is_uppercase())).unwrap_or(false)
                    && q[0] > r[0] + r[2] * 0.3
            }
        });
        if let Some(j) = lab {
            let (text, tok) = (nodes[j]["id_hint"]["text"].clone(), nodes[j]["id_hint"]["loc_token"].clone());
            nodes[i]["id_hint"]["text"] = text;
            if !tok.is_null() {
                nodes[i]["id_hint"]["loc_token"] = tok;
            }
            drop.insert(j);
        }
        nodes[i]["id_hint"]["role"] = json!("footer-hint");
        nodes[i]["id_hint"]["index"] = json!(0);
        nodes[i]["handlers"]["gamepad"] = json!(["A"]);
    }
    nodes.into_iter().enumerate().filter(|(i, _)| !drop.contains(i)).map(|(_, n)| n).collect()
}

/// `clipped_ok`: the nearest *visible* ancestor is a Flickable, so an item that `is_visible()` rejects is only scrolled away
/// (Steam's DOM lists such rows) and not `visible: false` (same rule as slint's own is_visible_or_clipped_by_flickable).
fn walk(it: &rx::ItemRc, cx: &mut Ctx, in_acc: bool, clipped_ok: bool) {
    let vis = it.is_visible();
    let (node, is_acc) = describe(it, cx, in_acc, vis || clipped_ok);
    if let Some(n) = node {
        cx.out.push(n);
    }
    let child_ok = if vis { clipped_ok || it.downcast::<rx::Flickable>().is_some() } else { clipped_ok };
    let mut c = it.first_child();
    while let Some(ch) = c {
        walk(&ch, cx, in_acc || is_acc, child_ok);
        c = ch.next_sibling();
    }
}

fn describe(it: &rx::ItemRc, cx: &mut Ctx, in_acc: bool, present: bool) -> (Option<Value>, bool) {
    let mut role: Option<String> = None;
    let mut text = String::new();
    let (mut touch, mut scroll, mut kb, mut focus) = (false, false, false, false);
    let mut long_press = false;
    let mut gp: Vec<String> = vec![];
    let mut token = String::new();
    let is_acc = false;
    let kind;
    if let Some(t) = it.downcast::<rx::ComplexText>() {
        text = t.as_pin_ref().text().to_string();
        kind = "Text";
        role = Some("label".into());
    } else if let Some(t) = it.downcast::<rx::SimpleText>() {
        text = t.as_pin_ref().text().to_string();
        kind = "Text";
        role = Some("label".into());
    } else if it.downcast::<rx::ImageItem>().is_some() {
        kind = "Image";
        role = Some("image".into());
    } else if it.downcast::<rx::TouchArea>().is_some() {
        kind = "TouchArea";
        role = Some("button".into());
        touch = true;
    } else if it.downcast::<rx::Flickable>().is_some() {
        kind = "Flickable";
        role = Some("scroll-region".into());
        scroll = true;
    } else if it.downcast::<rx::FocusScope>().is_some() {
        kind = "FocusScope";
        role = Some("row".into());
        kb = true;
        focus = true;
    } else if it.downcast::<rx::TextInput>().is_some() {
        kind = "TextInput";
        role = Some("text-field".into());
        kb = true;
        focus = true;
    } else {
        kind = "";
    }
    if it.is_accessible() {
        let acc_label = it.accessible_string_property(rx::AccessibleStringProperty::Label).map(|s| s.to_string()).unwrap_or_default();
        let desc = it.accessible_string_property(rx::AccessibleStringProperty::Description).map(|s| s.to_string()).unwrap_or_default();
        if let Some(m) = parse_meta(&desc) {
            if !m.role.is_empty() {
                role = Some(m.role.clone());
            }
            token = m.token.clone();
            if !acc_label.is_empty() {
                text = acc_label.clone();
            }
            gp = m.gamepad.clone();
            touch = m.flags.iter().any(|f| f == "touch");
            scroll = m.flags.iter().any(|f| f == "scroll");
            kb = m.flags.iter().any(|f| f == "keyboard");
            long_press = m.flags.iter().any(|f| f == "long_press");
            let mouse = m.flags.iter().any(|f| f == "mouse");
            return finish(it, cx, present, role, text, token, gp, touch, mouse, scroll, kb, long_press, kind, true);
        } else if let Some(r) = role_name(it.accessible_role()) {
            role = Some(r.into());
            if !acc_label.is_empty() {
                text = acc_label;
            }
        }
    }
    if in_acc && matches!(role.as_deref(), Some("label") | Some("button")) && kind != "Image" {
        return (None, false);
    }
    if focus {
        gp.extend(["FOCUS".to_string(), "BLUR".to_string()]);
    }
    let mouse = touch;
    finish(it, cx, present, role, text, token, gp, touch, mouse, scroll, kb, long_press, kind, is_acc)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    it: &rx::ItemRc,
    cx: &mut Ctx,
    present: bool,
    role: Option<String>,
    text: String,
    mut token: String,
    gp: Vec<String>,
    touch: bool,
    mouse: bool,
    scroll: bool,
    kb: bool,
    long_press: bool,
    kind: &str,
    is_acc: bool,
) -> (Option<Value>, bool) {
    let Some(role) = role else { return (None, false) };
    if !present {
        return (None, false);
    }
    let g = it.geometry();
    // map_to_window() maps a point in the item's *parent* space ("does not add this item's x and y") -> pass the origin.
    let p = it.map_to_window(g.origin);
    let (x, y, w, h) = (p.x, p.y, g.size.width, g.size.height);
    // off-screen but visible items are kept (Steam's DOM also lists scrolled-away rows); only empty boxes are dropped
    if w <= 0.0 || h <= 0.0 {
        return (None, false);
    }
    // the window's key sink FocusScope is not an element of the Steam tree
    if kind == "FocusScope" && !is_acc {
        return (None, false);
    }
    if role == "label" && text.trim().is_empty() {
        return (None, false);
    }
    if token.is_empty() && !text.is_empty() {
        token = crate::assets::token_for(&text).unwrap_or_default();
    }
    let slint_id = it
        .element_type_names_and_ids(0)
        .and_then(|v| v.into_iter().map(|(_, id)| id.to_string()).find(|s| !s.is_empty()))
        .unwrap_or_default();
    let key = format!("{role}|{}", if token.is_empty() { &text } else { &token });
    let idx = cx.counts.entry(key).or_insert(0);
    let index = *idx;
    *idx += 1;
    let mut hint = json!({"route": cx.route, "state": cx.state, "role": role, "owner": "steam", "index": index});
    if !text.is_empty() {
        hint["text"] = json!(text);
    }
    if !token.is_empty() {
        hint["loc_token"] = json!(token);
    }
    let r = |v: f32| (v * 10.0).round() / 10.0;
    let v = json!({
        "id_hint": hint,
        "rect": [r(x), r(y), r(w), r(h)],
        "visible": true,
        "kind": kind,
        "slint_id": slint_id,
        "handlers": {"gamepad": gp, "touch": touch, "mouse": mouse, "scroll": scroll, "long_press": long_press, "keyboard": kb},
    });
    (Some(v), is_acc)
}

/// Dump the current screen, merge into the per-run accumulator and write DECKSHELL_DUMP_TREE (or `explicit`).
/// Path semantics: ends with `/` or is an existing dir -> `<dir>/<route-slug>.json`; else one file with all screens.
pub fn dump(ui: &AppWindow, route: &str, state: &str, explicit: Option<&str>) -> Result<String, String> {
    let nodes = collect(ui, route, state);
    let n = nodes.len();
    let target = match explicit {
        Some(p) => p.to_string(),
        None => std::env::var("DECKSHELL_DUMP_TREE").map_err(|_| "DECKSHELL_DUMP_TREE unset".to_string())?,
    };
    let key = format!("{route}|{state}");
    let all: Vec<Value> = ACC.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(e) = a.iter_mut().find(|e| e.0 == key) {
            e.1 = nodes.clone();
        } else {
            a.push((key, nodes.clone()));
        }
        a.iter().flat_map(|e| e.1.clone()).collect()
    });
    let as_dir = target.ends_with('/') || std::path::Path::new(&target).is_dir();
    let (file, body) = if explicit.is_some() {
        (target.clone(), nodes) // explicit file: only the current screen
    } else if as_dir {
        let slug: String = route.trim_matches('/').replace('/', "_");
        let st = if state == "default" { String::new() } else { format!("@{state}") };
        let _ = std::fs::create_dir_all(&target);
        (format!("{}/{slug}{st}.json", target.trim_end_matches('/')), nodes)
    } else {
        (target.clone(), all)
    };
    std::fs::write(&file, serde_json::to_vec_pretty(&body).unwrap_or_default()).map_err(|e| e.to_string())?;
    eprintln!("[deckshell] dumptree {file} ({n} nodes)");
    Ok(format!("{file} {n}"))
}

/// Called from App for DECKSHELL_DUMP_TREE: dump the current screen of `app`.
pub fn dump_app(a: &crate::app::App) {
    let (route, state) = crate::control::route_state(a);
    let ui = a.ui();
    let _ = dump(&ui, &route, &state, None);
}

/// DECKSHELL_DUMP_TREE without a key script / control socket: dump once after startup (+ exit if DECKSHELL_EXIT).
pub fn init_oneshot() {
    if std::env::var_os("DECKSHELL_DUMP_TREE").is_none() || std::env::var_os("DECKSHELL_KEYS").is_some() || std::env::var_os("DECKSHELL_CONTROL").is_some() {
        return;
    }
    let t = slint::Timer::default();
    t.start(slint::TimerMode::SingleShot, std::time::Duration::from_secs(4), || {
        crate::with_app(|a| dump_app(a));
        if std::env::var_os("DECKSHELL_EXIT").is_some() {
            let _ = slint::quit_event_loop();
        }
    });
    ONESHOT.with(|o| *o.borrow_mut() = Some(t));
}
