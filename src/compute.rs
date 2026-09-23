//! 领域计算：补偿矩阵、变换、门多边形判定。
//! 规则：近奇异补偿矩阵不能被默默求逆 —— 由 cond_estimate 判定后交给候选裁定流程。

use serde_json::Value;

/// 条件数阈值：超过即视为近奇异，禁止静默求逆。
pub const COND_THRESHOLD: f64 = 1.0e6;
/// 边界点容差（变换后坐标单位）。
pub const BOUNDARY_EPS: f64 = 0.02;
/// 岭正则化系数（仅经人工裁定采纳后才允许使用）。
pub const REG_LAMBDA: f64 = 1.0e-3;

pub fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

/// Gauss-Jordan 求逆（部分主元）。奇异时返回 None，绝不静默产出垃圾逆矩阵。
pub fn mat_inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = identity(n);
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..n {
                        a[r][j] -= f * a[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    Some(inv)
}

fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .map(|r| r.iter().map(|x| x.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// 条件数估计 ||M||inf * ||M^-1||inf；完全奇异时为无穷大。
pub fn cond_estimate(m: &[Vec<f64>]) -> f64 {
    match mat_inverse(m) {
        Some(inv) => inf_norm(m) * inf_norm(&inv),
        None => f64::INFINITY,
    }
}

pub fn regularized(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let mut out = m.to_vec();
    for i in 0..n {
        out[i][i] += lambda;
    }
    out
}

pub fn apply_matrix(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

/// asinh 变换，坐标基准为 asinh(raw / cofactor)。
pub fn asinh_t(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pt {
    Inside,
    Outside,
    Boundary,
}

fn dist_point_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// 射线法 + 边界容差：恰好落在门边界上的点判为 Boundary，无法唯一决定，留给候选裁定。
pub fn classify(p: (f64, f64), poly: &[(f64, f64)]) -> Pt {
    let n = poly.len();
    if n < 3 {
        return Pt::Outside;
    }
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if dist_point_segment(p, a, b) < BOUNDARY_EPS {
            return Pt::Boundary;
        }
    }
    let (px, py) = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Pt::Inside
    } else {
        Pt::Outside
    }
}

/// 语义变更判定：门改名（仅 name 变化）不改变几何，不触发失效传播。
pub fn semantic_change(kind: &str, old: &Value, new: &Value) -> bool {
    if kind == "gate_polygon" {
        return old["polygon"] != new["polygon"] || old["channels"] != new["channels"];
    }
    old != new
}

/// 递归 JSON 差异，用于版本冲突时向双方展示差异。
pub fn json_diff(a: &Value, b: &Value, path: &str, out: &mut Vec<Value>) {
    match (a, b) {
        (Value::Object(ma), Value::Object(mb)) => {
            for (k, va) in ma {
                let p = format!("{path}.{k}");
                match mb.get(k) {
                    Some(vb) => json_diff(va, vb, &p, out),
                    None => out.push(serde_json::json!({"path": p, "old": va, "new": null})),
                }
            }
            for (k, vb) in mb {
                if !ma.contains_key(k) {
                    let p = format!("{path}.{k}");
                    out.push(serde_json::json!({"path": p, "old": null, "new": vb}));
                }
            }
        }
        (Value::Array(xa), Value::Array(xb)) if xa.len() == xb.len() => {
            for (i, (va, vb)) in xa.iter().zip(xb.iter()).enumerate() {
                json_diff(va, vb, &format!("{path}[{i}]"), out);
            }
        }
        _ => {
            if a != b {
                out.push(serde_json::json!({"path": path, "old": a, "new": b}));
            }
        }
    }
}
