//! Dense matrix algebra used for spillover/compensation.
//!
//! A near-singular spillover matrix MUST NOT be inverted silently: callers get
//! back a [`CompCandidates`] set of *legal* alternatives plus a condition
//! estimate, and an exactly singular matrix yields [`Singular`].

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    /// Row-major `rows * cols`.
    pub data: Vec<f64>,
}

/// Pivot smaller than this fraction of the column scale is treated as
/// "near singular" even when it is not exactly zero.
pub const NEAR_PIVOT: f64 = 1e-8;

#[derive(Debug)]
pub struct Singular {
    pub zero_pivot: usize,
}

impl Mat {
    pub fn new(rows: usize, cols: usize, data: Vec<f64>) -> Self {
        assert_eq!(data.len(), rows * cols);
        Mat { rows, cols, data }
    }

    pub fn from_rows(rows: &[Vec<f64>]) -> Self {
        let r = rows.len();
        let c = rows.first().map(|x| x.len()).unwrap_or(0);
        let mut data = Vec::with_capacity(r * c);
        for row in rows {
            assert_eq!(row.len(), c);
            data.extend_from_slice(row);
        }
        Mat::new(r, c, data)
    }

    pub fn identity(n: usize) -> Self {
        let mut data = vec![0.0; n * n];
        for i in 0..n {
            data[i * n + i] = 1.0;
        }
        Mat::new(n, n, data)
    }

    #[inline]
    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    pub fn inf_norm(&self) -> f64 {
        let mut m = 0.0;
        for r in 0..self.rows {
            let s: f64 = (0..self.cols).map(|c| self.at(r, c).abs()).sum();
            m = m.max(s);
        }
        m
    }

