//! Derivation rules. Everything here is pure: the store layer records
//! results + machine-readable events; these functions never mutate state.

use serde::{Deserialize, Serialize};

/// Condition estimate above which a spillover matrix must NOT be
/// silently inverted. Deliberately conservative (inf-norm estimate).
pub const NEAR_SINGULAR_COND: f64 = 1.0e8;
/// Ridge regularization offered as an explicit, human-approved candidate.
pub const RIDGE_LAMBDA: f64 = 1.0e-4;
/// Points within this distance of a gate edge are boundary points.
pub const BOUNDARY_EPS: f64 = 1.0e-9;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixVerdict {
    pub invertible: bool,
    pub cond_estimate: f64,
    pub min_pivot: f64,
    pub inverse: Option<Vec<f64>>, // row-major, only when safe
    pub candidates: Vec<CompCandidate>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompCandidate {
    pub label: String,
    pub detail: String,
}

/// Gauss-Jordan inverse with partial pivoting + inf-norm condition estimate.
/// A near-singular matrix is never silently inverted: the verdict carries
/// the diagnostics and the full set of legal candidates instead.
pub fn invert_checked(a: &[f64], n: usize) -> MatrixVerdict {
    let norm_a = inf_norm(a, n);
    let mut m = vec![0.0; n * 2 * n];
    for r in 0..n {
        for c in 0..n {
            m[r * 2 * n + c] = a[r * n + c];
        }
        m[r * 2 * n + n + r] = 1.0;
    }
    let mut min_pivot = f64::INFINITY;
    let mut singular = false;
    for col in 0..n {
        let mut piv = col;
        for r in col..n {
            if m[r * 2 * n + col].abs() > m[piv * 2 * n + col].abs() {
                piv = r;
            }
        }
        let pv = m[piv * 2 * n + col].abs();
        if pv < min_pivot {
            min_pivot = pv;
        }
        if pv < 1e-300 {
            singular = true;
            break;
        }
        if piv != col {
            for c in 0..2 * n {
                m.swap(piv * 2 * n + c, col * 2 * n + c);
            }
        }
        let d = m[col * 2 * n + col];
        for c in 0..2 * n {
            m[col * 2 * n + c] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = m[r * 2 * n + col];
                if f != 0.0 {
                    for c in 0..2 * n {
                        m[r * 2 * n + c] -= f * m[col * 2 * n + c];
                    }
                }
            }
        }
    }
    if singular || min_pivot == 0.0 {
        return MatrixVerdict {
            invertible: false,
            cond_estimate: f64::INFINITY,
            min_pivot: if min_pivot.is_finite() { min_pivot } else { 0.0 },
            inverse: None,
            candidates: near_singular_candidates("matrix is exactly singular"),
            reason: "matrix is exactly singular; inverse does not exist".into(),
        };
    }
    let mut inv = vec![0.0; n * n];
    for r in 0..n {
        for c in 0..n {
            inv[r * n + c] = m[r * 2 * n + n + c];
        }
    }
    let cond = norm_a * inf_norm(&inv, n);
    if !cond.is_finite() || cond >= NEAR_SINGULAR_COND {
        return MatrixVerdict {
            invertible: false,
            cond_estimate: cond,
            min_pivot,
            inverse: None,
            candidates: near_singular_candidates("condition estimate exceeds threshold"),
            reason: format!(
                "near-singular: cond≈{cond:.3e} >= {NEAR_SINGULAR_COND:.0e}; refusing silent inversion"
            ),
        };
    }
    MatrixVerdict {
        invertible: true,
        cond_estimate: cond,
        min_pivot,
        inverse: Some(inv),
        candidates: vec![],
        reason: format!("invertible: cond≈{cond:.3e}"),
    }
}

fn near_singular_candidates(trigger: &str) -> Vec<CompCandidate> {
    vec![
        CompCandidate {
            label: "reject-revision".into(),
            detail: format!("keep the previous active compensation matrix ({trigger})"),
        },
        CompCandidate {
            label: "ridge-regularize".into(),
            detail: format!("invert (S + {RIDGE_LAMBDA}·I) explicitly, recorded as an approximation"),
        },
        CompCandidate {
            label: "drop-channel".into(),
            detail: "exclude the collinear channel and recompute on the submatrix".into(),
        },
    ]
}

pub fn ridge_regularize(a: &[f64], n: usize, lambda: f64) -> Vec<f64> {
    let mut out = a.to_vec();
    for i in 0..n {
        out[i * n + i] += lambda;
    }
    out
}

