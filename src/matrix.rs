//! Compensation-matrix numerics with explicit singularity handling.
//!
//! A near-singular spillover matrix is never silently inverted: the exact
//! inverse is presented as one candidate alongside regularized alternatives,
//! and an exactly singular matrix yields no inverse at all. The reviewer must
//! adjudicate before compensated populations can be computed.

use crate::types::{CompensationMatrix, InversionCandidate, InversionCandidates, MatrixQuality};
use anyhow::{bail, Result};

/// Threshold below which the reciprocal condition number flags near-singularity.
pub const NEAR_SINGULAR_RCOND: f64 = 1e-6;

fn n_of(matrix: &CompensationMatrix) -> Result<usize> {
    let n = matrix.channels.len();
    if n == 0 || matrix.rows.len() != n || matrix.rows.iter().any(|r| r.len() != n) {
        bail!("compensation matrix must be square and match channel order");
    }
    Ok(n)
}

struct Lu {
    lu: Vec<Vec<f64>>,
    piv: Vec<usize>,
    pivots: Vec<f64>,
    scale: f64,
}

fn lu_decompose(a: &[Vec<f64>]) -> Lu {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut pivots = Vec::with_capacity(n);
    let scale = a
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0_f64, |m, v| m.max(v.abs()));
    for k in 0..n {
        let mut best = k;
        let mut best_val = lu[k][k].abs();
        for i in (k + 1)..n {
            if lu[i][k].abs() > best_val {
                best_val = lu[i][k].abs();
                best = i;
            }
        }
        if best != k {
            lu.swap(k, best);
            piv.swap(k, best);
        }
        let pivot = lu[k][k];
        pivots.push(pivot.abs());
        if pivot.abs() < scale * 1e-300 {
            for i in (k + 1)..n {
                lu[i][k] = 0.0;
            }
            continue;
        }
        for i in (k + 1)..n {
            let factor = lu[i][k] / pivot;
            lu[i][k] = factor;
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }
    Lu { lu, piv, pivots, scale }
}

fn lu_solve(lu: &Lu, b: &[f64]) -> Option<Vec<f64>> {
    let n = lu.lu.len();
    let mut x = vec![0.0; n];
    for i in 0..n {
        x[i] = b[lu.piv[i]];
        for j in 0..i {
            x[i] -= lu.lu[i][j] * x[j];
        }
    }
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu.lu[i][j] * x[j];
        }
        let pivot = lu.lu[i][i];
        if pivot.abs() < lu.scale * 1e-300 {
            return None;
        }
        x[i] /= pivot;
    }
    Some(x)
}

fn invert_with(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let lu = lu_decompose(a);
    let n = a.len();
    let mut cols = Vec::with_capacity(n);
    for k in 0..n {
        let mut b = vec![0.0; n];
        b[k] = 1.0;
        cols.push(lu_solve(&lu, &b)?);
    }
    let mut inv = vec![vec![0.0; n]; n];
    for (j, col) in cols.iter().enumerate() {
        for i in 0..n {
            inv[i][j] = col[i];
        }
    }
    Some(inv)
}

fn norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    (0..n)
        .map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>())
        .fold(0.0_f64, f64::max)
}

/// LAPACK-style 1-norm estimate of ||A^-1|| (Higham sign-pattern iteration).
fn estimate_inverse_norm1(a: &[Vec<f64>]) -> Option<f64> {
    let lu = lu_decompose(a);
    let n = a.len();
    if lu
        .pivots
        .iter()
        .any(|&p| p < lu.scale * 1e-300)
    {
        return None;
    }
    let mut x = vec![1.0 / n as f64; n];
    let mut gamma = 0.0;
    let mut prev_sign = vec![0_i8; n];
    for _ in 0..5 {
        let w = lu_solve(&lu, &x)?;
        let xi: Vec<i8> = w.iter().map(|v| if v >= 0.0 { 1 } else { -1 }).collect();
        let new_gamma: f64 = w.iter().map(|v| v.abs()).sum();
        if new_gamma <= gamma || xi == prev_sign {
            return Some(gamma.max(new_gamma));
        }
        gamma = new_gamma;
        prev_sign = xi.clone();
        // Solve A^T z = xi using transposed LU via a fresh factorization of A^T.
        let mut at = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                at[j][i] = a[i][j];
            }
        }
        let lut = lu_decompose(&at);
        let z = lu_solve(&lut, &xi.iter().map(|s| *s as f64).collect::<Vec<_>>())?;
        // Position of the largest |z| component picks the next unit vector.
        let p = z
            .iter()
            .enumerate()
            .fold((0usize, 0.0_f64), |(ip, m), (i, v)| {
                if v.abs() > m {
                    (i, v.abs())
                } else {
                    (ip, m)
                }
            })
            .0;
        let mut e = vec![0.0; n];
        e[p] = 1.0;
        x = e;
    }
    Some(gamma)
}

