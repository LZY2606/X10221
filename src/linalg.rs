//! Small dense linear algebra for spillover compensation.
//!
//! Near-singular spillover matrices must never be silently inverted:
//! `invert_strict` returns `NearSingular` (or `Singular`) with a measured
//! reciprocal condition number, and callers must then build explicit
//! truncated-SVD candidates for human review.

#[derive(Debug, Clone, PartialEq)]
pub enum InvertStatus {
    Determinate,
    NearSingular { rcond: f64 },
    Singular { rcond: f64 },
}

#[derive(Debug, Clone)]
pub struct InvertResult {
    pub inv: Vec<Vec<f64>>,
    pub rcond: f64,
    pub status: InvertStatus,
}

pub fn norm1(a: &[Vec<f64>]) -> f64 {
    if a.is_empty() { return 0.0; }
    let n = a.len();
    let m = a[0].len();
    (0..m).map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>()).fold(0.0, f64::max)
}

/// Gauss-Jordan inversion with explicit pivot tracking.
/// `rcond` is the reciprocal 1-norm condition estimate 1/(||A||1 ||A^-1||1).
pub fn invert_strict(a: &[Vec<f64>], rcond_threshold: f64) -> Result<InvertResult, InvertStatus> {
    let n = a.len();
    for row in a {
        if row.len() != n {
            return Err(InvertStatus::Singular { rcond: 0.0 });
        }
    }
    let mut m = vec![vec![0.0; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            m[i][j] = a[i][j];
        }
        m[i][n + i] = 1.0;
    }
    let mut pivots = Vec::with_capacity(n);
    for col in 0..n {
        let piv = (col..n).max_by(|x, y| m[*x][col].abs().partial_cmp(&m[*y][col].abs()).unwrap()).unwrap();
        let p = m[piv][col];
        if !p.is_finite() {
            return Err(InvertStatus::Singular { rcond: 0.0 });
        }
        pivots.push(p.abs());
        m.swap(piv, col);
        let inv = 1.0 / p;
        for j in 0..2 * n { m[col][j] *= inv; }
        for i in 0..n {
            if i != col {
                let factor = m[i][col];
                for j in 0..2 * n {
                    m[i][j] -= factor * m[col][j];
                }
            }
        }
    }
    let mut inv = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n { inv[i][j] = m[i][n + j]; }
    }
    let a1 = norm1(a);
    let inv1 = norm1(&inv);
    let rcond = if a1 > 0.0 && inv1.is_finite() { 1.0 / (a1 * inv1) } else { 0.0 };
    let rcond = if rcond.is_finite() { rcond } else { 0.0 };

    let smallest_pivot = pivots.iter().cloned().fold(f64::INFINITY, f64::min);
    let status = if rcond == 0.0 || smallest_pivot < 1e-12 {
        InvertStatus::Singular { rcond }
    } else if rcond < rcond_threshold {
        InvertStatus::NearSingular { rcond }
    } else {
        InvertStatus::Determinate
    };
    match status {
        InvertStatus::Determinate => Ok(InvertResult { inv, rcond, status }),
        s => Err(s),
    }
}

#[derive(Debug, Clone)]
pub struct Svd {
    pub u: Vec<Vec<f64>>,
    pub s: Vec<f64>,
    pub vt: Vec<Vec<f64>>,
}

/// One-sided (Hestenes) Jacobi SVD for small square matrices.
/// A = U S Vt. Columns of A V equal U S (up to sign conventions).
pub fn svd(a: &[Vec<f64>]) -> Svd {
    let n = a.len();
    let mut v = identity(n);
    let mut g = a.to_vec();
    for _ in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                let mut alpha = 0.0;
                let mut beta = 0.0;
                let mut gamma = 0.0;
                for i in 0..n {
                    alpha += g[i][p] * g[i][p];
                    beta += g[i][q] * g[i][q];
                    gamma += g[i][p] * g[i][q];
                }
                off += gamma.abs();
                if gamma.abs() <= 1e-15 * (alpha * beta).sqrt().max(1e-300) {
                    continue;
                }
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = if zeta >= 0.0 {
                    1.0 / (zeta + (1.0 + zeta * zeta).sqrt())
                } else {
                    -1.0 / (-zeta + (1.0 + zeta * zeta).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                for i in 0..n {
                    let gp = g[i][p];
                    let gq = g[i][q];
                    g[i][p] = c * gp - s * gq;
                    g[i][q] = s * gp + c * gq;
                }
                for i in 0..n {
                    let vp = v[i][p];
                    let vq = v[i][q];
                    v[i][p] = c * vp - s * vq;
                    v[i][q] = s * vp + c * vq;
                }
            }
        }
        if off < 1e-14 { break; }
    }
    let mut s: Vec<f64> = (0..n).map(|j| {
        let norm: f64 = (0..n).map(|i| g[i][j] * g[i][j]).sum::<f64>().sqrt();
        norm
    }).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| s[*b].partial_cmp(&s[*a]).unwrap());
    s = order.iter().map(|&k| s[k]).collect();

    // U columns = normalized columns of G in sorted order.
    let mut u = vec![vec![0.0; n]; n];
    for (out, &col) in order.iter().enumerate() {
        let norm = {
            let x: f64 = (0..n).map(|i| g[i][col] * g[i][col]).sum::<f64>().sqrt();
            x
        };
        if norm > 1e-300 {
            for i in 0..n { u[i][out] = g[i][col] / norm; }
        } else {
            // Null singular vector: reuse corresponding V column, orthonormalized.
            u[out][out] = 1.0;
        }
    }
    // Vt rows = rows of V^T in sorted column order.
    let mut vt = vec![vec![0.0; n]; n];
    for (out, &col) in order.iter().enumerate() {
        for i in 0..n { vt[out][i] = v[i][col]; }
    }
    Svd { u, s, vt }
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut v = vec![vec![0.0; n]; n];
    for i in 0..n { v[i][i] = 1.0; }
    v
}

/// Truncated SVD pseudo-inverse keeping singular values with `s >= rel * s_max`.
/// Returns (`pinv`, rank, kept_singular_values).
pub fn truncated_pinv(a: &[Vec<f64>], rel: f64) -> (Vec<Vec<f64>>, usize, Vec<f64>) {
    let n = a.len();
    let svd = svd(a);
    let smax = svd.s[0];
    let cutoff = rel * smax;
    let rank = svd.s.iter().filter(|&&x| x > cutoff).count().max(1);
    let mut pinv = vec![vec![0.0; n]; n];
    for k in 0..rank {
        let scale = 1.0 / svd.s[k];
        for i in 0..n {
            for j in 0..n {
                pinv[i][j] += scale * svd.vt[k][i] * svd.u[j][k];
            }
        }
    }
    (pinv, rank, svd.s[..rank].to_vec())
}

pub fn matvec(a: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    a.iter().map(|row| row.iter().zip(x).map(|(w, v)| w * v).sum()).collect()
}
