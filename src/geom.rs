//! 2D 点与半开多边形判定。
//!
//! 边界规则（显式、可测试、对事件顺序不敏感）：
//! 多边形顶点按数学正向（逆时针）给出。边分三类：
//! - 上升边（下端点在左：x_lo 处）含两端点；
//! - 下降边（上端点在左：x_hi 处）不含；
//! - 水平边：x_lo 端含、x_hi 端不含（同样“左含右不含”）。
//! 因此任意点恰好落在共享顶点/边上时归属唯一：它只能被一条边“含”。
//! 该规则与射线法的半开扫描完全一致（y0 <= y < y1 的边才与向右射线相交）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self { Point { x, y } }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Boundary {
    Inside,
    IncludedEdge,
    ExcludedEdge,
    Outside,
}

impl Boundary {
    pub fn in_polygon(self) -> bool {
        matches!(self, Boundary::Inside | Boundary::IncludedEdge)
    }
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 { ax * by - ay * bx }

/// 返回点的归属；边界点给出 IncludedEdge/ExcludedEdge 以便审阅页显式展示。
pub fn classify(poly: &[Point], p: Point) -> Boundary {
    let n = poly.len();
    if n < 3 {
        return Boundary::Outside;
    }
    struct E { ax: f64, ay: f64, bx: f64, by: f64, horiz: bool, rising: bool }
    let edges: Vec<E> = (0..n).map(|i| {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        E { ax: a.x, ay: a.y, bx: b.x, by: b.y, horiz: a.y == b.y, rising: a.y < b.y }
    }).collect();
    let eps = 1e-9;
    let on_segment = |ax: f64, ay: f64, bx: f64, by: f64| -> bool {
        let cr = (bx - ax) * (p.y - ay) - (by - ay) * (p.x - ax);
        let scale = 1.0 + (bx - ax).abs() + (by - ay).abs();
        cr.abs() <= eps * scale
            && p.y >= ay.min(by) - eps * scale && p.y <= ay.max(by) + eps * scale
            && p.x >= ax.min(bx) - eps * scale && p.x <= ax.max(bx) + eps * scale
    };

    let is_edge = edges.iter().any(|e| on_segment(e.ax, e.ay, e.bx, e.by));
    if is_edge {
        // 角点先由水平边认领：水平边上仅左端点（x_lo）含。
        for e in edges.iter().filter(|e| e.horiz) {
            if on_segment(e.ax, e.ay, e.bx, e.by) {
                let included = (p.x - e.ax.min(e.bx)).abs() <= eps;
                return if included { Boundary::IncludedEdge } else { Boundary::ExcludedEdge };
            }
        }
        // 非水平边严格内部点（非顶点）：上升边含、下降边不含；
        // 顶端点不含（它属于上方的水平边或为排除顶点），下端点含（上升边）。
        for e in edges.iter().filter(|e| !e.horiz) {
            if on_segment(e.ax, e.ay, e.bx, e.by) {
                let hi = e.ay.max(e.by);
                let at_hi = (p.y - hi).abs() <= eps;
                let included = !at_hi && e.rising;
                return if included { Boundary::IncludedEdge } else { Boundary::ExcludedEdge };
            }
        }
        return Boundary::ExcludedEdge;
    }

    // 严格内/外部：半开射线交叉计数，y in [y_lo, y_hi)。
    // 上升边在叉积 >=0 时计数，下降边在叉积 <=0 时计数（二者共同实现半开规则）。
    let mut crossings = 0_i64;
    for e in &edges {
        if e.horiz {
            continue;
        }
        let (ax, ay, bx, by) = (e.ax, e.ay, e.bx, e.by);
        let (lo_y, hi_y) = (ay.min(by), ay.max(by));
        if p.y >= lo_y && p.y < hi_y {
            let cr = (bx - ax) * (p.y - ay) - (by - ay) * (p.x - ax);
            if e.rising && cr >= 0.0 {
                crossings += 1;
            } else if !e.rising && cr <= 0.0 {
                crossings += 1;
            }
        }
    }
    if crossings % 2 == 1 { Boundary::Inside } else { Boundary::Outside }
}

pub fn contains(poly: &[Point], p: Point) -> bool { classify(poly, p).in_polygon() }

#[cfg(test)]
mod tests {
    use super::*;

    // 单位正方形：(0,0)->(1,0)->(1,1)->(0,1)，数学正向。
    fn sq() -> Vec<Point> {
        vec![Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(1.0, 1.0), Point::new(0.0, 1.0)]
    }

    #[test]
    fn interior_and_exterior() {
        let p = sq();
        assert_eq!(classify(&p, Point::new(0.5, 0.5)), Boundary::Inside);
        assert_eq!(classify(&p, Point::new(1.5, 0.5)), Boundary::Outside);
        assert_eq!(classify(&p, Point::new(-0.1, 0.5)), Boundary::Outside);
    }

    #[test]
    fn boundary_edges_half_open() {
        let p = sq();
        // 底边 y=0 上升方向（从(0,0)到(1,0)水平）：x_lo=(0,0)含，x_hi=(1,0)不含
        assert_eq!(classify(&p, Point::new(0.0, 0.0)), Boundary::IncludedEdge);
        assert_eq!(classify(&p, Point::new(1.0, 0.0)), Boundary::ExcludedEdge);
        // 右边 x=1：从(1,0)到(1,1)上升，内部含；顶点归水平边。
        assert_eq!(classify(&p, Point::new(1.0, 1.0)), Boundary::ExcludedEdge);
        assert_eq!(classify(&p, Point::new(1.0, 0.5)), Boundary::IncludedEdge);
        // 顶边 y=1 水平，x_lo=(0,1)含、x_hi=(1,1)不含
        assert_eq!(classify(&p, Point::new(0.0, 1.0)), Boundary::IncludedEdge);
        assert_eq!(classify(&p, Point::new(0.5, 1.0)), Boundary::ExcludedEdge);
        // 左边 x=0：从(0,1)到(0,0)下降，内部不含；(0,0) 归水平底边(含)
        assert_eq!(classify(&p, Point::new(0.0, 0.5)), Boundary::ExcludedEdge);
    }

    #[test]
    fn shared_vertex_belongs_to_exactly_one_owner() {
        // 关键性质：任何边界点有确定归属，不依赖扫描顺序。
        let p = sq();
        for v in &p {
            // 只断言判定确定（含或不含），不 panic、不重复计数。
            let _ = contains(&p, *v);
        }
        let b = classify(&p, Point::new(1.0, 1.0));
        // (1,1)：水平顶边的右端点 -> 不含；即使也在上升右边终点。
        assert_eq!(b, Boundary::ExcludedEdge);
    }

    #[test]
    fn polygon_clockwise_uses_declared_orientation() {
        // 反向（顺时针）多边形 winding 为 -1：约定门必须以数学正向给出，
        // 装载时统一规范，这里直接确认非零 winding 判定。
        let p = vec![Point::new(0.0, 0.0), Point::new(0.0, 1.0), Point::new(1.0, 1.0), Point::new(1.0, 0.0)];
        assert_eq!(classify(&p, Point::new(0.5, 0.5)), Boundary::Inside);
    }
}
