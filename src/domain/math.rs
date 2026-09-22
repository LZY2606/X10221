//! 领域数值层：补偿矩阵、信号变换、多边形门。
//! 所有判定都返回结构化结果，绝不“默默求逆”。

use serde::{Deserialize, Serialize};

/// 近奇异阈值：条件数超过该值时禁止直接给出唯一逆矩阵。
pub const NEAR_SINGULAR_COND: f64 = 1.0e6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixVerdict {
    pub singular: bool,
    pub near_singular: bool,
    /// 1-范数条件数估计
    pub condition_number: Option<f64>,
    pub determinant: f64,
    /// 良态时的唯一逆
    pub inverse: Option<Vec<Vec<f64>>>,
    /// 近奇异时保留的全部合法候选；真奇异时为空
    pub candidates: Vec<Candidate>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    /// 候选稳定身份，如 c1-ridge
    pub candidate_id: String,
    pub label: String,
    pub inverse: Vec<Vec<f64>>,
    pub justification: String,
}

fn lu_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, f64)> {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut det_sign = 1.0f64;
    for col in 0..n {
        let mut best = col;
        for r in col + 1..n {
            if lu[r][col].abs() > lu[best][col].abs() {
                best = r;
            }
        }
        if lu[best][col].abs() < 1e-15 {
            return None;
        }
        if best != col {
            lu.swap(best, col);
            piv.swap(best, col);
            det_sign = -det_sign;
        }
        let pivot = lu[col][col];
        for r in col + 1..n {
            let factor = lu[r][col] / pivot;
            lu[r][col] = factor;
            for c in col + 1..n {
                lu[r][c] -= factor * lu[col][c];
            }
        }
    }
    Some((lu, piv, det_sign))
}

fn lu_inverse(lu: &[Vec<f64>], piv: &[usize]) -> Vec<Vec<f64>> {
    let n = lu.len();
    let mut inv = vec![vec![0.0f64; n]; n];
    for j in 0..n {
        let mut b = vec![0.0f64; n];
        b[j] = 1.0;
        let mut pb = vec![0.0f64; n];
        for i in 0..n {
            pb[i] = b[piv[i]];
        }
        // 前向代入 L y = P b
        let mut y = vec![0.0f64; n];
        for i in 0..n {
            let mut s = pb[i];
            for k in 0..i {
                s -= lu[i][k] * y[k];
            }
            y[i] = s;
        }
        // 回代 U x = y
        let mut x = vec![0.0f64; n];
        for i in (0..n).rev() {
            let mut s = y[i];
            for k in i + 1..n {
                s -= lu[i][k] * x[k];
            }
            x[i] = s / lu[i][i];
        }
        for i in 0..n {
            inv[i][j] = x[i];
        }
    }
    inv
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut c = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[i][k] * b[k][j];
            }
            c[i][j] = s;
        }
    }
    c
}

fn norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    (0..n)
        .map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// Tikhonov 正则化逆：(AᵀA + λ²I)⁻¹ Aᵀ
fn ridge_inverse(a: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut ata = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[k][i] * a[k][j];
            }
            ata[i][j] = s;
        }
    }
    for i in 0..n {
        ata[i][i] += lambda * lambda;
    }
    if let Some((lu, piv, _)) = lu_decompose(&ata) {
        let ata_inv = lu_inverse(&lu, &piv);
        let mut out = vec![vec![0.0f64; n]; n];
        for i in 0..n {
            for j in 0..n {
                let mut s = 0.0;
                for k in 0..n {
                    s += ata_inv[i][k] * a[j][k];
                }
                out[i][j] = s;
            }
        }
        return out;
    }
    // 极端兜底：单位阵（不会用于真奇异，因为候选只在近奇异时生成）
    let mut id = vec![vec![0.0; n]; n];
    for (i, row) in id.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    id
}

