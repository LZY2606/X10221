//! 补偿矩阵求逆，带近奇异守卫：绝不允许默默求逆。
//! 触发守卫时返回全部合法候选（Tikhonov 正则化 / 恒等跳过），由人工裁定。

/// 求逆守卫报告：判定依据（行列式、条件数估计、阈值）会写入机器可读事件。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GuardReport {
    pub determinant: f64,
    pub condition_estimate: f64,
    pub det_threshold: f64,
    pub cond_threshold: f64,
    pub near_singular: bool,
}

pub const DET_THRESHOLD: f64 = 1e-12;
pub const COND_THRESHOLD: f64 = 1e8;

fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// Gauss-Jordan 部分主元求逆；同时给出守卫报告。
/// 近奇异（|det| 过小或条件数过大）时返回 Err(report)，调用方不得继续默默使用结果。
pub fn guarded_inverse(m: &[Vec<f64>]) -> Result<(Vec<Vec<f64>>, GuardReport), GuardReport> {
    let n = m.len();
    let report = |det: f64, cond: f64| GuardReport {
        determinant: det,
        condition_estimate: cond,
        det_threshold: DET_THRESHOLD,
        cond_threshold: COND_THRESHOLD,
        near_singular: !det.is_finite() || det.abs() < DET_THRESHOLD || cond > COND_THRESHOLD,
    };
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(report(f64::NAN, f64::INFINITY));
    }
    // 增广 [M | I]
    let mut aug: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            r
        })
        .collect();
    let mut det_sign = 1.0f64;
    let mut det_abs_log = 0.0f64;
    let mut min_pivot = f64::INFINITY;
    let mut max_pivot = 0.0f64;
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if aug[r][col].abs() > aug[piv][col].abs() {
                piv = r;
            }
        }
        let pivot_val = aug[piv][col];
        if pivot_val.abs() < 1e-300 {
            // 精确奇异：行列式为 0。
            return Err(report(0.0, f64::INFINITY));
        }
        if piv != col {
            aug.swap(piv, col);
            det_sign = -det_sign;
        }
        min_pivot = min_pivot.min(pivot_val.abs());
        max_pivot = max_pivot.max(pivot_val.abs());
        det_abs_log += pivot_val.abs().ln();
        let inv_piv = 1.0 / pivot_val;
        for j in 0..(2 * n) {
            aug[col][j] *= inv_piv;
        }
        for r in 0..n {
            if r != col {
                let factor = aug[r][col];
                if factor != 0.0 {
                    for j in 0..(2 * n) {
                        aug[r][j] -= factor * aug[col][j];
                    }
                }
            }
        }
    }
    let inv: Vec<Vec<f64>> = aug.into_iter().map(|row| row[n..].to_vec()).collect();
    let det = det_sign * det_abs_log.exp();
    let cond = inf_norm(m) * inf_norm(&inv);
    let rep = report(det, cond);
    if rep.near_singular {
        Err(rep)
    } else {
        Ok((inv, rep))
    }
}

/// 近奇异时的合法候选之一：Tikhonov 正则化逆 (S + λI)^-1。
pub fn tikhonov_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let regularized: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .enumerate()
                .map(|(j, v)| if i == j { v + lambda } else { *v })
                .collect()
        })
        .collect();
    // 正则化后通常远离奇异；若仍触发守卫则放弃该候选。
    guarded_inverse(&regularized).ok().map(|(inv, _)| inv)
}

/// 矩阵乘向量：comp = M * raw。
pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_inverse() {
        let m = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let (inv, rep) = guarded_inverse(&m).expect("identity must invert");
        assert!(!rep.near_singular);
        assert!((inv[0][0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn upper_triangular_exact() {
        // S = [[1, 0.5], [0, 1]] → inv = [[1, -0.5], [0, 1]]，二进制下精确。
        let m = vec![vec![1.0, 0.5], vec![0.0, 1.0]];
        let (inv, rep) = guarded_inverse(&m).unwrap();
        assert!(!rep.near_singular);
        assert_eq!(inv[0][1], -0.5);
        let comp = mat_vec(&inv, &[1000.0, 50.0]);
        assert_eq!(comp, vec![975.0, 50.0]);
    }

    #[test]
    fn exactly_singular_rejected() {
        let m = vec![vec![1.0, 2.0], vec![0.5, 1.0]]; // det = 0
        let err = guarded_inverse(&m).unwrap_err();
        assert!(err.near_singular);
        assert_eq!(err.determinant, 0.0);
    }

    #[test]
    fn near_singular_rejected_but_tikhonov_available() {
        let m = vec![vec![1.0, 0.999999], vec![0.999998, 1.0]];
        let err = guarded_inverse(&m).unwrap_err();
        assert!(err.near_singular);
        assert!(tikhonov_inverse(&m, 1e-6).is_some());
    }
}