    pub fn mul(&self, other: &Mat) -> Mat {
        assert_eq!(self.cols, other.rows);
        let mut out = vec![0.0; self.rows * other.cols];
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self.at(i, k);
                if a == 0.0 {
                    continue;
                }
                for j in 0..other.cols {
                    out[i * other.cols + j] += a * other.at(k, j);
                }
            }
        }
        Mat::new(self.rows, other.cols, out)
    }

    pub fn mul_vec(&self, v: &[f64]) -> Vec<f64> {
        assert_eq!(self.cols, v.len());
        (0..self.rows)
            .map(|i| (0..self.cols).map(|j| self.at(i, j) * v[j]).sum())
            .collect()
    }

    /// Determinant through LU decomposition with partial pivoting.
    pub fn determinant(&self) -> f64 {
        assert_eq!(self.rows, self.cols);
        let n = self.rows;
        let mut a = self.data.clone();
        let mut det = 1.0;
        for col in 0..n {
            let mut piv = col;
            let mut best = a[col * n + col].abs();
            for r in (col + 1)..n {
                let v = a[r * n + col].abs();
                if v > best {
                    best = v;
                    piv = r;
                }
            }
            if best == 0.0 {
                return 0.0;
            }
            if piv != col {
                for k in 0..n {
                    a.swap(col * n + k, piv * n + k);
                }
                det = -det;
            }
            let pivot = a[col * n + col];
            det *= pivot;
            for r in (col + 1)..n {
                let f = a[r * n + col] / pivot;
                a[r * n + col] = f;
                for k in (col + 1)..n {
                    a[r * n + k] -= f * a[col * n + k];
                }
            }
        }
        det
    }

    /// Gaussian-Jordan inverse with partial pivoting.
    /// Returns the inverse and the smallest relative pivot (a condition proxy).
    pub fn inverse(&self) -> Result<(Mat, f64), Singular> {
        assert_eq!(self.rows, self.cols);
        let n = self.rows;
        let mut a = self.data.clone();
        let mut inv = Mat::identity(n).data;
        let mut col_scale = vec![0.0f64; n];
        for c in 0..n {
            for r in 0..n {
                col_scale[c] = col_scale[c].max(a[r * n + c].abs());
            }
            if col_scale[c] == 0.0 {
                col_scale[c] = 1.0;
            }
        }
        let mut min_rel_pivot = f64::INFINITY;
        for col in 0..n {
            let mut piv = col;
            let mut best = a[col * n + col].abs();
            for r in (col + 1)..n {
                let v = a[r * n + col].abs();
                if v > best {
                    best = v;
                    piv = r;
                }
            }
            if best == 0.0 {
                return Err(Singular { zero_pivot: col });
            }
            min_rel_pivot = min_rel_pivot.min(best / col_scale[col]);
            if piv != col {
                for k in 0..n {
                    a.swap(col * n + k, piv * n + k);
                    inv.swap(col * n + k, piv * n + k);
                }
            }
            let pivot = a[col * n + col];
            for k in 0..n {
                a[col * n + k] /= pivot;
                inv[col * n + k] /= pivot;
            }
            for r in 0..n {
                if r == col {
                    continue;
                }
                let f = a[r * n + col];
                if f == 0.0 {
                    continue;
                }
                for k in 0..n {
                    a[r * n + k] -= f * a[col * n + k];
                    inv[r * n + k] -= f * inv[col * n + k];
                }
            }
        }
        Ok((Mat::new(n, n, inv), min_rel_pivot))
    }

    /// Reciprocal condition estimate: 1 / (||A||_inf * ||A^-1||_inf).
    /// Smaller => closer to singular.
    pub fn rcond(&self) -> f64 {
        match self.inverse() {
            Ok((inv, _)) => {
                let na = self.inf_norm();
                let ni = inv.inf_norm();
                if na == 0.0 || ni.is_infinite() {
                    0.0
                } else {
                    1.0 / (na * ni)
                }
            }
            Err(_) => 0.0,
        }
    }

    /// Tikhonov/ridge stabilised solve: (A + lambda*diag(scale))^-1.
    pub fn ridge_inverse(lambda_rel: f64) -> impl Fn(&Mat) -> Mat {
        move |m: &Mat| {
            let n = m.rows;
            let mut reg = m.data.clone();
            for i in 0..n {
                let scale = (0..n).fold(0.0f64, |acc, j| acc.max(m.at(i, j).abs())).max(1.0);
                reg[i * n + i] += lambda_rel * scale;
            }
            Mat::new(n, n, reg).inverse().expect("ridge matrix invertible").0
        }
    }

    /// Nullspace-clamped pseudo solve: zero the inverse action along the
    /// near-dependant pivot direction instead of blowing it up.
    pub fn clamped_inverse(&self) -> Mat {
        let n = self.rows;
        let mut a = self.data.clone();
        let mut inv = Mat::identity(n).data;
        let mut col_scale = vec![0.0f64; n];
        for c in 0..n {
            for r in 0..n {
                col_scale[c] = col_scale[c].max(a[r * n + c].abs());
            }
            if col_scale[c] == 0.0 {
                col_scale[c] = 1.0;
            }
        }
        let mut dead_cols = Vec::new();
        for col in 0..n {
            let mut piv = col;
            let mut best = a[col * n + col].abs();
            for r in (col + 1)..n {
                let v = a[r * n + col].abs();
                if v > best {
                    best = v;
                    piv = r;
                }
            }
            let rel = best / col_scale[col];
            if piv != col {
                for k in 0..n {
                    a.swap(col * n + k, piv * n + k);
                    inv.swap(col * n + k, piv * n + k);
                }
            }
            if best == 0.0 || rel < NEAR_PIVOT {
                // Clamp this direction: zero the row in both factors.
                for k in 0..n {
                    a[col * n + k] = 0.0;
                    inv[col * n + k] = 0.0;
                }
                dead_cols.push(col);
                continue;
            }
            let pivot = a[col * n + col];
            for k in 0..n {
                a[col * n + k] /= pivot;
                inv[col * n + k] /= pivot;
            }
            for r in 0..n {
                if r == col {
                    continue;
                }
                let f = a[r * n + col];
                if f == 0.0 {
                    continue;
                }
                for k in 0..n {
                    a[r * n + k] -= f * a[col * n + k];
                    inv[r * n + k] -= f * inv[col * n + k];
                }
            }
        }
        let _ = dead_cols;
        Mat::new(n, n, inv)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub formula: String,
    pub matrix: Mat,
    pub residual_norm: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompCandidates {
    pub rcond: f64,
    pub min_rel_pivot: f64,
    pub candidates: Vec<Candidate>,
}

/// Try to invert a spillover matrix.
///
/// * exactly singular  -> `Err(Singular)`
/// * near singular (smallest relative pivot below [`NEAR_PIVOT`]) ->
///   `Ok(None)` plus the full set of legal candidates; the caller must pick
///   one explicitly (or reject), no silent inverse is returned
/// * well conditioned -> `Ok((Some(inverse), diagnostics))`
pub fn invert_spillover(spill: &Mat) -> Result<(Option<Mat>, CompCandidates), Singular> {
    let (plain, min_rel) = spill.inverse()?;
    let rcond = spill.rcond();
    let diag = CompCandidates {
        rcond,
        min_rel_pivot: min_rel,
        candidates: build_candidates(spill),
    };
    if min_rel < NEAR_PIVOT {
        return Ok((None, diag));
    }
    Ok((Some(plain), diag))
}

fn build_candidates(spill: &Mat) -> Vec<Candidate> {
    let identity = Mat::identity(spill.rows);
    let mk = |id: &str, label: &str, formula: &str, m: Mat| {
        let back = spill.mul(&m);
        let mut diff = 0.0f64;
        for i in 0..spill.rows {
            for j in 0..spill.cols {
                diff += (back.at(i, j) - identity.at(i, j)).powi(2);
            }
        }
        Candidate {
            id: id.to_string(),
            label: label.to_string(),
            formula: formula.to_string(),
            matrix: m,
            residual_norm: diff.sqrt(),
        }
    };
    vec![
        mk(
            "ridge-weak",
            "Ridge λ=1e-6·scale",
            "(S + 1e-6·diag(scale))^-1",
            Mat::ridge_inverse(1e-6)(spill),
        ),
        mk(
            "ridge-strong",
            "Ridge λ=1e-2·scale",
            "(S + 1e-2·diag(scale))^-1",
            Mat::ridge_inverse(1e-2)(spill),
        ),
        mk(
            "null-clamped",
            "Null-direction clamped",
            "zero inverse action on near-dependant axis",
            spill.clamped_inverse(),
        ),
    ]
}
