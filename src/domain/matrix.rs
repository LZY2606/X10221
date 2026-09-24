//! 方阵：高斯-约旦求逆、条件数估计、脊正则化候选。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Matrix {
    pub labels: Vec<String>,
    pub rows: Vec<Vec<f64>>,
}

/// 近奇异判定的公开阈值：条件数估计超过它即不允许默默求逆。
pub const NEAR_SINGULAR_CONDITION: f64 = 1.0e8;

impl Matrix {
    pub fn n(&self) -> usize {
        self.labels.len()
    }

    pub fn validate(&self) -> Result<(), String> {
        let n = self.n();
        if n == 0 {
            return Err("矩阵为空".to_string());
        }
        if self.rows.len() != n || self.rows.iter().any(|r| r.len() != n) {
            return Err("矩阵不是与标签一致的方阵".to_string());
        }
        if self.rows.iter().flatten().any(|v| !v.is_finite()) {
            return Err("矩阵含非有限元素".to_string());
        }
        Ok(())
    }

    /// 条件数的廉价估计：行范数和最大者 / 高斯主元最小相对幅度。
    pub fn condition_estimate(&self) -> f64 {
        let n = self.n();
        let mut a = self.rows.clone();
        let mut max_pivot = 0.0f64;
        let mut min_pivot = f64::MAX;
        for col in 0..n {
            let pivot = (col..n)
                .map(|r| a[r][col].abs())
                .fold(0.0f64, f64::max);
            if pivot <= f64::EPSILON {
                return f64::INFINITY;
            }
            max_pivot = max_pivot.max(pivot);
            min_pivot = min_pivot.min(pivot);
            let pr = (col..n).max_by(|x, y| a[*x][col].abs().total_cmp(&a[*y][col].abs())).unwrap();
            if pr != col {
                a.swap(pr, col);
            }
            let pv = a[col][col];
            for r in (col + 1)..n {
                let factor = a[r][col] / pv;
                for c in col..n {
                    a[r][c] -= factor * a[col][c];
                }
            }
        }
        if !min_pivot.is_finite() || min_pivot == 0.0 {
            f64::INFINITY
        } else {
            max_pivot / min_pivot
        }
    }

    /// 精确求逆（高斯-约旦全选列归一），奇异返回 Err。
    pub fn inverse(&self) -> Result<Matrix, String> {
        self.validate()?;
        let n = self.n();
        let mut a = vec![0.0f64; n * n * 2];
        for r in 0..n {
            for c in 0..n {
                a[r * 2 * n + c] = self.rows[r][c];
                a[r * 2 * n + n + c] = if r == c { 1.0 } else { 0.0 };
            }
        }
        for col in 0..n {
            let pivot_row = (col..n)
                .max_by(|x, y| a[x * 2 * n + col].abs().total_cmp(&a[y * 2 * n + col].abs()))
                .unwrap();
            let pv = a[pivot_row * 2 * n + col];
            if pv.abs() <= 1e-14 {
                return Err(format!("矩阵在第 {} 列奇异，无法求逆", col));
            }
            if pivot_row != col {
                for c in 0..(2 * n) {
                    a.swap(col * 2 * n + c, pivot_row * 2 * n + c);
                }
            }
            let pv = a[col * 2 * n + col];
            for c in 0..(2 * n) {
                a[col * 2 * n + c] /= pv;
            }
            for r in 0..n {
                if r == col {
                    continue;
                }
                let factor = a[r * 2 * n + col];
                if factor == 0.0 {
                    continue;
                }
                for c in 0..(2 * n) {
                    a[r * 2 * n + c] -= factor * a[col * 2 * n + c];
                }
            }
        }
        let rows = (0..n)
            .map(|r| (0..n).map(|c| a[r * 2 * n + n + c]).collect())
            .collect();
        Ok(Matrix { labels: self.labels.clone(), rows })
    }

    /// 脊正则化“解”：(S^T S + λI)^-1 S^T，用于近奇异时保留多个合法候选。
    pub fn ridge_solution(&self, lambda: f64) -> Matrix {
        let n = self.n();
        let st = transpose(self);
        let sts = multiply(&st, self);
        let reg = Matrix {
            labels: self.labels.clone(),
            rows: sts
                .rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    row.iter()
                        .enumerate()
                        .map(|(j, v)| v + if i == j { lambda } else { 0.0 })
                        .collect()
                })
                .collect(),
        };
        let inv = reg.inverse().expect("ridge 矩阵正定必可逆");
        multiply(&inv, &st)
    }

    pub fn apply(&self, v: &[f64]) -> Vec<f64> {
        self.rows
            .iter()
            .map(|row| row.iter().zip(v).map(|(a, x)| a * x).sum())
            .collect()
    }

    /// 残差 ||S * C * x - x|| 的最大相对幅度，用单位基向量估计。
    pub fn roundtrip_residual(&self, comp: &Matrix) -> f64 {
        let prod = multiply(self, comp);
        let n = self.n();
        let mut worst = 0.0f64;
        for i in 0..n {
            let mut x = vec![0.0; n];
            x[i] = 1.0;
            let back = prod.apply(&x);
            let err = back
                .iter()
                .zip(&x)
                .map(|(b, a)| (b - a).abs())
                .fold(0.0f64, f64::max);
            worst = worst.max(err);
        }
        worst
    }
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let n = a.n();
    let rows = (0..n)
        .map(|r| {
            (0..n)
                .map(|c| (0..n).map(|k| a.rows[r][k] * b.rows[k][c]).sum())
                .collect()
        })
        .collect();
    Matrix { labels: b.labels.clone(), rows }
}

pub fn transpose(a: &Matrix) -> Matrix {
    let n = a.n();
    Matrix {
        labels: a.labels.clone(),
        rows: (0..n).map(|r| (0..n).map(|c| a.rows[c][r]).collect()).collect(),
    }
}

/// 矩阵类别：近奇异矩阵不能被默默求逆。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatrixDiag {
    Invertible,
    NearSingular,
    Singular,
}

pub fn classify(m: &Matrix) -> MatrixDiag {
    if m.inverse().is_err() {
        MatrixDiag::Singular
    } else {
        let cond = m.condition_estimate();
        if !cond.is_finite() || cond > NEAR_SINGULAR_CONDITION {
            MatrixDiag::NearSingular
        } else {
            MatrixDiag::Invertible
        }
    }
}
