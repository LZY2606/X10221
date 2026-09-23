//! 多边形门控几何：射线法判定 + 边界面判定。
//!
//! 规则（坐标基准：arcsinh 变换空间）：
//! - 多边形边上的点属于门内（边界包含，boundary=true）；
//! - 距任意边小于 [`crate::domain::gate::BOUNDARY_EPS`] 视为边界点，
//!   用于捕获「恰好落在门边上」的事件。

use serde_json::Value;

pub const BOUNDARY_EPS: f64 = 1e-9;

/// 点到线段的最短距离。
fn distance_to_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let qx = ax + t * dx;
    let qy = ay + t * dy;
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

/// 判定 (x, y) 与多边形的关系，返回 (是否在门内, 是否边界点)。
pub fn point_in_polygon(x: f64, y: f64, polygon: &[(f64, f64)]) -> (bool, bool) {
    let n = polygon.len();
    if n < 3 {
        return (false, false);
    }
    let mut boundary = false;
    let mut inside = false;
    for i in 0..n {
        let (ax, ay) = polygon[i];
        let (bx, by) = polygon[(i + 1) % n];
        if distance_to_segment(x, y, ax, ay, bx, by) < BOUNDARY_EPS {
            boundary = true;
        }
        let intersects = ((ay > y) != (by > y))
            && (x < (bx - ax) * (y - ay) / (by - ay) + ax);
        if intersects {
            inside = !inside;
        }
    }
    (inside || boundary, boundary)
}

/// 从 JSON 门定义读取多边形坐标 `Vec<(f64,f64)>`。
pub fn polygon_of(gate: &Value) -> Vec<(f64, f64)> {
    gate["polygon"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<(f64, f64)> {
        vec![
            (0.5, 0.5),
            (4.0, 0.5),
            (4.0, 2.0),
            (0.5, 2.0),
        ]
    }

    #[test]
    fn interior_and_exterior() {
        assert_eq!(point_in_polygon(1.5, 1.0, &square()).0, true);
        assert_eq!(point_in_polygon(5.0, 1.0, &square()).0, false);
        assert_eq!(point_in_polygon(1.5, 3.0, &square()).0, false);
    }

    #[test]
    fn edge_point_is_inside_and_boundary() {
        let (inside, boundary) = point_in_polygon(1.5, 2.0, &square());
        assert!(inside);
        assert!(boundary);
        let (inside2, boundary2) = point_in_polygon(0.5, 0.5, &square());
        assert!(inside2);
        assert!(boundary2);
    }
}
