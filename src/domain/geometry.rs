//! 门多边形与点判定。
//!
//! 边界规则（显式、可随规则版本变化）：事件坐标落在多边形任意一条
//! 边（含顶点）上时判定为“在门内”。边界点不是随机舍入误差，因此
//! 在重放结果中稳定。

use serde::{Deserialize, Serialize};

pub const GEOMETRY_RULE: &str = "geometry.polygon_closed_edges.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    /// 坐标基准记录在门材料上；此处为该基准下的数值。
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<Point>,
    /// 坐标基准：原始通道 / 补偿后 / 变换后（含变换参数引用）。
    pub basis: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum PointStatus {
    Inside,
    Boundary,
    Outside,
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64) -> f64 {
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

fn on_segment(a: Point, b: Point, p: Point, eps: f64) -> bool {
    // 点在边的包围盒内，且叉积近似为 0（相对尺度的容差）。
    let box_min_x = a.x.min(b.x) - eps;
    let box_max_x = a.x.max(b.x) + eps;
    let box_min_y = a.y.min(b.y) - eps;
    let box_max_y = a.y.max(b.y) + eps;
    let in_box = p.x >= box_min_x && p.x <= box_max_x && p.y >= box_min_y && p.y <= box_max_y;
    if !in_box {
        return false;
    }
    let cr = cross(a.x, a.y, b.x, b.y, p.x, p.y);
    let scale = (a.x.hypot(a.y) + b.x.hypot(b.y) + p.x.hypot(p.y)).max(1.0);
    cr.abs() <= eps * scale
}

/// 判定点相对多边形的状态。
pub fn classify(poly: &Polygon, p: Point) -> PointStatus {
    let verts = &poly.vertices;
    let n = verts.len();
    if n < 3 || !p.x.is_finite() || !p.y.is_finite() {
        return PointStatus::Outside;
    }

    let span_x = {
        let xs: Vec<f64> = verts.iter().map(|v| v.x).collect();
        xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
    };
    let span_y = {
        let ys: Vec<f64> = verts.iter().map(|v| v.y).collect();
        ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min)
    };
    let eps = 1e-9_f64 * span_x.max(span_y).max(1.0);

    // 先判定是否在任意一条边上（含顶点）。
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        if on_segment(a, b, p, eps) {
            return PointStatus::Boundary;
        }
    }

    // 射线法（水平向右）。
    let mut inside = false;
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let crosses = (a.y > p.y) != (b.y > p.y);
        if crosses {
            let x_hit = a.x + (b.x - a.x) * (p.y - a.y) / (b.y - a.y);
            if x_hit > p.x {
                inside = !inside;
            }
        }
    }
    if inside {
        PointStatus::Inside
    } else {
        PointStatus::Outside
    }
}

/// 批量分类，返回每个事件的状态。
pub fn classify_all(poly: &Polygon, points: &[Point]) -> Vec<PointStatus> {
    points.iter().map(|p| classify(poly, *p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Polygon {
        Polygon {
            basis: "transformed:asinh.v1".to_string(),
            vertices: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
        }
    }

    #[test]
    fn inside_outside_and_boundary() {
        let p = square();
        assert_eq!(classify(&p, Point { x: 5.0, y: 5.0 }), PointStatus::Inside);
        assert_eq!(classify(&p, Point { x: -1.0, y: 5.0 }), PointStatus::Outside);
        // 正好落在底边。
        assert_eq!(classify(&p, Point { x: 5.0, y: 0.0 }), PointStatus::Boundary);
        // 顶点。
        assert_eq!(classify(&p, Point { x: 0.0, y: 0.0 }), PointStatus::Boundary);
        // 斜边边界点（三角形）。
        let tri = Polygon {
            basis: "raw".to_string(),
            vertices: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 0.0, y: 10.0 },
            ],
        };
        assert_eq!(classify(&tri, Point { x: 5.0, y: 5.0 }), PointStatus::Boundary);
    }

    #[test]
    fn non_finite_points_never_count_inside() {
        let p = square();
        assert_eq!(
            classify(&p, Point { x: f64::NAN, y: 5.0 }),
            PointStatus::Outside
        );
    }
}
