//! DeckyState glue: navigation, focus, scrolling, background value loading. Everything runs on the UI thread; the backend works on
//! short-lived threads and hands results back through `invoke_from_event_loop`.
use super::backend::{self, Change, Loaded};
use super::{load_defs, Item, Node, PanelDef, SCALE};
use crate::input::Action;
use crate::{AppWindow, DeckyItem, DeckyNode, DeckyState};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;

const VIEW_H: f32 = 588.0; // css px

struct Deck {
    ui: slint::Weak<AppWindow>,
    list: PanelDef,
    plugs: Vec<PanelDef>,
    cur: Option<usize>,
    items: Vec<Item>,
    focus: i32,
    scroll: f32, // css px
    active: bool,
    list_focus: i32,
    data: Value,
    status: String,
    gen: u64,
}

thread_local! { static DECK: RefCell<Option<Deck>> = const { RefCell::new(None) }; }

fn with<R>(f: impl FnOnce(&mut Deck) -> R) -> Option<R> {
    DECK.with(|d| d.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(f)))
}

pub fn init(ui: &AppWindow) {
    let (list, plugs) = load_defs();
    let mut d = Deck { ui: ui.as_weak(), list, plugs, cur: None, items: vec![], focus: 0, scroll: 0.0, active: false, list_focus: 0, data: Value::Null, status: String::new(), gen: 0 };
    d.show_list();
    DECK.with(|c| *c.borrow_mut() = Some(d));
    let st = ui.global::<DeckyState>();
    st.on_activate(|i| {
        with(|d| d.activate(i));
    });
    st.on_adjust_to(|i, fr| {
        with(|d| d.set_frac(i, fr));
    });
    st.on_scrolled(|dy| {
        with(|d| {
            d.scroll += dy / SCALE;
            d.clamp_scroll();
            d.push();
        });
    });
    st.on_back(|| {
        with(|d| d.back());
    });
    with(|d| d.push());
}

pub fn active() -> bool {
    with(|d| d.active).unwrap_or(false)
}
pub fn set_active(on: bool) {
    with(|d| {
        d.active = on;
        d.push();
    });
}
/// Decky rail tab selected: shows the plugin list (or the plugin that was open).
pub fn open() {
    with(|d| {
        d.active = true;
        if d.cur.is_none() {
            d.show_list();
        }
        d.push();
    });
}
/// Open a plugin panel directly (name as in Decky's list, e.g. "CSS Loader"); used by the control socket / parity runner.
pub fn open_plugin(name: &str) -> bool {
    with(|d| {
        d.active = true;
        match d.plugs.iter().position(|p| p.name == name) {
            Some(i) => {
                d.show_plugin(i);
                true
            }
            None => false,
        }
    })
    .unwrap_or(false)
}
/// pad input; true = consumed
pub fn handle(a: Action) -> bool {
    with(|d| d.handle(a)).unwrap_or(false)
}

fn px(v: f32) -> f32 {
    v * SCALE
}

fn vm_node(n: &Node) -> DeckyNode {
    DeckyNode { desc: n.desc.clone().into(), label: n.label.clone().into(), x: px(n.rect[0]), y: px(n.rect[1]), w: px(n.rect[2]), h: px(n.rect[3]) }
}

impl Deck {
    fn def(&self) -> &PanelDef {
        match self.cur {
            Some(i) => &self.plugs[i],
            None => &self.list,
        }
    }

    fn show_list(&mut self) {
        self.cur = None;
        self.items = self.list.items.clone();
        self.focus = self.list_focus.clamp(0, self.items.len().saturating_sub(1) as i32);
        self.scroll = 0.0;
        self.data = Value::Null;
        self.status.clear();
        self.gen += 1;
        self.ensure_visible();
        self.push();
    }

    fn show_plugin(&mut self, i: usize) {
        self.cur = Some(i);
        self.items = self.plugs[i].items.clone();
        self.focus = self.first_focusable();
        self.scroll = 0.0;
        self.data = Value::Null;
        self.status = if super::dry_run() { "dry-run: backend writes are logged only".into() } else { String::new() };
        self.gen += 1;
        let gen = self.gen;
        let name = self.plugs[i].name.clone();
        let snapshot = self.items.clone();
        let weak = self.ui.clone();
        std::thread::spawn(move || {
            let loaded = backend::load(&name, &snapshot);
            let _ = weak.upgrade_in_event_loop(move |_| {
                with(|d| {
                    if d.gen == gen {
                        d.apply_loaded(loaded);
                    }
                });
            });
        });
        self.push();
    }

    fn apply_loaded(&mut self, l: Loaded) {
        for u in l.updates {
            if let Some(it) = self.items.get_mut(u.idx) {
                if let Some(v) = u.on {
                    it.on = v;
                }
                if let Some(v) = u.value {
                    it.value = v;
                }
                if let Some(v) = u.frac {
                    it.frac = v;
                }
                if let Some(v) = u.label {
                    it.label = v;
                }
                if let Some(v) = u.options {
                    it.options = v;
                }
            }
        }
        self.data = l.data;
        if !l.status.is_empty() {
            self.status = l.status;
        }
        self.push();
    }

