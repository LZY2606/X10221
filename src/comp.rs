//! Compensation matrix algebra.
//! Rule: a near-singular spillover matrix must NEVER be silently inverted.
//! `try_invert` returns a structured `SingularReport` instead, which the
//! pipeline turns into a machine-readable `compensation.rejected_near_singular`
//! event together with the legal candidate resolutions.

pub const PIVOT_EPS: f64 = 1e-9;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SingularReport {
    pub determinant: f64,
    pub min_pivot: f64,
    pub max_diag: f64,
    pub threshold: f64,
    pub reason: String,
}

/// Gaussian elimination with partial pivoting. Returns the inverse, or a
/// `SingularReport` when the matrix is singular / near-singular.
pub fn try_invert(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, SingularReport> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(SingularReport {
            determinant: 0.0,
            min_pivot: 0.0,
            max_diag: 0.0,
            threshold: PIVOT_EPS,
            reason: "matrix is not square".into(),
        });
    }
    let max_diag = (0..n).map(|i| m[i][i].abs()).fold(0.0, f64::max);
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut piv = col;
        for r in col..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pivot_val = a[piv][col];
        min_pivot = min_pivot.min(pivot_val.abs());
        det *= pivot_val;
        if pivot_val.abs() <= PIVOT_EPS * max_diag.max(1e-300) {
            return Err(SingularReport {
                determinant: det,
                min_pivot,
                max_diag,
                threshold: PIVOT_EPS * max_diag.max(1e-300),
                reason: format!(
                    "near-singular: pivot {pivot_val:.3e} at column {col} <= threshold {:.3e}",
                    PIVOT_EPS * max_diag.max(1e-300)
                ),
            });
        }
        if piv != col {
            a.swap(piv, col);
            inv.swap(piv, col);
            det = -det;
        }
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..n {
                        a[r][j] -= f * a[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    Ok(inv)
}

/// Compensate one event: compensated = raw * S^-1 (row vector convention).
pub fn compensate_event(raw: &[f64], inv: &[Vec<f64>], channels: &[usize]) -> Vec<f64> {
    let mut out = raw.to_vec();
    let n = channels.len();
    let mut tmp = vec![0.0; n];
    for (j, _) in channels.iter().enumerate() {
        let mut acc = 0.0;
        for (i, &ci) in channels.iter().enumerate() {
            acc += raw[ci] * inv[i][j];
        }
        tmp[j] = acc;
    }
    for (j, &cj) in channels.iter().enumerate() {
        out[cj] = tmp[j];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned() {
        let m = vec![vec![1.0, 0.02], vec![0.15, 1.0]];
        let inv = try_invert(&m).unwrap();
        // m * inv ≈ I
        let p00 = m[0][0] * inv[0][0] + m[0][1] * inv[1][0];
        let p11 = m[1][0] * inv[0][1] + m[1][1] * inv[1][1];
        assert!((p00 - 1.0).abs() < 1e-12);
        assert!((p11 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_near_singular() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-12]];
        let report = try_invert(&m).unwrap_err();
        assert!(report.min_pivot <= report.threshold);
        assert!(report.reason.contains("near-singular"));
    }

    #[test]
    fn rejects_exactly_singular() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(try_invert(&m).is_err());
    }
}
