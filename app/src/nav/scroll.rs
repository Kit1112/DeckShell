//! Scroll model + smooth scroll curves of the navigation engine.
//! Ports: scroller class `A` (sp.js:82645-82760, module 84719), animation base `r`/`o` (sp.js:82305-82385),
//! ScrollPanel hook `u`/`l` (sp.js:82840-82905, module 50908), library fast scroll curve (sp.js:516876-517135).

use super::geom::Rect;

/// `behavior` of Element.scrollTo: instant ("auto") or the 200 ms animated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    Auto,
    Smooth,
}

/// Easing functions (sp.js:82312-82333).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// default: `0.5 - cos(pi t)/2`
    Sine,
    Linear,
    /// `t<.5 ? 4t^3 : (t-1)(2t-2)^2+1`
    CubicInOut,
}
impl Timing {
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Timing::Linear => t,
            Timing::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    (t - 1.0) * (2.0 * t - 2.0) * (2.0 * t - 2.0) + 1.0
                }
            }
            Timing::Sine => 0.5 - (t * std::f32::consts::PI).cos() / 2.0,
        }
    }
}

/// Which scroller a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    Node(usize),
    Window,
}

/// "Scroll `target` to absolute (left, top)". The UI layer applies it (instantly, or through [`ScrollAnim`]) and
/// reports the real position back with `Tree::set_scroll`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollReq {
    pub target: ScrollTarget,
    pub left: Option<f32>,
    pub top: Option<f32>,
    pub behavior: Behavior,
}

/// State of one scrollable box (overflow:auto/scroll). Mirrors the `A` scroller wrapper (sp.js:82645): `top()/left()`
/// return the *target* of a running smooth scroll (`m_scrollTopTarget ?? scrollTop`).
#[derive(Debug, Clone, Default)]
pub struct ScrollBox {
    pub top: f32,
    pub left: f32,
    pub max_top: f32,
    pub max_left: f32,
    /// CSS overflow-y/x is auto|scroll (`aF`, sp.js:95116 region).
    pub overflow_y: bool,
    pub overflow_x: bool,
    /// position:fixed|sticky — ancestor walk stops here (sp.js:82585).
    pub fixed: bool,
    pub target_top: Option<f32>,
    pub target_left: Option<f32>,
    /// CSS scroll-padding-* of this box.
    pub padding: Edges,
}
impl ScrollBox {
    pub fn vertical(max_top: f32) -> Self {
        ScrollBox { max_top, overflow_y: true, ..Default::default() }
    }
    pub fn horizontal(max_left: f32) -> Self {
        ScrollBox { max_left, overflow_x: true, ..Default::default() }
    }
    pub fn scroll_top(&self) -> f32 {
        self.target_top.unwrap_or(self.top)
    }
    pub fn scroll_left(&self) -> f32 {
        self.target_left.unwrap_or(self.left)
    }
    pub fn overflows(&self, axis: super::geom::Axis) -> bool {
        match axis {
            super::geom::Axis::X => self.overflow_x,
            super::geom::Axis::Y => self.overflow_y,
        }
    }
}

/// CSS scroll-margin / scroll-padding edges in px.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Edges {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}
impl Edges {
    pub fn all(v: f32) -> Self {
        Edges { left: v, right: v, top: v, bottom: v }
    }
    pub fn top(v: f32) -> Self {
        Edges { top: v, ..Default::default() }
    }
}

/// `Q(container, rect)` (sp.js:82442): the container's rect shrunk by its scroll-padding.
pub fn shrink_by_padding(r: &Rect, p: &Edges) -> Rect {
    Rect {
        left: (r.left + p.left).max(0.0),
        right: (r.right - p.right).max(0.0),
        top: (r.top + p.top).max(0.0),
        bottom: (r.bottom - p.bottom).max(0.0),
    }
}

/// `O(target, container, margin, axis)` (sp.js:82513): minimal scroll delta that makes `target` visible inside
/// `container` ("nearest" semantics; a target larger than the container aligns its leading edge).
pub fn nearest_delta(target: &Rect, container: &Rect, margin: &Edges, y_axis: bool) -> f32 {
    let (r, z, he) = if y_axis { (target.top, target.bottom, target.height()) } else { (target.left, target.right, target.width()) };
    let (ae, re, se) = if y_axis { (container.top, container.bottom, container.height()) } else { (container.left, container.right, container.width()) };
    let (v, ee) = if y_axis { (margin.top, margin.bottom) } else { (margin.left, margin.right) };
    if r < ae && z > re {
        return 0.0;
    }
    let de = r - v;
    let oe = z + ee;
    let fe = he > se;
    if (de < ae && !fe) || (oe > re && fe) {
        de - ae
    } else if (de < ae && fe) || (oe > re && !fe) {
        oe - re
    } else {
        0.0
    }
}

