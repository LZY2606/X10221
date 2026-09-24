//! 多边形门判定。边界上的点不做确定性归类，返回 Boundary，
//! 由上层生成“进/出”两个合法候选并等待裁定。

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

pub const BOUNDARY_EPS: f64 = 1e-9;

fn on_segment(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> bool {
    let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    let scale = ((b.0 - a.0).abs() + (b.1 - a.1).abs()).max(1.0);
    if cross.abs() > BOUNDARY_EPS * scale {
        return false;
    }
    let dot = (p.0 - a.0) * (b.0 - a.0) + (p.1 - a.1) * (b.1 - a.1);
    let len2 = (b.0 - a.0).powi(2) + (b.1 - a.1).powi(2);
    dot >= -BOUNDARY_EPS && dot <= len2 + BOUNDARY_EPS
}

pub fn classify(poly: &[(f64, f64)], p: (f64, f64)) -> Membership {
    let n = poly.len();
    if n < 3 {
        return Membership::Outside;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        if on_segment(poly[j], poly[i], p) {
            return Membership::Boundary;
        }
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > p.1) != (yj > p.1) {
            let x_int = xi + (p.1 - yi) * (xj - xi) / (yj - yi);
            if p.0 < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside { Membership::Inside } else { Membership::Outside }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    }

    #[test]
    fn inside_outside_boundary() {
        let s = square();
        assert_eq!(classify(&s, (5.0, 5.0)), Membership::Inside);
        assert_eq!(classify(&s, (20.0, 5.0)), Membership::Outside);
        // 边界点：精确落在边上
        assert_eq!(classify(&s, (5.0, 0.0)), Membership::Boundary);
        assert_eq!(classify(&s, (0.0, 0.0)), Membership::Boundary);
    }
}
