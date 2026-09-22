//! Compensation / transform / gating domain math.
//!
//! All "risky" steps refuse to silently invent an answer:
//! - [`try_invert`] reports a near-singular spillover matrix instead of
//!   returning a numerically meaningless inverse.
//! - [`point_in_polygon`] distinguishes *strictly inside* from *exactly on
//!   the boundary* so boundary points become candidates, not silent members.

use serde_json::{json, Value};

pub const NEAR_SINGULAR_CONDITION: f64 = 1.0e8;
pub const PIVOT_FLOOR: f64 = 1.0e-10;

#[derive(Debug, Clone)]
pub struct SingularityReport {
    pub min_pivot: f64,
    pub max_pivot: f64,
    pub condition_estimate: f64,
}

impl SingularityReport {
    pub fn near_singular(&self) -> bool {
        self.min_pivot.abs() < PIVOT_FLOOR
            || self.condition_estimate > NEAR_SINGULAR_CONDITION
    }

    pub fn to_json(&self) -> Value {
        json!({
            "min_pivot": self.min_pivot,
            "max_pivot": self.max_pivot,
            "condition_estimate": self.condition_estimate,
            "threshold_condition": NEAR_SINGULAR_CONDITION,
            "pivot_floor": PIVOT_FLOOR,
        })
    }
}

/// Gaussian elimination with partial pivoting, returning the inverse or a
/// singularity report. The inverse is never returned for a (near-)singular
/// matrix; callers must record the report and keep every legal candidate.
pub fn try_invert(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, SingularityReport> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(SingularityReport {
            min_pivot: 0.0,
            max_pivot: 0.0,
            condition_estimate: f64::INFINITY,
        });
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        inv[i][i] = 1.0;
    }
    let mut min_pivot = f64::INFINITY;
    let mut max_pivot = 0.0f64;

    for col in 0..n {
        let pivot_row = (col..n)
            .max_by(|&x, &y| {
                a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap()
            })
            .unwrap();
        a.swap(col, pivot_row);
        inv.swap(col, pivot_row);
        let pivot = a[col][col];
        let scale = row_scale(&a);
        let rel = pivot.abs() / scale.max(1.0);
        min_pivot = min_pivot.min(rel);
        max_pivot = max_pivot.max(rel);
        if pivot.abs() < PIVOT_FLOOR {
            return Err(SingularityReport {
                min_pivot,
                max_pivot,
                condition_estimate: f64::INFINITY,
            });
        }
        for j in 0..n {
            a[col][j] /= pivot;
            inv[col][j] /= pivot;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            for j in 0..n {
                a[r][j] -= factor * a[col][j];
                inv[r][j] -= factor * inv[col][j];
            }
        }
    }
    let condition = if min_pivot > 0.0 && min_pivot.is_finite() {
        max_pivot / min_pivot
    } else {
        f64::INFINITY
    };
    let report = SingularityReport {
        min_pivot,
        max_pivot,
        condition_estimate: condition,
    };
    if report.near_singular() {
        return Err(report);
    }
    Ok(inv)
}

fn row_scale(a: &[Vec<f64>]) -> f64 {
    a.iter().flatten().fold(0.0f64, |acc, v| acc.max(v.abs()))
}

/// Ridge-regularised inverse `(S + lambda*I)^{-1}`: an explicitly labelled
/// candidate for near-singular spillover, never a silent replacement.
pub fn ridge_invert(m: &[Vec<f64>], lambda: f64) -> Result<Vec<Vec<f64>>, SingularityReport> {
    let n = m.len();
    let mut reg = m.to_vec();
    for i in 0..n {
        reg[i][i] += lambda;
    }
    try_invert(&reg)
}

/// Compensated row vector: `out[j] = sum_i raw[i] * m[i][j]`, carrying the
/// input channel order and units through unchanged.
pub fn compensate(row: &[f64], m: &[Vec<f64>]) -> Vec<f64> {
    (0..row.len())
        .map(|j| (0..row.len()).map(|i| row[i] * m[i][j]).sum())
        .collect()
}

/// asinh(x / cofactor); the only normalised transform layer.
pub fn asinh_value(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolygonMembership {
    Outside,
    Inside,
    Boundary,
}

/// Exact edge test runs before ray casting: a point lying on a polygon edge is
/// reported as [`PolygonMembership::Boundary`], never folded into inside/outside.
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]]) -> PolygonMembership {
    let n = poly.len();
    if n < 3 {
        return PolygonMembership::Outside;
    }
    let scale = poly
        .iter()
        .flatten()
        .fold(1.0f64, |acc, v| acc.max(v.abs()));
    let eps = scale * 1.0e-9;
    for i in 0..n {
        let (ax, ay) = (poly[i][0], poly[i][1]);
        let (bx, by) = (poly[(i + 1) % n][0], poly[(i + 1) % n][1]);
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if cross.abs() <= eps
            && x >= ax.min(bx) - eps
            && x <= ax.max(bx) + eps
            && y >= ay.min(by) - eps
            && y <= ay.max(by) + eps
        {
            return PolygonMembership::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (ax, ay) = (poly[i][0], poly[i][1]);
        let (bx, by) = (poly[j][0], poly[j][1]);
        if (ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PolygonMembership::Inside
    } else {
        PolygonMembership::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-8, "expected {b}, got {a}");
    }

    #[test]
    fn inverts_well_conditioned_matrix() {
        let m = vec![
            vec![1.0, 0.15, 0.02],
            vec![0.05, 1.0, 0.10],
            vec![0.01, 0.08, 1.0],
        ];
        let inv = try_invert(&m).expect("healthy matrix must invert");
        for i in 0..3 {
            for j in 0..3 {
                let prod: f64 = (0..3).map(|k| m[i][k] * inv[k][j]).sum();
                near(prod, if i == j { 1.0 } else { 0.0 });
            }
        }
    }

    #[test]
    fn refuses_exactly_singular_matrix() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(try_invert(&m).is_err());
    }

    #[test]
    fn refuses_near_singular_matrix() {
        // Two rows coincide to ~1e-12: a naive silent inverse is nonsense.
        let m = vec![
            vec![1.0, 0.2, 0.0],
            vec![1.0, 0.2, 1.0e-12],
            vec![0.0, 0.1, 1.0],
        ];
        let report = try_invert(&m).expect_err("near-singular must be refused");
        assert!(report.near_singular());
        // The labelled ridge candidate still exists and is usable.
        assert!(ridge_invert(&m, 1.0e-3).is_ok());
    }

    #[test]
    fn polygon_distinguishes_boundary() {
        let poly = [
            [100.0, 100.0],
            [1000.0, 100.0],
            [1000.0, 1000.0],
            [100.0, 1000.0],
        ];
        assert_eq!(point_in_polygon(500.0, 500.0, &poly), PolygonMembership::Inside);
        assert_eq!(point_in_polygon(500.0, 100.0, &poly), PolygonMembership::Boundary);
        assert_eq!(point_in_polygon(50.0, 50.0, &poly), PolygonMembership::Outside);
        assert_eq!(point_in_polygon(1000.0, 100.0, &poly), PolygonMembership::Boundary);
    }

    #[test]
    fn compensation_roundtrip() {
        let m = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let inv = try_invert(&m).unwrap();
        let row = vec![300.0, 220.0];
        let out = compensate(&row, &inv);
        // compensated then spilled again reproduces the raw measurement
        let back = compensate(&out, &m);
        near(back[0], row[0]);
        near(back[1], row[1]);
    }
}
