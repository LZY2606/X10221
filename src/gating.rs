//! Polygon gating with explicit boundary handling. An event that lands within
//! `eps` of a gate edge is *not* silently assigned inside or outside; the
//! engine surfaces it as a boundary candidate set for adjudication.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate {
    Inside,
    Outside,
    Boundary,
}

fn dist_point_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-300 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let mut t = ((px - ax) * dx + (py - ay) * dy) / len2;
    t = t.clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn locate_point(p: (f64, f64), poly: &[(f64, f64)], eps: f64) -> Locate {
    let n = poly.len();
    if n < 3 {
        return Locate::Outside;
    }
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if dist_point_segment(p, a, b) < eps {
            return Locate::Boundary;
        }
    }
    // even-odd ray cast
    let (px, py) = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Locate::Inside
    } else {
        Locate::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    }

    #[test]
    fn inside_outside() {
        let s = square();
        assert_eq!(locate_point((5.0, 5.0), &s, 0.5), Locate::Inside);
        assert_eq!(locate_point((15.0, 5.0), &s, 0.5), Locate::Outside);
        assert_eq!(locate_point((-1.0, 5.0), &s, 0.5), Locate::Outside);
    }

    #[test]
    fn on_edge_is_boundary() {
        let s = square();
        assert_eq!(locate_point((5.0, 0.0), &s, 0.5), Locate::Boundary);
        assert_eq!(locate_point((5.0, 0.4), &s, 0.5), Locate::Boundary);
        assert_eq!(locate_point((0.0, 0.0), &s, 0.5), Locate::Boundary); // vertex
    }

    #[test]
    fn degenerate_polygon_is_outside() {
        let line = vec![(0.0, 0.0), (1.0, 1.0)];
        assert_eq!(locate_point((0.5, 0.5), &line, 0.01), Locate::Outside);
    }
}
