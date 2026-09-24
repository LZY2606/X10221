//! 门多边形与点-多边形关系。
//!
//! 边界点（恰好落在多边形边上）不做静默归类：返回 [`Membership::Boundary`]，
//! 由审阅者显式裁定（计入/排除）。门与父群以稳定身份引用，改名不改身份时谱系不断。

use serde::{Deserialize, Serialize};

/// 坐标基准：散点使用的两个通道（及各自变换版本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordBasis {
    pub x_channel: String,
    pub y_channel: String,
    /// 变换参数版本 rev（None 表示原始线性坐标）。
    pub x_transform_rev: Option<String>,
    pub y_transform_rev: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Membership {
    Inside,
    Outside,
    /// 落在多边形边界上：歧义，不静默决定。
    Boundary,
    /// 参与坐标中有无法变换/缺失的值：不能判定。
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolygonGate {
    /// 稳定身份（改名不换身份）。
    pub stable_id: String,
    /// 可变人类可读名。
    pub name: String,
    /// 父门稳定身份；根群为 None。
    pub parent_gate_id: Option<String>,
    pub basis: CoordBasis,
    /// 有序顶点（闭合多边形，首尾无需重复）。
    pub polygon: Vec<Point>,
    /// 本版本序号（同一 stable_id 单调递增）。
    pub ordinal: u64,
    /// 本版本内容指纹（由写入侧计算，与 GateVersion.rev 一致）。
    pub rev: String,
}

const BOUNDARY_EPS: f64 = 1e-9;

/// 判定点在多边形内/外/边界上。坐标缺失（None）返回 Unknown。
pub fn classify(gate: &PolygonGate, x: Option<f64>, y: Option<f64>) -> Membership {
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => (x, y),
        _ => return Membership::Unknown,
    };
    let poly = &gate.polygon;
    if poly.len() < 3 {
        return Membership::Unknown;
    }

    let mut on_edge = false;
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        // 点到线段距离：近零即边界
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len2 = dx * dx + dy * dy;
        let t = if len2 == 0.0 {
            0.0
        } else {
            (((x - a.x) * dx + (y - a.y) * dy) / len2).clamp(0.0, 1.0)
        };
        let px = a.x + t * dx;
        let py = a.y + t * dy;
        let dist = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
        if dist <= BOUNDARY_EPS {
            on_edge = true;
        }
        // 射线法
        if (a.y > y) != (b.y > y) {
            let xcross = a.x + (y - a.y) * dx / dy;
            if xcross > x {
                inside = !inside;
            }
        }
    }
    if on_edge {
        Membership::Boundary
    } else if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> PolygonGate {
        PolygonGate {
            stable_id: "gate#g".into(),
            name: "g".into(),
            parent_gate_id: None,
            basis: CoordBasis {
                x_channel: "FSC-A".into(),
                y_channel: "SSC-A".into(),
                x_transform_rev: None,
                y_transform_rev: None,
            },
            polygon: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
            ordinal: 1,
            rev: "test-rev".into(),
        }
    }

    #[test]
    fn inside_outside() {
        let g = square();
        assert_eq!(classify(&g, Some(5.0), Some(5.0)), Membership::Inside);
        assert_eq!(classify(&g, Some(20.0), Some(5.0)), Membership::Outside);
    }

    #[test]
    fn edge_points_are_boundary_not_silently_classified() {
        let g = square();
        assert_eq!(classify(&g, Some(5.0), Some(0.0)), Membership::Boundary);
        assert_eq!(classify(&g, Some(10.0), Some(5.0)), Membership::Boundary);
        assert_eq!(classify(&g, Some(0.0), Some(0.0)), Membership::Boundary);
    }

    #[test]
    fn unknown_when_coord_missing() {
        let g = square();
        assert_eq!(classify(&g, None, Some(1.0)), Membership::Unknown);
        assert_eq!(classify(&g, Some(f64::NAN), Some(1.0)), Membership::Unknown);
    }
}
