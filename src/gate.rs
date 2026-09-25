//! 门多边形/矩形与父子关系分开版本化。
//! 边界点判定显式：boundary_inclusive=false 时边上的点不计入（边界点场景）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum GateShape {
    Polygon { vertices: Vec<Point> },
    Rectangle { x0: f64, x1: f64, y0: f64, y1: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDef {
    /// 门控使用的变换坐标基准（钉住变换版本）。
    pub x_basis: String,
    pub y_basis: String,
    pub shape: GateShape,
    /// 边界点是否计入；false 时严格在内才算（边界点反例）。
    pub boundary_inclusive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Membership {
    Inside,
    Boundary,
    Outside,
    Invalid,
}

const EPS: f64 = 1e-9;

impl GateDef {
    pub fn membership(&self, p: Point) -> Membership {
        if !p.x.is_finite() || !p.y.is_finite() {
            return Membership::Invalid;
        }
        match &self.shape {
            GateShape::Rectangle { x0, x1, y0, y1 } => {
                let on_x = (p.x - x0).abs() < EPS || (p.x - x1).abs() < EPS;
                let on_y = (p.y - y0).abs() < EPS || (p.y - y1).abs() < EPS;
                let in_x = p.x > *x0 - EPS && p.x < *x1 + EPS;
                let in_y = p.y > *y0 - EPS && p.y < *y1 + EPS;
                classify(in_x, in_y, on_x || on_y, self.boundary_inclusive)
            }
            GateShape::Polygon { vertices } => {
                if vertices.len() < 3 {
                    return Membership::Invalid;
                }
                let (inside, on_edge) = point_in_polygon(vertices, p);
                classify(inside, true, on_edge, self.boundary_inclusive)
            }
        }
    }
}

fn classify(in_bounds: bool, _axis_ok: bool, on_boundary: bool, inclusive: bool) -> Membership {
    if on_boundary && !inclusive {
        Membership::Boundary
    } else if in_bounds {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

/// 返回 (严格在内或边界上, 是否在边界上)。
fn point_in_polygon(verts: &[Point], p: Point) -> (bool, bool) {
    let mut inside = false;
    let n = verts.len();
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        if on_segment(a, b, p) {
            return (true, true);
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x_cross = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x_cross {
                inside = !inside;
            }
        }
    }
    (inside, false)
}

fn on_segment(a: Point, b: Point, p: Point) -> bool {
    let cross = (p.x - a.x) * (b.y - a.y) - (p.y - a.y) * (b.x - a.x);
    if cross.abs() > EPS {
        return false;
    }
    let dot = (p.x - a.x) * (p.x - b.x) + (p.y - a.y) * (p.y - b.y);
    dot <= EPS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> GateDef {
        GateDef {
            x_basis: "transform:asinh:channel:FITC".into(),
            y_basis: "transform:asinh:channel:PE".into(),
            shape: GateShape::Rectangle { x0: 0.0, x1: 10.0, y0: 0.0, y1: 10.0 },
            boundary_inclusive: false,
        }
    }

    #[test]
    fn interior_is_inside() {
        assert_eq!(rect().membership(Point { x: 5.0, y: 5.0 }), Membership::Inside);
    }

    #[test]
    fn edge_point_is_boundary_when_exclusive() {
        assert_eq!(rect().membership(Point { x: 0.0, y: 5.0 }), Membership::Boundary);
    }

    #[test]
    fn boundary_flag_changes_membership() {
        let mut g = rect();
        g.boundary_inclusive = true;
        assert_eq!(g.membership(Point { x: 0.0, y: 5.0 }), Membership::Inside);
    }

    #[test]
    fn polygon_edges_are_detected() {
        let g = GateDef {
            x_basis: "x".into(),
            y_basis: "y".into(),
            shape: GateShape::Polygon {
                vertices: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 10.0, y: 0.0 },
                    Point { x: 10.0, y: 10.0 },
                ],
            },
            boundary_inclusive: false,
        };
        assert_eq!(g.membership(Point { x: 2.0, y: 2.0 }), Membership::Inside);
        assert_eq!(g.membership(Point { x: 5.0, y: 0.0 }), Membership::Boundary);
        assert_eq!(g.membership(Point { x: 20.0, y: 20.0 }), Membership::Outside);
    }
}
