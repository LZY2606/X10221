//! Minimal linear algebra used by compensation review.
//!
//! Deliberately small: reviewers must be able to read exactly how a matrix was
//! judged singular and how a ridge-regularised candidate was produced.

#[derive(Debug, Clone, PartialEq)]
pub struct MatrixVerdict {
    pub condition_number_1norm: f64,
    pub reciprocal_condition: f64,
    pub near_singular: bool,
    pub threshold: f64,
}

pub fn is_finite_matrix(m: &[Vec<f64>]) -> bool {
    m.iter().flatten().all(|v| v.is_finite())
}

fn shape(m: &[Vec<f64>]) -> Option<(usize, usize)> {
    let rows = m.len();
    if rows == 0 {
        return None;
    }
    let cols = m[0].len();
    if cols == 0 || m.iter().any(|r| r.len() != cols) {
        return None;
    }
    Some((rows, cols))
}

fn norm_1(m: &[Vec<f64>]) -> f64 {
    let (_, cols) = shape(m).unwrap_or((0, 0));
    (0..cols)
        .map(|c| m.iter().map(|row| row[c].abs()).sum::<f64>())
        .fold(0.0_f64, f64::max)
}

/// Gauss-Jordan inversion with partial pivoting.
/// Returns `None` when a pivot is smaller than `1e-12` times the column scale.
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let (n, nc) = shape(m)?;
    if n != nc {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| {
        let mut r = vec![0.0_f64; n];
        r[i] = 1.0;
        r
    }).collect();

    for col in 0..n {
        let pivot = (col..n)
            .max_by(|a_i, b_i| a[*a_i][col].abs().partial_cmp(&a[*b_i][col].abs()).unwrap())
            .unwrap();
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        if pivot != col {
            a.swap(pivot, col);
            inv.swap(pivot, col);
        }
        let pivot_val = a[col][col];
        for k in 0..n {
            a[col][k] /= pivot_val;
            inv[col][k] /= pivot_val;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor == 0.0 {
                continue;
            }
            for k in 0..n {
                a[row][k] -= factor * a[col][k];
                inv[row][k] -= factor * inv[col][k];
            }
        }
    }
    Some(inv)
}

/// Reciprocal 1-norm condition estimate. `rcond < threshold` is treated as
/// near-singular so callers are forced to adjudicate instead of silently
/// inverting.
pub fn verdict(m: &[Vec<f64>], threshold: f64) -> Option<MatrixVerdict> {
    if !is_finite_matrix(m) {
        return None;
    }
    let n = shape(m)?.0;
    if n != shape(m)?.1 {
        return None;
    }
    let inv = inverse(m)?;
    let cond = norm_1(m) * norm_1(&inv);
    let rcond = if cond.is_finite() && cond > 0.0 { 1.0 / cond } else { 0.0 };
    Some(MatrixVerdict {
        condition_number_1norm: cond,
        reciprocal_condition: rcond,
        near_singular: !rcond.is_finite() || rcond < threshold,
        threshold,
    })
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Option<Vec<f64>> {
    let (rows, cols) = shape(m)?;
    if cols != v.len() {
        return None;
    }
    Some((0..rows)
        .map(|r| (0..cols).map(|c| m[r][c] * v[c]).sum())
        .collect())
}

pub fn transpose(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let (rows, cols) = shape(m).unwrap_or((0, 0));
    (0..cols)
        .map(|c| (0..rows).map(|r| m[r][c]).collect())
        .collect()
}

pub fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let (ar, ac) = shape(a)?;
    let (br, bc) = shape(b)?;
    if ac != br {
        return None;
    }
    Some((0..ar).map(|i| {
        (0..bc).map(|j| (0..ac).map(|k| a[i][k] * b[k][j]).sum()).collect()
    }).collect())
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

/// Ridge-regularised unmixing candidate: `(S^T S + lambda I)^-1 S^T`.
/// This is never applied automatically; it is stored as an explicit candidate.
pub fn ridge_unmixing(s: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let st = transpose(s);
    let mut sts = mat_mul(&st, s)?;
    let n = sts.len();
    for i in 0..n {
        sts[i][i] += lambda;
    }
    let sts_inv = inverse(&sts)?;
    mat_mul(&sts_inv, &st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_well_conditioned() {
        let m = identity(3);
        let v = verdict(&m, 1e-6).unwrap();
        assert!(!v.near_singular);
        assert!((v.condition_number_1norm - 1.0).abs() < 1e-9);
    }

    #[test]
    fn exactly_singular_matrix_has_no_inverse() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(inverse(&m).is_none());
        assert!(verdict(&m, 1e-6).is_none());
    }

    #[test]
    fn near_singular_matrix_is_flagged() {
        // Rows are almost parallel: inversion exists analytically but is unstable.
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-9]];
        let v = verdict(&m, 1e-6).unwrap();
        assert!(v.near_singular, "cond={}", v.condition_number_1norm);
    }

    #[test]
    fn ridge_stabilises_near_singular_matrix() {
        let s = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-9]];
        let m = ridge_unmixing(&s, 1e-3).unwrap();
        // Finite, and unmixing an identity-ish input stays bounded.
        assert!(is_finite_matrix(&m));
        let out = mat_vec(&m, &[1.0, 1.0]).unwrap();
        assert!(out.iter().all(|x| x.abs() < 10.0));
    }
}
