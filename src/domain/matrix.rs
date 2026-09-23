//! 小型稠密线性代数：只实现补偿判定所需的缩放行列式、LU 求逆与 ridge 解。
//!
//! 关键规则：近奇异矩阵绝不直接求逆后当成唯一结论。调用方必须先看
//! [`scaled_determinant`] 与 [`NEAR_SINGULAR_THRESHOLD`] 的比较结果。

/// 本工具固定的近奇异阈值：列归一化后行列式绝对值小于该值即判定近奇异。
pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-3;

fn is_square(m: &[Vec<f64>]) -> bool {
    let n = m.len();
    n > 0 && m.iter().all(|r| r.len() == n)
}

/// 列 2-范数归一化后的行列式绝对值。
///
/// 对行=探测器、列=荧光染料的 spillover 矩阵，这是“相对接近奇异”的量度，
/// 不受各荧光尺度影响。无法计算（非方阵/零列）时返回 NaN。
pub fn scaled_determinant(m: &[Vec<f64>]) -> f64 {
    if !is_square(m) {
        return f64::NAN;
    }
    let n = m.len();
    let mut scaled = vec![vec![0.0; n]; n];
    for c in 0..n {
        let norm = (0..n).map(|r| m[r][c] * m[r][c]).sum::<f64>().sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return f64::NAN;
        }
        for r in 0..n {
            scaled[r][c] = m[r][c] / norm;
        }
    }
    let d = determinant(&scaled);
    if !d.is_finite() {
        return f64::NAN;
    }
    d.abs()
}

/// 直接计算（归一化矩阵）行列式：用带部分主元的高斯消元。
fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a = m.to_vec();
    let mut det = 1.0;
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pv = a[piv][col];
        if pv.abs() < f64::EPSILON {
            return 0.0;
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        det *= pv;
        for r in (col + 1)..n {
            let factor = a[r][col] / pv;
            for c in col..n {
                a[r][c] -= factor * a[col][c];
            }
        }
    }
    det
}

/// LU 求逆（部分主元）。近奇异/奇异时返回 None——调用方不得绕过该判定。
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    if !is_square(m) {
        return None;
    }
    let n = m.len();
    if scaled_determinant(m) < NEAR_SINGULAR_THRESHOLD {
        return None;
    }
    let mut a = vec![vec![0.0; n]; n];
    let mut p: Vec<usize> = (0..n).collect();
    for r in 0..n {
        a[r].copy_from_slice(&m[r]);
    }
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < f64::EPSILON {
            return None;
        }
        a.swap(piv, col);
        p.swap(piv, col);
        let pv = a[col][col];
        for r in (col + 1)..n {
            let factor = a[r][col] / pv;
            for c in col..n {
                a[r][c] -= factor * a[col][c];
            }
        }
    }
    // 逐列解 A X = I，先 Ly=Pb，再 Ux=y
    let mut inv = vec![vec![0.0; n]; n];
    for rhs in 0..n {
        // p 记录行交换结果：Pb[i] = I[p[i]][rhs]
        let mut b = vec![0.0; n];
        for i in 0..n {
            b[i] = if p[i] == rhs { 1.0 } else { 0.0 };
        }
        let mut y = vec![0.0; n];
        for i in 0..n {
            let mut s = b[i];
            for j in 0..i {
                s -= a[i][j] * y[j];
            }
            y[i] = s;
        }
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let mut s = y[i];
            for j in (i + 1)..n {
                s -= a[i][j] * x[j];
            }
            if a[i][i].abs() < f64::EPSILON {
                return None;
            }
            x[i] = s / a[i][i];
        }
        for i in 0..n {
            inv[i][rhs] = x[i];
        }
    }
    Some(inv)
}

/// ridge 正则解 C = (SᵀS + λI)^{-1} Sᵀ，作为近奇异时的合法候选之一。
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let mut sts = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += m[k][i] * m[k][j];
            }
            sts[i][j] = s + if i == j { lambda } else { 0.0 };
        }
    }
    let inv_sts = inverse(&sts).expect("ridge 使矩阵正定，必有逆");
    // C = inv(STS) * S^T
    let mut c = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += inv_sts[i][k] * m[j][k];
            }
            c[i][j] = s;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_well_conditioned_and_inverts() {
        let m = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        assert!((scaled_determinant(m.clone()) - 1.0).abs() < 1e-12);
        assert_eq!(inverse(&m), Some(m));
    }

    #[test]
    fn near_duplicate_rows_are_near_singular_and_no_inverse() {
        let m = vec![
            vec![1.0, 0.15, 0.02, 0.0],
            vec![0.12, 0.98, 0.01, 0.03],
            vec![0.05, 0.09, 1.0, 0.0],
            vec![0.1200002, 0.9800001, 0.0100001, 0.0300001],
        ];
        let sd = scaled_determinant(&m);
        assert!(sd < NEAR_SINGULAR_THRESHOLD, "sd={sd}");
        assert!(inverse(&m).is_none());
    }

    #[test]
    fn ridge_returns_finite_approximate() {
        let m = vec![
            vec![1.0, 0.15, 0.02, 0.0],
            vec![0.12, 0.98, 0.01, 0.03],
            vec![0.05, 0.09, 1.0, 0.0],
            vec![0.1200002, 0.9800001, 0.0100001, 0.0300001],
        ];
        let c = ridge_inverse(&m, 0.01);
        assert!(c.iter().flatten().all(|v| v.is_finite()));
    }
}