pub fn assess_matrix(spill: &[Vec<f64>]) -> MatrixVerdict {
    let n = spill.len();
    if n == 0 || spill.iter().any(|r| r.len() != n) {
        return MatrixVerdict {
            singular: true,
            near_singular: false,
            condition_number: None,
            determinant: 0.0,
            inverse: None,
            candidates: vec![],
            reason: "矩阵不是方阵".into(),
        };
    }
    let Some((lu, piv, sign)) = lu_decompose(spill) else {
        return MatrixVerdict {
            singular: true,
            near_singular: false,
            condition_number: None,
            determinant: 0.0,
            inverse: None,
            candidates: vec![],
            reason: "主元为零：矩阵精确奇异，无法求逆".into(),
        };
    };
    let mut det = sign;
    for i in 0..n {
        det *= lu[i][i];
    }
    let inv = lu_inverse(&lu, &piv);
    let cond = norm1(spill) * norm1(&inv);
    if !cond.is_finite() || cond >= NEAR_SINGULAR_COND {
        let l1 = 1e-3;
        let l2 = 1e-2;
        let c1 = Candidate {
            candidate_id: "c1-ridge-1e-3".into(),
            label: "脊回归正则化 λ=1e-3".into(),
            inverse: ridge_inverse(spill, l1),
            justification: "Tikhonov 正则 (AᵀA+λ²I)⁻¹Aᵀ，弱正则，保留尽量多原始结构".into(),
        };
        let c2 = Candidate {
            candidate_id: "c2-ridge-1e-2".into(),
            label: "脊回归正则化 λ=1e-2".into(),
            inverse: ridge_inverse(spill, l2),
            justification: "更强的脊正则化，数值稳定但偏差更大，供裁定比较".into(),
        };
        // 第三个候选：直接逆（标注数值风险），保留以暴露差异
        let c3 = Candidate {
            candidate_id: "c3-unstable-direct".into(),
            label: "未正则化直接逆（数值不稳定）".into(),
            inverse: inv.clone(),
            justification: "条件数超阈，直接逆会放大噪声；仅作为对照候选，不推荐".into(),
        };
        return MatrixVerdict {
            singular: false,
            near_singular: true,
            condition_number: Some(cond),
            determinant: det,
            inverse: None,
            candidates: vec![c1, c2, c3],
            reason: format!(
                "条件数 {cond:.3e} 超过阈值 {NEAR_SINGULAR_COND:.0e}：保留全部合法候选，等待人工裁定，不默默求逆"
            ),
        };
    }
    MatrixVerdict {
        singular: false,
        near_singular: false,
        condition_number: Some(cond),
        determinant: det,
        inverse: Some(inv),
        candidates: vec![],
        reason: format!("条件数 {cond:.3e}，良态，可直接补偿"),
    }
}

pub fn apply_compensation(row: &[f64], inverse: &[Vec<f64>]) -> Vec<f64> {
    let n = row.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += inverse[i][j] * row[j];
        }
        out[i] = s;
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum TransformSpec {
    /// 线性（含增益）：y = x * gain + offset
    Linear { gain: f64, offset: f64 },
    /// 双曲反正弦（FlowJo 风格 biexponential 的稳健替代）：
    /// y = asinh(x / cofactor)；坐标基准 = transformed detector unit
    Asinh { cofactor: f64 },
    /// 以 10 为底对数：x<=0 不可转换，保留原值并附诊断
    Log10 { decades: f64 },
}

impl TransformSpec {
    pub fn apply(&self, x: f64) -> Result<f64, String> {
        if !x.is_finite() {
            return Err("NON_FINITE".into());
        }
        match self {
            TransformSpec::Linear { gain, offset } => Ok(x * gain + offset),
            TransformSpec::Asinh { cofactor } => {
                if *cofactor <= 0.0 {
                    return Err("BAD_COFACTOR".into());
                }
                Ok((x / cofactor).asinh())
            }
            TransformSpec::Log10 { .. } => {
                if x <= 0.0 {
                    Err("NON_POSITIVE_FOR_LOG".into())
                } else {
                    Ok(x.log10())
                }
            }
        }
    }
    pub fn basis(&self) -> &'static str {
        match self {
            TransformSpec::Linear { .. } => "linear scaled detector unit",
            TransformSpec::Asinh { .. } => "asinh-transformed detector unit",
            TransformSpec::Log10 { .. } => "log10 decades",
        }
    }
    pub fn summary(&self) -> String {
        match self {
            TransformSpec::Linear { gain, offset } => format!("linear gain={gain} offset={offset}"),
            TransformSpec::Asinh { cofactor } => format!("asinh cofactor={cofactor}"),
            TransformSpec::Log10 { decades } => format!("log10 decades={decades}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 点在多边形内（含边界——边界点必须稳定归属，不能抖动）。
/// 使用绕数法判定内部，再用叉积/边参数判定是否恰在边上。
pub fn point_in_polygon(p: Point, poly: &[Point]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        // 边界判定（含顶点、边）
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        let dot = (p.x - a.x) * (p.x - b.x) + (p.y - a.y) * (p.y - b.y);
        let edge_len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        // 含 float32 FCS 存储量化（变换坐标上约 1e-4）：法向距离 < 1e-3 且投影在段内
        let on_segment = edge_len > 0.0 && cross.abs() / edge_len < 1e-3 && dot <= 1e-6;
        if on_segment {
            return true;
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x_cross = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
            if p.x < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}
