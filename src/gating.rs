//! 变换与门评估：边界点不强行归类，产生 include/exclude 两个合法候选。
use crate::model::{PointClass, TransformDiag};

/// 变换结果：value 为 None 表示无法转换，原始值留在原位置并附诊断。
#[derive(Clone, Debug)]
pub struct Transformed {
    pub value: Option<f64>,
    pub diag: Option<TransformDiag>,
}

/// log10 变换，带量程守卫。
/// x < min_signal（含 <=0）：无法转换，保留原值 + below_transform_range。
/// x > max_signal：极端信号，仍转换但附 extreme_signal 诊断。
pub fn transform_value(raw: &str, min_signal: f64, max_signal: f64) -> Transformed {
    let x: f64 = match raw.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            return Transformed {
                value: None,
                diag: Some(TransformDiag::Unparseable { raw: raw.to_string() }),
            }
        }
    };
    if !x.is_finite() || x < min_signal {
        return Transformed {
            value: None,
            diag: Some(TransformDiag::BelowRange { kept_raw: x }),
        };
    }
    if x > max_signal {
        return Transformed {
            value: Some(x.log10()),
            diag: Some(TransformDiag::ExtremeSignal { value: x }),
        };
    }
    Transformed { value: Some(x.log10()), diag: None }
}

fn seg_dist_sq(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    };
    let cx = a.0 + t * dx - p.0;
    let cy = a.1 + t * dy - p.1;
    cx * cx + cy * cy
}

/// 射线法点在多边形内判定；距任一边 <= eps 判为 Boundary（不强行归类）。
pub fn classify(p: (f64, f64), poly: &[(f64, f64)], eps: f64) -> PointClass {
    let n = poly.len();
    if n < 3 {
        return PointClass::Outside;
    }
    let eps_sq = eps * eps;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if seg_dist_sq(p, a, b) <= eps_sq {
            return PointClass::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

/// 边界判定的相对容差：按多边形坐标尺度缩放。
pub fn boundary_eps(poly: &[(f64, f64)]) -> f64 {
    let scale = poly
        .iter()
        .flat_map(|(x, y)| [x.abs(), y.abs()])
        .fold(1.0f64, f64::max);
    scale * 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    }

    #[test]
    fn inside_outside_boundary() {
        let sq = square();
        let eps = boundary_eps(&sq);
        assert_eq!(classify((5.0, 5.0), &sq, eps), PointClass::Inside);
        assert_eq!(classify((15.0, 5.0), &sq, eps), PointClass::Outside);
        assert_eq!(classify((5.0, 0.0), &sq, eps), PointClass::Boundary);
        assert_eq!(classify((0.0, 0.0), &sq, eps), PointClass::Boundary);
    }

    #[test]
    fn transform_ranges() {
        let t = transform_value("1000", 1.0, 262144.0);
        assert_eq!(t.value, Some(3.0));
        assert!(t.diag.is_none());
        let t = transform_value("-5", 1.0, 262144.0);
        assert_eq!(t.value, None);
        assert_eq!(t.diag.unwrap().kind(), "below_transform_range");
        let t = transform_value("1e13", 1.0, 262144.0);
        assert_eq!(t.value, Some(13.0));
        assert_eq!(t.diag.unwrap().kind(), "extreme_signal");
        let t = transform_value("not_a_number", 1.0, 262144.0);
        assert_eq!(t.value, None);
        assert_eq!(t.diag.unwrap().kind(), "unparseable_raw");
    }
}
