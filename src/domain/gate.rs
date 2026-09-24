//! 门几何：多边形（射线法）与矩形；边界点算入（流式惯例，显式测试）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GateGeometry {
    Polygon { points: Vec<Point> },
    Rectangle { x_min: f64, x_max: f64, y_min: f64, y_max: f64 },
}

const EPS: f64 = 1e-9;

impl GateGeometry {
    pub fn contains(&self, p: Point) -> bool {
        match self {
            GateGeometry::Rectangle { x_min, x_max, y_min, y_max } => {
                p.x >= *x_min - EPS && p.x <= *x_max + EPS && p.y >= *y_min - EPS && p.y <= *y_max + EPS
            }
            GateGeometry::Polygon { points } => polygon_contains(points, p),
        }
    }

    pub fn vertices(&self) -> Vec<Point> {
        match self {
            GateGeometry::Rectangle { x_min, x_max, y_min, y_max } => vec![
                Point { x: *x_min, y: *y_min },
                Point { x: *x_max, y: *y_min },
                Point { x: *x_max, y: *y_max },
                Point { x: *x_min, y: *y_max },
            ],
            GateGeometry::Polygon { points } => points.clone(),
        }
    }
}

fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    if cross.abs() > EPS {
        return false;
    }
    p.x >= a.x.min(b.x) - EPS && p.x <= a.x.max(b.x) + EPS
        && p.y >= a.y.min(b.y) - EPS && p.y <= a.y.max(b.y) + EPS
}

fn polygon_contains(points: &[Point], p: Point) -> bool {
    let n = points.len();
    if n < 3 {
        return false;
    }
    for i in 0..n {
        if on_segment(points[i], points[(i + 1) % n], p) {
            return true; // 边界点算入
        }
    }
    let mut inside = false;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let intersects = (a.y > p.y) != (b.y > p.y)
            && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x + EPS;
        if intersects {
            inside = !inside;
        }
    }
    inside
}
