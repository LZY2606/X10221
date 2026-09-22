//! 最小线性代数：3x3/通用小矩阵的求逆、1 范数条件数与候选逆矩阵。
//! 近奇异矩阵绝不默默求逆，调用方必须在 safe_inverse 返回 None 时保留全部候选。
use crate::model::NEAR_SINGULAR_RCOND;

pub fn det3(m: &[Vec<f64>]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

pub fn inv3(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let d = det3(m);
    if !d.is_finite() || d.abs() < f64::EPSILON {
        return None;
    }
    let mut inv = vec![vec![0.0; 3]; 3];
    inv[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d;
    inv[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d;
    inv[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d;
    inv[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d;
    inv[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d;
    inv[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d;
    inv[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d;
    inv[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d;
    inv[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d;
    Some(inv)
}

pub fn norm1(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    (0..n)
        .map(|j| (0..n).map(|i| m[i][j].abs()).sum::<f64>())
        .fold(0.0_f64, f64::max)
}

/// 1 范数条件数；奇异时返回 inf。
pub fn cond1(m: &[Vec<f64>], inv: Option<&[Vec<f64>]>) -> f64 {
    match inv {
        Some(im) => norm1(m) * norm1(im),
        None => f64::INFINITY,
    }
}

/// 可逆性诊断。
#[derive(Clone, Debug)]
pub struct InvertReport {
    pub determinant: f64,
    pub condition_1: f64,
    pub rcond_1: f64,
    pub classification: String, // invertible | near-singular | singular
    pub inverse: Option<Vec<Vec<f64>>>,
}

pub fn inspect(m: &[Vec<f64>]) -> InvertReport {
    let det = det3(m);
    let inv = inv3(m);
    let cond = cond1(m, inv.as_deref());
    let rcond = if cond.is_finite() && cond > 0.0 {
        1.0 / cond
    } else {
        0.0
    };
    let classification = if det.abs() < crate::model::SINGULAR_DET {
        "singular".to_string()
    } else if rcond < NEAR_SINGULAR_RCOND {
        "near-singular".to_string()
    } else {
        "invertible".to_string()
    };
    InvertReport {
        determinant: det,
        condition_1: cond,
        rcond_1: rcond,
        classification,
        inverse: inv,
    }
}

/// A*b
pub fn mat_vec(m: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    m.iter().map(|row| row.iter().zip(b).map(|(a, x)| a * x).sum()).collect()
}

/// Aᵀ A
fn ata(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut r = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            r[i][j] = (0..n).map(|k| a[k][i] * a[k][j]).sum();
        }
    }
    r
}

/// Aᵀ b
fn atb(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = a.len();
    (0..n)
        .map(|i| (0..n).map(|k| a[k][i] * b[k]).sum())
        .collect()
}

fn solve3(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let inv = inv3(a)?;
    Some(mat_vec(&inv, b))
}

/// Moore–Penrose 伪逆作用于 b（以 AᵀA 方程表达，显式标注“伪逆候选”）。
pub fn pinv_apply(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = a.len();
    let mut g = ata(a);
    for i in 0..n {
        g[i][i] += 1e-12;
    }
    solve3(&g, &atb(a, b))
}

/// Tikhonov 正则化逆：(AᵀA + λI)⁻¹Aᵀ b。
pub fn tikhonov_apply(a: &[Vec<f64>], b: &[f64], lambda: f64) -> Option<Vec<f64>> {
    let n = a.len();
    let mut g = ata(a);
    for i in 0..n {
        g[i][i] += lambda;
    }
    solve3(&g, &atb(a, b))
}

pub fn tikhonov_lambda(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    let diag = (0..n).fold(0.0_f64, |acc, i| acc.max(a[i][i].abs()));
    1e-6 * diag.max(1.0)
}
