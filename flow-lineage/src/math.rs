//! Numerical primitives for compensation and polygon gating.
//!
//! Near-singular compensation matrices must never be silently inverted:
//! callers first inspect [`Matrix::rcond`] and refuse the operation.

#[derive(Clone, Debug)]
pub struct Matrix {
    pub n: usize,
    /// row-major n*n
    pub a: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatrixProblem {
    NotSquare,
    Singular { index: usize },
}

impl Matrix {
    pub fn new(a: Vec<Vec<f64>>) -> Self {
        let n = a.len();
        Matrix { n, a: a.into_iter().flatten().collect() }
    }

    pub fn row(&self, i: usize) -> &[f64] {
        &self.a[i * self.n..(i + 1) * self.n]
    }

    fn lu(&self) -> Result<(Vec<f64>, Vec<usize>), MatrixProblem> {
        let n = self.n;
        if self.a.len() != n * n {
            return Err(MatrixProblem::NotSquare);
        }
        let mut lu = self.a.clone();
        let mut piv: Vec<usize> = (0..n).collect();
        for k in 0..n {
            let mut best = k;
            let mut bestv = lu[k * n + k].abs();
            for i in (k + 1)..n {
                let v = lu[i * n + k].abs();
                if v > bestv {
                    bestv = v;
                    best = i;
                }
            }
            if bestv < 1e-15 {
                return Err(MatrixProblem::Singular { index: k });
            }
            if best != k {
                piv.swap(k, best);
                for j in 0..n {
                    lu.swap(k * n + j, best * n + j);
                }
            }
            for i in (k + 1)..n {
                let f = lu[i * n + k] / lu[k * n + k];
                lu[i * n + k] = f;
                for j in (k + 1)..n {
                    lu[i * n + j] -= f * lu[k * n + j];
                }
            }
        }
        Ok((lu, piv))
    }

    pub fn determinant(&self) -> Result<f64, MatrixProblem> {
        let (lu, piv) = self.lu()?;
        let mut swaps = 0;
        let mut seen = vec![false; self.n];
        for (i, &p) in piv.iter().enumerate() {
            if !seen[p] {
                let mut j = p;
                while j != i && !seen[j] {
                    seen[j] = true;
                    swaps += 1;
                    j = piv[j];
                }
            }
            seen[p] = true;
        }
        let mut det = if swaps % 2 == 0 { 1.0 } else { -1.0 };
        for i in 0..self.n {
            det *= lu[i * self.n + i];
        }
        Ok(det)
    }

    /// Reciprocal 1-norm condition number via explicit inverse.
    /// Returns 0.0 when the matrix is exactly singular.
    pub fn rcond(&self) -> f64 {
        let n = self.n;
        let inv = match self.inverse() {
            Some(m) => m,
            None => return 0.0,
        };
        let norm_a = (0..n)
            .map(|j| (0..n).map(|i| self.a[i * n + j].abs()).sum::<f64>())
            .fold(0.0f64, f64::max);
        let norm_inv = (0..n)
            .map(|j| (0..n).map(|i| inv[i * n + j].abs()).sum::<f64>())
            .fold(0.0f64, f64::max);
        if norm_a == 0.0 || norm_inv.is_infinite() {
            return 0.0;
        }
        1.0 / (norm_a * norm_inv)
    }

    pub fn inverse(&self) -> Option<Vec<f64>> {
        let n = self.n;
        let (lu, piv) = self.lu().ok()?;
        let mut inv = vec![0.0; n * n];
        for col in 0..n {
            let mut b = vec![0.0; n];
            b[col] = 1.0;
            // forward solve L y = P b
            let mut y = vec![0.0; n];
            for i in 0..n {
                let mut s = b[piv[i]];
                for j in 0..i {
                    s -= lu[i * n + j] * y[j];
                }
                y[i] = s;
            }
            // back solve U x = y
            let mut x = vec![0.0; n];
            for i in (0..n).rev() {
                let mut s = y[i];
                for j in (i + 1)..n {
                    s -= lu[i * n + j] * x[j];
                }
                let d = lu[i * n + n - 1];
                if d.abs() < 1e-30 {
                    return None;
                }
                x[i] = s / lu[i * n + i];
            }
            for i in 0..n {
                inv[i * n + col] = x[i];
            }
        }
        Some(inv)
    }

    /// Solve `A x = b`. Errors if singular; callers must gate on rcond first.
    pub fn solve(&self, b: &[f64]) -> Result<Vec<f64>, MatrixProblem> {
        let (lu, piv) = self.lu()?;
        let n = self.n;
        let mut y = vec![0.0; n];
        for i in 0..n {
            let mut s = b[piv[i]];
            for j in 0..i {
                s -= lu[i * n + j] * y[j];
            }
            y[i] = s;
        }
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let mut s = y[i];
            for j in (i + 1)..n {
                s -= lu[i * n + j] * x[j];
            }
            x[i] = s / lu[i * n + i];
        }
        Ok(x)
    }
}

/// Inverse hyperbolic sine used for biexponential-like display transform.
pub fn asinh(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Boundary {
    Inclusive,
    Exclusive,
}

/// Point-in-polygon with exact boundary control.
pub fn point_in_polygon(
    point: (f64, f64),
    polygon: &[(f64, f64)],
    boundary: Boundary,
    eps: f64,
) -> bool {
    let (px, py) = point;
    let mut inside = false;
    let mut n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        let on_edge = point_on_segment(px, py, xi, yi, xj, yj, eps);
        if on_edge {
            return boundary == Boundary::Inclusive;
        }
        if ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / (yj - yi) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    let _ = &mut n;
    inside
}

fn point_on_segment(
    px: f64,
    py: f64,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    eps: f64,
) -> bool {
    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    if cross.abs() > eps * (1.0 + (x2 - x1).abs().max((y2 - y1).abs())) {
        return false;
    }
    let dot = (px - x1) * (px - x2) + (py - y1) * (py - y2);
    dot <= eps * eps
}
