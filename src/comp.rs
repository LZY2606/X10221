//! Compensation matrix algebra.
//!
//! Rule encoded here: a near-singular spillover matrix must never be silently
//! inverted. `analyze` reports determinant / condition number and the engine
//! is required to route through the candidate/adjudication flow instead.

pub const DET_EPS: f64 = 1e-5;
pub const COND_LIMIT: f64 = 1e6;

#[derive(Debug, Clone)]
pub struct MatrixAnalysis {
    pub determinant: f64,
    pub condition: f64,
    pub near_singular: bool,
}

pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0f64;
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return 0.0;
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        let d = a[col][col];
        det *= d;
        for r in (col + 1)..n {
            let f = a[r][col] / d;
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    det
}

pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(piv, col);
        inv.swap(piv, col);
        let d = a[col][col];
        for c in 0..n {
            a[col][c] /= d;
            inv[col][c] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                for c in 0..n {
                    a[r][c] -= f * a[col][c];
                    inv[r][c] -= f * inv[col][c];
                }
            }
        }
    }
    Some(inv)
}

pub fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

pub fn analyze(m: &[Vec<f64>]) -> MatrixAnalysis {
    let det = determinant(m);
    let condition = match inverse(m) {
        Some(inv) => inf_norm(m) * inf_norm(&inv),
        None => f64::INFINITY,
    };
    let near_singular = det.abs() < DET_EPS || !(condition.is_finite()) || condition > COND_LIMIT;
    MatrixAnalysis { determinant: det, condition, near_singular }
}

/// Ridge-regularized inverse: (M + lambda*I)^-1
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a = m.to_vec();
    for i in 0..n {
        a[i][i] += lambda;
    }
    inverse(&a)
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter().map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum()).collect()
}

pub fn identity(n: usize) -> Vec<Vec<f64>> {
    (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Vec<Vec<f64>> {
        vec![vec![1.0, 0.012], vec![0.35, 1.0]]
    }

    #[test]
    fn inverse_roundtrip() {
        let m = healthy();
        let inv = inverse(&m).unwrap();
        // M * M^-1 ~= I
        for i in 0..2 {
            for j in 0..2 {
                let s: f64 = (0..2).map(|k| m[i][k] * inv[k][j]).sum();
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((s - expect).abs() < 1e-9, "entry {i},{j} = {s}");
            }
        }
    }

    #[test]
    fn healthy_matrix_is_not_flagged() {
        let a = analyze(&healthy());
        assert!(!a.near_singular, "cond={} det={}", a.condition, a.determinant);
        assert!(a.condition < 100.0);
    }

    #[test]
    fn near_singular_matrix_is_flagged() {
        let m = vec![vec![1.0, 0.999999], vec![0.999999, 1.0]];
        let a = analyze(&m);
        assert!(a.near_singular, "det={} cond={}", a.determinant, a.condition);
        assert!(a.determinant.abs() < DET_EPS || a.condition > COND_LIMIT);
    }

    #[test]
    fn singular_matrix_has_no_inverse() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        assert!(inverse(&m).is_none());
        let a = analyze(&m);
        assert!(a.near_singular);
    }

    #[test]
    fn ridge_makes_near_singular_invertible() {
        let m = vec![vec![1.0, 0.999999], vec![0.999999, 1.0]];
        let inv = ridge_inverse(&m, 1e-2).expect("ridge inverse exists");
        let v = mat_vec(&inv, &[1000.0, 900.0]);
        assert!(v.iter().all(|x| x.is_finite()));
    }
}