fn round_mat(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    m.iter()
        .map(|r| {
            r.iter()
                .map(|v| {
                    let scaled = v * 1e10;
                    if scaled.is_finite() { scaled.round() / 1e10 } else { *v }
                })
                .collect()
        })
        .collect()
}

pub fn analyze(matrix: &CompensationMatrix) -> Result<InversionCandidates> {
    let n = n_of(matrix)?;
    let a = &matrix.rows;
    let lu = lu_decompose(a);
    let scale = lu.scale.max(1.0);
    let rank = lu.pivots.iter().filter(|&&p| p >= 1e-12 * scale).count();
    let smallest_pivot = lu.pivots.iter().cloned().fold(f64::INFINITY, f64::min);

    if rank < n {
        return Ok(InversionCandidates {
            candidates: vec![],
            quality: MatrixQuality {
                invertible: false,
                rcond: 0.0,
                log10_condition: f64::INFINITY,
                rank,
                smallest_pivot,
                code: "SINGULAR".to_string(),
                message: format!(
                    "spillover matrix is singular (rank {}/{}); no unique inverse exists",
                    rank, n
                ),
            },
            uniquely_determined: false,
        });
    }

    let a_norm = norm1(a).max(1e-300);
    let inv_norm = estimate_inverse_norm1(a).unwrap_or(f64::INFINITY);
    let cond = a_norm * inv_norm;
    let rcond = (1.0 / cond).clamp(0.0, 1.0);

    let exact = invert_with(a).expect("full-rank matrix is invertible");
    let exact_round = round_mat(&exact);

    if rcond >= NEAR_SINGULAR_RCOND {
        return Ok(InversionCandidates {
            candidates: vec![InversionCandidate {
                kind: "exact".to_string(),
                label: "exact inverse".to_string(),
                epsilon: None,
                inverse: exact_round,
                amplification: cond,
                legal: true,
                rationale: format!(
                    "matrix is well conditioned (1/cond = {:.3e}); unique inverse",
                    rcond
                ),
            }],
            quality: MatrixQuality {
                invertible: true,
                rcond,
                log10_condition: cond.log10(),
                rank,
                smallest_pivot,
                code: "OK".to_string(),
                message: "well-conditioned spillover matrix".to_string(),
            },
            uniquely_determined: true,
        });
    }

    let mut candidates = vec![InversionCandidate {
        kind: "reject".to_string(),
        label: "reject matrix".to_string(),
        epsilon: None,
        inverse: vec![],
        amplification: cond,
        legal: true,
        rationale:
            "refuse numerical inversion: noise would be amplified beyond the safe threshold"
                .to_string(),
    }];
    for eps in [1e-8_f64, 1e-6, 1e-4] {
        let ridge = eps * scale;
        let mut reg = a.to_vec();
        for i in 0..n {
            reg[i][i] += ridge;
        }
        if let Some(inv) = invert_with(&reg) {
            let reg_amp = norm1(&inv) * norm1(&reg);
            candidates.push(InversionCandidate {
                kind: "ridge".to_string(),
                label: format!("Tikhonov ridge epsilon={:e} * max|S|", eps),
                epsilon: Some(eps),
                inverse: round_mat(&inv),
                amplification: reg_amp,
                legal: true,
                rationale:
                    "regularized inverse is a defensible approximation, but it changes spillover"
                        .to_string(),
            });
        }
    }
    candidates.push(InversionCandidate {
        kind: "exact".to_string(),
        label: "exact inverse (noise-amplified)".to_string(),
        epsilon: None,
        inverse: exact_round,
        amplification: cond,
        legal: true,
        rationale: "mathematically exact but amplifies measurement noise strongly".to_string(),
    });

    Ok(InversionCandidates {
        candidates,
        quality: MatrixQuality {
            invertible: true,
            rcond,
            log10_condition: cond.log10().min(1e9),
            rank,
            smallest_pivot,
            code: "NEAR_SINGULAR".to_string(),
            message: format!(
                "near-singular spillover matrix (1/cond = {:.3e}); human adjudication required",
                rcond
            ),
        },
        uniquely_determined: false,
    })
}

/// Compensate a single event: x_comp = S^-1 * x_raw over `channels`.
pub fn apply_inverse(inverse: &[Vec<f64>], raw: &[f64]) -> Vec<f64> {
    inverse
        .iter()
        .map(|row| {
            row.iter()
                .zip(raw.iter())
                .map(|(a, x)| a * x)
                .sum::<f64>()
        })
        .collect()
}

pub fn is_square(matrix: &CompensationMatrix) -> bool {
    n_of(matrix).is_ok()
}