fn inf_norm(a: &[f64], n: usize) -> f64 {
    (0..n)
        .map(|r| (0..n).map(|c| a[r * n + c].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// Apply compensation: comp = S⁻¹ · measured over the fluorescence channels.
pub fn compensate_event(inv: &[f64], n: usize, measured: &[f64]) -> Vec<f64> {
    (0..n)
        .map(|r| (0..n).map(|c| inv[r * n + c] * measured[c]).sum())
        .collect()
}

/// arcsinh transform (domain-standard cofactor scaling). Non-finite or
/// non-convertible inputs stay raw and are reported via diagnostics.
pub fn arcsinh_transform(x: f64, cofactor: f64) -> Option<f64> {
    if !x.is_finite() || cofactor <= 0.0 {
        return None;
    }
    Some((x / cofactor).asinh())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointPosition {
    Inside,
    Outside,
    /// Exactly on an edge (within BOUNDARY_EPS): cannot be uniquely decided
    /// by geometry alone; policy decides and the event log records the basis.
    Boundary,
}

/// Ray-casting point-in-polygon with explicit boundary detection.
pub fn classify_point(px: f64, py: f64, poly: &[(f64, f64)]) -> PointPosition {
    // boundary check first
    for i in 0..poly.len() {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % poly.len()];
        if point_seg_dist(px, py, x1, y1, x2, y2) <= BOUNDARY_EPS {
            return PointPosition::Boundary;
        }
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PointPosition::Inside
    } else {
        PointPosition::Outside
    }
}

fn point_seg_dist(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0);
    let cx = x1 + t * dx;
    let cy = y1 + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMembership {
    pub count: usize,
    pub boundary_count: usize,
    pub boundary_indices: Vec<usize>,
}

/// Boundary policy: boundary points are INCLUDED (deterministic default)
/// and every one of them is reported so the decision basis is auditable.
pub fn gate_membership(xs: &[f64], ys: &[f64], poly: &[(f64, f64)]) -> GateMembership {
    let mut count = 0;
    let mut boundary = Vec::new();
    for (i, (&x, &y)) in xs.iter().zip(ys.iter()).enumerate() {
        match classify_point(x, y, poly) {
            PointPosition::Inside => count += 1,
            PointPosition::Boundary => {
                count += 1;
                boundary.push(i);
            }
            PointPosition::Outside => {}
        }
    }
    GateMembership {
        count,
        boundary_count: boundary.len(),
        boundary_indices: boundary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        // 2x2 spillover: FITC<->PE
        let s = vec![1.0, 0.15, 0.02, 1.0];
        let v = invert_checked(&s, 2);
        assert!(v.invertible, "{}", v.reason);
        assert!(v.cond_estimate < 100.0);
        let inv = v.inverse.unwrap();
        // S * S^-1 ≈ I
        let mut prod = vec![0.0; 4];
        for r in 0..2 {
            for c in 0..2 {
                prod[r * 2 + c] = (0..2).map(|k| s[r * 2 + k] * inv[k * 2 + c]).sum();
            }
        }
        assert!((prod[0] - 1.0).abs() < 1e-9);
        assert!((prod[3] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn near_singular_matrix_is_not_silently_inverted() {
        // two nearly identical rows: PE and PE-TxRed collinear
        let s = vec![1.0, 0.5, 1.0, 0.5 + 1e-12];
        let v = invert_checked(&s, 2);
        assert!(!v.invertible);
        assert!(v.inverse.is_none());
        assert!(v.cond_estimate >= NEAR_SINGULAR_COND || v.cond_estimate.is_infinite());
        // all legal candidates preserved
        let labels: Vec<&str> = v.candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["reject-revision", "ridge-regularize", "drop-channel"]);
    }

    #[test]
    fn exactly_singular_matrix_rejected() {
        let s = vec![1.0, 2.0, 2.0, 4.0];
        let v = invert_checked(&s, 2);
        assert!(!v.invertible);
        assert!(v.reason.contains("singular"));
    }

    #[test]
    fn ridge_candidate_is_explicit_and_invertible() {
        let s = vec![1.0, 0.5, 1.0, 0.5 + 1e-12];
        let reg = ridge_regularize(&s, 2, RIDGE_LAMBDA);
        let v = invert_checked(&reg, 2);
        assert!(v.invertible, "ridge-regularized matrix must invert");
    }

    #[test]
    fn boundary_points_detected_not_dropped() {
        let poly = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(classify_point(5.0, 5.0, &poly), PointPosition::Inside);
        assert_eq!(classify_point(15.0, 5.0, &poly), PointPosition::Outside);
        assert_eq!(classify_point(10.0, 5.0, &poly), PointPosition::Boundary); // on edge
        assert_eq!(classify_point(0.0, 0.0, &poly), PointPosition::Boundary); // vertex
        let m = gate_membership(&[5.0, 10.0, 15.0], &[5.0, 5.0, 5.0], &poly);
        assert_eq!(m.count, 2); // inside + boundary included
        assert_eq!(m.boundary_count, 1);
        assert_eq!(m.boundary_indices, vec![1]);
    }

    #[test]
    fn transform_keeps_nonconvertible_raw() {
        assert!(arcsinh_transform(100.0, 150.0).is_some());
        assert!(arcsinh_transform(f64::NAN, 150.0).is_none());
        assert!(arcsinh_transform(100.0, 0.0).is_none());
        // negative fluorescence is convertible under arcsinh (unlike log)
        assert!(arcsinh_transform(-5.0, 150.0).is_some());
    }
}
