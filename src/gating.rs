//! Polygon gating with explicit boundary-point classification.
//! Points lying on a gate edge are ambiguous: both legal verdicts are kept
//! as candidates instead of being silently assigned.

pub const BOUNDARY_EPS: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq)]
pub enum GateVerdict {
    Inside,
    Outside,
    Boundary { edge: usize, distance: f64 },
}

fn dist_point_segment(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-300 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn classify(px: f64, py: f64, poly: &[(f64, f64)]) -> GateVerdict {
    let n = poly.len();
    let mut min_d = f64::INFINITY;
    let mut min_edge = 0;
    for i in 0..n {
        let d = dist_point_segment(px, py, poly[i], poly[(i + 1) % n]);
        if d < min_d {
            min_d = d;
            min_edge = i;
        }
    }
    if min_d <= BOUNDARY_EPS {
        return GateVerdict::Boundary { edge: min_edge, distance: min_d };
    }
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
        GateVerdict::Inside
    } else {
        GateVerdict::Outside
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
        assert_eq!(classify(5.0, 5.0, &square()), GateVerdict::Inside);
        assert_eq!(classify(15.0, 5.0, &square()), GateVerdict::Outside);
    }

    #[test]
    fn boundary_point_is_flagged_not_assigned() {
        match classify(5.0, 0.0, &square()) {
            GateVerdict::Boundary { distance, .. } => assert!(distance <= BOUNDARY_EPS),
            v => panic!("boundary point must be flagged, got {:?}", v),
        }
    }
}
