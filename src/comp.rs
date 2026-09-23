//! Compensation matrix evaluation. A near-singular matrix is never silently
//! inverted: evaluation returns a verdict and the caller must persist
//! machine-readable candidates for human adjudication.

pub const NEAR_SINGULAR_THRESHOLD: f64 = 1.0e4;
pub const RIDGE_LAMBDAS: [f64; 3] = [1.0e-6, 1.0e-4, 1.0e-2];

#[derive(Debug, Clone, PartialEq)]
pub enum MatrixVerdict {
    Ok { inverse: Vec<Vec<f64>>, condition: f64 },
    NearSingular { condition: f64, determinant: f64 },
    Singular { determinant: f64 },
}

pub fn frobenius_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .flat_map(|r| r.iter())
        .map(|v| v * v)
        .sum::<f64>()
        .sqrt()
}

/// Gauss-Jordan inverse with partial pivoting.
/// Returns None when a pivot is exactly zero (singular matrix).
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut pivot = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        if a[pivot][col] == 0.0 {
            return None;
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for row in 0..n {
            if row != col {
                let f = a[row][col];
                for j in 0..n {
                    a[row][j] -= f * a[col][j];
                    inv[row][j] -= f * inv[col][j];
                }
            }
        }
    }
    Some(inv)
}

pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0;
    for col in 0..n {
        let mut pivot = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[pivot][col].abs() {
                pivot = row;
            }
        }
        if a[pivot][col] == 0.0 {
            return 0.0;
        }
        if pivot != col {
            a.swap(col, pivot);
            det = -det;
        }
        det *= a[col][col];
        for row in (col + 1)..n {
            let f = a[row][col] / a[col][col];
            for j in col..n {
                a[row][j] -= f * a[col][j];
            }
        }
    }
    det
}

/// Frobenius condition estimate: ||A||_F * ||A^-1||_F.
pub fn condition_estimate(m: &[Vec<f64>]) -> Option<f64> {
    inverse(m).map(|inv| frobenius_norm(m) * frobenius_norm(&inv))
}

pub fn evaluate(m: &[Vec<f64>]) -> MatrixVerdict {
    let det = determinant(m);
    match inverse(m) {
        None => MatrixVerdict::Singular { determinant: det },
        Some(inv) => {
            let cond = frobenius_norm(m) * frobenius_norm(&inv);
            if cond > NEAR_SINGULAR_THRESHOLD {
                MatrixVerdict::NearSingular { condition: cond, determinant: det }
            } else {
                MatrixVerdict::Ok { inverse: inv, condition: cond }
            }
        }
    }
}

/// Ridge-regularised inverse candidate: (A + lambda*I)^-1.
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut reg = m.to_vec();
    for i in 0..n {
        reg[i][i] += lambda;
    }
    inverse(&reg)
}

/// Apply compensation: compensated = M^-1 * observed (column vector).
pub fn apply_inverse(inv: &[Vec<f64>], observed: &[f64]) -> Vec<f64> {
    inv.iter()
        .map(|row| row.iter().zip(observed).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_matrix_inverts_cleanly() {
        let m = vec![vec![1.0, 0.15], vec![0.08, 1.0]];
        match evaluate(&m) {
            MatrixVerdict::Ok { inverse, condition } => {
                assert!(condition < 10.0, "cond={condition}");
                // A * A^-1 ~= I
                let p00 = m[0][0] * inverse[0][0] + m[0][1] * inverse[1][0];
                let p11 = m[1][0] * inverse[0][1] + m[1][1] * inverse[1][1];
                assert!((p00 - 1.0).abs() < 1e-9);
                assert!((p11 - 1.0).abs() < 1e-9);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn near_singular_matrix_is_flagged_not_inverted() {
        let m = vec![vec![1.0, 0.999999], vec![0.999999, 1.0]];
        match evaluate(&m) {
            MatrixVerdict::NearSingular { condition, determinant } => {
                assert!(condition > NEAR_SINGULAR_THRESHOLD);
                assert!(determinant.abs() < 1e-4);
            }
            other => panic!("expected NearSingular, got {other:?}"),
        }
    }

    #[test]
    fn exactly_singular_matrix_has_no_inverse() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert_eq!(evaluate(&m), MatrixVerdict::Singular { determinant: 0.0 });
    }

    #[test]
    fn ridge_candidates_are_better_conditioned() {
        let m = vec![vec![1.0, 0.999999], vec![0.999999, 1.0]];
        let raw_cond = condition_estimate(&m).unwrap();
        for lambda in RIDGE_LAMBDAS {
            let inv = ridge_inverse(&m, lambda).expect("ridge inverse exists");
            let mut reg = m.clone();
            reg[0][0] += lambda;
            reg[1][1] += lambda;
            let cond = frobenius_norm(&reg) * frobenius_norm(&inv);
            assert!(cond < raw_cond);
        }
    }
}
