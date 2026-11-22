//! Focus memory across page navigation. Port of module 33787 (sp.js:81132-81240):
//! `SerializeNavState` / `RestoreSerializedNavState` + `PushState/PopState` (class `f`) and `SaveState/RestoreState`
//! (class `p`). Route pops (B) restore the previous page this way; restore scrolls instantly
//! (`Controller.RestoreHistoryTransaction` => `BIsRestoringHistory()`).

use std::collections::HashMap;

use super::tree::{FocusSource, NodeId, Tree};

/// Restore policy (`Er`/`Xh`/`GV`, sp.js:81138): re-focus if the tree had focus / always / never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreFocus {
    #[default]
    IfHadFocus = 0,
    Always = 1,
    Never = 2,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NavNodeState {
    /// `rgChildren`
    pub children: Option<Vec<NavNodeState>>,
    /// `sNavKey`
    pub nav_key: Option<String>,
    /// `iActiveChild` (-1 / absent = none)
    pub active: Option<i32>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NavState {
    pub root: NavNodeState,
    /// `bHadFocus`
    pub had_focus: bool,
}

#[derive(Default)]
pub struct History {
    stacks: HashMap<NodeId, Vec<NavState>>,
    saved: HashMap<(NodeId, String), NavState>,
}

impl Tree {
    /// `SerializeNavNode(node, includeChildren, includeAllChildren)` (sp.js:81150).
    pub fn serialize_node(&self, id: NodeId, include_children: bool, all: bool) -> NavNodeState {
        let (ch, c) = self.children_and_active_idx(id);
        let ch = ch.to_vec();
        let mut st = NavNodeState::default();
        if !ch.is_empty() && c != -1 && include_children {
            st.children = Some(ch.iter().enumerate().map(|(z, &d)| self.serialize_node(d, z as i32 == c || all, all)).collect());
        }
        if let Some(k) = self.nav_key_owned(id) {
            st.nav_key = Some(k);
        }
        if c != -1 {
            st.active = Some(c);
        }
        st
    }
    /// `SerializeNavState` (sp.js:81143).
    pub fn serialize_state(&self, id: NodeId, include_children: bool, all: bool) -> NavState {
        NavState { root: self.serialize_node(id, include_children, all), had_focus: self.focus_within(id) && self.tree_is_active_context() }
    }

    /// `RestoreSerializedNavState(node, state, policy)` (sp.js:81157).
    pub fn restore_state_snapshot(&mut self, id: NodeId, st: &NavState, policy: RestoreFocus) {
        // RestoreHistoryTransaction: scrolls are instant while restoring
        let prev = self.restoring_history;
        self.restoring_history = true;
        self.restore_node(id, &st.root);
        if policy == RestoreFocus::Always || (policy == RestoreFocus::IfHadFocus && st.had_focus) {
            self.take_focus(id, FocusSource::Application, None);
        }
        self.restoring_history = prev;
    }
    fn restore_node(&mut self, y: NodeId, i: &NavNodeState) {
        let g = i.active.unwrap_or(-1);
        self.set_active_idx_pub(y, g);
        let Some(c) = &i.children else { return };
        if c.is_empty() {
            return;
        }
        let d = self.node_children(y);
        let z = self.keyed_children(y);
        for o in c {
            let Some(key) = &o.nav_key else { continue };
            if let Some(&m) = z.get(key) {
                self.restore_node(m, o);
            }
        }
        if g != -1 {
            if let Some(key) = c.get(g as usize).and_then(|x| x.nav_key.clone()) {
                if let Some(&o) = z.get(&key) {
                    if let Some(i) = d.iter().position(|&x| x == o) {
                        self.set_active_idx_pub(y, i as i32);
                    }
                }
            }
        }
        // un-keyed nodes: restore by sequential position
        let (mut s, mut b) = (0usize, 0usize);
        while s < d.len() && b < c.len() {
            while s < d.len() && self.nav_key(d[s]).is_some() {
                s += 1;
            }
            while b < c.len() && c[b].nav_key.is_some() {
                b += 1;
            }
            if s >= d.len() || b >= c.len() {
                break;
            }
            self.restore_node(d[s], &c[b]);
            s += 1;
            b += 1;
        }
    }

    /// `PushState()` (class `f`).
    pub fn push_state(&mut self, id: NodeId) {
        let st = self.serialize_state(id, true, true);
        self.history.stacks.entry(id).or_default().push(st);
    }
    /// `PopState(policy)`.
    pub fn pop_state(&mut self, id: NodeId, policy: RestoreFocus) {
        if let Some(st) = self.history.stacks.get_mut(&id).and_then(|s| s.pop()) {
            self.restore_state_snapshot(id, &st, policy);
        }
    }
    /// `SaveState(key)` (class `p`).
    pub fn save_state(&mut self, id: NodeId, key: &str) {
        let st = self.serialize_state(id, true, true);
        self.history.saved.insert((id, key.to_string()), st);
    }
    /// `RestoreState(key, policy)` -> found?
    pub fn restore_state(&mut self, id: NodeId, key: &str, policy: RestoreFocus) -> bool {
        match self.history.saved.get(&(id, key.to_string())).cloned() {
            Some(st) => {
                self.restore_state_snapshot(id, &st, policy);
                true
            }
            None => false,
        }
    }
}
