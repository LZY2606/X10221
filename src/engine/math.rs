//! Domain math: spillover/compensation, bi-exponential (logicle-like) transform,
//! convex polygon membership with explicit boundary semantics.
//!
//! Every numeric helper uses f64 internally; f32 values imported from FCS are
//! widened losslessly and the original representation is kept by the caller.

use serde::{Deserialize, Serialize};

/// Condition-number threshold above which a spillover matrix is treated as
/// near-singular and must NOT be silently inverted.
pub const SINGULAR_RCOND: f64 = 1e-3;
/// Relative tolerance used for boundary classification.
pub const EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatrixStatus {
    Invertible,
    NearSingular,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixReport {
    pub status: MatrixStatus,
    /// 1 / inf-norm condition number; small means near-singular.
    pub rcond: f64,
    pub determinant: f64,
    pub reason: String,
}

fn inf_norm(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .map(|row| row.iter().map(|v| v.abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = vec![vec![0.0f64; n]; n];
    for (i, row) in inv.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())
            .unwrap();
        if a[pivot][col].abs() < 1e-15 {
            return None;
        }
        a.swap(pivot, col);
        inv.swap(pivot, col);
        let p = a[col][col];
        for k in 0..n {
            a[col][k] /= p;
            inv[col][k] /= p;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor == 0.0 {
                continue;
            }
            for k in 0..n {
                a[row][k] -= factor * a[col][k];
                inv[row][k] -= factor * inv[col][k];
            }
        }
    }
    Some(inv)
}

fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0;
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())
            .unwrap();
        if a[pivot][col].abs() < 1e-15 {
            return 0.0;
        }
        if pivot != col {
            a.swap(pivot, col);
            det = -det;
        }
        let p = a[col][col];
        det *= p;
        for row in (col + 1)..n {
            let factor = a[row][col] / p;
            for k in col..n {
                a[row][k] -= factor * a[col][k];
            }
        }
    }
    det
}

/// Gauss-Jordan inversion with an explicit reciprocal-condition estimate so
/// near-singular compensation matrices never get silently inverted.
pub fn analyze_matrix(m: &[Vec<f64>]) -> MatrixReport {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return MatrixReport {
            status: MatrixStatus::NearSingular,
            rcond: 0.0,
            determinant: 0.0,
            reason: "matrix is not square".to_string(),
        };
    }
    let det = determinant(m);
    if det.abs() < 1e-12 {
        return MatrixReport {
            status: MatrixStatus::NearSingular,
            rcond: 0.0,
            determinant: det,
            reason: format!("determinant {det:.3e} below 1e-12"),
        };
    }
    let inv = match invert(m) {
        Some(v) => v,
        None => {
            return MatrixReport {
                status: MatrixStatus::NearSingular,
                rcond: 0.0,
                determinant: det,
                reason: "Gauss-Jordan elimination failed".to_string(),
            }
        }
    };
    let norm = inf_norm(m);
    let norm_inv = inf_norm(&inv);
    let rcond = if norm > 0.0 && norm_inv > 0.0 {
        1.0 / (norm * norm_inv)
    } else {
        0.0
    };
    let status = if rcond < SINGULAR_RCOND {
        MatrixStatus::NearSingular
    } else {
        MatrixStatus::Invertible
    };
    let reason = match status {
        MatrixStatus::Invertible => format!("rcond {rcond:.3e} >= {SINGULAR_RCOND}"),
        MatrixStatus::NearSingular => {
            format!("rcond {rcond:.3e} < {SINGULAR_RCOND}; inversion withheld")
        }
    };
    MatrixReport {
        status,
        rcond,
        determinant: det,
        reason,
    }
}

pub fn invert_matrix(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    match analyze_matrix(m).status {
        MatrixStatus::Invertible => invert(m),
        MatrixStatus::NearSingular => None,
    }
}

/// A single ridge-regularized candidate offered when the spillover matrix is
/// near-singular. `lambda` is the ridge strength; residuals let the reviewer
/// judge how much the candidate violates exact compensation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RidgeCandidate {
    pub id: String,
    pub label: String,
    pub lambda: f64,
    pub matrix: Vec<Vec<f64>>,
    pub max_residual: f64,
    pub rcond: f64,
}

pub fn ridge_candidates(m: &[Vec<f64>]) -> Vec<RidgeCandidate> {
    let n = m.len();
    [1e-3f64, 1e-2, 1e-1]
        .iter()
        .map(|&lambda| {
            let reg: Vec<Vec<f64>> = (0..n)
                .map(|i| {
                    (0..n)
                        .map(|j| if i == j { m[i][j] + lambda } else { m[i][j] })
                        .collect()
                })
                .collect();
            let inv = invert(&reg).expect("regularized matrix is invertible");
            let report = analyze_matrix(&reg);
            let mut max_residual = 0.0;
            for i in 0..n {
                for j in 0..n {
                    let s: f64 = (0..n).map(|k| inv[i][k] * m[k][j]).sum();
                    let target = if i == j { 1.0 } else { 0.0 };
                    max_residual = max_residual.max((s - target).abs());
                }
            }
            RidgeCandidate {
                id: format!("ridge-{lambda:.0e}"),
                label: format!("ridge regression λ={lambda}"),
                lambda,
                matrix: inv,
                max_residual,
                rcond: report.rcond,
            }
        })
        .collect()
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
        .collect()
}

/// Logicle-like bi-exponential transform (asinh parameterization).
/// x: signal in the same unit as `top` (usually raw ADC/event counts).
/// Returns a dimension-less display coordinate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LogicleParams {
    pub t: f64,
    pub w: f64,
    pub m: f64,
    pub a: f64,
}

impl Default for LogicleParams {
    fn default() -> Self {
        LogicleParams {
            t: 262_144.0,
            w: 0.5,
            m: 4.5,
            a: 0.0,
        }
    }
}

