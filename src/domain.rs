use serde_json::Value;

pub fn fnv1a_64(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

pub fn det2(matrix: &[[f64; 2]; 2]) -> f64 {
    matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]
}

pub fn invert2(matrix: &[[f64; 2]; 2]) -> Option<[[f64; 2]; 2]> {
    let determinant = det2(matrix);
    if determinant.abs() < f64::EPSILON {
        return None;
    }
    Some([
        [matrix[1][1] / determinant, -matrix[0][1] / determinant],
        [-matrix[1][0] / determinant, matrix[0][0] / determinant],
    ])
}

pub fn multiply2(vector: [f64; 2], matrix: [[f64; 2]; 2]) -> [f64; 2] {
    [
        matrix[0][0] * vector[0] + matrix[0][1] * vector[1],
        matrix[1][0] * vector[0] + matrix[1][1] * vector[1],
    ]
}

pub fn arcsinh(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

fn cross(origin: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - origin[0]) * (b[1] - origin[1]) - (a[1] - origin[1]) * (b[0] - origin[0])
}

fn on_segment(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> bool {
    let epsilon = 1e-9;
    if cross(a, b, point).abs() > epsilon {
        return false;
    }
    point[0] >= a[0].min(b[0]) - epsilon
        && point[0] <= a[0].max(b[0]) + epsilon
        && point[1] >= a[1].min(b[1]) - epsilon
        && point[1] <= a[1].max(b[1]) + epsilon
}

pub fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> (bool, bool) {
    let mut inside = false;
    let mut boundary = false;
    let mut j = polygon.len().saturating_sub(1);
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[j];
        if on_segment(a, b, point) {
            boundary = true;
        }
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let crossing = a[0]
                + ((point[1] - a[1]) * (b[0] - a[0])) / (b[1] - a[1]);
            if point[0] < crossing {
                inside = !inside;
            }
        }
        j = i;
    }
    (inside || boundary, boundary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_singular_and_near_singular_matrices() {
        let singular = [[1.0, 1.0], [1.000001, 1.0]];
        assert!(det2(&singular).abs() < 1e-5);
        assert!(invert2(&[[1.0, 1.0], [1.0, 1.0]]).is_none());
    }

    #[test]
    fn boundary_points_are_retained_and_flagged() {
        let polygon = [[100.0, 50.0], [180.0, 50.0], [180.0, 150.0], [100.0, 150.0]];
        assert_eq!(point_in_polygon([100.0, 100.0], &polygon), (true, true));
        assert_eq!(point_in_polygon([120.0, 95.0], &polygon), (true, false));
        assert_eq!(point_in_polygon([5.0, 5.0], &polygon), (false, false));
    }

    #[test]
    fn compensation_matches_two_by_two_inverse() {
        let spillover = [[1.0, 0.02], [0.08, 1.0]];
        let inverse = invert2(&spillover).unwrap();
        let compensated = multiply2([210.0, 70.0], inverse);
        assert!((compensated[0] - 208.9342948717949).abs() < 1e-9);
        assert!((compensated[1] - 53.2852564102564).abs() < 1e-9);
    }
}
