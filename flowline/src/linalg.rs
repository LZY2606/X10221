//! Small dense linear algebra for compensation matrices.
//!
//! Inversion is never implicit: callers receive [`InvertStatus`] so a
//! near-singular compensation matrix cannot be silently inverted.

pub const NEAR_SINGULAR_PIVOT_RATIO: f64 = 1e-3;
pub const SINGULAR_PIVOT_RATIO: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub enum InvertStatus {
    Ok,
    NearSingular { reason: String, pivot_ratio: f64, condition_estimate: f64 },
    Singular { reason: String, pivot_ratio: f64 },
}

#[derive(Debug, Clone)]
pub struct InvertOutcome {
    pub inverse: Vec<Vec<f64>>,
    pub status: InvertStatus,
    /// largest-magnitude pivot / smallest-magnitude pivot
    pub condition_estimate: f64,
}

/// Gaussian elimination with partial pivoting, augmented matrix form.
pub fn invert(m: &[Vec<f64>]) -> Result<InvertOutcome, String> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err("matrix must be square".into());
    }
    let scale = m
        .iter()
        .flat_map(|r| r.iter().map(|v| v.abs()))
        .fold(1.0f64, f64::max);
    let mut aug: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = r.clone();
            v.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            v
        })
        .collect();
    let mut max_pivot = 0.0f64;
    let mut min_pivot = f64::MAX;

    for col in 0..n {
        let piv = (col..n)
            .max_by(|&a, &b| {
                aug[a][col].abs().partial_cmp(&aug[b][col].abs()).unwrap()
            })
            .ok_or("pivot selection failed")?;
        aug.swap(col, piv);
        let p = aug[col][col];
        if p.is_nan() || p.is_infinite() {
            return Err("matrix contains non-finite entries".into());
        }
        max_pivot = max_pivot.max(p.abs());
        min_pivot = min_pivot.min(p.abs());
        let inv = 1.0 / p;
        for x in aug[col].iter_mut() {
            *x *= inv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = aug[r][col];
            for k in 0..2 * n {
                aug[r][k] -= factor * aug[col][k];
            }
        }
    }

    let cond = if min_pivot > 0.0 {
        max_pivot / min_pivot
    } else {
        f64::INFINITY
    };
    let pivot_ratio = min_pivot / max_pivot.max(scale.max(1.0));
    let inverse: Vec<Vec<f64>> = aug.iter().map(|r| r[n..].to_vec()).collect();

    let status = if pivot_ratio < SINGULAR_PIVOT_RATIO || min_pivot == 0.0 {
        InvertStatus::Singular {
            reason: format!(
                "smallest pivot {min_pivot:.3e} below singular threshold \
                 ({SINGULAR_PIVOT_RATIO:.0e} of matrix scale)"
            ),
            pivot_ratio,
        }
    } else if pivot_ratio < NEAR_SINGULAR_PIVOT_RATIO || cond > 100.0 {
        InvertStatus::NearSingular {
            reason: format!(
                "condition estimate {cond:.1}; pivot ratio {pivot_ratio:.3e} \
                 below {NEAR_SINGULAR_PIVOT_RATIO:.0e}"
            ),
            pivot_ratio,
            condition_estimate: cond,
        }
    } else {
        InvertStatus::Ok
    };
    Ok(InvertOutcome {
        inverse,
        status,
        condition_estimate: cond,
    })
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_spillover_inverts() {
        let m = vec![
            vec![1.0, 0.06, 0.0],
            vec![0.15, 1.0, 0.04],
            vec![0.0, 0.08, 1.0],
        ];
        let out = invert(&m).unwrap();
        assert_eq!(out.status, InvertStatus::Ok);
        let back = out
            .inverse
            .iter()
            .map(|r| mat_vec(&m, r))
            .collect::<Vec<_>>();
        for (i, row) in back.iter().enumerate() {
            for (j, x) in row.iter().enumerate() {
                assert!((x - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn near_singular_chain_is_flagged() {
        let m = vec![
            vec![1.0, 0.70, 0.0],
            vec![0.70, 1.0, 0.70],
            vec![0.0, 0.70, 1.0],
        ];
        let out = invert(&m).unwrap();
        assert!(matches!(out.status, InvertStatus::NearSingular { .. }));
        assert!(out.condition_estimate > 100.0);
    }

    #[test]
    fn exactly_singular_chain_is_flagged() {
        let q = 1.0 / 2.0f64.sqrt();
        let m = vec![
            vec![1.0, q, 0.0],
            vec![q, 1.0, q],
            vec![0.0, q, 1.0],
        ];
        let out = invert(&m).unwrap();
        assert!(matches!(out.status, InvertStatus::Singular { .. }));
    }
}
