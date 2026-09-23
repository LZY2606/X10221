//! Polygon membership with explicit boundary handling.
//!
//! A point exactly on a polygon edge is classified as `Boundary`, not silently
//! absorbed. Boundary points generate two legitimate membership candidates
//! (include / exclude) that require a ruling when the gate's edge convention is
//! not pinned.

use crate::models::Point;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

const EPS: f64 = 1e-9;

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

/// Ray-casting point-in-polygon plus an exact-edge test.
pub fn classify(px: f64, py: f64, poly: &[Point]) -> Membership {
    if !px.is_finite() || !py.is_finite() || poly.len() < 3 {
        return Membership::Outside;
    }
    let mut boundary = false;
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        let (ax, ay) = (a.x.value, a.y.value);
        let (bx, by) = (b.x.value, b.y.value);
        // Exact-on-segment test: zero cross product and within the bounding box.
        let cr = cross(bx - ax, by - ay, px - ax, py - ay);
        let in_box = px >= ax.min(bx) - EPS
            && px <= ax.max(bx) + EPS
            && py >= ay.min(by) - EPS
            && py <= ay.max(by) + EPS;
        if cr.abs() <= EPS * (1.0 + ax.abs() + ay.abs() + bx.abs() + by.abs()) && in_box {
            boundary = true;
        }
        // Standard ray casting.
        let intersects = (ay > py) != (by > py)
            && px < (bx - ax) * (py - ay) / (by - ay) + ax;
        if intersects {
            inside = !inside;
        }
    }
    if boundary {
        Membership::Boundary
    } else if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}
