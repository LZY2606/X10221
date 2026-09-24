//! 领域原语：单位/坐标基准、补偿矩阵、可逆性诊断、asinh 变换、门与点在多边形判定。
//! 约定：导入层保留原始表示（字符串、标记），本模块只作用于派生层。

use serde::{Deserialize, Serialize};

/// 带单位/坐标基准的量。raw_repr 始终保留导入时的原始表示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Measure {
    pub value: Option<f64>,
    /// 物理单位或坐标基准，例如 "channel:FITC/raw_scale"、"dimensionless/asinh:T1"
    pub basis: String,
    #[serde(default)]
    pub raw_repr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Matrix {
    /// 行 i 列 j：检测器 i 收到的信号中来自荧光素 j 的比例（SPILL 约定）。
    pub rows: Vec<Vec<f64>>,
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCand {
    pub id: String,
    pub label: String,
    /// 候选补偿后值（每个事件一个向量）
    pub values: Vec<Vec<f64>>,
    /// 与“理想逆矩阵”的残差范数（相对）
    pub residual: f64,
    /// 正则化强度（奇异/近奇异时才非零）
    pub ridge: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompOutcome {
    pub status: String, // "ok" | "singular" | "near_singular"
    pub condition: f64,
    pub determinant: f64,
    pub values: Option<Vec<Vec<f64>>>,
    pub candidates: Vec<CompCand>,
    pub diagnostics: Vec<String>,
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = b[0].len();
    let k = b.len();
    let mut c = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            c[i][j] = s;
        }
    }
    c
}

fn det(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a = m.to_vec();
    let mut d = 1.0;
    for i in 0..n {
        let mut p = i;
        for r in (i + 1)..n {
            if a[r][i].abs() > a[p][i].abs() {
                p = r;
            }
        }
        if a[p][i].abs() < 1e-14 {
            return 0.0;
        }
        if p != i {
            a.swap(p, i);
            d = -d;
        }
        d *= a[i][i];
        for r in (i + 1)..n {
            let f = a[r][i] / a[i][i];
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
        }
    }
    d
}

/// 用幂迭代估计 2-范数条件数；奇异时返回 inf。
fn cond2(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let ata = |mm: &[Vec<f64>]| -> Vec<Vec<f64>> {
        let mut c = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                let mut s = 0.0;
                for k in 0..n {
                    s += mm[k][i] * mm[k][j];
                }
                c[i][j] = s;
            }
        }
        c
    };
    let a2 = ata(m);
    let spectral = |q: &[Vec<f64>]| -> f64 {
        let mut v = vec![1.0 / (n as f64).sqrt(); n];
        let mut lam = 0.0;
        for _ in 0..200 {
            let mut w = vec![0.0; n];
            for i in 0..n {
                for j in 0..n {
                    w[i] += q[i][j] * v[j];
                }
            }
            let norm = w.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-300);
            v = w.into_iter().map(|x| x / norm).collect();
            let mut nv = vec![0.0; n];
            for i in 0..n {
                for j in 0..n {
                    nv[i] += q[i][j] * v[j];
                }
            }
            lam = v.iter().zip(&nv).map(|(a, b)| a * b).sum();
        }
        lam.max(0.0)
    };
    let smax = spectral(&a2);
    if smax <= 1e-20 {
        return f64::INFINITY;
    }
    // 最小奇异值：用位移求逆不便，改用 A^T A 反幂近似；此处用迹的比例下界更稳：
    // cond >= smax^0.5 / smin^0.5，以 A^TA 最小特征值近似。
    let tr: f64 = (0..n).map(|i| a2[i][i]).sum::<f64>();
    if tr <= 0.0 {
        return f64::INFINITY;
    }
    // 用反幂迭代求最小特征值
    let mut v = vec![0.0; n];
    for i in 0..n {
        v[i] = if i == 0 { 1.0 } else { 0.37 * (i as f64 + 1.0) };
    }
    let mut smin = 0.0;
    for _ in 0..300 {
        // 解 a2 * w = v（高斯消元）
        let mut aug = vec![vec![0.0; n + 1]; n];
        for i in 0..n {
            for j in 0..n {
                aug[i][j] = a2[i][j];
            }
            aug[i][n] = v[i];
        }
        for col in 0..n {
            let mut p = col;
            for r in (col + 1)..n {
                if aug[r][col].abs() > aug[p][col].abs() {
                    p = r;
                }
            }
            if aug[p][col].abs() < 1e-18 {
                return f64::INFINITY;
            }
            aug.swap(p, col);
            for r in 0..n {
                if r == col {
                    continue;
                }
                let f = aug[r][col] / aug[col][col];
                for c in col..=n {
                    aug[r][c] -= f * aug[col][c];
                }
            }
        }
        let w: Vec<f64> = (0..n).map(|i| aug[i][n] / aug[i][i]).collect();
        let norm = w.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-300);
        v = w.into_iter().map(|x| x / norm).collect();
        let mut nv = vec![0.0; n];
        for i in 0..n {
            for j in 0..n {
                nv[i] += a2[i][j] * v[j];
            }
        }
        let lam: f64 = v.iter().zip(&nv).map(|(a, b)| a * b).sum();
        smin = 1. / lam.max(1e-300);
    }
    if smin <= 1e-18 {
        return f64::INFINITY;
    }
    (smax / smin).sqrt()
}

fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut aug = vec![vec![0.0; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = m[i][j];
        }
        aug[i][n + i] = 1.0;
    }
    for col in 0..n {
        let mut p = col;
        for r in (col + 1)..n {
            if aug[r][col].abs() > aug[p][col].abs() {
                p = r;
            }
        }
        if aug[p][col].abs() < 1e-14 {
            return None;
        }
        aug.swap(p, col);
        let piv = aug[col][col];
        for c in 0..2 * n {
            aug[col][c] /= piv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = aug[r][col];
            for c in 0..2 * n {
                aug[r][c] -= f * aug[col][c];
            }
        }
    }
    Some(aug.iter().map(|r| r[n..].to_vec()).collect())
}

fn ridge_inverse(m: &[Vec<f64>], ridge: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let mut ata = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += m[k][i] * m[k][j];
            }
            ata[i][j] = s + if i == j { ridge } else { 0.0 };
        }
    }
    let inv = inverse(&ata).unwrap_or_else(|| vec![vec![f64::NAN; n]; n]);
    // (M^T M + λI)^-1 M^T
    let mt: Vec<Vec<f64>> = (0..n)
        .map(|j| (0..n).map(|i| m[i][j]).collect())
        .collect();
    mat_mul(&inv, &mt)
}

/// 阈值：条件数超过此值视为近奇异，绝对奇异（det≈0 或逆不存在）直接拒绝静默求逆。
pub const NEAR_SINGULAR_COND: f64 = 1e6;