impl LogicleParams {
    /// `p` is the top-of-scale exponent; A reserves extra top decades.
    pub fn p_val(&self) -> f64 {
        (self.m - self.a) * 10f64.ln()
    }
    /// `c` sets the negative linearization width W (Parks-style logicle family).
    pub fn c_val(&self) -> f64 {
        (-self.p_val() * self.w / self.m).exp()
    }
    fn denom(&self) -> f64 {
        let p = self.p_val();
        let c2 = self.c_val().powi(2);
        p.exp() - c2 * (-p).exp() + c2 - 1.0
    }
    /// Normalized bi-exponential scale: xnorm(y_raw), xnorm(0)=0, xnorm(1)=1.
    fn xnorm(&self, y: f64) -> f64 {
        let p = self.p_val();
        let c2 = self.c_val().powi(2);
        let num = (p * y).exp() - c2 * (-p * y).exp() + c2 - 1.0;
        num / self.denom()
    }
    pub fn transform(&self, x: f64) -> f64 {
        // Invert the monotone bi-exponential scale by Newton steps on y_raw,
        // then report the dimensionless "decade" display coordinate M*y_raw.
        let target = x / self.t;
        let p = self.p_val();
        let c2 = self.c_val().powi(2);
        let denom = self.denom();
        // xnorm maps y in roughly [-0.75, 1]; clamp each Newton step.
        let mut y = target.clamp(-0.9, 1.2);
        for _ in 0..60 {
            let e1 = (p * y).exp();
            let e2 = (-p * y).exp();
            let f = (e1 - c2 * e2 + c2 - 1.0) / denom - target;
            let df = p * (e1 + c2 * e2) / denom;
            let step = f / df;
            y = (y - step).clamp(-0.9, 1.2);
            if step.abs() < 1e-13 {
                break;
            }
        }
        self.m * y
    }
    pub fn coordinate_basis(&self) -> String {
        "logicle-family bi-exponential display coordinate (dimensionless decades); scale normalized xnorm(0)=0 xnorm(1)=T; params T/W/M/A".to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundaryKind {
    Inside,
    Boundary,
    Outside,
}

/// Classify point against a simple convex polygon (crossing number for
/// generality; vertices assumed ordered, either orientation). Boundary points
/// are reported explicitly rather than coerced to inside/outside.
pub fn classify_polygon(point: (f64, f64), poly: &[(f64, f64)]) -> BoundaryKind {
    if poly.len() < 3 {
        return BoundaryKind::Outside;
    }
    let (px, py) = point;
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        let cross = (px - ax) * (by - ay) - (py - ay) * (bx - ax);
        let edge_len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let tol = EPS * (1.0 + edge_len + ax.abs().max(ay.abs()));
        if cross.abs() <= tol {
            let dot = (px - ax) * (bx - ax) + (py - ay) * (by - ay);
            let on_segment = dot >= -tol && dot <= edge_len * edge_len + tol;
            if on_segment {
                return BoundaryKind::Boundary;
            }
        }
        let intersects = ((ay > py) != (by > py))
            && (px < (bx - ax) * (py - ay) / (by - ay) + ax);
        if intersects {
            inside = !inside;
        }
    }
    if inside {
        BoundaryKind::Inside
    } else {
        BoundaryKind::Outside
    }
}

/// Gate inclusion rule used for percentages: boundary points are included
/// (closed gate). The classification itself is kept visible in the review UI.
pub fn included(kind: BoundaryKind) -> bool {
    matches!(kind, BoundaryKind::Inside | BoundaryKind::Boundary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invertible_matrix_roundtrip() {
        let m = vec![vec![1.0, 0.05], vec![0.02, 1.0]];
        let rep = analyze_matrix(&m);
        assert_eq!(rep.status, MatrixStatus::Invertible);
        let inv = invert_matrix(&m).unwrap();
        let v = vec![100.0, 40.0];
        let comp = mat_vec(&inv, &v);
        assert!((comp[0] - 98.0).abs() < 1e-9);
    }

    #[test]
    fn near_singular_is_withheld() {
        let m = vec![vec![1.0, 0.9999], vec![1.0, 1.0]];
        let rep = analyze_matrix(&m);
        assert_eq!(rep.status, MatrixStatus::NearSingular);
        assert!(invert_matrix(&m).is_none());
        let cands = ridge_candidates(&m);
        assert_eq!(cands.len(), 3);
        assert!(cands[0].max_residual < cands[2].max_residual + EPS);
    }

    #[test]
    fn boundary_classification() {
        let poly = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(classify_polygon((5.0, 5.0), &poly), BoundaryKind::Inside);
        assert_eq!(classify_polygon((10.0, 5.0), &poly), BoundaryKind::Boundary);
        assert_eq!(classify_polygon((0.0, 0.0), &poly), BoundaryKind::Boundary);
        assert_eq!(classify_polygon((11.0, 5.0), &poly), BoundaryKind::Outside);
    }

    #[test]
    fn logicle_monotone_and_zero() {
        let p = LogicleParams::default();
        let z = p.transform(0.0);
        assert!(p.transform(1000.0) > z);
        assert!(p.transform(-100.0) < z);
        assert!(z.is_finite());
        assert!(z.abs() < 1e-9);
        let top = p.transform(p.t);
        assert!((top - p.m).abs() < 1e-7, "top maps to M: {top} vs {}", p.m);
        for &xv in &[-500.0, -10.0, 0.0, 1.0, 100.0, 10_000.0, 262_144.0] {
            let y = p.transform(xv);
            assert!(y.is_finite(), "finite at {xv}");
        }
        assert!(p.transform(50.0) - p.transform(49.0) > 0.0);
    }
}
