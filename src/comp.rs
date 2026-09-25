//! 补偿：spillover 元数据与补偿矩阵分开版本化。
//! 推导规则 CASE-COMP-01：近奇异补偿矩阵不能被默默求逆。
//!  - 严格奇异：判定 MATRIX_SINGULAR，不产生任何补偿后结果（错误即结果）。
//!  - 近奇异：保留全部合法候选（截断伪逆/岭估计），交人工裁定。
//!  - 良态：直接求逆，单候选。

use serde::{Deserialize, Serialize};

/// Spillover 元数据（单染对照的解释层），与矩阵正文分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    /// 通道顺序（探测器基准），矩阵按此顺序解释。
    pub channels: Vec<String>,
    /// 单染对照材料版本：channel -> blob/material 版本 id。
    pub single_stain_refs: Vec<SingleStainRef>,
    /// 采集平台给出的 spillover 词法表示（原始保留）。
    pub source_keyword: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SingleStainRef {
    pub channel: String,
    pub material_version_id: String,
    pub control_label: String,
}

/// 补偿矩阵正文 S（spillover，对角线为 1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    /// 行主序 n*n；补偿后 x_c = S^{-1} x_raw。
    pub rows: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompVerdict {
    /// 良态矩阵，唯一可逆解。
    Invertible,
    /// 近奇异：存在多个合法候选，禁止静默求逆。
    NearSingular,
    /// 严格奇异：无法补偿，禁止产出补偿后事件。
    Singular,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompCandidate {
    /// 候选身份（在该分析内稳定）：inverse / pinv_tau<r> / ridge_<lambda>
    pub key: String,
    pub label: String,
    /// 候选使用的逆矩阵 M（x_c = M x_raw），行主序。
    pub inverse: Vec<Vec<f64>>,
    /// 判定依据（机器可读）。
    pub rationale: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompAnalysis {
    pub verdict: CompVerdict,
    pub condition_number: f64,
    /// 机器可读判定依据事件负载（与 store 中 JUDGEMENT 事件呼应）。
    pub rule_findings: Vec<String>,
    /// 全部合法候选；严格奇异时为空。
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<String>,
}

/// 近奇异阈值（缩放矩阵条件数）。阈值以上进入多候选区。
pub const NEAR_SINGULAR_KAPPA: f64 = 30.0;

impl CompMatrix {
    pub fn dim(&self) -> usize {
        self.channels.len()
    }

    pub fn is_well_formed(&self) -> bool {
        let n = self.dim();
        self.rows.len() == n && self.rows.iter().all(|r| r.len() == n)
    }

    /// 分析矩阵：严格奇异/近奇异/可逆。
    pub fn analyze(&self) -> CompAnalysis {
        if !self.is_well_formed() {
            return CompAnalysis {
                verdict: CompVerdict::Singular,
                condition_number: f64::INFINITY,
                rule_findings: vec!["MATRIX_NOT_SQUARE".to_string()],
                candidates: vec![],
                diagnostics: vec!["matrix shape does not match channel list".to_string()],
            };
        }
        let n = self.dim();
        let (scaled, _scale) = scaled_copy(&self.rows);
        let inv = gauss_inverse(&scaled);
        match inv {
            Some(m) => {
                let kappa = condition_number(&scaled);
                if kappa >= NEAR_SINGULAR_KAPPA {
                    let pinv = pseudo_inverse_svd(&scaled, NEAR_SINGULAR_KAPPA);
                    let ridge = ridge_inverse(&scaled, 1e-6);
                    let candidates = vec![
                        CompCandidate {
                            key: "inverse".into(),
                            label: "条件数过高的直接逆（不稳定）".into(),
                            inverse: descale_inverse(&m, &self.rows),
                            rationale: vec![format!("KAPPA={:.3}>={NEAR_SINGULAR_KAPPA}", kappa)],
                        },
                        CompCandidate {
                            key: "pinv_truncated".into(),
                            label: "截断伪逆（小奇异值归零）".into(),
                            inverse: descale_inverse(&pinv, &self.rows),
                            rationale: vec![
                                "TRUNCATED_SVD".into(),
                                format!("KAPPA={:.3}", kappa),
                            ],
                        },
                        CompCandidate {
                            key: "ridge_1e-6".into(),
                            label: "岭估计 (S^T S + 1e-6 I)^-1 S^T".into(),
                            inverse: descale_inverse(&ridge, &self.rows),
                            rationale: vec!["RIDGE_LAMBDA=1e-6".into()],
                        },
                    ];
                    CompAnalysis {
                        verdict: CompVerdict::NearSingular,
                        condition_number: kappa,
                        rule_findings: vec![
                            "NEAR_SINGULAR_MATRIX_MULTI_CANDIDATE".to_string(),
                            "NO_SILENT_INVERSION".to_string(),
                        ],
                        candidates,
                        diagnostics: vec![],
                    }
                } else {
                    CompAnalysis {
                        verdict: CompVerdict::Invertible,
                        condition_number: kappa,
                        rule_findings: vec!["MATRIX_INVERTIBLE_UNIQUE".to_string()],
                        candidates: vec![CompCandidate {
                            key: "inverse".into(),
                            label: "唯一逆矩阵".into(),
                            inverse: descale_inverse(&m, &self.rows),
                            rationale: vec![format!("KAPPA={:.3}", kappa)],
                        }],
                        diagnostics: vec![],
                    }
                }
            }
            None => CompAnalysis {
                verdict: CompVerdict::Singular,
                condition_number: f64::INFINITY,
                rule_findings: vec![
                    "MATRIX_SINGULAR".to_string(),
                    "COMPENSATION_BLOCKED".to_string(),
                ],
                candidates: vec![],
                diagnostics: vec!["determinant below pivot floor".to_string()],
            },
        }
    }
}

const PIVOT_FLOOR: f64 = 1e-12;

/// 按对角线缩放：M = D S D，D_i = 1/max(1, |S_ii|)；对角线为 0 时不缩放。
fn scaled_copy(rows: &[Vec<f64>]) -> (Vec<Vec<f64>>, Vec<f64>) {
    let n = rows.len();
    let mut d = vec![1.0f64; n];
    for i in 0..n {
        let diag = rows[i][i].abs();
        if diag > 1.0 {
            d[i] = 1.0 / diag;
        }
    }
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            m[i][j] = rows[i][j] * d[i] * d[j];
        }
    }
    (m, d)
}

