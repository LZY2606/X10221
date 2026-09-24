//! 补偿矩阵：spillover 矩阵 → 补偿矩阵（求逆）。
//!
//! 核心规则（机器可执行、界面可读）：
//! - 近奇异 spillover 矩阵**不能被默默求逆**。
//! - [`invert_spillover`] 返回 [`CompInversion`]，其中携带条件数倒数 `rcond` 与判定状态；
//!   当 `rcond < RCOND_REJECT` 时直接拒绝（`Rejected`，无逆矩阵）；
//!   当 `RCOND_REJECT <= rcond < RCOND_WARN` 时为 `NeedsDecision`，系统保留全部合法候选
//!   （`exact_inverse` / `ridge_regularized`），等待显式人工裁定，绝不默认选一个。

use serde::{Deserialize, Serialize};

/// 近奇异硬拒绝阈值（1-范数条件数倒数）。
pub const RCOND_REJECT: f64 = 1e-12;
/// 需要人工裁定阈值：低于该值时保留多候选而不是默默求逆。
pub const RCOND_WARN: f64 = 1e-4;
/// Tikhonov/ridge 正则化候选的默认 λ（相对单位矩阵尺度）。
pub const RIDGE_LAMBDA: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InversionStatus {
    /// 条件良好，直接求逆。
    Ok,
    /// 近奇异：保留全部合法候选，等待人工裁定。
    NeedsDecision,
    /// （近）奇异到数值上无法可靠求逆：拒绝，不产出补偿矩阵。
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LegalCandidate {
    /// 候选身份，例如 `exact_inverse`、`ridge_regularized`。
    pub kind: String,
    pub matrix: Vec<Vec<f64>>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompInversion {
    pub n: usize,
    pub rcond: f64,
    pub status: InversionStatus,
    /// Ok / NeedsDecision 时的精确逆；Rejected 时为 None。
    pub exact_inverse: Option<Vec<Vec<f64>>>,
    /// NeedsDecision 时额外保留的全部合法候选（含精确逆与正则化候选）。
    pub candidates: Vec<LegalCandidate>,
    pub diagnostics: Vec<String>,
}

/// 方阵（行主序）。
pub fn lu_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, f64)> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut sign = 1.0_f64;
    for col in 0..n {
        let mut max = lu[col][col].abs();
        let mut row = col;
        for i in (col + 1)..n {
            if lu[i][col].abs() > max {
                max = lu[i][col].abs();
                row = i;
            }
        }
        if max <= f64::EPSILON {
            return None;
        }
        if row != col {
            lu.swap(row, col);
            piv.swap(row, col);
            sign = -sign;
        }
        let pivot = lu[col][col];
        for i in (col + 1)..n {
            lu[i][col] /= pivot;
            let factor = lu[i][col];
            for j in (col + 1)..n {
                let v = lu[i][j] - factor * lu[col][j];
                lu[i][j] = v;
            }
        }
    }
    Some((lu, piv, sign))
}

fn solve_lu(lu: &[Vec<f64>], piv: &[usize], b: &[f64]) -> Vec<f64> {
    let n = lu.len();
    let mut x: Vec<f64> = (0..n).map(|i| b[piv[i]]).collect();
    // 前向代入 Ly = Pb
    for i in 0..n {
        for j in 0..i {
            x[i] -= lu[i][j] * x[j];
        }
    }
    // 回代 Ux = y
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu[i][j] * x[j];
        }
        x[i] /= lu[i][i];
    }
    x
}

fn invert_with_lu(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let (lu, piv, _) = lu_decompose(a)?;
    let n = a.len();
    let mut cols = Vec::with_capacity(n);
    for k in 0..n {
        let mut e = vec![0.0; n];
        e[k] = 1.0;
        cols.push(solve_lu(&lu, &piv, &e));
    }
    let mut inv = vec![vec![0.0; n]; n];
    for (k, col) in cols.iter().enumerate() {
        for i in 0..n {
            inv[i][k] = col[i];
        }
    }
    Some(inv)
}