/// `E(J)`: |v| < 1 counts as "no scroll" (sp.js:82585).
pub fn negligible(v: f32) -> bool {
    v > -1.0 && v < 1.0
}

/// A running scroll animation (class `r`, sp.js:82305): fixed duration, rAF-driven, easing per [`Timing`].
#[derive(Debug, Clone, Copy)]
pub struct ScrollAnim {
    pub from_top: f32,
    pub from_left: f32,
    pub to_top: f32,
    pub to_left: f32,
    pub start_ms: f64,
    pub duration_ms: f32,
    pub timing: Timing,
}
/// Duration of every `scrollTo({behavior:"smooth"})` of the NoTransform scroller (sp.js:82708 `msDuration: 200`).
pub const SMOOTH_MS: f32 = 200.0;

impl ScrollAnim {
    /// `A.scrollTo` smooth branch (sp.js:82694-82726). `running` = an animation was active: it gets cancelled and the new
    /// one is `linear`. Returns `None` when there is nothing to scroll.
    pub fn start(now_ms: f64, cur: (f32, f32), to: (f32, f32), running: bool) -> Option<ScrollAnim> {
        let timing = if running { Timing::Linear } else { Timing::Sine };
        let dist = (cur.1 - to.1).abs().max((cur.0 - to.0).abs());
        if dist > 0.0 {
            Some(ScrollAnim { from_left: cur.0, from_top: cur.1, to_left: to.0, to_top: to.1, start_ms: now_ms, duration_ms: SMOOTH_MS, timing })
        } else {
            None
        }
    }
    /// ScrollPanel D-pad scroll (hook `u`, sp.js:82840): `max(min(dist/1000*200, 500), 300)` ms.
    pub fn start_panel(now_ms: f64, cur: (f32, f32), to: (f32, f32), running: bool) -> Option<ScrollAnim> {
        let timing = if running { Timing::Linear } else { Timing::Sine };
        let dist = (cur.1 - to.1).abs().max((cur.0 - to.0).abs());
        if dist > 0.0 {
            let d = (dist / 1000.0 * 200.0).min(500.0).max(300.0);
            Some(ScrollAnim { from_left: cur.0, from_top: cur.1, to_left: to.0, to_top: to.1, start_ms: now_ms, duration_ms: d, timing })
        } else {
            None
        }
    }
    /// Library fast-scroll: 350 ms cubic-in-out (sp.js:516876-517135).
    pub fn start_fast(now_ms: f64, cur_top: f32, to_top: f32) -> ScrollAnim {
        ScrollAnim { from_left: 0.0, from_top: cur_top, to_left: 0.0, to_top, start_ms: now_ms, duration_ms: 350.0, timing: Timing::CubicInOut }
    }
    /// Position at `now` and whether the animation is finished (`End()` jumps to the final value, sp.js:82342).
    pub fn at(&self, now_ms: f64) -> ((f32, f32), bool) {
        let el = (now_ms - self.start_ms) as f32;
        if el >= self.duration_ms {
            return ((self.to_left, self.to_top), true);
        }
        let k = self.timing.apply((el / self.duration_ms).max(0.0));
        ((self.from_left + (self.to_left - self.from_left) * k, self.from_top + (self.to_top - self.from_top) * k), false)
    }
}