/// 缩放逆还原：S = D^-1 M D^-1 => S^-1 = D M^-1 D。
fn descale_inverse(minv_scaled: &[Vec<f64>], original: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = original.len();
    let mut d = vec![1.0f64; n];
    for i in 0..n {
        let diag = original[i][i].abs();
        if diag > 1.0 {
            d[i] = 1.0 / diag;
        }
    }
    let mut out = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            out[i][j] = minv_scaled[i][j] * d[i] * d[j];
        }
    }
    out
}

/// 带部分主元的高斯求逆；主元过小视为严格奇异。
fn gauss_inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a = vec![vec![0.0; 2 * n]; n];
    for i in 0..n {
        a[i][..n].copy_from_slice(&m[i]);
        a[i][n + i] = 1.0;
    }
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if a[pivot][col].abs() < PIVOT_FLOOR {
            return None;
        }
        a.swap(pivot, col);
        let pv = a[col][col];
        for j in 0..2 * n {
            a[col][j] /= pv;
        }
        for r in 0..n {
            if r != col {
                let factor = a[r][col];
                for j in 0..2 * n {
                    a[r][j] -= factor * a[col][j];
                }
            }
        }
    }
    let mut inv = vec![vec![0.0; n]; n];
    for i in 0..n {
        inv[i].copy_from_slice(&a[i][n..]);
    }
    Some(inv)
}

/// 条件数估计：用 S^T S 的特征值比（对称 Jacobi 分解）。
fn condition_number(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let ata = matmul(&transpose(m), m);
    let (vals, _) = jacobi_eigen(&ata);
    let mut max_v = 0.0f64;
    let mut min_v = f64::INFINITY;
    for v in vals {
        let mag = v.abs();
        if mag > PIVOT_FLOOR {
            max_v = max_v.max(mag);
            min_v = min_v.min(mag);
        }
    }
    if !min_v.is_finite() || min_v <= PIVOT_FLOOR {
        f64::INFINITY
    } else {
        (max_v / min_v).sqrt()
    }
}

/// 截断伪逆：S+ = (S^T S)+ S^T；小特征值（条件数超阈的方向）归零。
fn pseudo_inverse_svd(m: &[Vec<f64>], kappa_floor: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let ata = matmul(&transpose(m), m);
    let (vals, vecs) = jacobi_eigen(&ata);
    let mut max_v = 0.0f64;
    for v in &vals {
        max_v = max_v.max(v.abs());
    }
    let cutoff = max_v / (kappa_floor * kappa_floor);
    // pinv(S^T S) = V diag(1/lambda 或 0) V^T
    let mut pinv_ata = vec![vec![0.0; n]; n];
    for k in 0..n {
        let lam = vals[k];
        let recip = if lam.abs() > cutoff.abs().max(PIVOT_FLOOR) { 1.0 / lam } else { 0.0 };
        for i in 0..n {
            for j in 0..n {
                pinv_ata[i][j] += recip * vecs[i][k] * vecs[j][k];
            }
        }
    }
    matmul(&pinv_ata, &transpose(m))
}

