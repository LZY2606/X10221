//! Polygon membership in the transformed coordinate basis. Boundary handling
//! is explicit: an unspecified boundary convention keeps both legal counts as
//! candidates rather than silently picking one.

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BoundaryConvention {
    Include,
    Exclude,
}

pub fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

/// Distance of point p to segment a-b.
fn dist_to_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Returns true when the point lies (numerically) on the polygon boundary.
pub fn on_boundary(x: f64, y: f64, vertices: &[[f64; 2]], eps: f64) -> bool {
    let n = vertices.len();
    for i in 0..n {
        let a = vertices[i];
        let b = vertices[(i + 1) % n];
        if dist_to_segment(x, y, a[0], a[1], b[0], b[1]) <= eps {
            return true;
        }
    }
    false
}

/// Interior test by ray casting; boundary points reported separately.
pub fn inside(x: f64, y: f64, vertices: &[[f64; 2]], convention: BoundaryConvention, eps: f64) -> bool {
    let boundary = on_boundary(x, y, vertices, eps);
    if boundary {
        return convention == BoundaryConvention::Include;
    }
    let n = vertices.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (vertices[i][0], vertices[i][1]);
        let (xj, yj) = (vertices[j][0], vertices[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Membership over an arbitrary point set. Returns (count_include, count_exclude,
/// boundary_point_ids). When boundary points exist and the gate does not fix a
/// convention, the reviewer gets both legal counts.
pub fn count_points(points: &[(String, f64, f64)], vertices: &[[f64; 2]], gate_boundary: &str, eps: f64)
    -> (i64, i64, Vec<String>) {
    let mut inc = 0i64;
    let mut exc = 0i64;
    let mut boundary_ids = Vec::new();
    for (id, x, y) in points {
        if on_boundary(*x, *y, vertices, eps) {
            boundary_ids.push(id.clone());
            inc += 1; // include convention counts boundary in
        } else if inside(*x, *y, vertices, BoundaryConvention::Exclude, eps) {
            inc += 1;
            exc += 1;
        }
    }
    (inc, exc, boundary_ids)
}