/// 对事件矩阵 X（行=事件，列=检测器）施加 SPILL 补偿：Xcomp = X * S^-1。
pub fn compensate(matrix: &Matrix, events: &[Vec<f64>]) -> CompOutcome {
    let n = matrix.rows.len();
    let mut diag = Vec::with_capacity(n);
    let mut diagnostics = Vec::new();
    if matrix.detectors.len() != n
        || matrix.fluorochromes.len() != n
        || matrix.rows.iter().any(|r| r.len() != n)
    {
        return CompOutcome {
            status: "singular".into(),
            condition: f64::INFINITY,
            determinant: f64::NAN,
            values: None,
            candidates: vec![],
            diagnostics: vec!["矩阵不是方阵或标签长度不匹配".into()],
        };
    }
    let determinant = det(&matrix.rows);
    let condition = cond2(&matrix.rows);
    let inv = inverse(&matrix.rows);
    let singular = inv.is_none() || determinant.abs() < 1e-12 || !condition.is_finite();
    if singular {
        diagnostics.push(format!(
            "补偿矩阵奇异：det={:.3e}，条件数={:.3e}，拒绝静默求逆"
        ));
        return CompOutcome {
            status: "singular".into(),
            condition,
            determinant,
            values: None,
            candidates: vec![],
            diagnostics,
        };
    }
    let inv = inv.unwrap();
    for i in 0..n {
        diag.push(matrix.rows[i][i]);
    }
    let apply = |m: &[Vec<f64>]| -> Vec<Vec<f64>> {
        events
            .iter()
            .map(|x| {
                (0..n)
                    .map(|j| {
                        let mut s = 0.0;
                        for k in 0..n {
                            s += x[k] * m[k][j];
                        }
                        s
                    })
                    .collect()
            })
            .collect()
    };
    let values = apply(&inv);
    if condition >= NEAR_SINGULAR_COND {
        diagnostics.push(format!(
            "补偿矩阵近奇异：det={:.3e}，条件数={:.3e}；保留多个岭回归候选，需人工裁定"
        ));
        let scale = diag.iter().cloned().fold(0.0_f64, f64::max).abs().max(1e-12);
        let mut candidates = Vec::new();
        for (idx, &rel) in [1e-2_f64, 1e-3, 1e-4].iter().enumerate() {
            let ridge = rel * scale;
            let rinv = ridge_inverse(&matrix.rows, ridge);
            let cand_values = apply(&rinv);
            // 残差：M*M_ridge^-1 与单位阵的偏差
            let back = mat_mul(&matrix.rows, &rinv);
            let mut res = 0.0;
            for i in 0..n {
                for j in 0..n {
                    let want = if i == j { 1.0 } else { 0.0 };
                    res += (back[i][j] - want).powi(2);
                }
            }
            candidates.push(CompCand {
                id: format!("ridge-{}", idx),
                label: format!("岭回归 λ={:.0e}（相对 {:.0e}）", ridge, rel),
                values: cand_values,
                residual: res.sqrt(),
                ridge,
            });
        }
        return CompOutcome {
            status: "near_singular".into(),
            condition,
            determinant,
            values: Some(values),
            candidates,
            diagnostics,
        };
    }
    CompOutcome {
        status: "ok".into(),
        condition,
        determinant,
        values: Some(values),
        candidates: vec![],
        diagnostics,
    }
}

/// asinh 变换参数：y = asinh(x / cofactor) / ln(10)，坐标基准 "dimensionless/asinh:<id>"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformSpec {
    pub id: String,
    pub channel: String,
    pub cofactor: f64,
}

pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    (x / cofactor.max(1e-12)).asinh() / std::f64::consts::LN_10
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum BoundaryRule {
    /// 边界点默认算入门内（闭合多边形，与流式软件默认一致）
    EdgeInclusive,
    /// 边界点算在门外
    EdgeExclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub id: String,
    pub lineage_id: String,
    pub name: String,
    /// 坐标基准固定为门草案中钉住的 transform 版本，顶点以变换后无量纲坐标表示
    pub basis: String,
    pub x_axis: AxisRef,
    pub y_axis: AxisRef,
    pub vertices: Vec<[f64; 2]>,
    #[serde(default)]
    pub boundary: Option<BoundaryRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisRef {
    pub channel: String,
    pub transform_id: String,
    pub transform_mvid: String,
}

/// 射线法点在多边形内判定。
pub fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let [x, y] = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        let intersect = (yi > y) != (yj > y)
            && x < (xj - xi) * (y - yi) / (yj - yi).max(1e-300) + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 边界距离（到任意边的最小归一化距离），用于标记边界点。
pub fn distance_to_edges(p: [f64; 2], poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut best = f64::MAX;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[j], poly[i]);
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let len2 = dx * dx + dy * dy;
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2.max(1e-300))
            .clamp(0.0, 1.0);
        let qx = a[0] + t * dx;
        let qy = a[1] + t * dy;
        let d = ((p[0] - qx).powi(2) + (p[1] - qy).powi(2)).sqrt();
        if d < best {
            best = d;
        }
        j = i;
    }
    best
}

/// 门归属结果；boundary 为边界点 id（由调用方按公差识别）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GateMembership {
    pub inside: Vec<String>,
    pub boundary: Vec<String>,
}

pub const BOUNDARY_TOL: f64 = 1e-7;
