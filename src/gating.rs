#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

const BOUNDARY_EPS: f64 = 1e-7;

fn distance_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let vx = b.0 - a.0;
    let vy = b.1 - a.1;
    let wx = p.0 - a.0;
    let wy = p.1 - a.1;
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
    };
    let dx = wx - t * vx;
    let dy = wy - t * vy;
    (dx * dx + dy * dy).sqrt()
}

/// Ray-casting 点在多边形判定；落在多边形边上（数值容差内）返回 `Boundary`，
/// 表示归属无法唯一决定，必须保留 in/out 两个候选而不是静默归类。
pub fn classify(polygon: &[(f64, f64)], point: (f64, f64)) -> Membership {
    let n = polygon.len();
    if n < 3 {
        return Membership::Outside;
    }
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        if distance_to_segment(point, a, b) < BOUNDARY_EPS {
            return Membership::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > point.1) != (yj > point.1)
            && point.0 < (xj - xi) * (point.1 - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
    }

    #[test]
    fn inside_outside() {
        assert_eq!(classify(&square(), (2.0, 2.0)), Membership::Inside);
        assert_eq!(classify(&square(), (-1.0, 2.0)), Membership::Outside);
        assert_eq!(classify(&square(), (5.0, 5.0)), Membership::Outside);
    }

    #[test]
    fn edge_is_boundary() {
        assert_eq!(classify(&square(), (2.5, 0.0)), Membership::Boundary);
        assert_eq!(classify(&square(), (4.0, 2.0)), Membership::Boundary);
        assert_eq!(classify(&square(), (2.5, 1e-10)), Membership::Boundary);
    }

    #[test]
    fn vertex_is_boundary() {
        assert_eq!(classify(&square(), (0.0, 0.0)), Membership::Boundary);
    }
}