/// 岭估计 (S^T S + lambda I)^-1 S^T。
fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let mut ata = matmul(&transpose(m), m);
    for i in 0..n {
        ata[i][i] += lambda;
    }
    let inv = gauss_inverse(&ata).expect("regularized matrix invertible");
    matmul(&inv, &transpose(m))
}

fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b[0].len();
    debug_assert_eq!(a[0].len(), k);
    let mut out = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            out[i][j] = s;
        }
    }
    out
}

fn transpose(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = m.len();
    let k = m[0].len();
    let mut out = vec![vec![0.0; n]; k];
    for i in 0..n {
        for j in 0..k {
            out[j][i] = m[i][j];
        }
    }
    out
}

/// 实对称矩阵的循环 Jacobi 特征分解，返回 (特征值, 列特征向量)。
fn jacobi_eigen(a: &[Vec<f64>]) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut m = a.to_vec();
    let mut v = vec![vec![0.0; n]; n];
    for i in 0..n {
        v[i][i] = 1.0;
    }
    for _ in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                off += m[p][q] * m[p][q];
            }
        }
        if off < 1e-24 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                if m[p][q].abs() < 1e-15 {
                    continue;
                }
                let tau = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                let mpp = m[p][p];
                let mqq = m[q][q];
                let mpq = m[p][q];
                m[p][p] = mpp - t * mpq;
                m[q][q] = mqq + t * mpq;
                m[p][q] = 0.0;
                m[q][p] = 0.0;
                for i in 0..n {
                    if i != p && i != q {
                        let mip = m[i][p];
                        let miq = m[i][q];
                        m[i][p] = c * mip - s * miq;
                        m[p][i] = m[i][p];
                        m[i][q] = s * mip + c * miq;
                        m[q][i] = m[i][q];
                    }
                }
                for i in 0..n {
                    let vip = v[i][p];
                    let viq = v[i][q];
                    v[i][p] = c * vip - s * viq;
                    v[i][q] = s * vip + c * viq;
                }
            }
        }
    }
    let vals = (0..n).map(|i| m[i][i]).collect();
    (vals, v)
}

/// 对一行原始信号做补偿；任一原始量不可转换（溢出等）时整行不产出数值，
/// 诊断随事件保留，不能为画图丢掉。
pub fn compensate_row(inverse: &[Vec<f64>], raw: &[Option<f64>]) -> Option<Vec<f64>> {
    if raw.iter().any(|v| v.is_none()) || raw.len() != inverse.len() {
        return None;
    }
    let x: Vec<f64> = raw.iter().map(|v| v.unwrap()).collect();
    let n = inverse.len();
    Some((0..n).map(|i| (0..n).map(|j| inverse[i][j] * x[j]).sum()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_like_matrix_is_invertible_single_candidate() {
        let m = CompMatrix {
            channels: vec!["FITC".into(), "PE".into()],
            rows: vec![vec![1.0, 0.02], vec![0.03, 1.0]],
        };
        let a = m.analyze();
        assert_eq!(a.verdict, CompVerdict::Invertible);
        assert_eq!(a.candidates.len(), 1);
        let inv = &a.candidates[0].inverse;
        let comp = compensate_row(inv, &[Some(100.0), Some(50.0)]).unwrap();
        assert!((comp[0] - 99.0).abs() < 1e-6);
    }

    #[test]
    fn strictly_singular_matrix_is_blocked() {
        let m = CompMatrix {
            channels: vec!["A".into(), "B".into()],
            rows: vec![vec![1.0, 0.5], vec![2.0, 1.0]],
        };
        let a = m.analyze();
        assert_eq!(a.verdict, CompVerdict::Singular);
        assert!(a.candidates.is_empty());
        assert!(a.rule_findings.iter().any(|r| r == "MATRIX_SINGULAR"));
    }

    #[test]
    fn near_singular_keeps_all_legal_candidates() {
        // 第二行几乎是第一行的两倍：条件数远超阈值。
        let m = CompMatrix {
            channels: vec!["A".into(), "B".into()],
            rows: vec![vec![1.0, 0.5], vec![1.999999, 1.0]],
        };
        let a = m.analyze();
        assert_eq!(a.verdict, CompVerdict::NearSingular);
        assert_eq!(a.candidates.len(), 3);
        assert!(a.rule_findings.iter().any(|r| r == "NO_SILENT_INVERSION"));
        // 截断伪逆在近共线方向上给出更小的系数
        let pinv = &a.candidates[1].inverse;
        let ridge = &a.candidates[2].inverse;
        assert!(pinv.iter().flatten().zip(ridge.iter().flatten()).any(|(p, r)| (p - r).abs() > 1e-9));
    }
}
