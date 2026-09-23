//! Pure derivation rules for the flow-cytometry provenance bench.
//!
//! Domain invariants enforced here:
//! - a near-singular compensation (spillover) matrix must never be silently
//!   inverted; callers get an explicit singular verdict instead;
//! - extreme / saturated raw signals are kept in place with diagnostics and
//!   never normalized away for plotting;
//! - points on a gate boundary are ambiguous and yield multiple legal
//!   candidates instead of a forced in/out decision.

pub const SATURATION: f64 = 262_143.0; // 18-bit detector full scale
pub const SINGULAR_COND_THRESHOLD: f64 = 1.0e8;
pub const BOUNDARY_EPS: f64 = 0.05; // display (asinh) units

#[derive(Debug, Clone)]
pub struct MatrixAnalysis {
    pub condition: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub singular: bool,
}

/// Analyze a spillover matrix. Never silently invert: when the matrix is
/// exactly singular or its infinity-norm condition estimate exceeds the
/// threshold, `singular` is true and `inverse` is `None`.
pub fn analyze_matrix(m: &[Vec<f64>]) -> MatrixAnalysis {
    let norm_a = m
        .iter()
        .map(|r| r.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0_f64, f64::max);
    match invert(m) {
        Some(inv) => {
            let norm_inv = inv
                .iter()
                .map(|r| r.iter().map(|v| v.abs()).sum::<f64>())
                .fold(0.0_f64, f64::max);
            let condition = norm_a * norm_inv;
            let singular = !condition.is_finite() || condition > SINGULAR_COND_THRESHOLD;
            MatrixAnalysis {
                condition,
                inverse: if singular { None } else { Some(inv) },
                singular,
            }
        }
        None => MatrixAnalysis {
            condition: f64::INFINITY,
            inverse: None,
            singular: true,
        },
    }
}

/// Gauss-Jordan inverse with partial pivoting; `None` when a pivot vanishes.
fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = vec![vec![0.0; n]; n];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
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
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..n {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..n {
                        a[r][j] -= f * a[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    Some(inv)
}

/// arcsinh display transform, the standard flow-cytometry cofactor scaling.
pub fn arcsinh_transform(v: f64, cofactor: f64) -> f64 {
    (v / cofactor).asinh()
}

#[derive(Debug, Clone)]
pub struct CompEvent {
    pub values: Vec<f64>,
    pub diagnostics: Vec<String>,
    pub convertible: bool,
}

/// Apply the compensation inverse to one event's raw fluorescence values.
/// Extreme signals (saturated or negative raw values) are not normalized:
/// they stay at their raw position and carry diagnostics.
pub fn compensate_event(raw: &[f64], inv: &[Vec<f64>]) -> CompEvent {
    let mut diagnostics = Vec::new();
    for (i, v) in raw.iter().enumerate() {
        if *v >= SATURATION {
            diagnostics.push(format!("channel {i} saturated at {v} (18-bit full scale)"));
        }
        if *v < 0.0 {
            diagnostics.push(format!("channel {i} negative raw value {v}"));
        }
    }
    if !diagnostics.is_empty() {
        return CompEvent {
            values: raw.to_vec(),
            diagnostics,
            convertible: false,
        };
    }
    let n = inv.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        for j in 0..n {
            out[i] += inv[i][j] * raw[j];
        }
    }
    CompEvent {
        values: out,
        diagnostics,
        convertible: true,
    }
}

/// True when any raw value (scatter or fluorescence) is extreme.
pub fn extreme_diagnostics(values: &[f64], names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (v, name) in values.iter().zip(names.iter()) {
        if *v >= SATURATION {
            out.push(format!("{name} saturated at {v} (18-bit full scale)"));
        }
        if *v < 0.0 {
            out.push(format!("{name} negative raw value {v}"));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    In,
    Out,
    Boundary,
}

/// Point-in-polygon with an explicit boundary band. A point within `eps` of
/// any edge is `Boundary`: both `in` and `out` remain legal candidates and
/// the caller must keep all of them until a reviewer adjudicates.
pub fn classify(x: f64, y: f64, poly: &[(f64, f64)], eps: f64) -> Membership {
    if min_edge_distance(x, y, poly) <= eps {
        return Membership::Boundary;
    }
    if point_in_poly(x, y, poly) {
        Membership::In
    } else {
        Membership::Out
    }
}

fn point_in_poly(x: f64, y: f64, poly: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn min_edge_distance(x: f64, y: f64, poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    let mut best = f64::INFINITY;
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        best = best.min(point_segment_distance(x, y, ax, ay, bx, by));
    }
    best
}

fn point_segment_distance(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline_matrix() -> Vec<Vec<f64>> {
        vec![
            vec![1.0, 0.15, 0.02],
            vec![0.35, 1.0, 0.05],
            vec![0.02, 0.08, 1.0],
        ]
    }

    #[test]
    fn well_conditioned_matrix_inverts() {
        let a = analyze_matrix(&baseline_matrix());
        assert!(!a.singular);
        let inv = a.inverse.expect("inverse");
        // M * M^-1 ~= I
        for i in 0..3 {
            for j in 0..3 {
                let mut s = 0.0;
                for k in 0..3 {
                    s += baseline_matrix()[i][k] * inv[k][j];
                }
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((s - expect).abs() < 1e-9, "entry {i},{j} = {s}");
            }
        }
    }

    #[test]
    fn near_singular_matrix_is_not_silently_inverted() {
        let m = vec![
            vec![1.0, 0.999, 0.5],
            vec![0.999, 1.0, 0.5],
            vec![0.5, 0.5, 1.0],
        ];
        let a = analyze_matrix(&m);
        assert!(a.singular, "condition {}", a.condition);
        assert!(a.inverse.is_none());
    }

    #[test]
    fn exactly_singular_matrix_is_not_inverted() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let a = analyze_matrix(&m);
        assert!(a.singular);
        assert!(a.inverse.is_none());
    }

    #[test]
    fn extreme_signals_stay_raw_with_diagnostics() {
        let inv = analyze_matrix(&baseline_matrix()).inverse.unwrap();
        let r = compensate_event(&[262_143.0, 100.0, 50.0], &inv);
        assert!(!r.convertible);
        assert_eq!(r.values, vec![262_143.0, 100.0, 50.0]);
        assert!(!r.diagnostics.is_empty());
        let r = compensate_event(&[100.0, -5.0, 50.0], &inv);
        assert!(!r.convertible);
    }

    #[test]
    fn boundary_point_yields_ambiguous_membership() {
        let poly = vec![(3.0, 1.0), (6.0, 1.0), (6.0, 2.2), (3.0, 2.2)];
        assert_eq!(classify(3.001, 1.5, &poly, BOUNDARY_EPS), Membership::Boundary);
        assert_eq!(classify(4.0, 1.5, &poly, BOUNDARY_EPS), Membership::In);
        assert_eq!(classify(1.0, 1.5, &poly, BOUNDARY_EPS), Membership::Out);
    }
}
