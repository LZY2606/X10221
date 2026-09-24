//! 推导规则引擎：补偿矩阵求逆（拒绝默默逆近奇异矩阵）、arcsinh 变换、
//! 门多边形求值（边界点显式化）、派生值计算。全部为纯函数，便于单元测试。
use serde_json::{json, Map, Value};

/// 主元比低于该阈值视为近奇异，禁止默默求逆。
pub const SINGULAR_THRESHOLD: f64 = 1e-6;
/// 门边界判定距离阈值（门坐标单位）。
pub const BOUNDARY_EPS: f64 = 1e-4;
/// 荧光通道固定顺序。
pub const FLUOR: [&str; 3] = ["FITC-A", "PE-A", "APC-A"];

#[derive(Debug, Clone)]
pub struct Inverse {
    pub matrix: Vec<Vec<f64>>,
    pub determinant: f64,
    pub min_pivot_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct SingularReport {
    pub determinant: f64,
    pub min_pivot_ratio: f64,
    pub reason: String,
}

/// 高斯消元（部分主元）求逆。奇异或近奇异时返回诊断，绝不默默给出逆矩阵。
pub fn invert_matrix(m: &[Vec<f64>]) -> Result<Inverse, SingularReport> {
    let n = m.len();
    let max_abs = m
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0f64, |a, b| a.max(b.abs()));
    let mut a: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = r.clone();
            row.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            row
        })
        .collect();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pv = a[piv][col];
        if pv == 0.0 {
            return Err(SingularReport {
                determinant: 0.0,
                min_pivot_ratio: 0.0,
                reason: format!("第 {} 列主元为 0，矩阵奇异", col),
            });
        }
        min_pivot = min_pivot.min(pv.abs());
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        det *= pv;
        for r in 0..n {
            if r != col && a[r][col] != 0.0 {
                let f = a[r][col] / pv;
                for c in 0..2 * n {
                    a[r][c] -= f * a[col][c];
                }
            }
        }
    }
    for r in 0..n {
        let d = a[r][r];
        for c in 0..2 * n {
            a[r][c] /= d;
        }
    }
    let ratio = if max_abs > 0.0 { min_pivot / max_abs } else { 0.0 };
    if ratio < SINGULAR_THRESHOLD {
        return Err(SingularReport {
            determinant: det,
            min_pivot_ratio: ratio,
            reason: format!(
                "最小主元比 {:.3e} 低于阈值 {:.0e}，矩阵近奇异，拒绝默默求逆",
                ratio, SINGULAR_THRESHOLD
            ),
        });
    }
    let inv = a.iter().map(|r| r[n..].to_vec()).collect();
    Ok(Inverse { matrix: inv, determinant: det, min_pivot_ratio: ratio })
}

/// arcsinh 变换：arcsinh(x / cofactor)。
pub fn arcsinh(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateHit {
    Inside,
    Outside,
    /// 命中第几条边（0 基）。
    Boundary(usize),
}

fn seg_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
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

/// 射线法 + 边界距离：点恰在边上（距离 < eps）时显式返回 Boundary，
/// 绝不悄悄归入内部或外部。
pub fn locate_point(x: f64, y: f64, poly: &[[f64; 2]]) -> GateHit {
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        if seg_dist(x, y, a[0], a[1], b[0], b[1]) < BOUNDARY_EPS {
            return GateHit::Boundary(i);
        }
    }
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        GateHit::Inside
    } else {
        GateHit::Outside
    }
}

/// 单个事件的派生层：补偿后值与变换后值；不可转换的原始值留在原位置并附诊断。
#[derive(Debug, Clone)]
pub struct DerivedEvent {
    pub id: String,
    pub raw: Map<String, Value>,
    pub compensated: Option<[f64; 3]>,
    pub transformed: Option<[f64; 3]>,
    pub diagnostics: Vec<String>,
}

pub fn derive_event(
    raw: &Map<String, Value>,
    inverse: Option<&Inverse>,
    cofactor: f64,
    import_diagnostics: &[String],
) -> DerivedEvent {
    let mut diagnostics: Vec<String> = import_diagnostics.to_vec();
    let mut obs = [0.0f64; 3];
    let mut convertible = true;
    for (i, ch) in FLUOR.iter().enumerate() {
        match raw.get(*ch).and_then(|v| v.as_f64()) {
            Some(v) => obs[i] = v,
            None => {
                convertible = false;
                diagnostics.push(format!("unconvertible:{}（原始表示保留在原位置）", ch));
            }
        }
    }
    let compensated = if convertible {
        inverse.map(|inv| {
            let mut out = [0.0f64; 3];
            for i in 0..3 {
                out[i] = (0..3).map(|j| inv.matrix[i][j] * obs[j]).sum();
            }
            out
        })
    } else {
        None
    };
    let transformed = compensated.map(|c| {
        [
            arcsinh(c[0], cofactor),
            arcsinh(c[1], cofactor),
            arcsinh(c[2], cofactor),
        ]
    });
    DerivedEvent {
        id: String::new(),
        raw: raw.clone(),
        compensated,
        transformed,
        diagnostics,
    }
}

/// 取事件在指定坐标基准下的二维坐标。
pub fn project(ev: &DerivedEvent, chx: &str, chy: &str, basis: &str) -> Option<(f64, f64)> {
    if basis == "raw" {
        let x = ev.raw.get(chx)?.as_f64()?;
        let y = ev.raw.get(chy)?.as_f64()?;
        Some((x, y))
    } else {
        let t = ev.transformed?;
        let ix = FLUOR.iter().position(|c| *c == chx)?;
        let iy = FLUOR.iter().position(|c| *c == chy)?;
        Some((t[ix], t[iy]))
    }
}

#[derive(Debug, Clone)]
pub struct GateEval {
    pub members: Vec<String>,
    /// (event_id, edge_index)
    pub boundary: Vec<(String, usize)>,
    /// 因不可转换等原因无法求值的事件。
    pub skipped: Vec<String>,
}

/// 在候选事件集合上求值门多边形。
pub fn eval_gate<'a>(
    points: impl Iterator<Item = (&'a str, Option<(f64, f64)>)>,
    poly: &[[f64; 2]],
) -> GateEval {
    let mut eval = GateEval { members: vec![], boundary: vec![], skipped: vec![] };
    for (id, pt) in points {
        match pt {
            None => eval.skipped.push(id.to_string()),
            Some((x, y)) => match locate_point(x, y, poly) {
                GateHit::Inside => eval.members.push(id.to_string()),
                GateHit::Boundary(e) => eval.boundary.push((id.to_string(), e)),
                GateHit::Outside => {}
            },
        }
    }
    eval
}

/// 解析门 payload 中的多边形。
pub fn polygon_of(gate_payload: &Value) -> Vec<[f64; 2]> {
    gate_payload["polygon"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|p| Some([p[0].as_f64()?, p[1].as_f64()?]))
                .collect()
        })
        .unwrap_or_default()
}

/// 稳定身份：series@branch#version。
pub fn aid(series: &str, branch: &str, version: i64) -> String {
    format!("{}@{}#{}", series, branch, version)
}

/// 机器可读的判定依据事件 payload 构造辅助。
pub fn judgment(rule: &str, detail: Value) -> Value {
    json!({"rule": rule, "detail": detail})
}
