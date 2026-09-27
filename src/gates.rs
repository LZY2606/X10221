//! 门几何与逐事件判定。边界点（恰在多边形边上）无法唯一决定门内/门外，
//! 必须保留为独立候选，而不是任意归入一侧。

use crate::domain::{GatePolygon, Point, TransformParams, ValueStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    /// 恰在边界上：合法归属不唯一
    Boundary,
    /// 所需通道存在无效值（如 NaN），无法判定，留在原位置并附诊断
    Undefined,
}

fn cross(a: Point, b: Point, p: Point) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

pub fn point_membership(gate: &GatePolygon, p: Point) -> Membership {
    if p.x.is_nan() || p.y.is_nan() {
        return Membership::Undefined;
    }
    let n = gate.vertices.len();
    if n < 3 {
        return Membership::Undefined;
    }
    let mut inside = false;
    let mut on_edge = false;
    let mut i = 0usize;
    let mut j = n - 1;
    while i < n {
        let vi = gate.vertices[i];
        let vj = gate.vertices[j];
        let c = cross(vj, vi, p);
        if c == 0.0
            && p.x <= vi.x.max(vj.x)
            && p.x >= vi.x.min(vj.x)
            && p.y <= vi.y.max(vj.y)
            && p.y >= vi.y.min(vj.y)
        {
            on_edge = true;
        }
        if (vi.y > p.y) != (vj.y > p.y) {
            let hit_x = vj.x + (vi.x - vj.x) * (p.y - vj.y) / (vi.y - vj.y);
            if p.x < hit_x {
                inside = !inside;
            }
        }
        j = i;
        i += 1;
    }
    if on_edge {
        Membership::Boundary
    } else if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

/// asinh 变换；NaN 透传保留。
pub fn transform_value(raw: f64, cofactor: f64) -> f64 {
    if raw.is_nan() {
        return f64::NAN;
    }
    ((raw / cofactor).asinh()) * std::f64::consts::LN_10
}

/// 从原始事件 BTreeMap 取某通道在给定 transform 下的坐标值。
pub fn coordinate(
    channels: &std::collections::BTreeMap<String, crate::domain::ChannelValue>,
    name: &str,
    transform: &TransformParams,
) -> (f64, ValueStatus) {
    let cv = channels
        .get(name)
        .unwrap_or_else(|| panic!("event 缺少通道 {name}（导入时应保证模式一致）"));
    let raw = cv.raw;
    if raw.is_nan() {
        return (f64::NAN, cv.status);
    }
    let mode = transform.mode.get(name).map(String::as_str).unwrap_or("asinh");
    let val = match mode {
        "linear" => raw,
        _ => {
            let co = transform.cofactors.get(name).copied().unwrap_or(150.0);
            transform_value(raw, co)
        }
    };
    (val, cv.status)
}

/// 三个合法候选：边界点归入、边界点排除、仅边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateCandidates {
    pub inclusive: Vec<String>,
    pub exclusive: Vec<String>,
    pub boundary_only: Vec<String>,
    pub undefined: Vec<String>,
    pub boundary: bool,
}

pub fn classify_events(
    gate: &GatePolygon,
    event_ids: &[(String, f64, f64)],
) -> GateCandidates {
    let mut candidates = GateCandidates {
        inclusive: Vec::new(),
        exclusive: Vec::new(),
        boundary_only: Vec::new(),
        undefined: Vec::new(),
        boundary: false,
    };
    for (id, x, y) in event_ids {
        match point_membership(gate, Point { x: *x, y: *y }) {
            Membership::Inside => {
                candidates.inclusive.push(id.clone());
                candidates.exclusive.push(id.clone());
            }
            Membership::Boundary => {
                candidates.inclusive.push(id.clone());
                candidates.boundary_only.push(id.clone());
                candidates.boundary = true;
            }
            Membership::Outside => {
                candidates.exclusive.push(id.clone());
            }
            Membership::Undefined => {
                candidates.exclusive.push(id.clone());
                candidates.undefined.push(id.clone());
            }
        }
    }
    candidates.inclusive.sort();
    candidates.exclusive.sort();
    candidates.boundary_only.sort();
    candidates.undefined.sort();
    candidates
}

/// 对一组测量信号应用补偿： compensated = S^{-1} x。
/// 近奇异矩阵由调用方通过 CompensationMatrix::inverse() 拦截。
pub fn apply_compensation(inv: &[Vec<f64>], measured: &[f64]) -> Vec<f64> {
    let n = inv.len();
    let mut out = Vec::with_capacity(n);
    for row in inv {
        let mut s = 0.0;
        let mut bad = false;
        for j in 0..n {
            if measured[j].is_nan() {
                bad = true;
            }
            s += row[j] * measured[j];
        }
        if bad {
            s = f64::NAN;
        }
        out.push(s);
    }
    out
}
