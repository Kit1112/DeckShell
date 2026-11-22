//! Port of Steam's gamepad focus-navigation engine (Big Picture UI build 11146165).
//! See docs/engine/navigation.json for the digest; every function cites its bundle location
//! (tools/cdp/build/pretty/sp.js:LINE).
//!
//! Layout:
//!   * [`geom`]    rect helpers (module 92393)
//!   * [`tree`]    nodes, layouts, find-next-focus, TransferFocus, scroll-into-view (modules 74562, 70391, 84719, 48294)
//!   * [`history`] focus memory across pages: serialize/restore by navKey (module 33787)
//!   * [`scroll`]  scroll model, smooth curves, ScrollPanel step (modules 84719, 50908)
//!   * [`repeat`]  D-pad auto-repeat 400/50 ms + axis lock (module 20505)
//!
//! Usage from a screen: obtain the global tree with [`with`], register nodes (`add_node`) with a layout and the rects of
//! the widgets, keep rects fresh with `set_rect`, and consume [`NavEvent`]s through [`set_sink`]. The input pipeline calls
//! [`intercept`]; with an empty tree (nothing registered yet) it declines and the legacy per-screen handlers run.
//!
//! Feature flag: `DECKSHELL_NAV=steam` (default) enables the engine, `DECKSHELL_NAV=off|legacy|0` disables it.



pub mod geom;
pub mod history;
pub mod repeat;
pub mod scope;
pub mod scroll;
pub mod tree;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

pub use geom::Rect;
pub use scroll::{Behavior, Edges, ScrollAnim, ScrollBox, ScrollReq, ScrollTarget};
pub use tree::{Button, FocusSource, Layout, NavEvent, NodeId, Props, ScrollWhenChild, Tree};

/// Is the engine enabled? (`DECKSHELL_NAV`, default on.)
pub fn enabled() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| !matches!(std::env::var("DECKSHELL_NAV").as_deref(), Ok("off") | Ok("legacy") | Ok("0") | Ok("false")))
}

fn clock_ms() -> f64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

type Sink = Box<dyn FnMut(NavEvent)>;
thread_local! {
    static TREE: RefCell<Tree> = RefCell::new(Tree::new((1280.0, 800.0)));
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Run `f` on the UI thread's global navigation tree.
pub fn with<R>(f: impl FnOnce(&mut Tree) -> R) -> R {
    TREE.with(|t| f(&mut t.borrow_mut()))
}

/// Receive every [`NavEvent`] (focus changes, scroll requests, sounds). One sink per process.
pub fn set_sink(s: impl FnMut(NavEvent) + 'static) {
    SINK.with(|c| *c.borrow_mut() = Some(Box::new(s)));
}

/// Deliver pending events to the sink (also returned for tests/tracing).
pub fn flush_events() -> Vec<NavEvent> {
    let ev = with(|t| t.drain_events());
    SINK.with(|c| {
        if let Some(s) = c.borrow_mut().as_mut() {
            for e in ev.iter().cloned() {
                s(e);
            }
        }
    });
    ev
}

/// Map the app's `Action` to Steam's `GamepadButton`.
pub fn button_of(a: crate::input::Action) -> Option<Button> {
    use crate::input::Action as A;
    Some(match a {
        A::Up => Button::DirUp,
        A::Down => Button::DirDown,
        A::Left => Button::DirLeft,
        A::Right => Button::DirRight,
        A::A => Button::Ok,
        A::B => Button::Cancel,
        A::X => Button::Secondary,
        A::Y => Button::Options,
        A::LB => Button::BumperLeft,
        A::RB => Button::BumperRight,
        A::L2 => Button::TriggerLeft,
        A::R2 => Button::TriggerRight,
        A::Menu => Button::Start,
        A::View => Button::Select,
        A::Steam => Button::SteamGuide,
        A::Qam => Button::QuickMenu,
        A::Search | A::Char(_) | A::Backspace | A::KbBack(_) | A::KbShift(_) | A::KbCaps | A::KbPad { .. } | A::KbClick { .. } | A::KbA(_) => return None,
    })
}

/// Input-pipeline hook (called from `input.rs::send` on the UI thread). Returns true when the engine consumed the
/// action; false -> the legacy per-screen handler must run. Declines while no screen has registered nodes.
pub fn intercept(a: crate::input::Action, is_repeat: bool) -> bool {
    if !enabled() {
        return false;
    }
    let Some(b) = button_of(a) else { return false };
    let handled = with(|t| {
        if t.children(t.root()).is_empty() {
            return false;
        }
        t.now_ms = clock_ms();
        t.handle_button(b, is_repeat)
    });
    flush_events();
    handled
}

/// TabBar L1/R1 index step (sp.js:347290-347330): `wrapAround` defaults to true.
/// Returns the new index, or `None` when running off the end without wrap (FailedNav, nothing happens).
pub fn tab_step(idx: usize, delta: i32, n: usize, wrap_around: Option<bool>) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let wrap = wrap_around.unwrap_or(true);
    let t = idx as i32 + delta;
    if wrap {
        Some(((t % n as i32 + n as i32) % n as i32) as usize)
    } else if t >= 0 && (t as usize) < n {
        Some(t as usize)
    } else {
        None
    }
}
