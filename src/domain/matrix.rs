//! 补偿矩阵数值核心：带部分主元的 LU 分解、求逆、1-范数 rcond 估计，
//! 以及近奇异时保留的全部合法候选逆（Tikhonov 伪逆 / 岭正则逆）。

use crate::domain::RCOND_THRESHOLD;

/// 矩阵（行优先），元素为 f64。
pub type Mat = Vec<Vec<f64>>;

/// 矩阵乘向量。
pub fn mat_vec(a: &Mat, x: &[f64]) -> Vec<f64> {
    a.iter()
        .map(|row| row.iter().zip(x).map(|(a, x)| a * x).sum())
        .collect()
}

/// 矩阵乘矩阵。
pub fn mat_mul(a: &Mat, b: &Mat) -> Mat {
    let n = a.len();
    let m = b[0].len();
    let mut out = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut acc = 0.0;
            for k in 0..b.len() {
                acc += a[i][k] * b[k][j];
            }
            out[i][j] = acc;
        }
    }
    out
}

/// 转置。
pub fn transpose(a: &Mat) -> Mat {
    let n = a.len();
    let mut t = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            t[j][i] = a[i][j];
        }
    }
    t
}

/// 求单位矩阵。
pub fn identity(n: usize) -> Mat {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

/// 带部分主元的 LU 分解后解线性方程组 A x = b；主元过小时判定奇异，返回 None。
pub fn lu_solve(a: &Mat, b: &[f64]) -> Option<Vec<f64>> {
    let n = a.len();
    if a.iter().any(|r| r.len() != n) || b.len() != n || n == 0 {
        return None;
    }
    let mut lu = a.clone();
    let mut piv: Vec<usize> = (0..n).collect();

    for col in 0..n {
        let mut best = col;
        for row in (col + 1)..n {
            if lu[row][col].abs() > lu[best][col].abs() {
                best = row;
            }
        }
        if lu[best][col].abs() < 1e-15 {
            return None;
        }
        if best != col {
            lu.swap(best, col);
            piv.swap(best, col);
        }
        let pivot = lu[col][col];
        for row in (col + 1)..n {
            let factor = lu[row][col] / pivot;
            lu[row][col] = factor;
            for c in (col + 1)..n {
                lu[row][c] -= factor * lu[col][c];
            }
        }
    }

    let mut x: Vec<f64> = b.iter().map(|_, i| b[piv[i]]).collect();
    for i in 0..n {
        for j in 0..i {
            x[i] -= lu[i][j] * x[j];
        }
    }
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu[i][j] * x[j];
        }
        x[i] /= lu[i][i];
    }
    Some(x)
}

/// 直接求逆（逐列解 A e_i = x）；奇异返回 None。
pub fn inverse(a: &Mat) -> Option<Mat> {
    let n = a.len();
    let mut inv = vec![vec![0.0; n]; n];
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        let col = lu_solve(a, &e)?;
        for i in 0..n {
            inv[i][j] = col[i];
        }
    }
    Some(inv)
}

/// 1-范数（最大列绝对值和）。
pub fn norm1(a: &Mat) -> f64 {
    let n = a.len();
    (0..n)
        .map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// 条件数倒数估计 rcond = 1 / (||A||_1 · ||A^-1||_1)；奇异返回 None。
pub fn rcond(a: &Mat) -> Option<f64> {
    let inv = inverse(a)?;
    let denom = norm1(a) * norm1(&inv);
    if denom == 0.0 {
        None
    } else {
        Some(1.0 / denom)
    }
}

/// 一条补偿候选逆。
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: &'static str,
    pub label: &'static str,
    pub inverse: Mat,
}

/// 补偿矩阵分析结论。
#[derive(Clone, Debug)]
pub struct CompAnalysis {
    /// `ok`（可直接求逆）/ `indeterminate`（近奇异，禁止默默求逆）/ `singular`（精确奇异）。
    pub status: &'static str,
    pub rcond: Option<f64>,
    /// 仅在 ok 时提供直接逆；近奇异 / 奇异时为 None。
    pub inverse: Option<Mat>,
    /// 近奇异时保留的全部合法候选逆，等待人工裁定。
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<String>,
}

