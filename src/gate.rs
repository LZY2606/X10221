//! 门多边形与成员判定。落在边界面上的点不能被默默归到门内或门外：
//! 返回 `Boundary`，由上层保留全部合法候选（计入 / 不计入）等待人工裁定。

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    In,
    Out,
    Boundary,
}

pub type Pt = (f64, f64);

fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let qx = a.0 + t * dx;
    let qy = a.1 + t * dy;
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

pub fn boundary_eps(poly: &[Pt]) -> f64 {
    // 与坐标尺度成比例；所有域内点为整数/定点，容差远小于最小门边长。
    let scale = poly
        .iter()
        .flat_map(|p| [p.0.abs(), p.1.abs()])
        .fold(1.0, f64::max);
    (scale * 1e-9).max(1e-6)
}

pub fn classify(poly: &[Pt], p: Pt) -> Membership {
    if poly.len() < 3 {
        return Membership::Out;
    }
    let eps = boundary_eps(poly);
    // 先判边界（任意一条边距离 < eps）
    for w in poly.windows(2) {
        if dist_to_segment(p, w[0], w[1]) <= eps {
            return Membership::Boundary;
        }
    }
    if dist_to_segment(p, poly[poly.len() - 1], poly[0]) <= eps {
        return Membership::Boundary;
    }
    // 射线法
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let crosses = (yi > p.1) != (yj > p.1)
            && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi;
        if crosses {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Membership::In
    } else {
        Membership::Out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Vec<Pt> {
        vec![(20000.0, 5000.0), (90000.0, 5000.0), (90000.0, 60000.0), (20000.0, 60000.0)]
    }

    #[test]
    fn inside_outside() {
        let p = rect();
        assert_eq!(classify(&p, (50000.0, 25000.0)), Membership::In);
        assert_eq!(classify(&p, (5000.0, 1000.0)), Membership::Out);
        assert_eq!(classify(&p, (120000.0, 90000.0)), Membership::Out);
    }

    #[test]
    fn exact_edge_is_boundary() {
        let p = rect();
        assert_eq!(classify(&p, (20000.0, 30000.0)), Membership::Boundary);
        assert_eq!(classify(&p, (55000.0, 60000.0)), Membership::Boundary);
    }

    #[test]
    fn vertex_is_boundary() {
        let p = rect();
        assert_eq!(classify(&p, (20000.0, 5000.0)), Membership::Boundary);
        assert_eq!(classify(&p, (90000.0, 60000.0)), Membership::Boundary);
    }

    #[test]
    fn near_but_not_on_edge_is_decided() {
        let p = rect();
        assert_eq!(classify(&p, (20000.5, 30000.0)), Membership::In);
        assert_eq!(classify(&p, (19999.0, 30000.0)), Membership::Out);
    }
}
