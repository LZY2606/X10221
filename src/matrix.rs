//! Compensation matrix algebra with explicit near-singularity detection.
//! A near-singular spillover matrix must never be silently inverted.

pub const SINGULAR_DET_EPS: f64 = 1e-8;

#[derive(Debug, Clone, PartialEq)]
pub enum InverseVerdict {
    Ok { inverse: Vec<Vec<f64>>, determinant: f64 },
    NearSingular { determinant: f64, candidates: Vec<String> },
}

pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0;
    for i in 0..n {
        let mut piv = i;
        for r in (i + 1)..n {
            if a[r][i].abs() > a[piv][i].abs() {
                piv = r;
            }
        }
        if a[piv][i].abs() < 1e-300 {
            return 0.0;
        }
        if piv != i {
            a.swap(i, piv);
            det = -det;
        }
        det *= a[i][i];
        let d = a[i][i];
        for r in (i + 1)..n {
            let f = a[r][i] / d;
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
        }
    }
    det
}

fn raw_inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for i in 0..n {
        let mut piv = i;
        for r in (i + 1)..n {
            if a[r][i].abs() > a[piv][i].abs() {
                piv = r;
            }
        }
        if a[piv][i].abs() < 1e-300 {
            return None;
        }
        a.swap(i, piv);
        inv.swap(i, piv);
        let d = a[i][i];
        for c in 0..n {
            a[i][c] /= d;
            inv[i][c] /= d;
        }
        for r in 0..n {
            if r != i {
                let f = a[r][i];
                for c in 0..n {
                    a[r][c] -= f * a[i][c];
                    inv[r][c] -= f * inv[i][c];
                }
            }
        }
    }
    Some(inv)
}

/// Gate the inversion behind an explicit determinant check, normalised by scale.
pub fn checked_inverse(m: &[Vec<f64>]) -> InverseVerdict {
    let n = m.len();
    let det = determinant(m);
    let scale = m
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0_f64, |acc, v| acc.max(v.abs()))
        .max(1e-300);
    let threshold = SINGULAR_DET_EPS * scale.powi(n as i32);
    if det.abs() <= threshold {
        InverseVerdict::NearSingular {
            determinant: det,
            candidates: vec![
                "request_new_single_stain_controls".to_string(),
                "use_pseudoinverse_with_diagnostic".to_string(),
                "keep_uncompensated".to_string(),
            ],
        }
    } else {
        match raw_inverse(m) {
            Some(inverse) => InverseVerdict::Ok { inverse, determinant: det },
            None => InverseVerdict::NearSingular {
                determinant: det,
                candidates: vec!["request_new_single_stain_controls".to_string()],
            },
        }
    }
}

/// Apply a compensation matrix to a vector of detector values.
pub fn apply(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_matrix_inverts() {
        let m = vec![vec![1.0, 0.35], vec![0.12, 1.0]];
        match checked_inverse(&m) {
            InverseVerdict::Ok { inverse, determinant } => {
                assert!((determinant - 0.958).abs() < 1e-9);
                // M * M^-1 = I
                let id = apply(&inverse, &[m[0][0], m[1][0]]);
                assert!((id[0] - 1.0).abs() < 1e-9);
                assert!(id[1].abs() < 1e-9);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn near_singular_matrix_is_not_silently_inverted() {
        let m = vec![vec![1.0, 0.999999999], vec![0.999999999, 1.0]];
        match checked_inverse(&m) {
            InverseVerdict::NearSingular { determinant, candidates } => {
                assert!(determinant.abs() < 1e-8);
                assert!(candidates.len() >= 2, "all legal candidates preserved");
            }
            _ => panic!("near-singular matrix must not be silently inverted"),
        }
    }

    #[test]
    fn exactly_singular() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(matches!(checked_inverse(&m), InverseVerdict::NearSingular { .. }));
    }
}