/// ScrollPanel `ScrollOnGamepadDirection` (sp.js:82866-82905). `scroll_step_percent` default 30.
/// Returns the new target for the pressed direction, or `None` when already at that edge (+-2 px; focus may leave).
pub struct PanelScroll {
    pub step: f32,
    target_top: Option<f32>,
    target_left: Option<f32>,
}
impl PanelScroll {
    pub fn new(scroll_step_percent: Option<f32>) -> Self {
        PanelScroll { step: scroll_step_percent.unwrap_or(30.0) / 100.0, target_top: None, target_left: None }
    }
    /// `dir`: 'u','d','l','r'. Sizes are the element's scrollTop/scrollHeight/clientHeight etc.
    #[allow(clippy::too_many_arguments)]
    pub fn on_dir(&mut self, dir: char, scroll_top: f32, scroll_height: f32, client_h: f32, scroll_left: f32, scroll_width: f32, client_w: f32) -> Option<(Option<f32>, Option<f32>)> {
        const L: f32 = 2.0;
        let m = self.target_top.unwrap_or(scroll_top);
        let t = self.target_left.unwrap_or(scroll_left);
        match dir {
            'u' => {
                if m <= L {
                    return None;
                }
                self.target_top = Some((m - client_h * self.step).max(0.0));
            }
            'd' => {
                if m >= scroll_height - client_h - L {
                    return None;
                }
                self.target_top = Some((scroll_height - client_h).min(m + client_h * self.step));
            }
            'l' => {
                if t <= L {
                    return None;
                }
                self.target_left = Some((t - client_w * self.step).max(0.0));
            }
            'r' => {
                if t >= scroll_width - client_w - L {
                    return None;
                }
                self.target_left = Some((scroll_width - client_w).min(t + client_w * self.step));
            }
            _ => return None,
        }
        Some((self.target_top, self.target_left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sine_endpoints_and_mid() {
        assert!((Timing::Sine.apply(0.0)).abs() < 1e-6);
        assert!((Timing::Sine.apply(0.5) - 0.5).abs() < 1e-6);
        assert!((Timing::Sine.apply(1.0) - 1.0).abs() < 1e-6);
        assert!((Timing::CubicInOut.apply(0.25) - 0.0625).abs() < 1e-6);
        assert!((Timing::CubicInOut.apply(1.0) - 1.0).abs() < 1e-6);
    }
    #[test]
    fn retrigger_goes_linear_and_200ms() {
        let a = ScrollAnim::start(0.0, (0.0, 0.0), (0.0, 100.0), false).unwrap();
        assert_eq!(a.timing, Timing::Sine);
        assert_eq!(a.duration_ms, 200.0);
        let b = ScrollAnim::start(50.0, (0.0, 10.0), (0.0, 100.0), true).unwrap();
        assert_eq!(b.timing, Timing::Linear);
        let (p, done) = b.at(150.0);
        assert!(!done && (p.1 - 55.0).abs() < 1e-3);
        assert!(b.at(250.0).1);
        assert!(ScrollAnim::start(0.0, (0.0, 5.0), (0.0, 5.0), false).is_none());
    }
    #[test]
    fn panel_duration_clamped() {
        assert_eq!(ScrollAnim::start_panel(0.0, (0.0, 0.0), (0.0, 100.0), false).unwrap().duration_ms, 300.0);
        assert_eq!(ScrollAnim::start_panel(0.0, (0.0, 0.0), (0.0, 2000.0), false).unwrap().duration_ms, 400.0);
        assert_eq!(ScrollAnim::start_panel(0.0, (0.0, 0.0), (0.0, 5000.0), false).unwrap().duration_ms, 500.0);
    }
    #[test]
    fn panel_steps_30_percent_and_edges() {
        let mut p = PanelScroll::new(None);
        // 1000 content, 400 client: first down => 120
        let r = p.on_dir('d', 0.0, 1000.0, 400.0, 0.0, 0.0, 0.0).unwrap();
        assert!((r.0.unwrap() - 120.0).abs() < 1e-3);
        // accumulates over fast presses
        let r = p.on_dir('d', 0.0, 1000.0, 400.0, 0.0, 0.0, 0.0).unwrap();
        assert!((r.0.unwrap() - 240.0).abs() < 1e-3);
        // at top edge Up returns None (focus may leave)
        let mut q = PanelScroll::new(None);
        assert!(q.on_dir('u', 1.0, 1000.0, 400.0, 0.0, 0.0, 0.0).is_none());
        // at bottom edge Down returns None
        assert!(q.on_dir('d', 599.0, 1000.0, 400.0, 0.0, 0.0, 0.0).is_none());
    }
    #[test]
    fn nearest_semantics() {
        let cont = Rect::new(0.0, 0.0, 100.0, 100.0);
        let m = Edges::default();
        // fully visible -> 0
        assert_eq!(nearest_delta(&Rect::new(10.0, 10.0, 50.0, 50.0), &cont, &m, true), 0.0);
        // below -> scroll down by overshoot
        assert_eq!(nearest_delta(&Rect::new(10.0, 90.0, 50.0, 130.0), &cont, &m, true), 30.0);
        // above -> negative
        assert_eq!(nearest_delta(&Rect::new(10.0, -20.0, 50.0, 20.0), &cont, &m, true), -20.0);
        // bigger than the container: align leading edge
        assert_eq!(nearest_delta(&Rect::new(10.0, 20.0, 50.0, 220.0), &cont, &m, true), 20.0);
        // scroll margin extends target
        assert_eq!(nearest_delta(&Rect::new(10.0, 90.0, 50.0, 100.0), &cont, &Edges::all(5.0), true), 5.0);
    }
}
