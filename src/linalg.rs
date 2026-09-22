//! 近奇异/奇异补偿矩阵的显式判定。
//!
//! 规则（机器可读事件 `matrix_rejected` 会引用同一阈值）：
//! - 精确奇异：选主元后出现 0 主元；
//! - 近奇异：缩放主元比 `min |pivot| / col_scale <= NEAR_SINGULAR_EPS`。
//! 任一成立都不允许“默默求逆”：调用方只能保留诊断与候选矩阵，
//! 不能产出补偿结果。

use serde::{Deserialize, Serialize};

pub const NEAR_SINGULAR_EPS: f64 = 1e-9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixHealth {
    pub singular: bool,
    pub near_singular: bool,
    pub determinant: f64,
    /// 最小缩放主元（绝对值），越小越病态。
    pub min_scaled_pivot: f64,
    pub threshold: f64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LuSolution {
    pub health: MatrixHealth,
    pub x: Vec<f64>,
}

fn is_finite_matrix(matrix: &[Vec<f64>], n: usize) -> bool {
    matrix.len() == n && matrix.iter().all(|r| r.len() == n && r.iter().all(|v| v.is_finite()))
}

/// 带部分选主元的高斯消元，求解 `matrix * x = rhs`。
/// 矩阵奇异或近奇异时 `x` 为空，健康状态给出原因。
pub fn solve(matrix: &[Vec<f64>], rhs: &[f64]) -> LuSolution {
    let n = matrix.len();
    let health = diagnose(matrix);
    if health.singular || health.near_singular {
        return LuSolution { health, x: Vec::new() };
    }
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut x = rhs.to_vec();
    for k in 0..n {
        let mut pivot_row = k;
        let mut best = a[k][k].abs();
        for i in (k + 1)..n {
            if a[i][k].abs() > best {
                best = a[i][k].abs();
                pivot_row = i;
            }
        }
        if pivot_row != k {
            a.swap(k, pivot_row);
            x.swap(k, pivot_row);
        }
        for i in (k + 1)..n {
            let factor = a[i][k] / a[k][k];
            for j in k..n {
                a[i][j] -= factor * a[k][j];
            }
            x[i] -= factor * x[k];
        }
    }
    for i in (0..n).rev() {
        let mut acc = x[i];
        for j in (i + 1)..n {
            acc -= a[i][j] * x[j];
        }
        x[i] = acc / a[i][i];
    }
    LuSolution { health, x }
}

/// 判定矩阵健康度：非方阵/非有限值视为奇异，附带可读原因。
pub fn diagnose(matrix: &[Vec<f64>]) -> MatrixHealth {
    let n = matrix.len();
    let bad_shape = !is_finite_matrix(matrix, n);
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut det: f64 = 1.0;
    let mut min_scaled = f64::INFINITY;
    let mut singular = bad_shape;
    let mut reason: Option<String> = None;

    if bad_shape {
        reason = Some("matrix_not_square_or_finite".into());
    }

    if !singular {
        for k in 0..n {
            // 选主元前按行/列尺度归一，避免靠放大行“伪装”良态。
            let col_scale = (k..n).map(|i| a[i][k].abs()).fold(0.0_f64, f64::max).max(1.0);
            let mut pivot_row = k;
            let mut best = a[k][k].abs() / col_scale;
            for i in (k + 1)..n {
                let cand = a[i][k].abs() / col_scale;
                if cand > best {
                    best = cand;
                    pivot_row = i;
                }
            }
            if pivot_row != k {
                a.swap(k, pivot_row);
                det = -det;
            }
            let pivot = a[k][k];
            let scaled = pivot.abs() / col_scale;
            min_scaled = min_scaled.min(scaled);
            det *= pivot;
            if pivot == 0.0 || scaled < NEAR_SINGULAR_EPS {
                singular = true;
                reason = Some(if pivot == 0.0 {
                    "zero_pivot".into()
                } else {
                    "scaled_pivot_below_threshold".into()
                });
                break;
            }
            for i in (k + 1)..n {
                let factor = a[i][k] / pivot;
                for j in k..n {
                    a[i][j] -= factor * a[k][j];
                }
            }
        }
    }

    let near_singular = !singular && min_scaled <= NEAR_SINGULAR_EPS * 100.0;
    MatrixHealth {
        singular,
        near_singular,
        determinant: if singular { 0.0 } else { det },
        min_scaled_pivot: if min_scaled.is_infinite() { 0.0 } else { min_scaled },
        threshold: NEAR_SINGULAR_EPS,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_identity_and_known_spillover() {
        let id = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let sol = solve(&id, &[3.0, 5.0]);
        assert_eq!(sol.x, vec![3.0, 5.0]);

        let s = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let sol = solve(&s, &[1.2, 1.1]);
        assert!((sol.x[0] - 1.0).abs() < 1e-9);
        assert!((sol.x[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_exact_singular_matrix() {
        let s = vec![vec![1.0, 0.5], vec![2.0, 1.0]];
        let h = diagnose(&s);
        assert!(h.singular);
        let sol = solve(&s, &[1.0, 2.0]);
        assert!(sol.x.is_empty());
    }

    #[test]
    fn rejects_near_singular_matrix_without_inversion() {
        // 两行几乎线性相关：缩放主元低于阈值，禁止默默求逆。
        let eps = 1e-12;
        let s = vec![vec![1.0, 1.0], vec![1.0, 1.0 + eps]];
        let h = diagnose(&s);
        assert!(h.singular || h.near_singular, "health={h:?}");
        assert!(solve(&s, &[1.0, 1.0]).x.is_empty());
    }

    #[test]
    fn non_square_is_singular_diagnostic() {
        let s = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.0, 0.0]];
        assert!(diagnose(&s).singular);
    }
}
