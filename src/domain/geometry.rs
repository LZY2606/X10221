//! 点在多边形判定（射线法）与确定性的“半开”边界约定。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InPoly {
    In,
    Out,
    /// 恰好落在边上
    Boundary,
}

const EPS: f64 = 1e-9;

pub fn point_in_polygon(poly: &[[f64; 2)], p: [f64; 2]) -> InPoly {
    let n = poly.len();
    if n < 3 {
        return InPoly::Out;
    }
    let [px, py] = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        // 边上（容差随坐标尺度）
        let cross = (px - xi) * (yj - yi) - (py - yi) * (xj - xi);
        let on_seg = cross.abs() <= EPS * (1.0 + (xj - xi).abs() + (yj - yi).abs())
            && px <= xi.max(xj) + EPS
            && px >= xi.min(xj) - EPS
            && py <= yi.max(yj) + EPS
            && py >= yi.min(yj) - EPS;
        if on_seg {
            return InPoly::Boundary;
        }
        if (yi > py) != (yj > py) {
            let x_intersect = xi + (py - yi) * (xj - xi) / (yj - yi);
            if px < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside {
        InPoly::In
    } else {
        InPoly::Out
    }
}

/// halfopen：边界点按“右开/上开”规则确定归属（确定性）。
/// 顶点按边的方向：若该点在水平边（y 相同）上，y 较大的相邻边侧视为开，
/// 这里采用标准 GIS 半开规则：边界点在多边形“向上”的边上算入，其余不算。
pub fn halfopen_membership(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let [px, py] = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > py) != (yj > py) {
            let x_intersect = xi + (py - yi) * (xj - xi) / (yj - yi);
            if px < x_intersect {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}