    fn first_focusable(&self) -> i32 {
        self.items.iter().position(|i| i.focusable).map(|p| p as i32).unwrap_or(-1)
    }

    fn content_h(&self) -> f32 {
        self.items.iter().map(|i| i.y + i.h).fold(0.0, f32::max) + 24.0
    }

    fn clamp_scroll(&mut self) {
        let max = (self.content_h() - VIEW_H).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    fn ensure_visible(&mut self) {
        if let Some(it) = usize::try_from(self.focus).ok().and_then(|i| self.items.get(i)) {
            let (top, bot) = (it.y, it.y + it.h);
            if top - 72.0 < self.scroll {
                self.scroll = top - 72.0;
            } else if bot + 16.0 > self.scroll + VIEW_H {
                self.scroll = bot + 16.0 - VIEW_H;
            }
        }
        self.clamp_scroll();
    }

    fn back(&mut self) {
        if self.cur.is_some() {
            self.show_list();
        }
    }

    fn handle(&mut self, a: Action) -> bool {
        match a {
            Action::B => {
                if self.cur.is_some() {
                    self.back();
                    true
                } else {
                    false
                }
            }
            Action::Up | Action::Down => {
                self.vertical(a == Action::Down);
                true
            }
            Action::Left | Action::Right => {
                let right = a == Action::Right;
                if let Some(it) = usize::try_from(self.focus).ok().and_then(|i| self.items.get(i)).cloned() {
                    if it.kind == 2 {
                        let step = if it.options.len() > 1 { 1.0 / (it.options.len() - 1) as f32 } else { 0.05 };
                        let fr = (it.frac + if right { step } else { -step }).clamp(0.0, 1.0);
                        self.set_frac(self.focus, fr);
                        return true;
                    }
                    self.horizontal(right);
                } else if self.focus < 0 {
                    self.focus = if right { (self.focus - 1).max(-(self.def().header.len() as i32)) } else { (self.focus + 1).min(-1) };
                    self.push();
                }
                true
            }
            Action::A => {
                self.activate(self.focus);
                true
            }
            _ => false,
        }
    }

    fn vertical(&mut self, down: bool) {
        if self.focus < 0 {
            if down {
                self.focus = self.first_focusable();
            }
            self.ensure_visible();
            self.push();
            return;
        }
        let cur = self.items.get(self.focus as usize).cloned();
        let Some(cur) = cur else { return };
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, it) in self.items.iter().enumerate() {
            if !it.focusable {
                continue;
            }
            let dy = if down { it.y - cur.y } else { cur.y - it.y };
            if dy < 2.0 {
                continue;
            }
            let dx = (it.x + it.ctl[0] - (cur.x + cur.ctl[0])).abs();
            let better = match best {
                None => true,
                Some((_, by, bx)) => dy < by - 2.0 || ((dy - by).abs() <= 2.0 && dx < bx),
            };
            if better {
                best = Some((i, dy, dx));
            }
        }
        match best {
            Some((i, _, _)) => self.focus = i as i32,
            None if !down => self.focus = -1,
            None => {}
        }
        self.ensure_visible();
        self.push();
    }

    fn horizontal(&mut self, right: bool) {
        let Some(cur) = self.items.get(self.focus as usize).cloned() else { return };
        let mut best: Option<(usize, f32)> = None;
        for (i, it) in self.items.iter().enumerate() {
            if !it.focusable || (it.y - cur.y).abs() > 2.0 {
                continue;
            }
            let dx = (it.x + it.ctl[0]) - (cur.x + cur.ctl[0]);
            if (right && dx > 1.0 || !right && dx < -1.0) && best.map(|b| dx.abs() < b.1).unwrap_or(true) {
                best = Some((i, dx.abs()));
            }
        }
        if let Some((i, _)) = best {
            self.focus = i as i32;
            self.push();
        }
    }

    fn set_frac(&mut self, idx: i32, fr: f32) {
        let Some(it) = usize::try_from(idx).ok().and_then(|i| self.items.get_mut(i)) else { return };
        if it.kind != 2 {
            return;
        }
        it.frac = fr.clamp(0.0, 1.0);
        let item = it.clone();
        self.focus = idx;
        self.dispatch(&item, Change::Slider(item.frac));
        self.push();
    }

