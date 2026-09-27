//! 补偿矩阵分析：近奇异矩阵不能被默默求逆。
//! 用 1-范数条件数估计 rcond = 1/cond1；rcond < NEAR_SINGULAR_RCOND 即判定近奇异。

use crate::domain::CompensationMatrix;
use serde::{Deserialize, Serialize};

/// 近奇异阈值（机器可读判定依据的一部分）
pub const NEAR_SINGULAR_RCOND: f64 = 1e-6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixAnalysis {
    pub invertible: bool,
    pub near_singular: bool,
    pub rcond_estimate: f64,
    pub det_sign: i8,
    pub diag_min: f64,
    pub reason: String,
}

impl CompensationMatrix {
    pub fn size(&self) -> usize {
        self.rows.len()
    }

    /// 部分主元 LU 分解；返回 (lu, perm, sign, singular)
    fn lu_decomp(a: &[Vec<f64>]) -> (Vec<Vec<f64>>, Vec<usize>, i8, bool) {
        let n = a.len();
        let mut lu = a.to_vec();
        let mut perm: Vec<usize> = (0..n).collect();
        let mut sign = 1i8;
        let mut singular = false;
        for k in 0..n {
            let mut pivot = k;
            let mut best = lu[k][k].abs();
            for i in (k + 1)..n {
                if lu[i][k].abs() > best {
                    best = lu[i][k].abs();
                    pivot = i;
                }
            }
            if best < 1e-15 {
                singular = true;
                continue;
            }
            if pivot != k {
                lu.swap(k, pivot);
                perm.swap(k, pivot);
                sign = -sign;
            }
            let pivot_val = lu[k][k];
            for i in (k + 1)..n {
                let f = lu[i][k] / pivot_val;
                lu[i][k] = f;
                for j in (k + 1)..n {
                    lu[i][j] -= f * lu[k][j];
                }
            }
        }
        (lu, perm, sign, singular)
    }

    /// 显式求逆。调用方必须先检查 analyze()；近奇异/奇异时返回 Err，绝不静默给结果。
    pub fn inverse(&self) -> Result<Vec<Vec<f64>>, String> {
        let a = self.analyze();
        if !a.invertible {
            return Err(format!("矩阵无法求逆：{}", a.reason));
        }
        if a.near_singular {
            return Err(format!(
                "近奇异补偿矩阵不能被默默求逆：rcond={:.3e} < {:.0e}（{}）",
                a.rcond_estimate, NEAR_SINGULAR_RCOND, a.reason
            ));
        }
        Ok(self.inverse_unchecked())
    }

    pub fn inverse_unchecked(&self) -> Vec<Vec<f64>> {
        let n = self.size();
        let (lu, perm, _, _) = Self::lu_decomp(&self.rows);
        let mut inv = vec![vec![0.0f64; n]; n];
        for col in 0..n {
            let mut b = vec![0.0f64; n];
            b[col] = 1.0;
            // 行交换前向代入 P A = L U
            let mut pb = vec![0.0f64; n];
            for i in 0..n {
                pb[i] = b[perm[i]];
            }
            // L y = Pb
            let mut y = vec![0.0f64; n];
            for i in 0..n {
                let mut s = pb[i];
                for j in 0..i {
                    s -= lu[i][j] * y[j];
                }
                y[i] = s;
            }
            // U x = y
            let mut x = vec![0.0f64; n];
            for i in (0..n).rev() {
                let mut s = y[i];
                for j in (i + 1)..n {
                    s -= lu[i][j] * x[j];
                }
                x[i] = if lu[i][i] == 0.0 { f64::NAN } else { s / lu[i][i] };
            }
            for i in 0..n {
                inv[i][col] = x[i];
            }
        }
        inv
    }

