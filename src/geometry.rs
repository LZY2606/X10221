//! 门几何：半开约定的射线投射，边界点带稳定 boundary 标记（身份可断言）。
//! 多边形顶点与边界点：inside=true 且 boundary=true；矩形右/上边为半开排除侧。

use crate::models::{GateGeom, Point};

#[derive(Debug, Clone, PartialEq)]
pub struct PointClass {
    pub inside: bool,
    pub boundary: bool,
}

pub fn classify(geom: &GateGeom, x: f64, y: f64) -> PointClass {
    match geom {
        GateGeom::Polygon { polygon, .. } => {
            let (inside, boundary) = point_in_polygon(&Point { x, y }, &polygon.vertices);
            PointClass { inside, boundary }
        }
        GateGeom::Rectangle {
            x0, x1, y0, y1, ..
        } => {
            let on_closed_edge = (x == *x0 && y >= *y0 && y < *y1)
                || (y == *y0 && x >= *x0 && x < *x1);
            let on_open_edge = x == *x1 || y == *y1;
            PointClass {
                inside: x >= *x0 && x < *x1 && y >= *y0 && y < *y1,
                boundary: on_closed_edge || (on_open_edge && x >= *x0 && x <= *x1 && y >= *y0 && y <= *y1),
            }
        }
    }
}

fn point_in_polygon(p: &Point, poly: &[Point]) -> (bool, bool) {
    let n = poly.len();
    if n < 3 {
        return (false, false);
    }
    let mut inside = false;
    let mut on_border = false;
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        if on_segment(a, b, p) {
            on_border = true;
        }
        let crosses = (a.y > p.y) != (b.y > p.y);
        if crosses {
            let x_inter = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x_inter {
                inside = !inside;
            }
        }
    }
    if on_border {
        // 多边形采用闭边界：所有顶点与边上点归内，并带 boundary 标记
        return (true, true);
    }
    (inside, false)
}

fn on_segment(a: &Point, b: &Point, p: &Point) -> bool {
    let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    if cross.abs() > 1e-12 {
        return false;
    }
    let dot = (p.x - a.x) * (p.x - b.x) + (p.y - a.y) * (p.y - b.y);
    dot <= 1e-12
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Polygon;

    fn square() -> Vec<Point> {
        vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
            Point { x: 0.0, y: 10.0 },
        ]
    }

    fn poly_gate() -> GateGeom {
        GateGeom::Polygon {
            x_channel: "x".into(),
            y_channel: "y".into(),
            polygon: Polygon {
                vertices: square(),
                coordinate_system: "transformed".into(),
            },
        }
    }

    #[test]
    fn inside_and_boundary() {
        let g = poly_gate();
        assert_eq!(
            classify(&g, 5.0, 5.0),
            PointClass { inside: true, boundary: false }
        );
        let edge = classify(&g, 10.0, 5.0);
        assert!(edge.boundary && edge.inside);
        assert_eq!(classify(&g, 0.0, 0.0), PointClass { inside: true, boundary: true });
        assert!(!classify(&g, 11.0, 5.0).inside);
    }

    #[test]
    fn rectangle_half_open() {
        let g = GateGeom::Rectangle {
            x_channel: "x".into(),
            y_channel: "y".into(),
            x0: 0.0, x1: 10.0, y0: 0.0, y1: 10.0,
            coordinate_system: "transformed".into(),
        };
        assert!(classify(&g, 0.0, 5.0).inside);
        assert!(!classify(&g, 10.0, 5.0).inside); // 右边半开
        assert!(classify(&g, 10.0, 5.0).boundary);
        assert!(!classify(&g, 5.0, 10.0).inside); // 上边半开
    }
}
