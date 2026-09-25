//! 补偿矩阵与变换：近奇异矩阵不得被默默求逆。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Matrix {
    pub matrix_id: String,
    pub version: i64,
    pub channels: Vec<String>,
    /// rows[i][j] 带量纲（ratio），basis 说明估计来源。
    pub rows: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform {
    pub transform_id: String,
    pub version: i64,
    pub channels: Vec<String>,
    pub method: String, // "asinh"
    pub cofactor: f64,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InversionOutcome {
    pub status: String, // "ok" | "near_singular" | "singular"
    pub cond_1: f64,
    pub min_pivot: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub reason: String,
}

fn norm1(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut best = 0.0;
    for j in 0..n {
        let s: f64 = (0..n).map(|i| m[i][j].abs()).sum();
        best = best.max(s);
    }
    best
}

/// Gauss-Jordan 部分主元求逆，同时记录最小主元与条件数。
/// 当条件数超过阈值时拒绝返回逆矩阵（调用方必须显式选择候选）。
pub fn invert(m: &[Vec<f64>], cond_threshold: f64) -> InversionOutcome {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return InversionOutcome {
            status: "singular".into(),
            cond_1: f64::INFINITY,
            min_pivot: 0.0,
            inverse: None,
            reason: "矩阵非方阵或为空".into(),
        };
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap()).unwrap();
        if a[piv][col].abs() < 1e-15 {
            return InversionOutcome {
                status: "singular".into(),
                cond_1: f64::INFINITY,
                min_pivot: 0.0,
                inverse: None,
                reason: format!("列 {} 主元为 0", col),
            };
        }
        if piv != col {
            a.swap(piv, col);
            inv.swap(piv, col);
        }
        let p = a[col][col];
        min_pivot = min_pivot.min(p.abs());
        for k in 0..n {
            a[col][k] /= p;
            inv[col][k] /= p;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let f = a[row][col];
            for k in 0..n {
                a[row][k] -= f * a[col][k];
                inv[row][k] -= f * inv[col][k];
            }
        }
    }
    let cond = norm1(m) * norm1(&inv);
    if cond > cond_threshold {
        InversionOutcome {
            status: "near_singular".into(),
            cond_1: cond,
            min_pivot,
            inverse: None,
            reason: format!("1-范数条件数 {:.3e} 超过阈值 {:.0e}，拒绝默默求逆", cond, cond_threshold),
        }
    } else {
        InversionOutcome { status: "ok".into(), cond_1: cond, min_pivot, inverse: Some(inv), reason: "求逆成功".into() }
    }
}

/// 用 spillover 矩阵补偿一行信号；inverse 来自被显式接受的矩阵版本。
pub fn compensate_row(row: &[f64], inv: &[Vec<f64>], ch: &[usize]) -> Vec<f64> {
    let mut out = row.to_vec();
    let k = ch.len();
    for i in 0..k {
        let mut acc = 0.0;
        for j in 0..k {
            acc += inv[i][j] * row[ch[j]];
        }
        out[ch[i]] = acc;
    }
    out
}

pub fn asinh_transform(v: f64, cofactor: f64) -> f64 {
    (v / cofactor).asinh()
}

/// 阻尼伪逆候选（Tikhonov 风格）：仅作为候选暴露，绝不自动采用。
pub fn damped_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    // (A^T A + λI)^{-1} A^T —— 通过对增广方阵求逆
    let mut ata = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                ata[i][j] += m[k][i] * m[k][j];
            }
        }
        ata[i][i] += lambda;
    }
    let inv = invert(&ata, f64::INFINITY).inverse?;
    let mut out = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                out[i][j] += inv[i][k] * m[j][k];
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned() {
        let m = vec![vec![1.0, 0.12], vec![0.02, 1.0]];
        let r = invert(&m, 1e12);
        assert_eq!(r.status, "ok");
        let inv = r.inverse.unwrap();
        let back = compensate_row(&[100.0, 200.0], &inv, &[0, 1]);
        // M * y = x 的逆运算应回到 ~x
        let y = back;
        let x0 = 1.0 * y[0] + 0.12 * y[1];
        let x1 = 0.02 * y[0] + 1.0 * y[1];
        assert!((x0 - 100.0).abs() < 1e-6);
        assert!((x1 - 200.0).abs() < 1e-6);
    }

    #[test]
    fn refuses_near_singular() {
        let t = 0.999_999_999_999_9;
        let m = vec![vec![1.0, t], vec![t, 1.0]];
        let r = invert(&m, 1e12);
        assert_eq!(r.status, "near_singular");
        assert!(r.inverse.is_none());
    }

    #[test]
    fn singular_detected() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let r = invert(&m, 1e12);
        assert_eq!(r.status, "singular");
    }

    #[test]
    fn asinh_basic() {
        assert_eq!(asinh_transform(0.0, 150.0), 0.0);
        assert!(asinh_transform(150.0, 150.0) > 0.0);
    }

    #[test]
    fn damped_candidate_exists() {
        let t = 0.999_999_999_999_9;
        let m = vec![vec![1.0, t], vec![t, 1.0]];
        assert!(damped_inverse(&m, 1e-6).is_some());
    }
}
