//! 最小线性代数：部分主元高斯消去、可逆性报告、显式逆、岭回归近似。
//!
//! 关键安全规则：近奇异矩阵**绝不**被默默求逆。`inverse` 对近奇异 / 奇异矩阵返回错误，
//! 调用方必须改走显式保留的多候选（岭回归）并由人工裁定。

use crate::domain::InvertibilityReport;

/// 近奇异判定阈值：最小主元比例低于该值，或条件数高于该值。
pub const PIVOT_THRESHOLD: f64 = 1e-6;
pub const CONDITION_THRESHOLD: f64 = 1e8;

#[derive(Debug, Clone, PartialEq)]
pub struct MatrixError {
    pub report: InvertibilityReport,
}

impl std::fmt::Display for MatrixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "compensation matrix not safely invertible: {}", self.report.reason)
    }
}
impl std::error::Error for MatrixError {}

fn matrix_inf_norm(a: &[Vec<f64>]) -> f64 {
    a.iter()
        .map(|r| r.iter().map(|x| x.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// 带部分主元的 LU 分解，返回置换、LU、最小主元比例、是否奇异。
fn lu_decompose(a: &[Vec<f64>]) -> (Vec<usize>, Vec<Vec<f64>>, f64, bool) {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut perm: Vec<usize> = (0..n).collect();
    let mut min_ratio = f64::INFINITY;
    let mut singular = false;

    for k in 0..n {
        let mut pivot_row = k;
        let mut best = lu[k][k].abs();
        for i in (k + 1)..n {
            if lu[i][k].abs() > best {
                best = lu[i][k].abs();
                pivot_row = i;
            }
        }
        let col_max = (k..n).map(|i| lu[i][k].abs()).fold(0.0, f64::max);
        if col_max == 0.0 {
            singular = true;
            min_ratio = 0.0;
            break;
        }
        let ratio = best / col_max;
        if ratio < min_ratio {
            min_ratio = ratio;
        }
        if pivot_row != k {
            lu.swap(k, pivot_row);
            perm.swap(k, pivot_row);
        }
        let pivot = lu[k][k];
        if pivot.abs() < PIVOT_THRESHOLD * col_max.max(1.0) {
            singular = true;
        }
        for i in (k + 1)..n {
            let factor = lu[i][k] / pivot;
            lu[i][k] = factor;
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }
    (perm, lu, if min_ratio.is_finite() { min_ratio } else { 0.0 }, singular)
}

/// 评估可逆性。用 LU 估计条件数（简单的 inf-范数估计，足够支撑阈值判定）。
pub fn assess(a: &[Vec<f64>]) -> InvertibilityReport {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return InvertibilityReport {
            invertible: false,
            min_pivot_ratio: 0.0,
            condition_number: None,
            pivot_threshold: PIVOT_THRESHOLD,
            condition_threshold: CONDITION_THRESHOLD,
            near_singular: true,
            reason: "matrix is not square".to_string(),
        };
    }
    let (_, lu, min_ratio, singular) = lu_decompose(a);
    let mut cond: Option<f64> = None;
    let mut reason = String::new();

    if singular {
        reason = format!(
            "exactly or numerically singular during elimination (min pivot ratio {:.3e})",
            min_ratio
        );
    } else {
        // 条件数估计：原矩阵 inf 范数 × 显式逆的 inf 范数（逆由已通过安全阈值的 LU 求出，
        // 仅用于报告，不对外充当“默默求逆”的结果）。
        if let Ok(inv) = inverse_unchecked(a) {
            let c = matrix_inf_norm(a) * matrix_inf_norm(&inv);
            cond = Some(c);
        }
        let cond_bad = cond.map(|c| !c.is_finite() || c > CONDITION_THRESHOLD).unwrap_or(false);
        let pivot_bad = min_ratio < PIVOT_THRESHOLD;
        if pivot_bad || cond_bad {
            singular_near(&mut reason, min_ratio, cond);
        } else {
            reason = format!("well-conditioned (min pivot ratio {:.3e}, cond {:e})", min_ratio, cond.unwrap_or(0.0));
        }
    }
    let near = singular
        || min_ratio < PIVOT_THRESHOLD
        || cond.map(|c| !c.is_finite() || c > CONDITION_THRESHOLD).unwrap_or(false);

    InvertibilityReport {
        invertible: !near,
        min_pivot_ratio: min_ratio,
        condition_number: cond,
        pivot_threshold: PIVOT_THRESHOLD,
        condition_threshold: CONDITION_THRESHOLD,
        near_singular: near,
        reason,
    }
}

fn singular_near(reason: &mut String, min_ratio: f64, cond: Option<f64>) {
    *reason = format!(
        "near-singular spillover: min pivot ratio {:.3e} < {:.0e}, estimated cond {:?} > {:.0e}; inverse refused",
        min_ratio, PIVOT_THRESHOLD, cond, CONDITION_THRESHOLD
    );
}

fn solve_lu(perm: &[usize], lu: &[Vec<f64>], mut b: Vec<f64>) -> Vec<f64> {
    let n = lu.len();
    let pb: Vec<f64> = perm.iter().map(|&p| b[p]).collect();
    b = pb;
    // Ly = Pb
    for i in 0..n {
        let s: f64 = (0..i).map(|j| lu[i][j] * b[j]).sum();
        b[i] -= s;
    }
    // Ux = y
    for i in (0..n).rev() {
        let s: f64 = ((i + 1)..n).map(|j| lu[i][j] * b[j]).sum();
        b[i] = (b[i] - s) / lu[i][i];
    }
    b
}

/// 内部使用的求逆（不检查阈值），仅供条件数报告。
fn inverse_unchecked(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, MatrixError> {
    let n = a.len();
    let (perm, lu, _, singular) = lu_decompose(a);
    if singular {
        return Err(MatrixError { report: assess(a) });
    }
    let mut inv = vec![vec![0.0; n]; n];
    for col in 0..n {
        let mut e = vec![0.0; n];
        e[col] = 1.0;
        let x = solve_lu(&perm, &lu, e);
        for row in 0..n {
            inv[row][col] = x[row];
        }
    }
    Ok(inv)
}

/// 对外的安全求逆：近奇异 / 奇异一律拒绝。
pub fn inverse(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, MatrixError> {
    let report = assess(a);
    if !report.invertible {
        return Err(MatrixError { report });
    }
    inverse_unchecked(a).map_err(|e| MatrixError { report: e.report })
}

/// 岭回归近似：M = (SᵀS + λI)⁻¹ Sᵀ，对近奇异矩阵给出显式候选而非假装精确。
/// 内部对正则化后的良态矩阵使用 LU；若仍失败则报错。
pub fn ridge(a: &[Vec<f64>], lambda: f64) -> (Vec<Vec<f64>>, f64) {
    let n = a.len();
    let ata = |i: usize, j: usize| -> f64 {
        (0..n).map(|k| a[k][i] * a[k][j]).sum::<f64>() + if i == j { lambda } else { 0.0 }
    };
    let reg: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| ata(i, j)).collect()).collect();
    let (perm, lu, _, singular) = lu_decompose(&reg);
    assert!(!singular, "regularized matrix unexpectedly singular");
    // M = reg^{-1} A^T，逐列求解 reg * x = A^T 的列
    let mut m = vec![vec![0.0; n]; n];
    for col in 0..n {
        let rhs: Vec<f64> = (0..n).map(|row| a[col][row]).collect();
        let x = solve_lu(&perm, &lu, rhs);
        for row in 0..n {
            m[row][col] = x[row];
        }
    }
    // 残差 ||A M - I||_inf
    let mut resid = 0.0_f64;
    for i in 0..n {
        for j in 0..n {
            let v: f64 = (0..n).map(|k| a[i][k] * m[k][j]).sum::<f64>() - if i == j { 1.0 } else { 0.0 };
            resid = resid.max(v.abs());
        }
    }
    (m, resid)
}

/// 对一条荧光读数向量施加补偿矩阵：y = M x。
pub fn apply(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let s = vec![vec![1.0, 0.1], vec![0.05, 1.0]];
        let inv = inverse(&s).expect("should invert");
        let r = apply(&inv, &[1.0, 0.0]);
        assert!((r[0] - 1.0).abs() < 1e-9);
        assert!(r[1].abs() < 1e-9);
        let rep = assess(&s);
        assert!(rep.invertible && !rep.near_singular);
    }

    #[test]
    fn exactly_singular_is_refused() {
        let s = vec![vec![1.0, 0.5], vec![2.0, 1.0]];
        let err = inverse(&s).expect_err("singular must be refused");
        assert!(!err.report.invertible);
        assert!(err.report.near_singular);
    }

    #[test]
    fn near_singular_is_refused_but_ridge_gives_candidate() {
        // 两列几乎成比例：条件数远超阈值。
        let s = vec![vec![1.0, 1.00000001], vec![1.0, 1.0]];
        let err = inverse(&s).expect_err("near-singular must be refused, never silently inverted");
        assert!(err.report.near_singular);
        let (m, resid) = ridge(&s, 1e-2);
        assert!(resid.is_finite());
        assert_eq!(m.len(), 2);
        // 岭解不应表现得像精确逆（显式标注为近似候选）。
        assert!(resid > 0.0);
    }

    #[test]
    fn non_square_rejected() {
        let s = vec![vec![1.0, 0.0, 0.2]];
        assert!(inverse(&s).is_err());
    }
}
