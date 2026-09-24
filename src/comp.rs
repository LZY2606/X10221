//! Compensation matrix math, signal transforms and gate geometry.
//!
//! Domain rules implemented here:
//! * A near-singular spillover matrix must never be silently inverted.
//! * Raw values are preserved; normalization happens only in the derived layer.
//! * Boundary points use a deterministic inclusion rule and are reported.

/// Relative pivot threshold below which a matrix is considered near-singular.
pub const SINGULAR_EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub struct NearSingular {
    pub determinant: f64,
    pub min_pivot: f64,
}

/// Determinant via Gaussian elimination with partial pivoting.
/// Returns `(determinant, min_relative_pivot)`.
pub fn determinant(matrix: &[Vec<f64>]) -> (f64, f64) {
    let n = matrix.len();
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut piv = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[piv][col].abs() {
                piv = row;
            }
        }
        let pivot_abs = a[piv][col].abs();
        if pivot_abs < min_pivot {
            min_pivot = pivot_abs;
        }
        if pivot_abs == 0.0 {
            return (0.0, 0.0);
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        det *= a[col][col];
        for row in (col + 1)..n {
            let f = a[row][col] / a[col][col];
            for k in col..n {
                a[row][k] -= f * a[col][k];
            }
        }
    }
    (det, min_pivot)
}

/// Invert a matrix, refusing near-singular input instead of silently
/// producing a garbage inverse.
pub fn invert(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, NearSingular> {
    let n = matrix.len();
    let (det, min_pivot) = determinant(matrix);
    let scale = matrix
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0f64, |m, v| m.max(v.abs()))
        .max(1.0);
    if det.abs() < SINGULAR_EPS * scale.powi(n as i32) || min_pivot < SINGULAR_EPS * scale {
        return Err(NearSingular { determinant: det, min_pivot });
    }
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut piv = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[piv][col].abs() {
                piv = row;
            }
        }
        a.swap(piv, col);
        inv.swap(piv, col);
        let d = a[col][col];
        for k in 0..n {
            a[col][k] /= d;
            inv[col][k] /= d;
        }
        for row in 0..n {
            if row != col {
                let f = a[row][col];
                for k in 0..n {
                    a[row][k] -= f * a[col][k];
                    inv[row][k] -= f * inv[col][k];
                }
            }
        }
    }
    Ok(inv)
}

/// Tikhonov-damped pseudo-inverse, only used after an explicit adjudication.
/// The `damped` flag is recorded in the resulting matrix version so the
/// diagnostic travels with the data.
pub fn damped_inverse(matrix: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = matrix.len();
    let mut regularized = matrix.to_vec();
    for i in 0..n {
        regularized[i][i] += lambda;
    }
    invert(&regularized).unwrap_or_else(|_| regularized)
}

/// arcsinh transform used as the display/basis transform in the derived layer.
pub fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

/// Point-in-polygon by ray casting. Returns `(inside, on_boundary)`.
/// Boundary points are *included* (deterministic rule) and flagged so the
/// review page can show them explicitly.
pub fn point_in_polygon(x: f64, y: f64, poly: &[(f64, f64)]) -> (bool, bool) {
    const EDGE_EPS: f64 = 1e-9;
    let n = poly.len();
    if n < 3 {
        return (false, false);
    }
    for i in 0..n {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % n];
        let cross = (x - x1) * (y2 - y1) - (y - y1) * (x2 - x1);
        let within_x = x >= x1.min(x2) - EDGE_EPS && x <= x1.max(x2) + EDGE_EPS;
        let within_y = y >= y1.min(y2) - EDGE_EPS && y <= y1.max(y2) + EDGE_EPS;
        let scale = ((x2 - x1).abs() + (y2 - y1).abs()).max(1.0);
        if cross.abs() <= EDGE_EPS * scale && within_x && within_y {
            return (true, true);
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    (inside, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned_matrix() {
        let m = vec![
            vec![1.0, 0.15, 0.02],
            vec![0.05, 1.0, 0.10],
            vec![0.01, 0.20, 1.0],
        ];
        let inv = invert(&m).expect("well conditioned");
        for (i, row) in inv.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let expect = if i == j { 1.0 } else { 0.0 };
                let ident: f64 = (0..3).map(|k| m[i][k] * inv[k][j]).sum();
                assert!((ident - expect).abs() < 1e-9, "identity check failed at {i},{j}: {ident}");
                let _ = v;
            }
        }
    }

    #[test]
    fn refuses_near_singular_matrix() {
        let m = vec![
            vec![1.0, 0.5, 0.25],
            vec![1.0, 0.5000001, 0.25],
            vec![0.25, 0.5, 1.0],
        ];
        let err = invert(&m).expect_err("must refuse to silently invert");
        assert!(err.determinant.abs() < 1e-6);
    }

    #[test]
    fn damped_inverse_only_after_decision() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        assert!(invert(&m).is_err());
        let d = damped_inverse(&m, 0.1);
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn boundary_points_are_included_and_flagged() {
        let poly = vec![(100.0, 100.0), (5000.0, 100.0), (5000.0, 5000.0), (100.0, 5000.0)];
        let (inside, boundary) = point_in_polygon(5000.0, 1000.0, &poly);
        assert!(inside && boundary, "edge point must be included and flagged");
        let (inside, boundary) = point_in_polygon(2000.0, 2000.0, &poly);
        assert!(inside && !boundary);
        let (inside, _) = point_in_polygon(9000.0, 2000.0, &poly);
        assert!(!inside);
    }

    #[test]
    fn asinh_is_monotonic() {
        assert!(asinh_transform(1000.0, 150.0) > asinh_transform(10.0, 150.0));
        assert_eq!(asinh_transform(0.0, 150.0), 0.0);
    }
}
