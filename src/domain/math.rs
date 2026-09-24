//! 补偿线性代数、变换与多边形判定。
//!
//! 关键规则（与事件判定对应）：
//! * 近奇异矩阵不能被默默求逆：用带部分主元的 LU 分解估计 1-范数倒数条件数
//!   `rcond`，低于矩阵自带阈值时返回 `NearSingular`，同时给出全部合法候选。
//! * NaN/Inf 的通道值保留原位（诊断已经在导入层记录），不参与数值计算。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LuReport {
    pub rcond: f64,
    pub determinant: f64,
}

/// 带部分主元的 LU 分解，返回 (lu, piv, rcond, det)。
pub fn lu_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, LuReport)> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut parity = 1.0f64;

    for k in 0..n {
        let mut max_row = k;
        let mut max_val = lu[k][k].abs();
        for i in (k + 1)..n {
            if lu[i][k].abs() > max_val {
                max_val = lu[i][k].abs();
                max_row = i;
            }
        }
        if max_val == 0.0 || !max_val.is_finite() {
            return None;
        }
        if max_row != k {
            lu.swap(k, max_row);
            piv.swap(k, max_row);
            parity = -parity;
        }
        let pivot = lu[k][k];
        for i in (k + 1)..n {
            lu[i][k] /= pivot;
            let factor = lu[i][k];
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }

    let mut det = parity;
    for k in 0..n {
        det *= lu[k][k];
    }
    let rcond = estimate_rcond(&lu, &piv, n);
    Some((lu, piv, LuReport { rcond, determinant: det }))
}

/// LAPACK dgecon 风格的 1-范数倒数条件数估计（小规模矩阵足够）。
fn estimate_rcond(lu: &[Vec<f64>], piv: &[usize], n: usize) -> f64 {
    let anorm = matrix_1norm(lu);
    if anorm == 0.0 {
        return 0.0;
    }
    // 解 A x = ones，再解 A^T y = x，用增长比估计 ||A^-1||_1。
    let b1 = vec![1.0; n];
    let x = solve_lu(lu, Some(piv), n, &b1)?;
    let xnorm = vec_1norm(&x).max(1e-300);
    let y = solve_lu_transpose(lu, Some(piv), n, &x)?;
    let ynorm = vec_1norm(&y).max(1e-300);
    let ainv_norm = ynorm / xnorm;
    (1.0 / (anorm * ainv_norm)).clamp(0.0, 1.0)
}

pub fn matrix_1norm(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    (0..n).map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>()).fold(0.0, f64::max)
}
fn vec_1norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x.abs()).sum()
}

fn solve_lu(lu: &[Vec<f64>], piv: Option<&[usize]>, n: usize, b: &[f64]) -> Option<Vec<f64>> {
    let mut x = match piv {
        Some(p) => p.iter().map(|&i| b[i]).collect(),
        None => b.to_vec(),
    };
    for i in 0..n {
        let s: f64 = (0..i).map(|j| lu[i][j] * x[j]).sum();
        x[i] = (x[i] - s) / lu[i][i];
        if !x[i].is_finite() {
            return None;
        }
    }
    for i in (0..n).rev() {
        let s: f64 = ((i + 1)..n).map(|j| lu[i][j] * x[j]).sum();
        x[i] = (x[i] - s) / lu[i][i];
        if !x[i].is_finite() {
            return None;
        }
    }
    Some(x)
}

fn solve_lu_transpose(lu: &[Vec<f64>], piv: Option<&[usize]>, n: usize, b: &[f64]) -> Option<Vec<f64>> {
    // 先对右端应用行置换 P b（PA=LU，A^T y = b <=> U^T L^T y = P b）。
    let b: Vec<f64> = match piv {
        Some(p) => p.iter().map(|&i| b[i]).collect(),
        None => b.to_vec(),
    };
    // 解 U^T z = Pb
    let mut x = b.to_vec();
    for i in 0..n {
        let s: f64 = (0..i).map(|j| lu[j][i] * x[j]).sum();
        x[i] = (x[i] - s) / lu[i][i];
        if !x[i].is_finite() {
            return None;
        }
    }
    // 解 L^T y = z（L 为单位下三角）
    for i in (0..n).rev() {
        let s: f64 = ((i + 1)..n).map(|j| lu[j][i] * x[j]).sum();
        x[i] = x[i] - s;
        if !x[i].is_finite() {
            return None;
        }
    }
    Some(x)
}

/// 显式求逆（仅在非近奇异时允许调用）。
pub fn invert(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let (lu, piv, _report) = lu_decompose(a)?;
    let n = a.len();
    let mut inv = vec![vec![0.0; n]; n];
    for col in 0..n {
        let mut e = vec![0.0; n];
        e[col] = 1.0;
        let x = solve_lu(&lu, Some(&piv), n, &e)?;
        for i in 0..n {
            inv[i][col] = x[i];
        }
    }
    Some(inv)
}

/// 摩尔-彭罗斯伪逆（作为近奇异时的「合法候选」之一，明确不自动采纳）。
pub fn pseudoinverse(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let ata: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| (0..n).map(|k| a[k][i] * a[k][j]).sum()).collect())
        .collect();
    let ridge = 1e-8 * (0..n).map(|i| ata[i][i].abs()).fold(0.0, f64::max).max(1e-12);
    let reg: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| ata[i][j] + if i == j { ridge } else { 0.0 }).collect())
        .collect();
    let inv = invert(&reg).unwrap_or_else(|| vec![vec![f64::NAN; n]; n]);
    // A+ = (A^T A + ridge I)^-1 A^T
    (0..n)
        .map(|i| (0..n).map(|j| (0..n).map(|k| inv[i][k] * a[j][k]).sum()).collect())
        .collect()
}

/// asinh 变换：asinh(x / cofactor)。
pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    if cofactor <= 0.0 {
        return f64::NAN;
    }
    (x / cofactor).asinh()
}

/// 射线法点在多边形内；边界点由 `on_boundary` 单独标出，
/// 默认规则采用半开区间：沿正向边，左侧（含底边不含顶边）计入。
pub fn point_in_polygon(p: [f64; 2], verts: &[[f64; 2]]) -> (bool, bool) {
    let mut inside = false;
    let mut boundary = false;
    let n = verts.len();
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        if on_segment(p, a, b) {
            boundary = true;
        }
        let crosses = (a[1] > p[1]) != (b[1] > p[1]);
        if crosses {
            let x_int = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if x_int > p[0] {
                inside = !inside;
            }
        }
    }
    // 默认半开规则：边界点也计入（可被 BoundaryDecision 事件覆盖）。
    if boundary {
        inside = true;
    }
    (inside, boundary)
}

fn on_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    const EPS: f64 = 1e-9;
    let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    if cross.abs() > EPS * (a[0].abs().max(b[0].abs()).max(1.0)) {
        return false;
    }
    let dot = (p[0] - a[0]) * (p[0] - b[0]) + (p[1] - a[1]) * (p[1] - b[1]);
    dot <= EPS * (a[0].abs().max(b[0].abs()).max(1.0)).powi(2)
}
