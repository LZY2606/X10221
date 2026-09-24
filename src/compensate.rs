//! 补偿矩阵：近奇异矩阵不能被默默求逆。
//! 求逆前先做主元/行列式体检，失败时返回诊断与候选，绝不返回"差不多"的逆。

use serde::{Deserialize, Serialize};

pub const MIN_PIVOT_RATIO: f64 = 1e-8;
pub const MIN_DETERMINANT: f64 = 1e-12;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatrixCheck {
    pub ok: bool,
    pub determinant: f64,
    pub min_pivot_ratio: f64,
    pub pivot_threshold: f64,
    pub determinant_threshold: f64,
    pub diagnostics: Vec<String>,
    /// 无法唯一决定时保留的全部合法候选。
    pub candidates: Vec<String>,
}

/// 高斯消元（部分主元）求逆。返回 (逆矩阵, 体检报告) 或 体检报告（拒绝）。
pub fn try_invert(m: &[Vec<f64>]) -> Result<(Vec<Vec<f64>>, MatrixCheck), MatrixCheck> {
    let n = m.len();
    let mut diagnostics = Vec::new();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(MatrixCheck {
            ok: false,
            determinant: 0.0,
            min_pivot_ratio: 0.0,
            pivot_threshold: MIN_PIVOT_RATIO,
            determinant_threshold: MIN_DETERMINANT,
            diagnostics: vec!["矩阵不是非空方阵".into()],
            candidates: vec!["manual_edit".into()],
        });
    }
    let row_norms: Vec<f64> = m
        .iter()
        .map(|r| r.iter().fold(0.0f64, |a, &x| a.max(x.abs())))
        .collect();
    let max_norm = row_norms.iter().cloned().fold(0.0f64, f64::max);

    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let mut det_sign = 1.0f64;
    let mut det_abs_log = 0.0f64;
    let mut min_pivot_ratio = f64::INFINITY;

    for col in 0..n {
        let mut piv = col;
        for r in col..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pivot_val = a[piv][col];
        let denom = row_norms.get(piv).copied().unwrap_or(0.0).max(1e-300);
        min_pivot_ratio = min_pivot_ratio.min(pivot_val.abs() / denom);
        if pivot_val.abs() <= f64::EPSILON * max_norm.max(1.0) * 10.0 {
            diagnostics.push(format!("第 {col} 列主元 ≈ 0（{pivot_val:e}），矩阵奇异或近奇异"));
            return Err(MatrixCheck {
                ok: false,
                determinant: 0.0,
                min_pivot_ratio: if min_pivot_ratio.is_finite() { min_pivot_ratio } else { 0.0 },
                pivot_threshold: MIN_PIVOT_RATIO,
                determinant_threshold: MIN_DETERMINANT,
                diagnostics,
                candidates: vec![
                    "identity_fallback".into(),
                    "manual_edit".into(),
                    "drop_channel".into(),
                ],
            });
        }
        if piv != col {
            a.swap(piv, col);
            inv.swap(piv, col);
            det_sign = -det_sign;
        }
        det_abs_log += pivot_val.abs().ln();
        let inv_pivot = 1.0 / pivot_val;
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col] * inv_pivot;
            if factor == 0.0 {
                continue;
            }
            for c in 0..n {
                a[r][c] -= factor * a[col][c];
                inv[r][c] -= factor * inv[col][c];
            }
        }
        for c in 0..n {
            a[col][c] *= inv_pivot;
            inv[col][c] *= inv_pivot;
        }
    }

    let determinant = det_sign * det_abs_log.exp();
    if determinant.abs() < MIN_DETERMINANT {
        diagnostics.push(format!("行列式 {determinant:e} 低于阈值 {MIN_DETERMINANT:e}"));
    }
    if min_pivot_ratio < MIN_PIVOT_RATIO {
        diagnostics.push(format!(
            "最小主元比 {min_pivot_ratio:e} 低于阈值 {MIN_PIVOT_RATIO:e}"
        ));
    }
    let ok = diagnostics.is_empty();
    let check = MatrixCheck {
        ok,
        determinant,
        min_pivot_ratio,
        pivot_threshold: MIN_PIVOT_RATIO,
        determinant_threshold: MIN_DETERMINANT,
        diagnostics,
        candidates: if ok {
            vec![]
        } else {
            vec!["identity_fallback".into(), "manual_edit".into(), "drop_channel".into()]
        },
    };
    if ok {
        Ok((inv, check))
    } else {
        Err(check)
    }
}

/// 对事件行应用补偿：comp = raw x inverse。
pub fn apply(events: &[Vec<f64>], channels: &[usize], inverse: &[Vec<f64>]) -> Vec<Vec<f64>> {
    events
        .iter()
        .map(|row| {
            let mut out = row.to_vec();
            for (i, &ci) in channels.iter().enumerate() {
                let mut acc = 0.0;
                for (j, &cj) in channels.iter().enumerate() {
                    acc += row[cj] * inverse[j][i];
                }
                out[ci] = acc;
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_inverts() {
        let m = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let (inv, check) = try_invert(&m).unwrap();
        assert!(check.ok);
        let p00 = m[0][0] * inv[0][0] + m[0][1] * inv[1][0];
        let p11 = m[1][0] * inv[0][1] + m[1][1] * inv[1][1];
        assert!((p00 - 1.0).abs() < 1e-9);
        assert!((p11 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn singular_rejected_not_silently_inverted() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let err = try_invert(&m).unwrap_err();
        assert!(!err.ok);
        assert!(!err.candidates.is_empty());
    }

    #[test]
    fn near_singular_rejected() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-12]];
        let err = try_invert(&m).unwrap_err();
        assert!(!err.ok);
        assert!(err.diagnostics.iter().any(|d| d.contains("主元")));
    }
}
