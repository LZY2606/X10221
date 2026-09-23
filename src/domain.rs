//! Pure domain math for the flow boundary chamber:
//! spillover/compensation matrices, transforms, gate geometry, populations.
//!
//! None of these functions touch a database or mutate inputs; they return
//! explicit candidates + diagnostics so ambiguous decisions stay visible.
use serde::Serialize;
use serde_json::Value;

pub const EPS: f64 = 1e-12;
/// Relative pivot below which a matrix is treated as *exactly* singular.
pub const SINGULAR_TOL: f64 = 1e-12;
/// Condition number at/above which a matrix is *near singular*: it may be
/// numerically invertible but the inverse must never be accepted silently.
pub const NEAR_SINGULAR_COND: f64 = 1e6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MatrixCondition {
    WellConditioned,
    NearSingular,
    Singular,
}

#[derive(Debug, Clone)]
pub struct MatrixVerdict {
    pub condition: MatrixCondition,
    pub reciprocal_condition: f64,
    pub smallest_pivot: f64,
    pub largest_pivot: f64,
    pub candidates: Vec<InverseCandidate>,
    pub code: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct InverseCandidate {
    pub id: String,
    pub strategy: String,
    pub matrix: Vec<Vec<f64>>,
    pub residual_error: f64,
}

/// Square matrix over the fluorescent channels of one spillover basis.
#[derive(Debug, Clone)]
pub struct SquareMatrix {
    pub labels: Vec<String>,
    pub data: Vec<Vec<f64>>,
}

impl SquareMatrix {
    pub fn from_json(value: &Value) -> Self {
        let labels = value["labels"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        let data = value["matrix"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| r.as_array().unwrap().iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                    .collect()
            })
            .unwrap_or_default();
        SquareMatrix { labels, data }
    }

    pub fn dim(&self) -> usize {
        self.data.len()
    }

    /// Solve `X * self = I` (i.e. returns the inverse M^{-1}) with Gaussian
    /// elimination. `pivot_eps >= 0` turns the near-zero pivot selection into a
    /// deterministic alternative, which yields a *different but legal* inverse
    /// for rank-deficient-ish matrices, exposing ambiguity instead of hiding it.
    pub fn invert(&self, pivot_eps: f64) -> Option<Vec<Vec<f64>>> {
        let n = self.dim();
        if n == 0 || self.data.iter().any(|r| r.len() != n) {
            return None;
        }
        let mut aug = vec![vec![0.0; 2 * n]; n];
        for i in 0..n {
            for j in 0..n {
                aug[i][j] = self.data[i][j];
            }
            aug[i][n + i] = 1.0;
        }
        for col in 0..n {
            // scaled partial pivot
            let mut pivot = col;
            let mut best = -1.0;
            for row in col..n {
                let scale = (0..n).map(|j| aug[row][j].abs()).fold(0.0, f64::max).max(1e-300);
                let score = aug[row][col].abs() / scale;
                if score > best {
                    best = score;
                    pivot = row;
                }
            }
            // alternative strategy: prefer a non-maximal but still >= eps pivot
            if pivot_eps > 0.0 {
                let diag_scale = (0..n).map(|j| aug[col][j].abs()).fold(0.0, f64::max).max(1e-300);
                let diag_score = aug[col][col].abs() / diag_scale;
                if pivot != col && diag_score >= pivot_eps {
                    pivot = col;
                }
            }
            if aug[pivot][col].abs() < SINGULAR_TOL {
                return None;
            }
            aug.swap(pivot, col);
            let pivot_value = aug[col][col];
            for j in 0..2 * n {
                aug[col][j] /= pivot_value;
            }
            for row in 0..n {
                if row == col {
                    continue;
                }
                let factor = aug[row][col];
                if factor.abs() < EPS {
                    continue;
                }
                for j in 0..2 * n {
                    aug[row][j] -= factor * aug[col][j];
                }
            }
        }
        let mut inverse = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                inverse[i][j] = aug[i][n + j];
            }
        }
        Some(inverse)
    }

    pub fn multiply(&self, other: &[Vec<f64>]) -> Vec<Vec<f64>> {
        let n = self.dim();
        let mut out = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                let mut acc = 0.0;
                for k in 0..n {
                    acc += self.data[i][k] * other[k][j];
                }
                out[i][j] = acc;
            }
        }
        out
    }
}

fn residual_error(m: &SquareMatrix, inverse: &[Vec<f64>]) -> f64 {
    let product = m.multiply(inverse);
    let n = m.dim();
    let mut err = 0.0;
    for i in 0..n {
        for j in 0..n {
            let expected = if i == j { 1.0 } else { 0.0 };
            err = err.max((product[i][j] - expected).abs());
        }
    }
    err
}

