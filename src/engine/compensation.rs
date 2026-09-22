//! Compensation matrix handling.
//! A near-singular matrix must never be inverted silently: instead the engine
//! emits a candidate containing all legal resolution options.

use crate::engine::model::CompMatrix;

#[derive(Clone, Debug)]
pub struct SingularityReport {
    pub condition_number: f64,
    pub determinant: f64,
    pub near_singular: bool,
    pub threshold: f64,
}

pub const CONDITION_THRESHOLD: f64 = 1.0e6;

fn lu_determinant(matrix: &[Vec<f64>], n: usize) -> f64 {
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut det = 1.0;
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if a[pivot][col].abs() < 1.0e-15 {
            return 0.0;
        }
        if pivot != col {
            a.swap(pivot, col);
            det = -det;
        }
        let pivot_val = a[col][col];
        det *= pivot_val;
        for r in (col + 1)..n {
            let factor = a[r][col] / pivot_val;
            for c in (col + 1)..n {
                a[r][c] -= factor * a[col][c];
            }
        }
    }
    det
}

fn norm_inf(matrix: &[Vec<f64>], n: usize) -> f64 {
    let mut max = 0.0;
    for row in matrix.iter().take(n) {
        let s: f64 = row.iter().take(n).map(|v| v.abs()).sum();
        if s > max {
            max = s;
        }
    }
    max
}

fn invert(matrix: &[Vec<f64>], n: usize) -> Option<Vec<Vec<f64>>> {
    let mut aug: Vec<Vec<f64>> = (0..n)
        .map(|r| {
            let mut row = matrix[r].clone();
            let mut id = vec![0.0; n];
            id[r] = 1.0;
            row.extend(id);
            row
        })
        .collect();
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if aug[r][col].abs() > aug[pivot][col].abs() {
                pivot = r;
            }
        }
        if aug[pivot][col].abs() < 1.0e-14 {
            return None;
        }
        if pivot != col {
            aug.swap(pivot, col);
        }
        let p = aug[col][col];
        for v in aug[col].iter_mut() {
            *v /= p;
        }
        for r in 0..n {
            if r != col && aug[r][col].abs() > 0.0 {
                let f = aug[r][col];
                for c in 0..(2 * n) {
                    aug[r][c] -= f * aug[col][c];
                }
            }
        }
    }
    Some(aug.iter().map(|row| row[n..].to_vec()).collect())
}

pub fn analyze(matrix: &CompMatrix) -> SingularityReport {
    let n = matrix.order.len();
    let det = if matrix.matrix.len() == n && matrix.matrix.iter().all(|r| r.len() == n) {
        lu_determinant(&matrix.matrix, n)
    } else {
        0.0
    };
    let condition_number = if det.abs() < 1.0e-15 {
        f64::INFINITY
    } else {
        let inv = invert(&matrix.matrix, n);
        match inv {
            Some(inv) => {
                let a = norm_inf(&matrix.matrix, n);
                let ai = norm_inf(&inv, n);
                a * ai
            }
            None => f64::INFINITY,
        }
    };
    SingularityReport {
        condition_number,
        determinant: det,
        near_singular: !condition_number.is_finite() || condition_number > CONDITION_THRESHOLD,
        threshold: CONDITION_THRESHOLD,
    }
}

/// Apply compensation: compensated = matrix * raw. Returns None when the
/// matrix is near-singular: callers must surface the candidate instead.
pub fn apply(matrix: &CompMatrix, raw: &[f64]) -> Option<Vec<f64>> {
    let report = analyze(matrix);
    if report.near_singular {
        return None;
    }
    let n = matrix.order.len();
    Some(
        matrix
            .matrix
            .iter()
            .take(n)
            .map(|row| row.iter().take(n).zip(raw).map(|(a, x)| a * x).sum())
            .collect(),
    )
}