    fn activate(&mut self, idx: i32) {
        if idx < 0 {
            let k = (-1 - idx) as usize;
            if k == 0 && self.cur.is_some() {
                self.back();
            }
            return;
        }
        let Some(it) = self.items.get(idx as usize).cloned() else { return };
        if !it.focusable {
            return;
        }
        self.focus = idx;
        if self.cur.is_none() {
            // plugin list: open the plugin
            self.list_focus = idx;
            if let Some(p) = self.plugs.iter().position(|p| p.name == it.label) {
                self.show_plugin(p);
            }
            return;
        }
        let i = idx as usize;
        match it.kind {
            1 => {
                let on = !it.on;
                self.items[i].on = on;
                self.dispatch(&it, Change::Toggle(on));
            }
            3 => {
                if !it.options.is_empty() {
                    let cur = it.options.iter().position(|o| *o == it.value).unwrap_or(0);
                    let next = it.options[(cur + 1) % it.options.len()].clone();
                    self.items[i].value = next.clone();
                    self.dispatch(&it, Change::Select(next));
                } else {
                    self.dispatch(&it, Change::Press);
                }
            }
            4 => self.toggle_collapse(i),
            2 => {}
            _ => self.dispatch(&it, Change::Press),
        }
        self.push();
    }

    /// expand / collapse the option rows below a CSS Loader style section arrow
    fn toggle_collapse(&mut self, i: usize) {
        let on = !self.items[i].on;
        self.items[i].on = on;
        let key = format!("{}#{}", self.def().name, i);
        if on {
            let name = self.def().name.clone();
            let kids = backend::expand(&name, &self.items, i, &self.data);
            if kids.is_empty() {
                self.status = "no options".into();
                return;
            }
            let dh: f32 = kids.iter().map(|k| k.h).sum();
            let at = self.items[i].y + self.items[i].h;
            for it in self.items.iter_mut().filter(|it| it.y >= at - 0.5) {
                it.y += dh;
            }
            let mut y = at;
            let kids: Vec<Item> = kids
                .into_iter()
                .map(|mut k| {
                    k.y = y;
                    y += k.h;
                    k.child_of = Some(key.clone());
                    k
                })
                .collect();
            for (n, k) in kids.into_iter().enumerate() {
                self.items.insert(i + 1 + n, k);
            }
        } else {
            let mut dh = 0.0;
            self.items.retain(|it| {
                if it.child_of.as_deref() == Some(&key) {
                    dh += it.h;
                    false
                } else {
                    true
                }
            });
            let at = self.items[i].y + self.items[i].h;
            for it in self.items.iter_mut().filter(|it| it.y > at - 0.5) {
                it.y -= dh;
            }
        }
        self.clamp_scroll();
    }

    fn dispatch(&mut self, it: &Item, ch: Change) {
        let name = self.def().name.clone();
        let data = self.data.clone();
        let it = it.clone();
        let weak = self.ui.clone();
        let gen = self.gen;
        std::thread::spawn(move || {
            let msg = backend::apply(&name, &it, &data, ch);
            let _ = weak.upgrade_in_event_loop(move |_| {
                with(|d| {
                    if d.gen == gen && !msg.is_empty() {
                        d.status = msg;
                        d.push();
                    }
                });
            });
        });
    }

    fn push(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let st = ui.global::<DeckyState>();
        let def = self.def();
        st.set_active(self.active);
        st.set_view(if self.cur.is_some() { 1 } else { 0 });
        st.set_title(SharedString::from(if self.cur.is_some() { def.name.as_str() } else { "Decky" }));
        st.set_focus(self.focus);
        st.set_scroll(px(self.scroll));
        st.set_content_h(px(self.content_h()));
        st.set_status(SharedString::from(self.status.as_str()));
        st.set_root_label(SharedString::from(self.root_label().as_str()));
        st.set_root_desc(SharedString::from(def.root_desc.as_str()));
        let items: Vec<DeckyItem> = self
            .items
            .iter()
            .map(|it| DeckyItem {
                kind: it.kind,
                x: px(it.x),
                y: px(it.y),
                w: px(it.w),
                h: px(it.h),
                label: it.label.clone().into(),
                desc: it.desc.clone().into(),
                value: it.value.clone().into(),
                on: it.on,
                frac: it.frac,
                focusable: it.focusable,
                busy: it.busy,
                has_row: it.row.is_some(),
                row_label: it.row.as_ref().map(|r| r.0.clone()).unwrap_or_default().into(),
                row_desc: it.row.as_ref().map(|r| r.1.clone()).unwrap_or_default().into(),
                nodes: ModelRc::new(VecModel::from(it.nodes.iter().map(vm_node).collect::<Vec<_>>())),
                cx: px(it.ctl[0]),
                cy: px(it.ctl[1]),
                cw: px(it.ctl[2]),
                ch: px(it.ctl[3]),
            })
            .collect();
        st.set_items(ModelRc::new(VecModel::from(items)));
        st.set_header(ModelRc::new(VecModel::from(def.header.iter().map(vm_node).collect::<Vec<_>>())));
    }

    /// the whole popup is one clickable div whose text is every visible string; the ledger keeps the first 120 chars
    fn root_label(&self) -> String {
        let def = self.def();
        if self.cur.is_none() || self.items.len() == def.items.len() {
            return def.root_label.clone();
        }
        let mut t = def.name.clone();
        for it in &self.items {
            if it.kind == 7 {
                t = format!("{t} {}", it.label);
            } else if let Some(r) = &it.row {
                t = format!("{t} {}", r.0);
            }
        }
        t.chars().take(120).collect()
    }
}
