//! Polygon gate geometry with explicit boundary handling.
//! Points within `eps` of an edge or vertex are AMBIGUOUS: both "inside" and
//! "outside" are legal candidates and the decision is deferred to adjudication.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PointClass {
    Inside,
    Outside,
    Boundary,
}

pub fn classify(px: f64, py: f64, poly: &[(f64, f64)], eps: f64) -> (PointClass, f64) {
    let dist = min_edge_distance(px, py, poly);
    if dist <= eps {
        return (PointClass::Boundary, dist);
    }
    (if inside(px, py, poly) { PointClass::Inside } else { PointClass::Outside }, dist)
}

fn inside(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    let mut c = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            c = !c;
        }
        j = i;
    }
    c
}

fn min_edge_distance(px: f64, py: f64, poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    if n == 0 {
        return f64::INFINITY;
    }
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
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
        best = best.min(((px - cx).powi(2) + (py - cy).powi(2)).sqrt());
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQ: &[(f64, f64)] = &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];

    #[test]
    fn inside_outside_boundary() {
        assert_eq!(classify(5.0, 5.0, SQ, 1e-6).0, PointClass::Inside);
        assert_eq!(classify(20.0, 5.0, SQ, 1e-6).0, PointClass::Outside);
        // exactly on the edge y=0
        assert_eq!(classify(5.0, 0.0, SQ, 1e-6).0, PointClass::Boundary);
        // exactly on a vertex
        assert_eq!(classify(0.0, 0.0, SQ, 1e-6).0, PointClass::Boundary);
    }
}
