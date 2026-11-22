//! The gamepad focus-navigation tree. Faithful port of Steam's engine:
//!   * node class `D3`/`Bp` ........ sp.js:81387-82290 (module 74562)
//!   * nav tree `ot` (TransferFocus) . sp.js:461840-462110 (module 70391 region)
//!   * visible-child search ......... sp.js:82183-82260 (module 48294)
//!   * scroll-into-view `P`/`v` ..... sp.js:82343-82600 (module 84719)
//! Names in comments are the minified identifiers of the bundle (tools/cdp/build/pretty/sp.js).
//!
//! Rust model: nodes live in an arena and are addressed by [`NodeId`]; the DOM is replaced by explicit
//! `Rect`s (the "getBoundingClientRect" of the screen widgets) that the UI layer keeps up to date with
//! [`Tree::set_rect`]. Children are kept in registration order (= DOM order; use [`Tree::reorder_children`] when a
//! screen re-orders). Everything the engine wants the UI to do comes out as [`NavEvent`]s (see `drain_events`).

use std::collections::HashMap;
use std::rc::Rc;

use super::geom::{self, Axis, Rect};
use super::scroll::{self, Behavior, Edges, ScrollBox, ScrollReq, ScrollTarget};

pub type NodeId = usize;

/// `x` enum (sp.js:81410): flow-children / CSS-derived layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    None = 0,
    Column = 1,
    Row = 2,
    ColumnReverse = 3,
    RowReverse = 4,
    Grid = 5,
    Geometric = 6,
}

/// `g` enum (sp.js:81419): where a container puts focus when entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPrefer {
    First = 0,
    Last = 1,
    MaintainX = 2,
    MaintainY = 3,
    PreferredChild = 4,
}

/// `Yo` enum (sp.js:81428).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollType {
    Standard = 0,
    NoTransform = 1,
    NoTransformSparseContent = 2,
}

/// `P` enum (sp.js:81437): direction along a layout's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rel {
    Invalid = 0,
    Forward = 1,
    Backward = 2,
}

/// `EGamepadButton` (sp.js:80741, subset used by the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Ok = 1,
    Cancel = 2,
    Secondary = 3,
    Options = 4,
    BumperLeft = 5,
    BumperRight = 6,
    TriggerLeft = 7,
    TriggerRight = 8,
    DirUp = 9,
    DirDown = 10,
    DirLeft = 11,
    DirRight = 12,
    Select = 13,
    Start = 14,
    SteamGuide = 27,
    QuickMenu = 28,
}
impl Button {
    pub fn is_dir(self) -> bool {
        matches!(self, Button::DirUp | Button::DirDown | Button::DirLeft | Button::DirRight)
    }
    /// `G4` (sp.js:81362): axis of a D-pad button.
    pub fn axis(self) -> Option<Axis> {
        match self {
            Button::DirUp | Button::DirDown => Some(Axis::Y),
            Button::DirLeft | Button::DirRight => Some(Axis::X),
            _ => None,
        }
    }
}

/// `D$` (focus source): decides sounds and the instant/smooth scroll choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSource {
    Application,
    Browser,
    Gamepad,
}

/// Click/move handler: receives the node the handler is attached to, returns "handled".
pub type Handler = Rc<dyn Fn(NodeId) -> bool>;

