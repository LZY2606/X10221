//! 多边形门：边界点判定与多候选保留。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    pub gate_id: String,
    pub version: i64,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// 顶点坐标（坐标基准写在 basis）。
    pub vertices: Vec<[f64; 2]>,
    pub basis: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Membership {
    Inside,
    Outside,
    /// 距边界 <= eps：必须保留 include/exclude 两个候选，由人工裁定。
    Boundary,
}

pub fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (px, py) = (p[0], p[1]);
    let (ax, ay) = (a[0], a[1]);
    let (bx, by) = (b[0], b[1]);
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

pub fn classify(vertices: &[[f64; 2]], p: [f64; 2], eps: f64) -> Membership {
    let n = vertices.len();
    if n < 3 {
        return Membership::Outside;
    }
    for i in 0..n {
        let a = vertices[i];
        let b = vertices[(i + 1) % n];
        if dist_point_segment(p, a, b) <= eps {
            return Membership::Boundary;
        }
    }
    // 射线法
    let mut inside = false;
    let (px, py) = (p[0], p[1]);
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (vertices[i][0], vertices[i][1]);
        let (xj, yj) = (vertices[j][0], vertices[j][1]);
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
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

    fn rect() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_boundary() {
        let r = rect();
        assert_eq!(classify(&r, [5.0, 5.0], 1e-9), Membership::Inside);
        assert_eq!(classify(&r, [15.0, 5.0], 1e-9), Membership::Outside);
        // 精确落在边 y=0 上
        assert_eq!(classify(&r, [5.0, 0.0], 1e-9), Membership::Boundary);
        // 距边 0.5*eps
        assert_eq!(classify(&r, [5.0, 0.5e-9], 1e-9), Membership::Boundary);
        // 顶点本身
        assert_eq!(classify(&r, [0.0, 0.0], 1e-9), Membership::Boundary);
    }
}