fn norm_inf(a: &[Vec<f64>]) -> f64 {
    a.iter().map(|r| r.iter().map(|x| x.abs()).sum()).fold(0.0, f64::max)
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut c = vec![vec![0.0; n]; n];
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

fn mat_add(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    a.iter()
        .zip(b)
        .map(|(ra, rb)| ra.iter().zip(rb).map(|(x, y)| x + y).collect())
        .collect()
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

/// 估计 1-范数条件数倒数 rcond = 1/(||A|| * ||A^-1||)；不可逆时返回 0。
pub fn estimate_rcond(a: &[Vec<f64>]) -> f64 {
    let na = norm_inf(a);
    if na == 0.0 || !na.is_finite() {
        return 0.0;
    }
    match invert_with_lu(a) {
        Some(inv) => {
            let ni = norm_inf(&inv);
            if ni == 0.0 || !ni.is_finite() {
                0.0
            } else {
                (1.0 / (na * ni)).clamp(0.0, 1.0)
            }
        }
        None => 0.0,
    }
}

/// 对 spillover 矩阵求补偿逆矩阵，带近奇异判定与多候选保留。
pub fn invert_spillover(s: &[Vec<f64>]) -> CompInversion {
    let n = s.len();
    let mut diag = Vec::new();
    if n == 0 || s.iter().any(|r| r.len() != n) {
        return CompInversion {
            n,
            rcond: 0.0,
            status: InversionStatus::Rejected,
            exact_inverse: None,
            candidates: vec![],
            diagnostics: vec!["spillover 矩阵不是方阵".into()],
        };
    }
    for i in 0..n {
        diag.push(s[i][i]);
    }

    let rcond = estimate_rcond(s);
    let mut diagnostics = Vec::new();
    diagnostics.push(format!("rcond={rcond:.3e}"));

    if rcond < RCOND_REJECT {
        diagnostics.push("条件数超过硬阈值：拒绝默默求逆".into());
        return CompInversion {
            n,
            rcond,
            status: InversionStatus::Rejected,
            exact_inverse: None,
            candidates: vec![],
            diagnostics,
        };
    }

    let exact = match invert_with_lu(s) {
        Some(m) => m,
        None => {
            return CompInversion {
                n,
                rcond: 0.0,
                status: InversionStatus::Rejected,
                exact_inverse: None,
                candidates: vec![],
                diagnostics: vec!["LU 分解失败".into()],
            }
        }
    };

    let residual = mat_mul(s, &exact);
    let id = identity(n);
    let mut back_err = 0.0_f64;
    for i in 0..n {
        for j in 0..n {
            back_err = back_err.max((residual[i][j] - id[i][j]).abs());
        }
    }
    diagnostics.push(format!("S*S^-1 与单位阵最大偏差={back_err:.3e}"));

    let exact_candidate = LegalCandidate {
        kind: "exact_inverse".into(),
        matrix: exact.clone(),
        note: "对 spillover 矩阵直接求逆".into(),
    };

    if rcond < RCOND_WARN {
        // 近奇异但还能算出逆：保留全部合法候选，等待显式裁定。
        let ridge = mat_add(s, &scale(&identity(n), RIDGE_LAMBDA));
        let ridge_inv = invert_with_lu(&ridge);
        let mut candidates = vec![exact_candidate];
        if let Some(ri) = ridge_inv {
            candidates.push(LegalCandidate {
                kind: "ridge_regularized".into(),
                matrix: ri,
                note: format!("Tikhonov 正则 (S + {RIDGE_LAMBDA:.0e}·I)^-1"),
            });
        }
        diagnostics.push("近奇异：不默认选择，等待人工裁定候选".into());
        CompInversion {
            n,
            rcond,
            status: InversionStatus::NeedsDecision,
            exact_inverse: Some(exact),
            candidates,
            diagnostics,
        }
    } else {
        CompInversion {
            n,
            rcond,
            status: InversionStatus::Ok,
            exact_inverse: Some(exact),
            candidates: vec![exact_candidate],
            diagnostics,
        }
    }
}

fn scale(a: &[Vec<f64>], k: f64) -> Vec<Vec<f64>> {
    a.iter().map(|r| r.iter().map(|x| x * k).collect()).collect()
}

/// 用补偿矩阵乘事件向量：compensated = M · raw。
pub fn apply_compensation(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(x).map(|(a, v)| a * v).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_inverts_cleanly() {
        // 对角占优、条件良好的 spillover
        let s = vec![
            vec![1.0, 0.02, 0.01],
            vec![0.05, 1.0, 0.03],
            vec![0.02, 0.04, 1.0],
        ];
        let r = invert_spillover(&s);
        assert_eq!(r.status, InversionStatus::Ok);
        let inv = r.exact_inverse.unwrap();
        let back = mat_mul(&s, &inv);
        for i in 0..3 {
            for j in 0..3 {
                assert!((back[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn near_singular_keeps_all_candidates_without_silent_pick() {
        // 两行几乎相同 → 近奇异但尚未触及硬阈值
        let s = vec![
            vec![1.0, 0.02, 0.01],
            vec![0.999999, 0.0200001, 0.0100001],
            vec![0.02, 0.04, 1.0],
        ];
        let r = invert_spillover(&s);
        assert_eq!(r.status, InversionStatus::NeedsDecision);
        assert!(r.exact_inverse.is_some());
        let kinds: Vec<_> = r.candidates.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"exact_inverse"));
        assert!(kinds.contains(&"ridge_regularized"));
    }

    #[test]
    fn singular_is_rejected() {
        let s = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.5, 0.5, 0.5],
            vec![1.0, 0.0, 0.0],
        ];
        let r = invert_spillover(&s);
        assert_eq!(r.status, InversionStatus::Rejected);
        assert!(r.exact_inverse.is_none());
        assert!(r.candidates.is_empty());
    }

    #[test]
    fn apply_roundtrip() {
        let s = vec![vec![1.0, 0.1], vec![0.2, 1.0]];
        let r = invert_spillover(&s);
        let inv = r.exact_inverse.unwrap();
        let raw = vec![100.0, 200.0];
        let comp = apply_compensation(&inv, &raw);
        let again = apply_compensation(&s, &comp);
        for (a, b) in raw.iter().zip(&again) {
            assert!((a - b).abs() < 1e-7);
        }
    }
}
