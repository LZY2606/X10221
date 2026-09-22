//! Pure domain logic: compensation matrix conditioning, transforms,
//! polygon membership and population lineage / staleness.
//!
//! Nothing here touches SQLite or HTTP; all state comes in through arguments
//! so every function is reproducible from a recorded event.

use serde::{Deserialize, Serialize};

pub const NEAR_SINGULAR_COND: f64 = 1.0e6;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// No compensation applied: compensated == raw.
    Identity,
    /// Explicit solve of S x = b; only emitted when S is well conditioned.
    ExplicitInverse,
    /// Ridge regularised solution (S'S + ridge I) x = S'b.
    Ridge,
    /// Singular / near-singular, inversion refused.
    Refused,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationCandidate {
    pub kind: CandidateKind,
    pub invertible: bool,
    pub condition_estimate: f64,
    pub smallest_pivot: f64,
    pub ridge: Option<f64>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationReport {
    pub n: usize,
    pub determinant: f64,
    pub condition_estimate: f64,
    pub smallest_pivot: f64,
    pub status: String, // "well_conditioned" | "near_singular" | "singular"
    /// Every *legally admissible* candidate; refusing inversion is itself a
    /// candidate so the reviewer must decide, never an implicit inversion.
    pub candidates: Vec<CompensationCandidate>,
    /// Inverse is `None` unless the matrix is well conditioned.
    #[serde(with = "matrix_opt")]
    pub inverse: Option<Vec<Vec<f64>>>,
}

mod matrix_opt {
    use super::Vec;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(
        m: &Option<Vec<Vec<f64>>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        m.serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<d>>(d: D) -> Result<Option<Vec<Vec<f64>>>, D::Error> {
        Option::<Vec<Vec<f64>>>::deserialize(d)
    }
}

fn clone_matrix(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    m.to_vec()
}

/// Gaussian elimination with partial pivoting and explicit 1-norm scaling.
/// Returns (det, condition estimate, smallest scaled pivot, lu).
fn lu_diagnostics(m: &[Vec<f64>]) -> (f64, f64, f64, Vec<Vec<f64>>) {
    let n = m.len();
    let mut a = clone_matrix(m);
    let scale: Vec<f64> = (0..n)
        .map(|i| a[i].iter().fold(0.0f64, |x, y| x.max(y.abs())))
        .collect();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    for k in 0..n {
        let mut pivot = k;
        let mut best = -1.0f64;
        for i in k..n {
            let sc = if scale[i] == 0.0 { 1.0 } else { scale[i] };
            let v = a[i][k].abs() / sc;
            if v > best {
                best = v;
                pivot = i;
            }
        }
        if pivot != k {
            a.swap(k, pivot);
            det = -det;
        }
        let pv = a[k][k];
        let sc = if scale[k] == 0.0 { 1.0 } else { scale[k] };
        min_pivot = min_pivot.min(pv.abs() / sc);
        if pv.abs() < 1e-15 {
            det = 0.0;
            return (0.0, f64::INFINITY, 0.0, a);
        }
        det *= pv;
        for i in (k + 1)..n {
            let f = a[i][k] / pv;
            a[i][k] = f;
            for j in (k + 1)..n {
                a[i][j] -= f * a[k][j];
            }
        }
    }
    // 1-norm condition estimate via explicit inverse solves (n small).
    let inv = invert_lu(&a, n);
    let norm_m = one_norm(m);
    let norm_inv = inv.as_ref().map(one_norm).unwrap_or(f64::INFINITY);
    (det, norm_m * norm_inv, min_pivot, a)
}

fn one_norm(m: &[Vec<f64>]) -> f64 {
    if m.is_empty() {
        return 0.0;
    }
    (0..m[0].len())
        .map(|j| m.iter().map(|r| r[j].abs()).sum::<f64>())
        .fold(0.0f64, f64::max)
}

fn invert_lu(lu: &[Vec<f64>], n: usize) -> Option<Vec<Vec<f64>>> {
    let mut inv = vec![vec![0.0f64; n]; n];
    for col in 0..n {
        let mut b = vec![0.0f64; n];
        b[col] = 1.0;
        // forward substitution Ly = Pb (lu holds L below diagonal, U on/above)
        let mut y = vec![0.0f64; n];
        for i in 0..n {
            let mut s = b[i];
            for j in 0..i {
                s -= lu[i][j] * y[j];
            }
            y[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = y[i];
            for j in (i + 1)..n {
                s -= lu[i][j] * inv[j][col];
            }
            if lu[i][i].abs() < 1e-15 {
                return None;
            }
            inv[i][col] = s / lu[i][i];
        }
    }
    Some(inv)
}

/// Analyse a spillover matrix S (row-major). The inverse is only exposed when
/// the matrix is well conditioned; otherwise every admissible alternative is
/// returned and silent inversion is refused.
pub fn analyse_spillover(s: &[Vec<f64>]) -> CompensationReport {
    let n = s.len();
    let square = !s.is_empty() && s.iter().all(|r| r.len() == n);
    if !square {
        return CompensationReport {
            n,
            determinant: f64::NAN,
            condition_estimate: f64::INFINITY,
            smallest_pivot: 0.0,
            status: "singular".into(),
            candidates: vec![CompensationCandidate {
                kind: CandidateKind::Refused,
                invertible: false,
                condition_estimate: f64::INFINITY,
                smallest_pivot: 0.0,
                ridge: None,
                rationale: "spillover matrix is not square".into(),
            }],
            inverse: None,
        };
    }
    let (det, cond, min_pivot, _lu) = lu_diagnostics(s);
    let status = if det.abs() < 1e-12 || !min_pivot.is_finite() || min_pivot == 0.0 {
        "singular"
    } else if cond >= NEAR_SINGULAR_COND {
        "near_singular"
    } else {
        "well_conditioned"
    };

    let mut candidates = Vec::new();
    candidates.push(CompensationCandidate {
        kind: CandidateKind::Identity,
        invertible: false,
        condition_estimate: cond,
        smallest_pivot: min_pivot,
        ridge: None,
        rationale: "leave raw events uncompensated".into(),
    });
    let inverse = if status == "well_conditioned" {
        // Recover a clean inverse via Gauss-Jordan.
        gauss_jordan_inverse(s)
    } else {
        None
    };
    match status {
        "well_conditioned" => {
            candidates.push(CompensationCandidate {
                kind: CandidateKind::ExplicitInverse,
                invertible: true,
                condition_estimate: cond,
                smallest_pivot: min_pivot,
                ridge: None,
                rationale: format!(
                    "cond1 {:.3e} < {:.0e}; explicit inverse admissible",
                    cond, NEAR_SINGULAR_COND
                ),
            });
        }
        "near_singular" => {
            let ridge = ridge_strength(s);
            candidates.push(CompensationCandidate {
                kind: CandidateKind::Ridge,
                invertible: true,
                condition_estimate: cond,
                smallest_pivot: min_pivot,
                ridge: Some(ridge),
                rationale: format!(
                    "cond1 {:.3e} >= {:.0e}: inverse refused; ridge = {:.3e} offered only as explicit choice",
                    cond, NEAR_SINGULAR_COND, ridge
                ),
            });
            candidates.push(CompensationCandidate {
                kind: CandidateKind::Refused,
                invertible: false,
                condition_estimate: cond,
                smallest_pivot: min_pivot,
                ridge: None,
                rationale: "near-singular spillover must not be inverted silently".into(),
            });
        }
        _ => {
            candidates.push(CompensationCandidate {
                kind: CandidateKind::Refused,
                invertible: false,
                condition_estimate: cond,
                smallest_pivot: min_pivot,
                ridge: None,
                rationale: "singular spillover has no unique inverse".into(),
            });
        }
    }
    CompensationReport {
        n,
        determinant: det,
        condition_estimate: cond,
        smallest_pivot: min_pivot,
        status: status.into(),
        candidates,
        inverse,
    }
}

fn ridge_strength(s: &[Vec<f64>]) -> f64 {
    let scale = one_norm(s).max(1.0);
    scale * 1e-6
}

fn gauss_jordan_inverse(s: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = s.len();
    let mut a = vec![vec![0.0f64; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            a[i][j] = s[i][j];
        }
        a[i][n + i] = 1.0;
    }
    for k in 0..n {
        let mut p = k;
        for i in (k + 1)..n {
            if a[i][k].abs() > a[p][k].abs() {
                p = i;
            }
        }
        a.swap(k, p);
        let pv = a[k][k];
        if pv.abs() < 1e-12 {
            return None;
        }
        for x in a[k].iter_mut() {
            *x /= pv;
        }
        for i in 0..n {
            if i == k {
                continue;
            }
            let f = a[i][k];
            for j in 0..2 * n {
                a[i][j] -= f * a[k][j];
            }
        }
    }
    Some(a.iter().map(|r| r[n..].to_vec()).collect())
}

/// Solve ridge: (S'S + r I) x = S'b.
pub fn ridge_solve(s: &[Vec<f64>], b: &[f64], ridge: f64) -> Vec<f64> {
    let n = s.len();
    let mut ata = vec![vec![0.0f64; n]; n];
    let mut atb = vec![0.0f64; n];
    for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                ata[i][j] += s[k][i] * s[k][j];
            }
        }
        ata[i][i] += ridge;
        for k in 0..n {
            atb[i] += s[k][i] * b[k];
        }
    }
    if let Some(inv) = gauss_jordan_inverse(&ata) {
        (0..n)
            .map(|i| (0..n).map(|j| inv[i][j] * atb[j]).sum())
            .collect()
    } else {
        b.to_vec()
    }
}

/// Apply a chosen compensation candidate to one event (in detector order).
pub fn compensate_event(
    report: &CompensationReport,
    s: &[Vec<f64>],
    raw: &[f32],
    choice: CandidateKind,
) -> Vec<f32> {
    let b: Vec<f64> = raw.iter().map(|x| *x as f64).collect();
    let x: Vec<f64> = match choice {
        CandidateKind::Identity => b,
        CandidateKind::ExplicitInverse => match &report.inverse {
            Some(inv) => (0..s.len())
                .map(|i| (0..s.len()).map(|j| inv[i][j] * b[j]).sum())
                .collect(),
            None => b,
        },
        CandidateKind::Ridge => {
            let r = report
                .candidates
                .iter()
                .find_map(|c| c.ridge)
                .unwrap_or_else(|| ridge_strength(s));
            ridge_solve(s, &b, r)
        }
        CandidateKind::Refused => b,
    };
    x.into_iter().map(|v| v as f32).collect()
}