/// Tikhonov 正则化伪逆：(AᵀA + λI)⁻¹ Aᵀ。
pub fn tikhonov_pinv(a: &Mat, lambda: f64) -> Option<Mat> {
    let at = transpose(a);
    let mut ata = mat_mul(&at, a);
    for i in 0..ata.len() {
        ata[i][i] += lambda;
    }
    let ata_inv = inverse(&ata)?;
    Some(mat_mul(&ata_inv, &at))
}

/// 岭正则直接逆：(A + λI)⁻¹。
pub fn ridge_inverse(a: &Mat, lambda: f64) -> Option<Mat> {
    let mut reg = a.clone();
    for i in 0..reg.len() {
        reg[i][i] += lambda;
    }
    inverse(&reg)
}

/// 对补偿矩阵做完整分析。
///
/// 关键规则：当 rcond 小于 [`RCOND_THRESHOLD`] 时，即使数值上仍能算出一个逆，
/// 也**不得**把它当作唯一结论默默使用；必须标记 indeterminate 并保留全部合法候选。
pub fn analyze_compensation(a: &Mat) -> CompAnalysis {
    match inverse(a) {
        None => CompAnalysis {
            status: "singular",
            rcond: None,
            inverse: None,
            candidates: vec![],
            diagnostics: vec![
                "补偿矩阵奇异：LU 主元为零，无法唯一求逆；事件补偿被拒绝（不静默清洗）。"
                    .to_string(),
            ],
        },
        Some(inv) => {
            let rc = rcond(a).expect("已成功求逆即应可估 rcond");
            if rc < RCOND_THRESHOLD {
                let candidates = vec![
                    Candidate {
                        id: "cand_tikhonov_1e_6",
                        label: "Tikhonov 伪逆 (AᵀA+1e-6·I)⁻¹Aᵀ",
                        inverse: tikhonov_pinv(a, 1e-6)
                            .expect("正则化矩阵应可逆"),
                    },
                    Candidate {
                        id: "cand_ridge_1e_3",
                        label: "岭正则逆 (A+1e-3·I)⁻¹",
                        inverse: ridge_inverse(a, 1e-3).expect("岭正则矩阵应可逆"),
                    },
                ];
                CompAnalysis {
                    status: "indeterminate",
                    rcond: Some(rc),
                    inverse: None,
                    candidates,
                    diagnostics: vec![format!(
                        "补偿矩阵近奇异：rcond={rc:.3e} < 阈值 {RCOND_THRESHOLD:.0e}；\
                         直接逆不被采纳，保留 {} 条合法候选等待人工裁定。",
                        2
                    )],
                }
            } else {
                let _ = inv;
                CompAnalysis {
                    status: "ok",
                    rcond: Some(rc),
                    inverse: inverse(a),
                    candidates: vec![],
                    diagnostics: vec![format!(
                        "补偿矩阵良态：rcond={rc:.3e} ≥ 阈值 {RCOND_THRESHOLD:.0e}，采用直接逆。"
                    )],
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let a = vec![
            vec![1.0, 0.15, 0.02],
            vec![0.03, 1.0, 0.10],
            vec![0.01, 0.05, 1.0],
        ];
        let an = analyze_compensation(&a);
        assert_eq!(an.status, "ok");
        let inv = an.inverse.unwrap();
        let product = mat_mul(&a, &inv);
        for i in 0..3 {
            for j in 0..3 {
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((product[i][j] - expect).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn exactly_singular_is_rejected() {
        let a = vec![
            vec![1.0, 0.4, 0.1],
            vec![0.3, 1.0, 0.2],
            vec![1.3, 1.4, 0.3], // 行三恰为行一加行二
        ];
        let an = analyze_compensation(&a);
        assert_eq!(an.status, "singular");
        assert!(an.inverse.is_none());
        assert!(an.candidates.is_empty());
    }

    #[test]
    fn near_singular_keeps_candidates_and_no_silent_inverse() {
        let a = vec![
            vec![1.0, 0.4, 0.1],
            vec![0.3, 1.0, 0.2],
            vec![1.3, 1.4, 0.300_000_001],
        ];
        let an = analyze_compensation(&a);
        assert_eq!(an.status, "indeterminate");
        assert!(an.inverse.is_none(), "近奇异不得默默给出唯一逆");
        assert_eq!(an.candidates.len(), 2);
    }
}
