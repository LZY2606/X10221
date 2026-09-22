//! 领域规则：补偿矩阵、变换、门多边形。
//! 所有判定以机器可读的规则 id 返回，绝不静默处理。

use crate::model::rules;

/// 相对容差：|det| / frobenius^2 低于此值视为近奇异，禁止静默求逆。
pub const SINGULAR_RTOL: f64 = 1e-8;
/// 边界点判定容差（变换后坐标，单位：decades）。
pub const BOUNDARY_EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub enum GatePos {
    Inside,
    Outside,
    Boundary,
}

pub fn matrix2_det(m: &[[f64; 2]; 2]) -> f64 {
    m[0][0] * m[1][1] - m[0][1] * m[1][0]
}

pub fn matrix2_frobenius(m: &[[f64; 2]; 2]) -> f64 {
    (m[0][0].powi(2) + m[0][1].powi(2) + m[1][0].powi(2) + m[1][1].powi(2)).sqrt()
}

/// 近奇异矩阵返回 Err(NEAR_SINGULAR_NO_SILENT_INVERSE)，调用方必须保留候选而非静默求逆。
pub fn matrix2_inverse_checked(m: &[[f64; 2]; 2]) -> Result<[[f64; 2]; 2], String> {
    let det = matrix2_det(m);
    let fro = matrix2_frobenius(m);
    let scale = fro * fro;
    if !det.is_finite() || det.abs() <= SINGULAR_RTOL * scale.max(1e-300) {
        return Err(rules::NEAR_SINGULAR.to_string());
    }
    Ok([
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ])
}

pub fn apply_matrix2(m: &[[f64; 2]; 2], x: f64, y: f64) -> (f64, f64) {
    (m[0][0] * x + m[0][1] * y, m[1][0] * x + m[1][1] * y)
}

/// log10 变换。v<=0 或超出 decades 量程的值不可转换，必须留在原位置并附诊断。
pub fn log10_transform(v: f64, decades: f64) -> Result<f64, String> {
    if !v.is_finite() || v <= 0.0 {
        return Err(rules::EXTREME_KEPT_RAW.to_string());
    }
    let t = v.log10();
    if !(0.0..=decades).contains(&t) {
        return Err(rules::EXTREME_KEPT_RAW.to_string());
    }
    Ok(t)
}

fn dist_point_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
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

/// 射线法 + 显式边界规则：落在边上的点返回 Boundary，由调用方确定性地计入并记录。
pub fn point_in_polygon(px: f64, py: f64, poly: &[[f64; 2]]) -> GatePos {
    let n = poly.len();
    if n < 3 {
        return GatePos::Outside;
    }
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if dist_point_segment(px, py, a[0], a[1], b[0], b[1]) <= BOUNDARY_EPS {
            return GatePos::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        GatePos::Inside
    } else {
        GatePos::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = [[1.0, 0.15], [0.05, 1.0]];
        let inv = matrix2_inverse_checked(&m).expect("should invert");
        let (x, y) = apply_matrix2(&inv, 1.0, 1.0);
        assert!((x - 1.0).abs() < 1e-9 || (y - 1.0).abs() < 1e-9);
        // inv * m * v == v
        let (cx, cy) = apply_matrix2(&m, x, y);
        assert!((cx - 1.0).abs() < 1e-9 && (cy - 1.0).abs() < 1e-9);
    }

    #[test]
    fn near_singular_matrix_rejected() {
        let m = [[1.0, 0.99999999], [0.99999999, 1.0]];
        let err = matrix2_inverse_checked(&m).unwrap_err();
        assert_eq!(err, rules::NEAR_SINGULAR);
    }

    #[test]
    fn exactly_singular_rejected() {
        let m = [[1.0, 1.0], [1.0, 1.0]];
        assert!(matrix2_inverse_checked(&m).is_err());
    }

    #[test]
    fn transform_extreme_values_kept_raw() {
        assert!(log10_transform(-5.0, 6.0).is_err());
        assert!(log10_transform(0.0, 6.0).is_err());
        assert!(log10_transform(2e6, 6.0).is_err());
        assert!(log10_transform(f64::NAN, 6.0).is_err());
        assert_eq!(log10_transform(100.0, 6.0).unwrap(), 2.0);
    }

    #[test]
    fn polygon_boundary_deterministic() {
        let poly = [[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]];
        assert_eq!(point_in_polygon(2.0, 2.0, &poly), GatePos::Inside);
        assert_eq!(point_in_polygon(0.5, 2.0, &poly), GatePos::Outside);
        assert_eq!(point_in_polygon(1.0, 2.0, &poly), GatePos::Boundary);
        assert_eq!(point_in_polygon(3.0, 3.0, &poly), GatePos::Boundary);
    }
}
