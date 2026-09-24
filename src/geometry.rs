pub fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]], include_boundary: bool) -> bool {
    let mut inside = false;
    let mut j = polygon.len().saturating_sub(1);
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        if include_boundary && on_segment(point, pi, pj) {
            return true;
        }
        if (pi[1] > point[1]) != (pj[1] > point[1]) {
            let slope_x = (pj[0] - pi[0]) * (point[1] - pi[1]) / (pj[1] - pi[1]) + pi[0];
            if point[0] < slope_x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn on_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    let cross = (point[1] - a[1]) * (b[0] - a[0]) - (point[0] - a[0]) * (b[1] - a[1]);
    let epsilon = 1e-9;
    let bounds = point[0] >= a[0].min(b[0]) - epsilon
        && point[0] <= a[0].max(b[0]) + epsilon
        && point[1] >= a[1].min(b[1]) - epsilon
        && point[1] <= a[1].max(b[1]) + epsilon;
    cross.abs() <= epsilon && bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_points_on_polygon_edge() {
        let polygon = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(point_in_polygon([5.0, 0.0], &polygon, true));
        assert!(!point_in_polygon([5.0, 0.0], &polygon, false));
        assert!(point_in_polygon([4.0, 4.0], &polygon, true));
        assert!(!point_in_polygon([11.0, 1.0], &polygon, true));
    }
}
