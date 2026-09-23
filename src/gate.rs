//! 2-D polygon classification with explicit boundary detection.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Inside,
    Outside,
    Boundary,
}

pub fn label_point(polygon: &[[f64; 2]], epsilon: f64, point: [f64; 2]) -> Label {
    if polygon.len() < 3 {
        return Label::Outside;
    }
    if distance_to_polygon(polygon, point) <= epsilon {
        return Label::Boundary;
    }
    if point_in_polygon(polygon, point) {
        Label::Inside
    } else {
        Label::Outside
    }
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let vx = b[0] - a[0];
    let vy = b[1] - a[1];
    let wx = p[0] - a[0];
    let wy = p[1] - a[1];
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
    };
    let qx = a[0] + t * vx;
    let qy = a[1] + t * vy;
    ((p[0] - qx).powi(2) + (p[1] - qy).powi(2)).sqrt()
}

fn distance_to_polygon(polygon: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = polygon.len();
    (0..n)
        .map(|i| distance_to_segment(p, polygon[i], polygon[(i + 1) % n]))
        .fold(f64::INFINITY, f64::min)
}

/// Ray casting; callers must first rule out boundary within epsilon.
fn point_in_polygon(polygon: &[[f64; 2]], point: [f64; 2]) -> bool {
    let [x, y] = point;
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let xi = polygon[i][0];
        let yi = polygon[i][1];
        let xj = polygon[j][0];
        let yj = polygon[j][1];
        let intersects = ((yi > y) != (yj > y))
            && (x < (xj - xi) * (y - yi) / ((yj - yi) + f64::MIN_POSITIVE) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_boundary() {
        let g = square();
        assert_eq!(label_point(&g, 0.05, [5.0, 5.0]), Label::Inside);
        assert_eq!(label_point(&g, 0.05, [11.0, 5.0]), Label::Outside);
        assert_eq!(label_point(&g, 0.05, [0.0, 5.0]), Label::Boundary);
        assert_eq!(label_point(&g, 0.05, [10.0, 10.0]), Label::Boundary);
    }

    #[test]
    fn degenerate_polygon_is_outside() {
        assert_eq!(label_point(&[[0.0, 0.0], [1.0, 1.0]], 0.1, [0.5, 0.5]), Label::Outside);
    }
}
