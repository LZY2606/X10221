//! Small dense linear algebra for spillover/compensation checks.
//!
//! A near-singular spillover matrix must never be inverted silently: callers
//! receive the reciprocal condition number, the residual of each legal
//! candidate inverse, and must persist an explicit human decision.

#[derive(Debug, Clone, PartialEq)]
pub struct InverseCandidate {
    pub method: String,
    /// Relative ridge strength (0 for the plain LU inverse).
    pub ridge_rel: f64,
    pub inverse: Vec<Vec<f64>>,
    /// ||S * S^-1 - I||_inf, machine-scaled diagnostic.
    pub residual: f64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompCheck {
    pub dimension: usize,
    pub singular: bool,
    pub near_singular: bool,
    /// Reciprocal 1-norm condition number; None when exactly singular.
    pub rcond: Option<f64>,
    /// All legal candidate inverses. Empty for an exactly singular matrix.
    pub candidates: Vec<InverseCandidate>,
    pub diagnostic: Option<String>,
}

pub fn det(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    let mut m = a.to_vec();
    let mut d = 1.0;
    for col in 0..n {
        let mut pivot = col;
        for row in (col + 1)..n {
            if m[row][col].abs() > m[pivot][col].abs() {
                pivot = row;
            }
        }
        if m[pivot][col].abs() < 1e-15 {
            return 0.0;
        }
        if pivot != col {
            m.swap(pivot, col);
            d = -d;
        }
        let piv = m[col][col];
        d *= piv;
        for row in (col + 1)..n {
            let factor = m[row][col] / piv;
            for k in col..n {
                m[row][k] -= factor * m[col][k];
            }
        }
    }
    d
}

pub fn inverse(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    if a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut m: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = a[i].clone();
        row.extend(std::iter::repeat(0.0).take(n));
        row[n + i] = 1.0;
        m.push(row);
    }
    let scale = a
        .iter()
        .flat_map(|r| r.iter())
        .fold(1.0f64, |acc, v| acc.max(v.abs()));
    for col in 0..n {
        let mut pivot = col;
        for row in (col + 1)..n {
            if m[row][col].abs() > m[pivot][col].abs() {
                pivot = row;
            }
        }
        if m[pivot][col].abs() < 1e-14 * scale {
            return None;
        }
        if pivot != col {
            m.swap(pivot, col);
        }
        let piv = m[col][col];
        for k in col..2 * n {
            m[col][k] /= piv;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = m[row][col];
            if factor == 0.0 {
                continue;
            }
            for k in col..2 * n {
                m[row][k] -= factor * m[col][k];
            }
        }
    }
    Some(m.into_iter().map(|r| r[n..].to_vec()).collect())
}

fn norm1(a: &[Vec<f64>]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    let n = a.len();
    (0..n)
        .map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

fn residual_inf(s: &[Vec<f64>], inv: &[Vec<f64>]) -> f64 {
    let n = s.len();
    let mut worst = 0.0;
    for i in 0..n {
        for j in 0..n {
            let mut v = 0.0;
            for k in 0..n {
                v += s[i][k] * inv[k][j];
            }
            let target = if i == j { 1.0 } else { 0.0 };
            worst = worst.max((v - target).abs());
        }
    }
    worst
}

pub fn matvec(a: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    a.iter()
        .map(|row| row.iter().zip(x).map(|(a, v)| a * v).sum())
        .collect()
}

/// Analyze a spillover matrix. `near_threshold` is the rcond cutoff.
pub fn check_spillover(s: &[Vec<f64>], near_threshold: f64) -> CompCheck {
    let n = s.len();
    let Some(inv) = inverse(s) else {
        return CompCheck {
            dimension: n,
            singular: true,
            near_singular: false,
            rcond: None,
            candidates: vec![],
            diagnostic: Some(
                "spillover matrix is exactly singular (rank deficient); no compensation can be derived without an explicit decision".to_string(),
            ),
        };
    };
    let cond = norm1(s) * norm1(&inv);
    let rcond = if cond > 0.0 && cond.is_finite() {
        1.0 / cond
    } else {
        0.0
    };
    let near = rcond < near_threshold;
    let mut candidates = vec![InverseCandidate {
        method: "lu_inverse".to_string(),
        ridge_rel: 0.0,
        inverse: inv.clone(),
        residual: residual_inf(s, &inv),
        note: if near {
            "plain inverse exists but is numerically unstable; requires explicit reviewer choice".to_string()
        } else {
            "well-conditioned LU inverse".to_string()
        },
    }];
    if near {
        let scale = s
            .iter()
            .flat_map(|r| r.iter())
            .fold(1.0f64, |acc, v| acc.max(v.abs()));
        for (eps, label) in [
            (1e-8, "ridge_1e-8"),
            (1e-6, "ridge_1e-6"),
            (1e-4, "ridge_1e-4"),
        ] {
            let mut reg = s.to_vec();
            let ridge = eps * scale;
            for i in 0..n {
                reg[i][i] += ridge;
            }
            if let Some(rinv) = inverse(&reg) {
                candidates.push(InverseCandidate {
                    method: label.to_string(),
                    ridge_rel: eps,
                    inverse: rinv.clone(),
                    residual: residual_inf(s, &rinv),
                    note: format!("ridge regularized with {ridge:.3e} on the diagonal"),
                });
            }
        }
    }
    CompCheck {
        dimension: n,
        singular: false,
        near_singular: near,
        rcond: Some(rcond),
        candidates,
        diagnostic: near.then(|| {
            format!(
                "near-singular spillover: rcond={rcond:.3e} below threshold {near_threshold:.0e}; inverse must not be adopted silently"
            )
        }),
    }
}