/// Evaluate a spillover matrix: detect exact singularity, estimate condition
/// via LU pivot ratios, and enumerate compensation inverse *candidates* rather
/// than returning a single answer.
pub fn evaluate_matrix(m: &SquareMatrix) -> MatrixVerdict {
    let n = m.dim();
    // pivot magnitudes through a plain LU without row normalisation
    let mut work = m.data.clone();
    let mut pivots = Vec::new();
    let mut singular = false;
    for col in 0..n {
        let mut pivot = col;
        let mut best = work[col][col].abs();
        for row in (col + 1)..n {
            if work[row][col].abs() > best {
                best = work[row][col].abs();
                pivot = row;
            }
        }
        if best < SINGULAR_TOL {
            singular = true;
            pivots.push(best);
            break;
        }
        work.swap(pivot, col);
        pivots.push(work[col][col].abs());
        for row in (col + 1)..n {
            let factor = work[row][col] / work[col][col];
            for j in col..n {
                work[row][j] -= factor * work[col][j];
            }
        }
    }

    if singular {
        return MatrixVerdict {
            condition: MatrixCondition::Singular,
            reciprocal_condition: 0.0,
            smallest_pivot: pivots.last().copied().unwrap_or(0.0),
            largest_pivot: pivots.first().copied().unwrap_or(0.0),
            candidates: vec![],
            code: "COMP_MATRIX_SINGULAR",
        };
    }

    let largest = pivots.iter().cloned().fold(0.0_f64, f64::max);
    let smallest = pivots.iter().cloned().fold(f64::INFINITY, f64::min);
    let reciprocal = smallest / largest;
    let condition_number = if reciprocal > 0.0 { 1.0 / reciprocal } else { f64::INFINITY };
    let near = !condition_number.is_finite() || condition_number >= NEAR_SINGULAR_COND;

    // Primary candidate: normal partial-pivot inverse.
    let mut candidates = Vec::new();
    if let Some(inv) = m.invert(0.0) {
        let err = residual_error(m, &inv);
        candidates.push(InverseCandidate {
            id: "inv_partial_pivot".into(),
            strategy: "scaled-partial-pivoting".into(),
            matrix: inv,
            residual_error: err,
        });
    }
    // Alternative legal candidate for an ill-conditioned matrix: accept the
    // diagonal pivot even when it is not the maximal one. Retained so the
    // reviewer (not a silent default) chooses which inversion is trusted.
    if near {
        if let Some(inv) = m.invert(1e-9) {
            let err = residual_error(m, &inv);
            let duplicate = candidates.iter().any(|c| matrices_equal(&c.matrix, &inv, 1e-10));
            if !duplicate {
                candidates.push(InverseCandidate {
                    id: "inv_diagonal_pivot".into(),
                    strategy: "diagonal-pivot-tolerance".into(),
                    matrix: inv,
                    residual_error: err,
                });
            }
        }
    }

    MatrixVerdict {
        condition: if near {
            MatrixCondition::NearSingular
        } else {
            MatrixCondition::WellConditioned
        },
        reciprocal_condition: reciprocal,
        smallest_pivot: smallest,
        largest_pivot: largest,
        candidates,
        code: if near {
            "COMP_MATRIX_NEAR_SINGULAR"
        } else {
            "COMP_MATRIX_OK"
        },
    }
}

fn matrices_equal(a: &[Vec<f64>], b: &[Vec<f64>], tol: f64) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(ra, rb)| {
            ra.len() == rb.len() && ra.iter().zip(rb).all(|(x, y)| (x - y).abs() <= tol)
        })
}

/// Apply compensation to one raw event row: `comp = raw * M^{-1}`.
/// Channels outside the spillover basis pass through unchanged. NaN is
/// propagated per channel and never turned into a plotted zero.
pub fn compensate_event(
    raw: &[f64],
    channel_index: &[usize],
    inverse: &[Vec<f64>],
) -> Vec<f64> {
    let mut out = raw.to_vec();
    let n = inverse.len();
    for (out_idx, &channel) in channel_index.iter().enumerate().take(n) {
        let mut acc = 0.0;
        let mut any_nan = false;
        for (k, &src) in channel_index.iter().enumerate().take(n) {
            let value = raw.get(src).copied().unwrap_or(f64::NAN);
            if value.is_nan() {
                any_nan = true;
            }
            acc += value * inverse[k][out_idx];
        }
        out[channel] = if any_nan { f64::NAN } else { acc };
    }
    out
}

/// Inverse hyperbolic sine transform (flow cytometry "logicle-lite"), applied
/// per channel with its own cofactor. Cofanctors carry explicit units.
pub fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    if value.is_nan() {
        return f64::NAN;
    }
    (value / cofactor.max(1e-12)).asinh()
}

// ---------------------------------------------------------------------------
// Gate geometry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BoundaryStatus {
    Inside,
    Outside,
    Boundary,
}

/// Point-in-convex-or-simple polygon. Boundary points are returned as
/// `Boundary` rather than forced in/out, because gating ties cannot be
/// resolved without a stated tie-breaking convention.
pub fn polygon_status(
    point: (f64, f64),
    vertices: &[(f64, f64)],
    boundary_tol: f64,
) -> BoundaryStatus {
    let (x, y) = point;
    if x.is_nan() || y.is_nan() {
        return BoundaryStatus::Outside;
    }
    let n = vertices.len();
    if n < 3 {
        return BoundaryStatus::Outside;
    }
    // edge test first
    for i in 0..n {
        let (ax, ay) = vertices[i];
        let (bx, by) = vertices[(i + 1) % n];
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if cross.abs() <= boundary_tol {
            let on_segment =
                x >= ax.min(bx) - boundary_tol && x <= ax.max(bx) + boundary_tol
                && y >= ay.min(by) - boundary_tol && y <= ay.max(by) + boundary_tol;
            if on_segment {
                return BoundaryStatus::Boundary;
            }
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = vertices[i];
        let (xj, yj) = vertices[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        BoundaryStatus::Inside
    } else {
        BoundaryStatus::Outside
    }
}

/// Enumerate the legal membership resolutions for one gate. An unambiguous
/// point yields one candidate; a boundary point yields both `included` and
/// `excluded`, both marked tie, so nothing unique is silently decided.
pub fn membership_candidates(status: BoundaryStatus) -> Vec<(bool, bool)> {
    match status {
        BoundaryStatus::Inside => vec![(true, false)],
        BoundaryStatus::Outside => vec![(false, false)],
        BoundaryStatus::Boundary => vec![(true, true), (false, true)],
    }
}
