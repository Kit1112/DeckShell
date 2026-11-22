//! Geometry helpers of the focus-navigation engine.
//! Port of sp.js module 92393 (sp.js:81241-81385) + module 26756 `bZ` (sp.js:226336) + module 81133 `xr`.

/// Bounding client rect (CSS px, y down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}
impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Rect { left, top, right, bottom }
    }
    pub fn xywh(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { left: x, top: y, right: x + w, bottom: y + h }
    }
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.left + dx, self.top + dy, self.right + dx, self.bottom + dy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}
impl Axis {
    /// sp.js:81133 `xr = { x: "y", y: "x" }` — the tangent axis.
    pub fn tangent(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }
}

/// sp.js:81306 `UR(axis, a, b, tol = 0.001)`: do the ranges of `a` and `b` overlap on `axis`?
pub fn overlaps(axis: Axis, a: &Rect, b: &Rect) -> bool {
    const X: f32 = 0.001;
    match axis {
        Axis::X => a.right > b.left + X && a.left + X < b.right,
        Axis::Y => a.bottom > b.top + X && a.top + X < b.bottom,
    }
}

/// sp.js:81321 `ix(axis, a, b)`: size of the overlap of the ranges (>= 0).
pub fn overlap_amount(axis: Axis, a: &Rect, b: &Rect) -> f32 {
    let v = match axis {
        Axis::X => a.right.min(b.right) - a.left.max(b.left),
        Axis::Y => a.bottom.min(b.bottom) - a.top.max(b.top),
    };
    v.max(0.0)
}

/// sp.js:81331 `lr(axis, rect)` -> {min, max}.
pub fn range(axis: Axis, r: &Rect) -> (f32, f32) {
    match axis {
        Axis::X => (r.left, r.right),
        Axis::Y => (r.top, r.bottom),
    }
}

/// sp.js:81334 `_V(axis, point, rect)`: distance from a point's coordinate to the rect range on the axis.
pub fn point_dist(axis: Axis, point: (f32, f32), r: &Rect) -> f32 {
    let p = match axis {
        Axis::X => point.0,
        Axis::Y => point.1,
    };
    let (min, max) = range(axis, r);
    if p < min {
        min - p
    } else if p > max {
        p - max
    } else {
        0.0
    }
}

/// sp.js:81342 `xb(rect)` -> top-left point.
pub fn origin(r: &Rect) -> (f32, f32) {
    (r.left, r.top)
}

/// sp.js:226336 `bZ(a, b)`: euclidean gap between two rects (0 if they touch/overlap on both axes).
pub fn rect_distance(a: &Rect, b: &Rect) -> f32 {
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    if b.right < a.left {
        dx = a.left - b.right;
    } else if b.left > a.right {
        dx = b.left - a.right;
    }
    if b.bottom < a.top {
        dy = a.top - b.bottom;
    } else if b.top > a.bottom {
        dy = b.top - a.bottom;
    }
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_tolerance() {
        let a = Rect::xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::xywh(0.0, 10.0, 10.0, 10.0); // touching edge: no overlap on y
        assert!(!overlaps(Axis::Y, &a, &b));
        assert!(overlaps(Axis::X, &a, &b));
        let c = Rect::xywh(0.0, 9.0, 10.0, 10.0);
        assert!(overlaps(Axis::Y, &a, &c));
    }
    #[test]
    fn distances() {
        let a = Rect::xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::xywh(13.0, 14.0, 5.0, 5.0);
        assert_eq!(rect_distance(&a, &b), 5.0);
        assert_eq!(overlap_amount(Axis::X, &a, &Rect::xywh(4.0, 0.0, 10.0, 1.0)), 6.0);
        assert_eq!(point_dist(Axis::X, (20.0, 0.0), &a), 10.0);
    }
}
