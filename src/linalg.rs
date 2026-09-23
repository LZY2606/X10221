//! Dense linear algebra for spillover/compensation, with *explicit*
//! ill-conditioning judgement.
//!
//! A near-singular spillover matrix is never silently inverted: callers get a
//! [`InvertJudgement`] carrying the smallest scaled pivot and a condition
//! estimate. Only a matrix cleared as `Ok` may produce an inverse downstream.
//! Exactly singular matrices yield no candidates; near-singular matrices yield
//! regularized ridge candidates that a human must adjudicate.

use serde::Serialize;

/// If the smallest scaled pivot (|pivot| / column max magnitude) is below this,
/// the matrix is judged near-singular. 1e-10 is deliberately conservative for
/// flow spillover data.
pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-10;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvertStatus {
    /// Well conditioned; inverse usable.
    Ok,
    /// Exact zero pivot: no unique inverse and no regularized candidates are
    /// invented silently.
    Singular,
    /// Nonzero but tiny pivot: inverse exists numerically but is unstable;
    /// candidates, if any, require a human ruling.
    NearSingular,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LuResult {
    /// LU packed (L unit lower, U upper), with row pivots applied.
    pub lu: Vec<Vec<f64>>,
    pub pivots: Vec<usize>,
    pub min_scaled_pivot: f64,
    pub col_scales: Vec<f64>,
    pub singular: bool,
}

fn column_maxes(a: &[Vec<f64>]) -> Vec<f64> {
    let n = a.len();
    let mut scales = vec![0.0; n];
    for row in a {
        for (j, &v) in row.iter().enumerate() {
            scales[j] = scales[j].max(v.abs());
        }
    }
    scales
}

pub fn lu_partial_pivot(a: &[Vec<f64>]) -> LuResult {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut pivots: Vec<usize> = (0..n).collect();
    let scales = column_maxes(a);
    let mut min_scaled = f64::INFINITY;
    let mut singular = false;

    for k in 0..n {
        let mut best = k;
        let mut best_mag = -1.0;
        for i in k..n {
            let mag = lu[i][k].abs();
            if mag > best_mag {
                best_mag = mag;
                best = i;
            }
        }
        if best != k {
            lu.swap(k, best);
            pivots.swap(k, best);
        }
        let pivot = lu[k][k];
        let scale = if k < scales.len() { scales[k] } else { 1.0 };
        let scaled = if scale > 0.0 { pivot.abs() / scale } else { pivot.abs() };
        min_scaled = min_scaled.min(scaled);
        if pivot == 0.0 || !pivot.is_finite() {
            singular = true;
            break;
        }
        for i in (k + 1)..n {
            let factor = lu[i][k] / pivot;
            lu[i][k] = factor;
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }

    LuResult {
        lu,
        pivots,
        min_scaled_pivot: if min_scaled.is_finite() { min_scaled } else { 0.0 },
        col_scales: scales,
        singular,
    }
}

/// Rough condition estimate from the completed LU: ||U|| / min|scaled pivot|.
/// Good enough to order cases; not claimed to be an exact condition number.
pub fn condition_estimate(lu: &LuResult) -> f64 {
    let n = lu.lu.len();
    let mut u_max = 0.0_f64;
    for i in 0..n {
        for j in i..n {
            u_max = u_max.max(lu.lu[i][j].abs());
        }
    }
    let m = lu.min_scaled_pivot;
    if m <= 0.0 {
        f64::INFINITY
    } else {
        u_max / m
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvertJudgement {
    pub status: InvertStatus,
    pub min_pivot: f64,
    pub condition_estimate: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
}

fn invert_from_lu(lu: &LuResult) -> Vec<Vec<f64>> {
    let n = lu.lu.len();
    let mut inv = vec![vec![0.0; n]; n];
    let solve_rhs = |rhs: &[f64]| -> Vec<f64> {
        // Pb = rhs[pivots]
        let mut b: Vec<f64> = lu.pivots.iter().map(|&r| rhs[r]).collect();
        // Ly = b
        for i in 0..n {
            for j in 0..i {
                b[i] -= lu.lu[i][j] * b[j];
            }
        }
        // Ux = y
        for i in (0..n).rev() {
            for j in (i + 1)..n {
                b[i] -= lu.lu[i][j] * b[j];
            }
            b[i] /= lu.lu[i][i];
        }
        b
    };
    for col in 0..n {
        let mut rhs = vec![0.0; n];
        rhs[col] = 1.0;
        let x = solve_rhs(&rhs);
        for i in 0..n {
            inv[i][col] = x[i];
        }
    }
    inv
}

pub fn judge_invert(a: &[Vec<f64>]) -> InvertJudgement {
    let lu = lu_partial_pivot(a);
    let cond = condition_estimate(&lu);
    let min_pivot = lu.min_scaled_pivot;
    if lu.singular {
        return InvertJudgement {
            status: InvertStatus::Singular,
            min_pivot,
            condition_estimate: cond,
            inverse: None,
        };
    }
    if min_pivot < NEAR_SINGULAR_THRESHOLD {
        return InvertJudgement {
            status: InvertStatus::NearSingular,
            min_pivot,
            condition_estimate: cond,
            inverse: None,
        };
    }
    let inv = invert_from_lu(&lu);
    InvertJudgement {
        status: InvertStatus::Ok,
        min_pivot,
        condition_estimate: cond,
        inverse: Some(inv),
    }
}

/// Ridge-regularized pseudo-solution (S + lambda I)^{-1}. Returned only as an
/// explicitly labelled candidate for human adjudication; never substituted for
/// an accepted inverse automatically.
pub fn ridge_inverse(a: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut reg = a.to_vec();
    for i in 0..n {
        if reg[i].len() != n {
            return None;
        }
        reg[i][i] += lambda;
    }
    let j = judge_invert(&reg);
    j.inverse
}

pub fn mat_vec(a: &[Vec<f64>], x: &[f64]) -> Option<Vec<f64>> {
    if a.is_empty() || a[0].len() != x.len() {
        return None;
    }
    Some(
        a.iter()
            .map(|row| row.iter().zip(x).map(|(m, v)| m * v).sum())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned() {
        let a = vec![vec![1.0, 0.05], vec![0.02, 1.0]];
        let j = judge_invert(&a);
        assert_eq!(j.status, InvertStatus::Ok);
        let inv = j.inverse.unwrap();
        let prod = vec![
            vec![
                a[0][0] * inv[0][0] + a[0][1] * inv[1][0],
                a[0][0] * inv[0][1] + a[0][1] * inv[1][1],
            ],
            vec![
                a[1][0] * inv[0][0] + a[1][1] * inv[1][0],
                a[1][0] * inv[0][1] + a[1][1] * inv[1][1],
            ],
        ];
        for i in 0..2 {
            for k in 0..2 {
                assert!((prod[i][k] - if i == k { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn rejects_exact_singular_without_inverse() {
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let j = judge_invert(&a);
        assert_eq!(j.status, InvertStatus::Singular);
        assert!(j.inverse.is_none());
    }

    #[test]
    fn flags_near_singular_and_hides_inverse() {
        // Columns almost parallel: scaled pivot ~ 1e-13.
        let a = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-13]];
        let j = judge_invert(&a);
        assert_eq!(j.status, InvertStatus::NearSingular);
        assert!(j.inverse.is_none());
        let ridge = ridge_inverse(&a, 1e-6).unwrap();
        let applied = mat_vec(&ridge, &[2.0, 2.0]).unwrap();
        assert!(applied[0].is_finite());
    }
}