/// Node properties (the Focusable/NavigationItem props that the engine reads).
#[derive(Clone, Default)]
pub struct Props {
    /// `layout` / flow-children attribute. `None` = unset -> derived (see `css_layout`).
    pub layout: Layout,
    /// What the CSS-derived layout (`ko`, sp.js:81351) would be for this element. Default (block) = Column.
    pub css_layout: Option<Layout>,
    pub focusable: bool,
    pub focusable_if_empty: bool,
    pub child_focus_disabled: bool,
    /// `fnCanTakeFocus`: return false to disable the node.
    pub can_take_focus: Option<Rc<dyn Fn(NodeId) -> bool>>,
    pub nav_entry_prefer_position: Option<EntryPrefer>,
    pub reset_nav_on_entry: bool,
    pub nav_key: Option<String>,
    pub auto_focus: bool,
    pub preferred_focus: bool,
    pub scroll_into_view_when_child_focused: ScrollWhenChild,
    pub scroll_into_view_type: Option<ScrollType>,
    pub scroll_region_to_start_or_end: bool,
    pub scroll_margin: Edges,
    /// `fnScrollIntoViewHandler(node, prev, ancestor)`; returning true (= `!== false`) short-circuits.
    pub scroll_into_view_handler: Option<Rc<dyn Fn(NodeId, Option<NodeId>, NodeId) -> bool>>,
    pub no_focus_ring: bool,
    pub disable_nav_sounds: bool,
    /// onMoveUp/Right/Down/Left (index: up, right, down, left).
    pub on_move: [Option<Handler>; 4],
    /// onOKButton / onCancelButton / onSecondaryButton / onOptionsButton / bumpers ... handlers.
    pub on_button: Vec<(Button, Handler)>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScrollWhenChild {
    #[default]
    No,
    Yes,
    /// `"force"`: only this ancestor is scrolled (sp.js:82356).
    Force,
}

struct Node {
    alive: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// `m_ActiveChild` / `m_iLastActiveChildIndex` (focus memory).
    active_child: Option<NodeId>,
    last_active_idx: i32,
    props: Props,
    rect: Option<Rect>,
    scroll: Option<ScrollBox>,
    depth: u32,
    mounted: bool,
    has_focus: bool,
    focus_within: bool,
    auto_focus_child: bool,
    fie_ancestor: Option<NodeId>,
}

/// Things the engine asks the UI to do / the audio layer to play.
#[derive(Debug, Clone, PartialEq)]
pub enum NavEvent {
    /// vgp_onblur + vgp_onfocus: focus moved `from` -> `to` (tree.TransferFocusInternal).
    Focus { from: Option<NodeId>, to: Option<NodeId>, source: FocusSource, dir: Option<Button> },
    Scroll(ScrollReq),
    /// Direction nobody handled -> GamepadAudio FailedNav (sounds.json).
    FailedNav,
    /// Focus moved by gamepad -> BasicNav sound (unless node.disable_nav_sounds).
    BasicNav,
    /// Logical button nobody handled (reaches the root unhandled callbacks).
    Unhandled(Button),
}

/// Shadow of `Ze` (sp.js:462110): remembers the rect of the last node focused by a move on one axis.
#[derive(Default, Clone)]
struct MovementRect {
    node: Option<NodeId>,
    rect: Option<Rect>,
}

pub struct Tree {
    nodes: Vec<Node>,
    root: NodeId,
    last_focus: Option<NodeId>,
    was_active_for_last_focus_change: bool,
    move_x: MovementRect,
    move_y: MovementRect,
    /// `BIsActive()` — scroll only happens in the active tree.
    pub active: bool,
    /// innerWidth / innerHeight of the window.
    pub viewport: (f32, f32),
    pub window_scroll: ScrollBox,
    /// `performance.now()` (injected; tests set it).
    pub now_ms: f64,
    /// module var `g` (sp.js:82379): time of the last forced-instant scroll.
    last_jump_ms: Option<f64>,
    /// `Controller.BIsRestoringHistory()`.
    pub restoring_history: bool,
    events: Vec<NavEvent>,
    /// DeferredFocus queue (request, descendant?).
    queued_focus: Option<(NodeId, bool)>,
    global_buttons: Vec<(Button, Handler)>,
    pub history: super::history::History,
}

const MAX_JUMP_MS: f64 = 500.0; // `C = 500`

impl Tree {
    pub fn new(viewport: (f32, f32)) -> Self {
        let mut t = Tree {
            nodes: vec![],
            root: 0,
            last_focus: None,
            was_active_for_last_focus_change: false,
            move_x: MovementRect::default(),
            move_y: MovementRect::default(),
            active: true,
            viewport,
            window_scroll: ScrollBox::default(),
            now_ms: 0.0,
            last_jump_ms: None,
            restoring_history: false,
            events: vec![],
            queued_focus: None,
            global_buttons: vec![],
            history: Default::default(),
        };
        // `m_Root = new Node(this,null,null); SetProperties({layout: COLUMN})` (sp.js:461888)
        let root = t.alloc(None, Props { layout: Layout::Column, ..Default::default() }, Some(Rect::new(0.0, 0.0, viewport.0, viewport.1)));
        t.nodes[root].mounted = true;
        t.root = root;
        t
    }
    pub fn root(&self) -> NodeId {
        self.root
    }
    fn alloc(&mut self, parent: Option<NodeId>, props: Props, rect: Option<Rect>) -> NodeId {
        let depth = parent.map(|p| self.nodes[p].depth + 1).unwrap_or(0);
        self.nodes.push(Node {
            alive: true,
            parent,
            children: vec![],
            active_child: None,
            last_active_idx: -1,
            props,
            rect,
            scroll: None,
            depth,
            mounted: false,
            has_focus: false,
            focus_within: false,
            auto_focus_child: false,
            fie_ancestor: None,
        });
        self.nodes.len() - 1
    }
    fn n(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }
    fn nm(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }
    pub fn is_alive(&self, id: NodeId) -> bool {
        self.nodes.get(id).map(|n| n.alive).unwrap_or(false)
    }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.n(id).parent
    }
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.n(id).children
    }
    pub fn props(&self, id: NodeId) -> &Props {
        &self.n(id).props
    }
    pub fn rect_of(&self, id: NodeId) -> Option<Rect> {
        self.n(id).rect
    }
    /// `GetBoundingRect()` where missing counts as an empty rect.
    fn rect(&self, id: NodeId) -> Rect {
        self.n(id).rect.unwrap_or_default()
    }
    pub fn nav_key(&self, id: NodeId) -> Option<&str> {
        self.n(id).props.nav_key.as_deref()
    }
    pub fn has_focus(&self, id: NodeId) -> bool {
        self.n(id).has_focus
    }
    pub fn focus_within(&self, id: NodeId) -> bool {
        self.n(id).focus_within
    }
    /// `GetLastFocusedNode()`.
    pub fn focused(&self) -> Option<NodeId> {
        self.last_focus
    }
    pub fn drain_events(&mut self) -> Vec<NavEvent> {
        std::mem::take(&mut self.events)
    }

    // ------------------------------------------------------------------ registration (OnMount/OnUnmount)

    /// Register a node under `parent` (React `useEffect` -> `RegisterNavigationItem` -> `OnMount`, sp.js:81598).
    pub fn add_node(&mut self, parent: NodeId, props: Props, rect: Option<Rect>) -> NodeId {
        let id = self.alloc(Some(parent), props, rect);
        self.on_mount(id);
        id
    }
    /// Container with scrolling (overflow auto). Call right after `add_node`.
    pub fn set_scroll_box(&mut self, id: NodeId, sb: ScrollBox) {
        self.nm(id).scroll = Some(sb);
    }
    pub fn set_rect(&mut self, id: NodeId, r: Rect) {
        self.nm(id).rect = Some(r);
    }
    pub fn set_props(&mut self, id: NodeId, props: Props) {
        // SetProperties (sp.js:81503): focusableIfEmpty propagation.
        let was = self.n(id).props.focusable_if_empty;
        let now = props.focusable_if_empty;
        self.nm(id).props = props;
        if now && !was {
            self.propagate_fie(id, Some(id));
        } else if !now && was {
            let a = self.n(id).fie_ancestor;
            self.propagate_fie(id, a);
        }
    }
    /// Re-order children (screens call this when DOM order changes; `EnsureChildrenSorted`, sp.js:81716).
    pub fn reorder_children(&mut self, parent: NodeId, order: &[NodeId]) {
        let active = self.n(parent).active_child;
        self.nm(parent).children = order.to_vec();
        if let Some(a) = active {
            let i = self.n(parent).children.iter().position(|&c| c == a).map(|i| i as i32).unwrap_or(-1);
            self.nm(parent).last_active_idx = i;
        }
    }
    /// `SetFocusableIfEmptyAncestor` (sp.js:82284).
    fn set_fie_ancestor(&mut self, id: NodeId, a: Option<NodeId>) {
        self.nm(id).fie_ancestor = a;
        if !self.n(id).props.focusable_if_empty {
            self.propagate_fie(id, a);
        }
    }
    fn propagate_fie(&mut self, id: NodeId, a: Option<NodeId>) {
        for c in self.n(id).children.clone() {
            self.set_fie_ancestor(c, a);
        }
    }

    /// `AddChild` (sp.js:81577) + `OnMount` (sp.js:81598).
    fn on_mount(&mut self, id: NodeId) {
        let parent = self.n(id).parent.expect("only root has no parent");
        // AddChild
        self.nm(parent).children.push(id);
        if self.n(parent).props.focusable_if_empty {
            self.set_fie_ancestor(id, Some(parent));
        } else if let Some(a) = self.n(parent).fie_ancestor {
            self.set_fie_ancestor(id, Some(a));
        }
        self.nm(id).mounted = true;
        let fie = self.n(id).fie_ancestor;
        let o = fie.map(|a| self.n(a).has_focus).unwrap_or(false);
        let m = self.n(id).props.auto_focus || o;
        if self.wants_auto_focus(id) || m {
            let mut t: i32 = -1;
            if !self.n(id).children.is_empty() {
                t = self.n(id).children.iter().position(|&c| self.wants_auto_focus(c)).map(|i| i as i32).unwrap_or(-1);
            }
            if self.n(id).props.auto_focus || m || t != -1 {
                self.set_active_idx(id, t);
                // parent.m_element is always set in our model
                if o {
                    if self.queued_focus.is_none() {
                        self.queued_focus = Some((fie.unwrap(), true));
                    }
                } else {
                    self.queued_focus = Some((id, false));
                }
            }
        } else if !self.n(id).children.is_empty() {
            if let Some(i) = self.n(id).children.iter().position(|&c| self.n(c).focus_within) {
                self.set_active_idx(id, i as i32);
            }
        }
    }
    fn wants_auto_focus(&self, id: NodeId) -> bool {
        self.n(id).props.auto_focus || self.n(id).auto_focus_child
    }

    /// `OnUnmount` (sp.js:81649) + `RemoveChild` (sp.js:81689). Removes the whole subtree.
    pub fn remove_node(&mut self, id: NodeId) {
        if id == self.root || !self.is_alive(id) {
            return;
        }
        for c in self.n(id).children.clone() {
            self.remove_node(c);
        }
        if self.n(id).props.focusable_if_empty {
            let a = self.n(id).fie_ancestor;
            self.propagate_fie(id, a);
        }
        self.nm(id).mounted = false;
        let queued = self.queued_focus.map(|q| q.0 == id).unwrap_or(false);
        if self.n(id).has_focus || queued {
            if queued {
                self.queued_focus = None;
            }
            if let Some(a) = self.n(id).fie_ancestor {
                // OnFocusedDecendantRemoved -> DeferredFocus.RequestFocus(ancestor, {bFocusDescendant})
                self.queued_focus = Some((a, true));
            } else if self.n(id).has_focus {
                self.transfer_focus(FocusSource::Application, None, None);
            }
        }
        if let Some(p) = self.n(id).parent {
            // RemoveChild: active child cleared, last index NOT adjusted (sp.js:81692).
            if self.n(p).active_child == Some(id) {
                self.nm(p).active_child = None;
            }
            self.nm(p).children.retain(|&c| c != id);
        }
        self.nm(id).alive = false;
    }

    // ------------------------------------------------------------------ small accessors (ports)

    /// `GetFocusable()` (sp.js:81790): "none" | "self" | "children".
    fn focusable_kind(&self, id: NodeId) -> Focusable {
        let n = self.n(id);
        let p = &n.props;
        if !n.mounted || p.can_take_focus.as_ref().map(|f| !f(id)).unwrap_or(false) {
            return Focusable::None;
        }
        if p.focusable || (p.focusable_if_empty && (p.child_focus_disabled || n.children.is_empty())) {
            Focusable::SelfNode
        } else if !p.child_focus_disabled && !n.children.is_empty() {
            Focusable::Children
        } else {
            Focusable::None
        }
    }
    /// `GetLayout()` (sp.js:81820).
    pub fn layout(&self, id: NodeId) -> Layout {
        let n = self.n(id);
        if n.props.layout != Layout::None {
            n.props.layout
        } else if n.children.len() < 2 {
            Layout::None
        } else {
            n.props.css_layout.unwrap_or(Layout::Column)
        }
    }
    fn valid_idx(&self, id: NodeId, i: i32) -> bool {
        i >= 0 && (i as usize) < self.n(id).children.len()
    }
    /// `GetActiveChild()` (sp.js:81708).
    pub fn active_child(&self, id: NodeId) -> Option<NodeId> {
        let n = self.n(id);
        if let Some(a) = n.active_child {
            return Some(a);
        }
        if self.valid_idx(id, n.last_active_idx) {
            Some(n.children[n.last_active_idx as usize])
        } else {
            None
        }
    }
    /// `GetActiveChildIndex()` (sp.js:81719) — re-syncs the cached index.
    fn active_child_index(&mut self, id: NodeId) -> i32 {
        if let Some(a) = self.n(id).active_child {
            let i = self.n(id).last_active_idx;
            if self.valid_idx(id, i) && self.n(id).children[i as usize] == a {
                return i;
            }
            let ni = self.n(id).children.iter().position(|&c| c == a).map(|i| i as i32).unwrap_or(-1);
            self.nm(id).last_active_idx = ni;
        }
        self.n(id).last_active_idx
    }
    /// `GetActiveDescendant()` (sp.js:81702).
    pub fn active_descendant(&self, id: NodeId) -> NodeId {
        match self.active_child(id) {
            Some(c) => self.active_descendant(c),
            None => id,
        }
    }
    /// `SetActiveChild(node | index)` (sp.js:82262).
    fn set_active_idx(&mut self, id: NodeId, i: i32) {
        if self.valid_idx(id, i) {
            let c = self.n(id).children[i as usize];
            self.nm(id).active_child = Some(c);
        }
        self.nm(id).last_active_idx = i;
    }
    pub(crate) fn set_active_idx_pub(&mut self, id: NodeId, i: i32) {
        self.set_active_idx(id, i)
    }
    fn set_active_node(&mut self, id: NodeId, c: NodeId) {
        let i = self.n(id).children.iter().position(|&x| x == c).map(|i| i as i32).unwrap_or(-1);
        self.nm(id).active_child = Some(c);
        self.nm(id).last_active_idx = i;
    }
    /// `UpdateParentActiveChild()` (sp.js:81778).
    fn update_parent_active_child(&mut self, id: NodeId) {
        if let Some(p) = self.n(id).parent {
            self.set_active_node(p, id);
            self.update_parent_active_child(p);
        }
    }
    /// `GetChildren()` -> (children, lastActiveChildIndex) (sp.js:82254).
    pub fn children_and_active_idx(&self, id: NodeId) -> (&[NodeId], i32) {
        (&self.n(id).children, self.n(id).last_active_idx)
    }

    // ------------------------------------------------------------------ direction math

    /// `ComputeRelativeDirection(button, layout)` (sp.js:81860).
    pub fn relative_direction(button: Button, layout: Layout) -> Rel {
        let rev = matches!(layout, Layout::RowReverse | Layout::ColumnReverse);
        match layout {
            Layout::Row | Layout::RowReverse => match button {
                Button::DirLeft => if rev { Rel::Forward } else { Rel::Backward },
                Button::DirRight => if rev { Rel::Backward } else { Rel::Forward },
                _ => Rel::Invalid,
            },
            Layout::Column | Layout::ColumnReverse => match button {
                Button::DirUp => if rev { Rel::Forward } else { Rel::Backward },
                Button::DirDown => if rev { Rel::Backward } else { Rel::Forward },
                _ => Rel::Invalid,
            },
            Layout::Grid | Layout::Geometric => match button {
                Button::DirLeft | Button::DirUp => if rev { Rel::Forward } else { Rel::Backward },
                Button::DirRight | Button::DirDown => if rev { Rel::Backward } else { Rel::Forward },
                _ => Rel::Invalid,
            },
            Layout::None => Rel::Invalid,
        }
    }

    // ------------------------------------------------------------------ finding focus targets

    /// `FindFocusableNode(dir, rect)` (sp.js:81747).
    fn find_focusable_node(&self, id: NodeId, dir: Option<Button>, rect: Option<Rect>) -> Option<NodeId> {
        match self.focusable_kind(id) {
            Focusable::None => None,
            Focusable::SelfNode => Some(id),
            Focusable::Children => self.find_focusable_descendant(id, dir, rect),
        }
    }

    /// `FindFocusableDescendant(dir, rect)` (sp.js:81768-81830): where focus lands when entering a container.
    pub fn find_focusable_descendant(&self, id: NodeId, dir: Option<Button>, rect: Option<Rect>) -> Option<NodeId> {
        let m = dir.and_then(|b| b.axis());
        let p = &self.n(id).props;
        if p.child_focus_disabled {
            return None;
        }
        if self.n(id).children.is_empty() {
            return None;
        }
        let q = p.nav_entry_prefer_position;
        let mut v = {
            // GetActiveChildIndex without mutation
            let n = self.n(id);
            if let Some(a) = n.active_child {
                if self.valid_idx(id, n.last_active_idx) && n.children[n.last_active_idx as usize] == a {
                    n.last_active_idx
                } else {
                    n.children.iter().position(|&c| c == a).map(|i| i as i32).unwrap_or(-1)
                }
            } else {
                n.last_active_idx
            }
        };
        if p.reset_nav_on_entry && dir.is_some() {
            v = -1;
        }
        let layout = self.layout(id);
        let len = self.n(id).children.len() as i32;
        if !self.valid_idx(id, v) {
            v = if v >= len || layout == Layout::RowReverse || layout == Layout::ColumnReverse || q == Some(EntryPrefer::Last) { len - 1 } else { 0 };
        }
        let mut k: Option<NodeId> = None;
        let a: Option<Axis> = match q {
            Some(EntryPrefer::MaintainX) => Some(Axis::X),
            Some(EntryPrefer::MaintainY) => Some(Axis::Y),
            _ => {
                if layout == Layout::Geometric && m.is_some() {
                    m.map(|m| m.tangent())
                } else {
                    None
                }
            }
        };
        if (a.is_some() || rect.is_some()) && m.is_some() {
            let m = m.unwrap();
            let last = self.last_focus;
            if rect.is_some() || (last.is_some() && a == Some(m.tangent())) {
                let h = rect.unwrap_or_else(|| self.adjust_rect_for_last_movement(self.rect(last.unwrap()), m));
                k = self.find_closest_focusable_node_to_rect(id, dir, h);
            } else if a != Some(m.tangent()) {
                let h = Self::relative_direction(dir.unwrap(), Layout::Grid);
                let y = if h == Rel::Backward { len } else { -1 };
                k = self.find_next_focusable_child_in_direction(id, y, h, dir);
            }
        } else if q == Some(EntryPrefer::PreferredChild) {
            let mut level: Vec<NodeId> = self.n(id).children.clone();
            while !level.is_empty() {
                let mut next = vec![];
                for &c in &level {
                    let kk = if self.n(c).props.preferred_focus { self.find_focusable_node(c, dir, None) } else { None };
                    if kk.is_some() {
                        return kk;
                    }
                    next.extend(self.n(c).children.iter().copied());
                }
                level = next;
            }
        } else if q == Some(EntryPrefer::Last) {
            k = self.find_next_focusable_child_in_direction(id, v + 1, Rel::Backward, dir);
        }
        if k.is_none() {
            k = self.find_next_focusable_child_in_direction(id, v - 1, Rel::Forward, dir);
        }
        if k.is_none() {
            k = self.find_next_focusable_child_in_direction(id, v, Rel::Backward, dir);
        }
        if k.is_none() && p.focusable_if_empty {
            k = Some(id);
        }
        k
    }

    /// `FindNextFocusableChildInDirection(startIdx, rel, button)` (sp.js:81933): linear scan, no wrap.
    fn find_next_focusable_child_in_direction(&self, id: NodeId, start: i32, rel: Rel, dir: Option<Button>) -> Option<NodeId> {
        let t: i32 = if rel == Rel::Forward { 1 } else { -1 };
        let ch = &self.n(id).children;
        let mut l = start + t;
        while l >= 0 && (l as usize) < ch.len() {
            if let Some(q) = self.find_focusable_node(ch[l as usize], dir, None) {
                return Some(q);
            }
            l += t;
        }
        None
    }
    /// `ScanChildren(start, rel, pred)` (sp.js:81942).
    fn scan_children(&self, id: NodeId, start: i32, rel: Rel, mut pred: impl FnMut(NodeId) -> bool) -> i32 {
        let t: i32 = if rel == Rel::Forward { 1 } else { -1 };
        let ch = &self.n(id).children;
        let mut l = start;
        while l >= 0 && (l as usize) < ch.len() {
            if pred(ch[l as usize]) {
                return l;
            }
            l += t;
        }
        -1
    }

    /// `FindNextFocusableChild(button)` (sp.js:81870): the sibling that a D-pad press would move to, if any.
    pub fn find_next_focusable_child(&mut self, id: NodeId, button: Button) -> Option<NodeId> {
        let layout = self.layout(id);
        let rel = Self::relative_direction(button, layout);
        if rel == Rel::Invalid || (self.n(id).props.focusable && self.n(id).has_focus) {
            return None;
        }
        // EnsureChildrenSorted(true): children already kept in DOM order.
        match layout {
            Layout::Grid => {
                let i = self.active_child_index(id);
                self.find_next_focusable_child_in_grid(id, i, rel, button)
            }
            Layout::Geometric => self.find_next_focusable_child_geometric(id, rel, button),
            _ => {
                let mut t = self.active_child_index(id);
                if !self.valid_idx(id, t) {
                    t = if rel == Rel::Forward { -1 } else { self.n(id).children.len() as i32 };
                }
                self.find_next_focusable_child_in_direction(id, t, rel, Some(button))
            }
        }
    }

    /// `FindNextFocusableChildInGrid` (sp.js:81995-82033).
    fn find_next_focusable_child_in_grid(&mut self, id: NodeId, b: i32, rel: Rel, button: Button) -> Option<NodeId> {
        let vertical = matches!(button, Button::DirUp | Button::DirDown);
        if self.active_child(id).is_none() {
            // assert "No active child for grid navigation" -> FindFocusableDescendant(button)
            return self.find_focusable_descendant(id, Some(button), None);
        }
        let mut q = self.rect(self.active_descendant(id));
        if vertical {
            q = self.adjust_rect_for_last_movement(q, Axis::Y);
        }
        let adv = |i: i32| i + if rel == Rel::Forward { 1 } else { -1 };
        if vertical {
            let mut e = b;
            while e != -1 {
                let v = self.scan_children(id, adv(e), rel, |c| !geom::overlaps(Axis::Y, &q, &self.rect(c)));
                if v != -1 {
                    let first = self.rect(self.n(id).children[v as usize]);
                    if let Some(k) = self.find_closest_child_in_next_axially_aligned_set(id, Axis::X, rel, button, &q, v, Some(first)) {
                        return Some(k);
                    }
                }
                e = v;
            }
        } else {
            let step: i32 = if rel == Rel::Forward { 1 } else { -1 };
            let mut v = adv(b);
            while v >= 0 && (v as usize) < self.n(id).children.len() {
                let c = self.n(id).children[v as usize];
                if !geom::overlaps(Axis::Y, &q, &self.rect(c)) {
                    return None; // left/right never wraps to another row
                }
                if let Some(k) = self.find_focusable_node(c, Some(button), None) {
                    return Some(k);
                }
                v += step;
            }
        }
        None
    }

    /// `FindNextFocusableChildGeometric` (sp.js:82034-82052).
    fn find_next_focusable_child_geometric(&mut self, id: NodeId, rel: Rel, button: Button) -> Option<NodeId> {
        if self.active_child(id).is_none() {
            return self.find_focusable_descendant(id, Some(button), None);
        }
        let axis = button.axis()?;
        let l = self.adjust_rect_for_last_movement(self.rect(self.active_descendant(id)), axis);
        self.find_closest_child_in_direction(id, axis, l, rel, button)
    }

    /// `AdjustRectForLastMovementOnTangentAxis(rect, axis)` (sp.js:82053): keep the column (or row) of the previous move.
    fn adjust_rect_for_last_movement(&self, b: Rect, axis: Axis) -> Rect {
        match self.last_movement_rect(axis.tangent()) {
            Some(m) => match axis {
                Axis::X => Rect { left: b.left, right: b.right, top: m.top, bottom: m.bottom },
                Axis::Y => Rect { left: m.left, right: m.right, top: b.top, bottom: b.bottom },
            },
            None => b,
        }
    }
    /// `GetLastFocusedMovementRect(axis)` — live rect if the node still exists, else the cached one (class `Ze`).
    pub fn last_movement_rect(&self, axis: Axis) -> Option<Rect> {
        let m = match axis {
            Axis::X => &self.move_x,
            Axis::Y => &self.move_y,
        };
        match m.node {
            Some(n) if self.is_alive(n) => self.n(n).rect.or(m.rect),
            _ => m.rect,
        }
    }

    /// `FindClosestFocusableNodeToRect(dir, rect)` (sp.js:82066).
    fn find_closest_focusable_node_to_rect(&self, id: NodeId, dir: Option<Button>, rect: Rect) -> Option<NodeId> {
        let t = dir.and_then(|d| d.axis()).map(|a| a.tangent());
        let mut l: Vec<(NodeId, f32, f32)> = vec![]; // (child, overlap, dist)
        for &q in &self.n(id).children {
            if let Some(e) = self.n(q).rect {
                let v = geom::rect_distance(&rect, &e);
                let b = t.map(|t| geom::overlap_amount(t, &rect, &e)).unwrap_or(0.0);
                l.push((q, b, v));
            }
        }
        l.sort_by(|q, e| {
            if q.2 != e.2 {
                q.2.partial_cmp(&e.2).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                e.1.partial_cmp(&q.1).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        for (q, ..) in l {
            if let Some(e) = self.find_focusable_node(q, dir, Some(rect)) {
                return Some(e);
            }
        }
        None
    }

    /// `FindClosetChildInDirection(axis, rect, rel, button)` (sp.js:82082): geometric layout.
    fn find_closest_child_in_direction(&self, id: NodeId, axis: Axis, o: Rect, rel: Rel, button: Button) -> Option<NodeId> {
        let (lmin, lmax) = geom::range(axis, &o);
        let mut q: Vec<(NodeId, f32, f32)> = vec![]; // (child, overlap, dist)
        for &e in &self.n(id).children {
            if let Some(v) = self.n(e).rect {
                let (bmin, bmax) = geom::range(axis, &v);
                let k = if rel == Rel::Forward { bmin - lmax } else { lmin - bmax };
                if k >= 0.0 {
                    q.push((e, geom::overlap_amount(axis.tangent(), &o, &v), k));
                }
            }
        }
        q.sort_by(|e, v| {
            // overlap-having entries first, then smaller distance, then larger overlap
            if e.1 != 0.0 {
                if v.1 == 0.0 {
                    return std::cmp::Ordering::Less;
                }
            } else if v.1 != 0.0 {
                return std::cmp::Ordering::Greater;
            }
            // total_cmp: a NaN distance must not panic inside the sort (UI thread)
            let b = e.2 - v.2;
            if b != 0.0 {
                b.total_cmp(&0.0)
            } else {
                (v.1 - e.1).total_cmp(&0.0)
            }
        });
        for (e, ..) in q {
            if let Some(v) = self.find_focusable_node(e, Some(button), Some(o)) {
                return Some(v);
            }
        }
        None
    }

    /// `FindClosestChildInNextAxiallyAlignedSet(axis, rel, button, refRect, startIdx, firstRect)` (sp.js:82119):
    /// collect the next visual row, rank by x-overlap with the *remembered* column, tie -> nearest left edge.
    #[allow(clippy::too_many_arguments)]
    fn find_closest_child_in_next_axially_aligned_set(&self, id: NodeId, axis: Axis, rel: Rel, button: Button, reference: &Rect, start: i32, first: Option<Rect>) -> Option<NodeId> {
        let start = if start < 0 { 0 } else { start };
        let origin = geom::origin(reference);
        let mut e: Vec<(NodeId, f32, f32)> = vec![]; // child, overlap, dist
        self.scan_children(id, start, rel, |b| {
            let Some(k) = self.n(b).rect else { return false };
            if let Some(q) = first {
                if !geom::overlaps(axis.tangent(), &q, &k) {
                    return true; // next row reached -> stop
                }
            }
            e.push((b, geom::overlap_amount(axis, reference, &k), geom::point_dist(axis, origin, &k)));
            false
        });
        if rel == Rel::Backward {
            e.reverse();
        }
        e.sort_by(|b, k| {
            let a = k.1 - b.1;
            if a != 0.0 {
                a.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                (b.2 - k.2).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        for (b, ..) in e {
            if let Some(k) = self.find_focusable_node(b, Some(button), Some(*reference)) {
                return Some(k);
            }
        }
        None
    }

    // ------------------------------------------------------------------ navigation events

    /// `OnNavigationEvent` (sp.js:81839): internal navigation first, then onMoveX; true = handled.
    fn on_navigation_event(&mut self, id: NodeId, button: Button, is_repeat: bool) -> bool {
        if self.try_internal_navigation(id, button, is_repeat) {
            return true;
        }
        let idx = match button {
            Button::DirUp => 0,
            Button::DirRight => 1,
            Button::DirDown => 2,
            Button::DirLeft => 3,
            _ => return false,
        };
        match self.n(id).props.on_move[idx].clone() {
            Some(h) => h(id),
            None => false,
        }
    }

    /// `BTryInternalNavigation` (sp.js:81905).
    fn try_internal_navigation(&mut self, id: NodeId, button: Button, is_repeat: bool) -> bool {
        let Some(m) = self.find_next_focusable_child(id, button) else { return false };
        let axis = button.axis();
        let rel = Self::relative_direction(button, self.layout(id));
        if self.scroll_type(id) == ScrollType::NoTransformSparseContent || self.scroll_type(m) == ScrollType::NoTransformSparseContent {
            let (vw, vh) = self.viewport;
            let e = (if axis == Some(Axis::Y) { vh } else { vw }) / if is_repeat { 4.5 } else { 3.33 };
            let v = self.rect(m); // QE(element): position relative to the viewport
            let far = match (axis, rel) {
                (Some(Axis::Y), Rel::Forward) => v.top > vh && v.bottom > vh + e,
                (Some(Axis::Y), Rel::Backward) => v.bottom < 0.0 && v.top < -e,
                (Some(Axis::X), Rel::Forward) => v.left > vw && v.right > vw + e,
                (Some(Axis::X), Rel::Backward) => v.right < 0.0 && v.left < -e,
                _ => false,
            };
            if far {
                // Pl(m, m, "smooth", axis, E): scroll E px towards the target, focus stays
                self.scroll_node_into_view(m, Behavior::Smooth, axis, Some(e));
                return true;
            }
        }
        self.transfer_focus(FocusSource::Gamepad, Some(m), Some(button));
        true
    }

    /// `GetScrollIntoViewType()` (sp.js:81935): inherited down the parent chain; default Standard.
    pub fn scroll_type(&self, id: NodeId) -> ScrollType {
        match self.n(id).props.scroll_into_view_type {
            Some(t) => t,
            None => match self.n(id).parent {
                Some(p) => self.scroll_type(p),
                None => ScrollType::Standard,
            },
        }
    }

    /// `BHasNavTargetInDirection(button, bCrossTrees)` (sp.js:81874).
    pub fn has_nav_target_in_direction(&mut self, id: NodeId, button: Button) -> bool {
        let idx = match button {
            Button::DirUp => Some(0),
            Button::DirRight => Some(1),
            Button::DirDown => Some(2),
            Button::DirLeft => Some(3),
            _ => None,
        };
        if let Some(i) = idx {
            if self.n(id).props.on_move[i].is_some() {
                return true;
            }
        }
        self.find_next_focusable_child(id, button).is_some()
    }

    // ------------------------------------------------------------------ TransferFocus

    /// `TransferFocus(source, node, dir, isRepeat)` -> `TransferFocusInternal` (sp.js:462062).
    pub fn transfer_focus(&mut self, source: FocusSource, node: Option<NodeId>, dir: Option<Button>) {
        self.transfer_focus_internal(source, node, dir, false)
    }
    fn transfer_focus_internal(&mut self, source: FocusSource, node: Option<NodeId>, dir: Option<Button>, is_repeat: bool) {
        let axis = dir.and_then(|d| d.axis());
        let prev = self.last_focus;
        if prev == node && (self.was_active_for_last_focus_change || !self.active) {
            return;
        }
        let common = self.common_ancestor(prev, node);
        if let Some(p) = prev {
            self.nm(p).has_focus = false;
            let mut c = Some(p);
            while let Some(x) = c {
                if Some(x) == common {
                    break;
                }
                self.nm(x).focus_within = false;
                c = self.n(x).parent;
            }
        }
        if let Some(nn) = node {
            self.nm(nn).has_focus = true;
            let mut c = Some(nn);
            while let Some(x) = c {
                if Some(x) == common {
                    break;
                }
                self.nm(x).focus_within = true;
                c = self.n(x).parent;
            }
        }
        if let Some(nn) = node {
            // SetDOMFocusAndScroll (sp.js:82266)
            self.update_parent_active_child(nn);
            if self.active {
                self.scroll_into_view(nn, prev, source, dir, is_repeat);
            }
        }
        self.events.push(NavEvent::Focus { from: prev, to: node, source, dir });
        if source == FocusSource::Gamepad && node.map(|n| !self.n(n).props.disable_nav_sounds).unwrap_or(false) {
            self.events.push(NavEvent::BasicNav);
        }
        self.last_focus = node;
        self.was_active_for_last_focus_change = self.active;
        // movement rects (sp.js:462095)
        let rect = node.map(|n| self.rect(n));
        match axis {
            Some(Axis::X) => self.move_x = MovementRect { node, rect },
            Some(Axis::Y) => self.move_y = MovementRect { node, rect },
            None => {
                self.move_x = MovementRect::default();
                self.move_y = MovementRect::default();
            }
        }
    }
    /// `ht(a, b)` (sp.js:462108): lowest common ancestor.
    fn common_ancestor(&self, a: Option<NodeId>, b: Option<NodeId>) -> Option<NodeId> {
        let (mut x, mut y) = (b?, a?);
        while self.n(x).depth > self.n(y).depth && self.n(x).parent.is_some() {
            x = self.n(x).parent.unwrap();
        }
        while self.n(y).depth > self.n(x).depth && self.n(y).parent.is_some() {
            y = self.n(y).parent.unwrap();
        }
        while x != y {
            match (self.n(x).parent, self.n(y).parent) {
                (Some(px), Some(py)) => {
                    x = px;
                    y = py;
                }
                _ => return None,
            }
        }
        Some(x)
    }

    // ------------------------------------------------------------------ take focus API (handle methods)

    /// `BTakeFocus(source, dir)` (sp.js:81745).
    pub fn take_focus(&mut self, id: NodeId, source: FocusSource, dir: Option<Button>) -> bool {
        let m = self.find_focusable_node(id, dir, None);
        match m {
            Some(m) => {
                self.transfer_focus(source, Some(m), dir);
                true
            }
            None => false,
        }
    }
    /// `BChildTakeFocus` (sp.js:81758).
    pub fn child_take_focus(&mut self, id: NodeId, source: FocusSource, dir: Option<Button>) -> bool {
        match self.find_focusable_descendant(id, dir, None) {
            Some(m) => {
                self.transfer_focus(source, Some(m), dir);
                true
            }
            None => false,
        }
    }
    /// `BFocusFirstChild` / `BFocusLastChild` (sp.js:81762-81767).
    pub fn focus_first_child(&mut self, id: NodeId, source: FocusSource) -> bool {
        match self.find_next_focusable_child_in_direction(id, -1, Rel::Forward, None) {
            Some(m) => {
                self.transfer_focus(source, Some(m), None);
                true
            }
            None => false,
        }
    }
    pub fn focus_last_child(&mut self, id: NodeId, source: FocusSource) -> bool {
        let len = self.n(id).children.len() as i32;
        match self.find_next_focusable_child_in_direction(id, len, Rel::Backward, None) {
            Some(m) => {
                self.transfer_focus(source, Some(m), None);
                true
            }
            None => false,
        }
    }
    /// Tree `TakeFocus()` (sp.js:461972): last focused node, else root, else root itself.
    pub fn tree_take_focus(&mut self, source: FocusSource) {
        let mut ok = false;
        if let Some(l) = self.last_focus {
            ok = self.take_focus(l, source, None);
        }
        if !ok {
            ok = self.take_focus(self.root, source, None);
        }
        if !ok {
            let r = self.root;
            self.transfer_focus(source, Some(r), None);
        }
    }
    /// Run queued auto-focus requests (DeferredFocus.ExecuteQueuedFocus).
    pub fn flush_deferred_focus(&mut self) {
        if let Some((id, desc)) = self.queued_focus.take() {
            if !self.is_alive(id) {
                return;
            }
            if desc {
                self.child_take_focus(id, FocusSource::Application, None);
            } else {
                self.take_focus(id, FocusSource::Application, None);
            }
        }
    }

    // ------------------------------------------------------------------ visible child (module 48294)

    /// `BVisibleChildTakeFocus` (sp.js:81831): focus the child that is best visible in the window.
    pub fn visible_child_take_focus(&mut self, id: NodeId, source: FocusSource) -> bool {
        let vp = Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1);
        match self.best_visible(id, &vp, None) {
            Some((child, _)) => self.take_focus(child, source, None),
            None => false,
        }
    }
    fn best_visible(&self, u: NodeId, l: &Rect, d: Option<Rect>) -> Option<(NodeId, Visibility)> {
        let (c, _) = self.children_and_active_idx(u);
        let c = c.to_vec();
        let p = self.active_child(u);
        let mut h: Vec<(NodeId, Visibility)> = vec![];
        let mut w: Option<(NodeId, Visibility)> = None;
        let j = p.and_then(|p| self.n(p).rect);
        if let Some(p) = p {
            let nvis = self.visibility(p, l, d);
            if let Some(nv) = &nvis {
                if nv.off_screen.is_none() {
                    w = self.visible_inner(p, nv, nv.overlap, d);
                    if let Some(ww) = &w {
                        if ww.1.off_screen.is_none() {
                            return w;
                        }
                    }
                }
            }
            if let Some(nv) = nvis {
                h.push((p, nv));
            }
        }
        let y = d.or(j);
        for &x in &c {
            if Some(x) == p {
                continue;
            }
            if let Some(g) = self.visibility(x, l, y) {
                h.push((x, g));
            }
        }
        h.sort_by(|a, b| Self::cmp_vis(&a.1, &b.1));
        let mut best: Option<(NodeId, Visibility)> = None;
        for (x, g) in h {
            if g.off_screen.is_some() {
                if let Some(i) = &best {
                    if i.1.off_screen.is_none() || g.distance.unwrap_or(0.0) > i.1.distance.unwrap_or(0.0) {
                        break;
                    }
                }
            }
            let cand = if Some(x) == p { w.clone() } else { self.visible_inner(x, &g, g.overlap.or(Some(*l)), y) };
            if let Some(cc) = cand {
                if best.as_ref().map(|i| Self::cmp_vis(&cc.1, &i.1) == std::cmp::Ordering::Less).unwrap_or(true) {
                    best = Some(cc);
                }
            }
        }
        best
    }
    fn visible_inner(&self, u: NodeId, l: &Visibility, d: Option<Rect>, h: Option<Rect>) -> Option<(NodeId, Visibility)> {
        match self.focusable_kind(u) {
            Focusable::None => None,
            Focusable::Children => self.best_visible(u, &l.overlap.or(d).unwrap_or_default(), h),
            Focusable::SelfNode => Some((u, l.clone())),
        }
    }
    fn cmp_vis(d: &Visibility, h: &Visibility) -> std::cmp::Ordering {
        let dd = d.distance.unwrap_or(0.0);
        let hd = h.distance.unwrap_or(0.0);
        match (d.off_screen.is_some(), h.off_screen.is_some()) {
            (true, true) => dd.partial_cmp(&hd).unwrap_or(std::cmp::Ordering::Equal),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => dd.partial_cmp(&hd).unwrap_or(std::cmp::Ordering::Equal),
        }
    }
    /// `n(node, viewport, ref)` (sp.js:82216).
    fn visibility(&self, u: NodeId, l: &Rect, d: Option<Rect>) -> Option<Visibility> {
        let h = self.n(u).rect?;
        let c = self.focusable_kind(u);
        let mut f = None;
        match c {
            Focusable::None => return None,
            Focusable::SelfNode => {
                if h.top < l.top || h.right > l.right || h.bottom > l.bottom || h.left < l.left {
                    let w = h.top + h.height() / 2.0;
                    let j = h.left + h.width() / 2.0;
                    if w < l.top {
                        return Some(Visibility::off("top", l.top - w));
                    }
                    if j > l.right {
                        return Some(Visibility::off("right", j - l.right));
                    }
                    if w > l.bottom {
                        return Some(Visibility::off("bottom", w - l.bottom));
                    }
                    if j < l.left {
                        return Some(Visibility::off("left", l.left - j));
                    }
                }
                if let Some(d) = d {
                    f = Some(geom::rect_distance(&h, &d));
                }
            }
            Focusable::Children => {
                if h.bottom < l.top {
                    return Some(Visibility::off("top", l.top - h.bottom));
                }
                if h.left > l.right {
                    return Some(Visibility::off("right", h.left - l.right));
                }
                if h.top > l.bottom {
                    return Some(Visibility::off("bottom", h.top - l.bottom));
                }
                if h.right < l.left {
                    return Some(Visibility::off("left", l.left - h.right));
                }
            }
        }
        Some(Visibility {
            off_screen: None,
            distance: f,
            overlap: Some(Rect { top: h.top.max(l.top), right: h.right.min(l.right), bottom: h.bottom.min(l.bottom), left: h.left.max(l.left) }),
        })
    }

    // ------------------------------------------------------------------ scroll-into-view (module 84719)

    pub fn set_scroll(&mut self, id: Option<NodeId>, top: f32, left: f32) {
        let sb = match id {
            Some(i) => self.nm(i).scroll.as_mut(),
            None => Some(&mut self.window_scroll),
        };
        if let Some(sb) = sb {
            sb.top = top;
            sb.left = left;
            sb.target_top = None;
            sb.target_left = None;
        }
    }
    pub fn scroll_box(&self, id: Option<NodeId>) -> Option<&ScrollBox> {
        match id {
            Some(i) => self.n(i).scroll.as_ref(),
            None => Some(&self.window_scroll),
        }
    }

    /// `P(node, prevNode, source, dir, isRepeat)` (sp.js:82343 `ZQ`): scroll the focused node (and the ancestors with
    /// scrollIntoViewWhenChildFocused) into view; instant vs smooth decision included.
    fn scroll_into_view(&mut self, j: NodeId, prev: Option<NodeId>, source: FocusSource, dir: Option<Button>, is_repeat: bool) {
        // chain of (node, scrollType) — sp.js:82350
        let mut he: Vec<(NodeId, Option<ScrollType>)> = vec![(j, self.n(j).props.scroll_into_view_type)];
        let mut a = self.n(j).parent;
        while let Some(ae) = a {
            let re = self.n(ae).props.scroll_into_view_when_child_focused;
            let se = self.n(ae).props.scroll_into_view_type;
            if re != ScrollWhenChild::No {
                let v = (ae, se);
                if re == ScrollWhenChild::Force {
                    he = vec![v];
                } else {
                    he.push(v);
                }
            }
            if se.is_some() {
                let mut v = he.len() as i32 - 1;
                while v >= 0 && he[v as usize].1.is_none() {
                    he[v as usize].1 = se;
                    v -= 1;
                }
            }
            a = self.n(ae).parent;
        }
        while let Some((ae, re)) = he.pop() {
            let se = he.is_empty();
            let re = re.unwrap_or(ScrollType::Standard); // `I` (debug flag) false => Standard
            if let Some(h) = self.n(ae).props.scroll_into_view_handler.clone() {
                if h(j, prev, ae) {
                    continue;
                }
            }
            let ee = re == ScrollType::NoTransform || re == ScrollType::NoTransformSparseContent || !se;
            if prev.is_none() && source != FocusSource::Gamepad {
                // "No previous element for scrolling, will jump"
                self.scroll_node_into_view(ae, Behavior::Auto, None, None);
            } else {
                // `De`: rect of the ancestor/element being scrolled into view (viewport-relative)
                let de = self.rect(ae);
                let oe = self.viewport.1;
                let fe = 40.0;
                let x = if is_repeat { fe } else { (oe * 0.4).max(fe) };
                let pe = self.last_jump_ms.map(|g| self.now_ms - g < MAX_JUMP_MS).unwrap_or(false);
                let jump = pe || de.bottom < -x || de.top > oe + x;
                if jump {
                    self.last_jump_ms = Some(self.now_ms);
                }
                let mut behavior = if jump { Behavior::Auto } else { Behavior::Smooth };
                if self.restoring_history {
                    behavior = Behavior::Auto;
                }
                let _ = ee; // Standard type uses the browser scrollIntoView(block:nearest): same geometry here
                self.scroll_node_into_view(ae, behavior, None, None);
            }
        }
        if let Some(d) = dir {
            self.scroll_region_to_start_or_end(j, d);
        }
    }

    /// `v(_, element, behavior, axis, maxDistance)` (sp.js:82560): NoTransform scroller. Walk up the scrollable
    /// ancestors computing the minimal delta (respecting scroll-padding / scroll-margin), then scrollTo.
    fn scroll_node_into_view(&mut self, start: NodeId, behavior: Behavior, only_axis: Option<Axis>, max_dist: Option<f32>) {
        struct Step {
            cont: Option<NodeId>, // None = window
            top: f32,
            left: f32,
        }
        let mut steps: Vec<Step> = vec![];
        let mut cur = start;
        let mut target = self.rect(start);
        let mut re = max_dist.unwrap_or(f32::MAX);
        let viewport_rect = Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1);
        loop {
            // `_f(he) ?? qf(he)`: nearest scrollable ancestor, else the window
            let mut cont: Option<NodeId> = None;
            let mut p = self.n(cur).parent;
            while let Some(x) = p {
                if self.n(x).scroll.is_some() {
                    cont = Some(x);
                    break;
                }
                p = self.n(x).parent;
            }
            let margin = self.n(cur).props.scroll_margin; // `L(he)`
            let (crect, sb, padding) = match cont {
                Some(c) => {
                    let sb = self.n(c).scroll.clone().unwrap();
                    (self.rect(c), sb.clone(), sb.padding)
                }
                None => (viewport_rect, self.window_scroll.clone(), self.window_scroll.padding),
            };
            let oe = scroll::shrink_by_padding(&crect, &padding);
            let mut x = Step { cont, top: 0.0, left: 0.0 };
            if only_axis.map(|a| a == Axis::Y).unwrap_or(true) && sb.overflows(Axis::Y) {
                x.top = scroll::nearest_delta(&target, &oe, &margin, true);
                x.top = x.top.max(-sb.scroll_top()).min(sb.max_top - sb.scroll_top());
                if max_dist.is_some() {
                    x.top = re.min(x.top.abs()) * if x.top < 0.0 { -1.0 } else { 1.0 };
                    re -= x.top.abs();
                }
            }
            if only_axis.map(|a| a == Axis::X).unwrap_or(true) && sb.overflows(Axis::X) {
                x.left = scroll::nearest_delta(&target, &oe, &margin, false);
                x.left = x.left.max(-sb.scroll_left()).min(sb.max_left - sb.scroll_left());
                if max_dist.is_some() {
                    x.left = re.min(x.left.abs()) * if x.left < 0.0 { -1.0 } else { 1.0 };
                    re -= x.left.abs();
                }
            }
            let (dl, dt) = (x.left, x.top);
            steps.push(x);
            if (max_dist.is_some() && re == 0.0) || cont.is_none() {
                break;
            }
            if sb.fixed {
                break;
            }
            cur = cont.unwrap();
            target = target.translate(-dl, -dt);
        }
        for s in steps {
            if scroll::negligible(s.left) && scroll::negligible(s.top) {
                continue;
            }
            let sb = self.scroll_box(s.cont).cloned().unwrap_or_default();
            let top = (sb.scroll_top() + s.top).max(0.0).min(sb.max_top);
            let left = (sb.scroll_left() + s.left).max(0.0).min(sb.max_left);
            if scroll::negligible(sb.scroll_left() - left) && scroll::negligible(sb.scroll_top() - top) {
                continue;
            }
            // remember the animation target like the wrapper `A` does (m_scrollTopTarget)
            let bm = match s.cont {
                Some(c) => self.nm(c).scroll.as_mut(),
                None => Some(&mut self.window_scroll),
            };
            if let Some(b) = bm {
                if behavior == Behavior::Smooth {
                    b.target_top = Some(top);
                    b.target_left = Some(left);
                } else {
                    b.top = top;
                    b.left = left;
                    b.target_top = None;
                    b.target_left = None;
                }
            }
            let target = match s.cont {
                Some(c) => ScrollTarget::Node(c),
                None => ScrollTarget::Window,
            };
            self.events.push(NavEvent::Scroll(ScrollReq { target, left: Some(left), top: Some(top), behavior }));
        }
    }

    /// `D(node, button)` + `z()` (sp.js:82462-82511): when the focused node cannot move any further in the direction and
    /// an ancestor has scrollRegionToStartOrEnd, smooth-scroll that region to its edge.
    fn scroll_region_to_start_or_end(&mut self, j: NodeId, button: Button) {
        let Some(axis) = button.axis() else { return };
        let mut region: Option<NodeId> = None;
        let mut r = Some(j);
        while let Some(x) = r {
            if self.n(x).props.scroll_region_to_start_or_end {
                region = Some(x);
            }
            r = self.n(x).parent;
        }
        let Some(region) = region else { return };
        let mut r = Some(j);
        while let Some(x) = r {
            if self.has_nav_target_in_direction(x, button) {
                break;
            }
            if x != j && self.n(x).props.scroll_region_to_start_or_end {
                self.scroll_element_to_edge(x, button, axis);
            }
            if x == region {
                break;
            }
            r = self.n(x).parent;
        }
    }
    fn scroll_element_to_edge(&mut self, el: NodeId, button: Button, axis: Axis) {
        let Some(sb) = self.n(el).scroll.clone() else { return };
        if !sb.overflows(axis) {
            return;
        }
        let (top, left) = match axis {
            Axis::Y => (Some(if button == Button::DirDown { sb.max_top } else { 0.0 }), None),
            Axis::X => (None, Some(if button == Button::DirRight { sb.max_left } else { 0.0 })),
        };
        if let Some(b) = self.nm(el).scroll.as_mut() {
            if let Some(t) = top {
                b.target_top = Some(t);
            }
            if let Some(l) = left {
                b.target_left = Some(l);
            }
        }
        self.events.push(NavEvent::Scroll(ScrollReq { target: ScrollTarget::Node(el), left, top, behavior: Behavior::Smooth }));
    }

    // ------------------------------------------------------------------ buttons

    /// Register a root-level button handler (`RegisterGlobalButtonHandler`, sp.js:462003).
    pub fn register_global_button(&mut self, b: Button, h: Handler) {
        self.global_buttons.push((b, h));
    }

    /// Entry point for a gamepad button press (`vgp_*` event bubbling from the focused element, then
    /// `HandleButtonDownEventAsLogicalEvent`, sp.js:462033). Returns true if something handled it.
    pub fn handle_button(&mut self, button: Button, is_repeat: bool) -> bool {
        self.flush_deferred_focus();
        // no focus yet -> the tree focuses its first candidate (TakeFocus), like the root does on activation
        if self.last_focus.is_none() {
            self.tree_take_focus(FocusSource::Application);
        }
        let mut cur = self.last_focus;
        let mut handled = false;
        while let Some(id) = cur {
            if button.is_dir() {
                if self.on_navigation_event(id, button, is_repeat) {
                    handled = true;
                    break;
                }
            } else {
                let hs: Vec<Handler> = self.n(id).props.on_button.iter().filter(|(b, _)| *b == button).map(|(_, h)| h.clone()).collect();
                if hs.into_iter().any(|h| h(id)) {
                    handled = true;
                    break;
                }
            }
            cur = self.n(id).parent;
        }
        if !handled {
            // TryRootButtonListeners
            for (b, h) in self.global_buttons.clone() {
                if b == button && h(self.root) {
                    handled = true;
                    break;
                }
            }
        }
        if !handled {
            self.events.push(if button.is_dir() { NavEvent::FailedNav } else { NavEvent::Unhandled(button) });
        }
        self.flush_deferred_focus();
        handled
    }

    // ------------------------------------------------------------------ history helpers (history.rs)
    pub(crate) fn node_children(&self, id: NodeId) -> Vec<NodeId> {
        self.n(id).children.clone()
    }
    pub(crate) fn node_last_idx(&self, id: NodeId) -> i32 {
        self.n(id).last_active_idx
    }
    pub(crate) fn nav_key_owned(&self, id: NodeId) -> Option<String> {
        self.n(id).props.nav_key.clone()
    }
    pub(crate) fn tree_is_active_context(&self) -> bool {
        self.active
    }
    pub(crate) fn keyed_children(&self, id: NodeId) -> HashMap<String, NodeId> {
        self.n(id).children.iter().filter_map(|&c| self.n(c).props.nav_key.clone().map(|k| (k, c))).collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focusable {
    None,
    SelfNode,
    Children,
}
#[derive(Clone, Debug)]
struct Visibility {
    off_screen: Option<&'static str>,
    distance: Option<f32>,
    overlap: Option<Rect>,
}
impl Visibility {
    fn off(side: &'static str, d: f32) -> Self {
        Visibility { off_screen: Some(side), distance: Some(d), overlap: None }
    }
}
