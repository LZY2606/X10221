//! Derivation rules for the flow-cytometry lineage workbench.
//!
//! Hard rules encoded here:
//! 1. A near-singular compensation matrix is never silently inverted: derivation
//!    is blocked, a machine-readable diagnostic event is emitted, and all
//!    legitimate candidate resolutions are stored explicitly.
//! 2. A gate's percentage denominator is pinned to the parent population
//!    version used at derivation time.
//! 3. Revising an upstream gate marks downstream results stale via the
//!    dependency graph; old conclusions are never overwritten.
//! 4. Concurrent modification of the same source version yields a conflict
//!    event carrying both sides; nothing is silently merged.

use serde_json::{json, Value};
use crate::store::{Store, Event};

pub const NEAR_SINGULAR_EPS: f64 = 1e-6;

// ---------- linear algebra ----------

pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0f64;
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if a[pivot][col].abs() < 1e-300 {
            return 0.0;
        }
        if pivot != col {
            a.swap(pivot, col);
            det = -det;
        }
        det *= a[col][col];
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    det
}

/// |det| normalized by the product of row norms: scale-free singularity measure.
pub fn normalized_det(m: &[Vec<f64>]) -> f64 {
    let norm: f64 = m
        .iter()
        .map(|row| row.iter().map(|x| x * x).sum::<f64>().sqrt())
        .product();
    if norm == 0.0 {
        return 0.0;
    }
    determinant(m).abs() / norm
}

pub fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[pivot][col].abs() {
                pivot = r;
            }
        }
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(pivot, col);
        inv.swap(pivot, col);
        let d = a[col][col];
        for c in 0..n {
            a[col to push
        }
    }
    Some(inv)
}