    /// 1-范数条件数估计（Higham 风格，两趟）。
    pub fn analyze(&self) -> MatrixAnalysis {
        let n = self.size();
        if n == 0 || self.rows.iter().any(|r| r.len() != n) {
            return MatrixAnalysis {
                invertible: false,
                near_singular: true,
                rcond_estimate: 0.0,
                det_sign: 0,
                diag_min: f64::NAN,
                reason: "矩阵维度不为方阵".into(),
            };
        }
        let norm_one = |m: &[Vec<f64>]| {
            (0..n)
                .map(|j| (0..n).map(|i| m[i][j].abs()).sum::<f64>())
                .fold(0.0f64, f64::max)
        };
        let a_norm = norm_one(&self.rows);
        let (lu, perm, sign, singular) = Self::lu_decomp(&self.rows);
        let diag_min = (0..n).map(|i| lu[i][i].abs()).fold(f64::INFINITY, f64::min);
        if singular || diag_min == 0.0 || !a_norm.is_finite() {
            return MatrixAnalysis {
                invertible: false,
                near_singular: true,
                rcond_estimate: 0.0,
                det_sign: 0,
                diag_min,
                reason: "LU 分解出现零主元（严格奇异）".into(),
            };
        }

        let solve = |rhs: &[f64]| -> Vec<f64> {
            let mut b = vec![0.0; n];
            for i in 0..n {
                b[i] = rhs[perm[i]];
            }
            let mut y = vec![0.0; n];
            for i in 0..n {
                let mut s = b[i];
                for j in 0..i {
                    s -= lu[i][j] * y[j];
                }
                y[i] = s;
            }
            let mut x = vec![0.0; n];
            for i in (0..n).rev() {
                let mut s = y[i];
                for j in (i + 1)..n {
                    s -= lu[i][j] * x[j];
                }
                x[i] = s / lu[i][i];
            }
            x
        };
        let solve_t = |rhs: &[f64]| -> Vec<f64> {
            // U^T z = rhs
            let mut z = vec![0.0; n];
            for i in 0..n {
                let mut s = rhs[i];
                for j in 0..i {
                    s -= lu[j][i] * z[j];
                }
                z[i] = s / lu[i][i];
            }
            // L^T w = z
            let mut w = vec![0.0; n];
            for i in (0..n).rev() {
                let mut s = z[i];
                for j in (i + 1)..n {
                    s -= lu[j][i] * w[j];
                }
                w[i] = s;
            }
            // 撤销置换
            let mut x = vec![0.0; n];
            for i in 0..n {
                x[perm[i]] = w[i];
            }
            x
        };

        // 条件数估计迭代
        let mut x = vec![1.0 / n as f64; n];
        x = solve(&x);
        let mut est = x.iter().map(|v| v.abs()).sum::<f64>();
        for _ in 0..4 {
            let xi: Vec<f64> = x.iter().map(|v| if *v >= 0.0 { 1.0 } else { -1.0 }).collect();
            let z = solve_t(&xi);
            let z_norm = z.iter().map(|v| v.abs()).sum::<f64>();
            if z_norm <= est {
                break;
            }
            est = z_norm;
            let jmax = (0..n).max_by(|a, b| z[*a].abs().partial_cmp(&z[*b].abs()).unwrap()).unwrap();
            let mut e = vec![0.0; n];
            e[jmax] = 1.0;
            x = solve(&e);
            let x_norm = x.iter().map(|v| v.abs()).sum::<f64>();
            if x_norm <= est {
                break;
            }
            est = x_norm;
        }
        let inv_norm = est;
        let cond = a_norm * inv_norm;
        let rcond = if cond > 0.0 && cond.is_finite() { 1.0 / cond } else { 0.0 };
        let near = rcond < NEAR_SINGULAR_RCOND;
        MatrixAnalysis {
            invertible: true,
            near_singular: near,
            rcond_estimate: rcond,
            det_sign: sign,
            diag_min,
            reason: if near {
                format!("条件数过大 cond1≈{:.3e}，rcond 低于阈值", cond)
            } else {
                format!("条件数良好 cond1≈{:.3e}", cond)
            },
        }
    }
}
