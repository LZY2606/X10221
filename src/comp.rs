//! Compensation matrix math. A near-singular spillover matrix must not be
//! silently inverted: callers get the condition-number estimate and explicit
//! candidates instead.

pub const NEAR_SINGULAR_THRESHOLD: f64 = 1.0e6;
pub const RIDGE_EPSILON: f64 = 1.0e-6;

pub fn mat_identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

/// Gauss-Jordan inverse with partial pivoting.
pub fn mat_inv(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err("matrix must be square and non-empty".into());
    }
    let mut a = m.to_vec();
    let mut inv = mat_identity(n);
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pivot = a[piv][col];
        if pivot.abs() < 1.0e-300 || !pivot.is_finite() {
            return Err(format!("singular matrix: pivot ~ 0 at column {col}"));
        }
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                a[r][j] -= f * a[col][j];
                inv[r][j] -= f * inv[col][j];
            }
        }
    }
    Ok(inv)
}

fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter().map(|r| r.iter().map(|v| v.abs()).sum::<f64>()).fold(0.0, f64::max)
}

/// Condition number estimate in infinity norm. Returns Err for exact singular.
pub fn condition_estimate(m: &[Vec<f64>]) -> Result<f64, String> {
    let inv = mat_inv(m)?;
    Ok(inf_norm(m) * inf_norm(&inv))
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

pub fn mat_add_eps_identity(m: &[Vec<f64>], eps: f64) -> Vec<Vec<f64>> {
    let mut out = m.to_vec();
    for i in 0..m.len() {
        out[i][i] += eps;
    }
    out
}

/// Classification of a spillover matrix with documented diagnostics.
pub struct MatrixVerdict {
    pub cond: Option<f64>,
    pub singular: bool,
    pub singular_reason: Option<String>,
    pub near_singular: bool,
    pub threshold: f64,
}

pub fn classify(m: &[Vec<f64>]) -> MatrixVerdict {
    let threshold = NEAR_SINGULAR_THRESHOLD;
    match condition_estimate(m) {
        Ok(cond) => MatrixVerdict {
            cond: Some(cond),
            singular: false,
            singular_reason: None,
            near_singular: cond > threshold,
            threshold,
        },
        Err(reason) => MatrixVerdict { cond: None, singular: true, singular_reason: Some(reason), near_singular: true, threshold },
    }
}

/// Legal candidates offered when silent inversion is refused.
pub fn candidate_inverses(m: &[Vec<f64>]) -> Vec<(&'static str, Vec<Vec<f64>>)> {
    let regularized = mat_inv(&mat_add_eps_identity(m, RIDGE_EPSILON)).unwrap_or_else(|_| mat_identity(m.len()));
    vec![
        ("ridge-e6", regularized),
        ("identity", mat_identity(m.len())),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_roundtrip() {
        let m = vec![vec![1.0, 0.12], vec![0.04, 1.0]];
        let inv = mat_inv(&m).unwrap();
        let prod = |i: usize, j: usize| m[i][0] * inv[0][j] + m[i][1] * inv[1][j];
        assert!((prod(0, 0) - 1.0).abs() < 1e-9);
        assert!(prod(0, 1).abs() < 1e-9);
    }

    #[test]
    fn duplicate_rows_are_singular() {
        let m = vec![vec![1.0, 0.5], vec![1.0, 0.5]];
        assert!(mat_inv(&m).is_err());
        assert!(classify(&m).singular);
    }

    #[test]
    fn near_singular_is_flagged_not_silently_inverted() {
        let a = 0.999_999_9;
        let m = vec![vec![1.0, a], vec![a, 1.0]];
        let v = classify(&m);
        assert!(v.cond.unwrap() > NEAR_SINGULAR_THRESHOLD);
        assert!(v.near_singular);
        // exact inverse still exists numerically; policy layer refuses to use it silently
        assert!(mat_inv(&m).is_ok());
        let cands = candidate_inverses(&m);
        assert_eq!(cands.len(), 2);
    }

    #[test]
    fn healthy_matrix_is_below_threshold() {
        let m = vec![
            vec![1.0, 0.12, 0.02, 0.0],
            vec![0.04, 1.0, 0.10, 0.01],
            vec![0.01, 0.06, 1.0, 0.08],
            vec![0.0, 0.02, 0.05, 1.0],
        ];
        assert!(condition_estimate(&m).unwrap() < NEAR_SINGULAR_THRESHOLD);
        assert!(!classify(&m).near_singular);
    }
}
