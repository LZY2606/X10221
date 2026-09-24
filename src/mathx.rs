//! Linear algebra and gating math, dependency-free.
//!
//! SVD is computed as A = U S Vt via symmetric eigen-decompositions:
//! eigenvalues of A^T A give singular values (Jacobi), and of A A^T give U.
//! This is adequate for the small 3x3/4x4 matrices of the fixed case and keeps
//! the install reproducible with only `cargo build`.

pub fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = a[0].len();
    let mut t = vec![vec![0.0; n]; m];
    for i in 0..n {
        for j in 0..m {
            t[j][i] = a[i][j];
        }
    }
    t
}

pub fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b[0].len();
    let mut c = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            c[i][j] = s;
        }
    }
    c
}

/// Jacobi eigen-decomposition of a real symmetric matrix.
/// Returns (eigenvalues ascending, eigenvector columns).
pub fn jacobi_eigen(mut a: Vec<Vec<f64>>) -> (Vec<f64>, Vec<Vec<f64>>) {
    let n = a.len();
    let mut v = vec![vec![0.0; n]; n];
    for i in 0..n {
        v[i][i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0;
        for p in 0..n {
            for q in (p + 1)..n {
                off += a[p][q] * a[p][q];
            }
        }
        if off < 1e-26 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                if a[p][q].abs() < 1e-15 {
                    continue;
                }
                let tau = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                let app = a[p][p];
                let aqq = a[q][q];
                a[p][p] = app - t * a[p][q];
                a[q][q] = aqq + t * a[p][q];
                a[p][q] = 0.0;
                a[q][p] = 0.0;
                for r in 0..n {
                    if r != p && r != q {
                        let arp = a[r][p];
                        let arq = a[r][q];
                        a[r][p] = c * arp - s * arq;
                        a[p][r] = a[r][p];
                        a[r][q] = s * arp + c * arq;
                        a[q][r] = a[r][q];
                    }
                    let vrp = v[r][p];
                    let vrq = v[r][q];
                    v[r][p] = c * vrp - s * vrq;
                    v[r][q] = s * vrp + c * vrq;
                }
            }
        }
    }
    let mut vals: Vec<f64> = (0..n).map(|i| a[i][i]).collect();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&i, &j| vals[i].partial_cmp(&vals[j]).unwrap());
    let sorted_vals: Vec<f64> = order.iter().map(|&i| vals[i]).collect();
    let mut sorted_vecs = vec![vec![0.0; n]; n];
    for (col, &idx) in order.iter().enumerate() {
        for row in 0..n {
            sorted_vecs[row][col] = v[row][idx];
        }
    }
    vals = sorted_vals;
    (vals, sorted_vecs)
}

pub struct Svd {
    pub sigma: Vec<f64>,
    /// columns are right singular vectors V.
    pub v: Vec<Vec<f64>>,
    /// columns are left singular vectors U.
    pub u: Vec<Vec<f64>>,
}

pub fn svd(a: &[Vec<f64>]) -> Svd {
    let at = transpose(a);
    let ata = matmul(&at, a);
    let (vals, v) = jacobi_eigen(ata);
    let n = a.len();
    let m = a[0].len();
    let sigma: Vec<f64> = vals.iter().map(|x| if *x > 0.0 { x.sqrt() } else { 0.0 }).collect();
    // U columns: u_i = A v_i / sigma_i; when sigma ~ 0 derive from A A^T.
    let mut u = vec![vec![0.0; m]; n];
    let mut null_cols = Vec::new();
    for j in 0..m {
        if sigma[j] > 1e-9 {
            for i in 0..n {
                let mut s = 0.0;
                for k in 0..m {
                    s += a[i][k] * v[k][j];
                }
                u[i][j] = s / sigma[j];
            }
        } else {
            null_cols.push(j);
        }
    }
    if !null_cols.is_empty() {
        let aat = matmul(a, &at);
        let (uvals, uvecs) = jacobi_eigen(aat);
        let mut col = 0;
        for (idx, &lam) in uvals.iter().enumerate() {
            if lam <= 1e-18 {
                if col < null_cols.len() {
                    let target = null_cols[col];
                    for i in 0..n {
                        u[i][target] = uvecs[i][idx];
                    }
                    col += 1;
                }
            }
        }
    }
    Svd { sigma, v, u }
}

pub fn determinant(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    let mut m = a.to_vec();
    let mut det = 1.0;
    for i in 0..n {
        let mut pivot = i;
        for r in (i + 1)..n {
            if m[r][i].abs() > m[pivot][i].abs() {
                pivot = r;
            }
        }
        if m[pivot][i].abs() < 1e-15 {
            return 0.0;
        }
        if pivot != i {
            m.swap(pivot, i);
            det = -det;
        }
        let piv = m[i][i];
        det *= piv;
        for r in (i + 1)..n {
            let f = m[r][i] / piv;
            for c in i..n {
                m[r][c] -= f * m[i][c];
            }
        }
    }
    det
}

use crate::domain::MatrixVerdict;

/// Returns the matrix verdict. Status:
/// - "singular" when a singular value is at machine zero
/// - "near_singular" when kappa exceeds 1e6
/// - "ok" otherwise. A near-singular matrix must never be silently inverted.
pub fn verdict(rows: &[Vec<f64>], near_threshold: f64) -> MatrixVerdict {
    let det = determinant(rows);
    let s = svd(rows);
    let smin = *s.sigma.iter().fold(&f64::INFINITY, |a, b| if b < a { b } else { a });
    let smax = *s.sigma.iter().fold(&0.0_f64, |a, b| if b > a { b } else { a });
    let (kappa, status) = if smin <= 1e-10 || det.abs() < 1e-12 {
        (None, "singular".to_string())
    } else {
        let k = smax / smin;
        let st = if k > near_threshold { "near_singular".to_string() } else { "ok".to_string() };
        (Some(k), st)
    };
    MatrixVerdict {
        determinant: det,
        sigma_min: smin,
        sigma_max: smax,
        kappa,
        status,
    }
}

pub fn invert(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut m: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = vec![0.0; 2 * n];
        for j in 0..n {
            row[j] = a[i][j];
        }
        row[n + i] = 1.0;
        m.push(row);
    }
    for i in 0..n {
        let mut pivot = i;
        for r in (i + 1)..n {
            if m[r][i].abs() > m[pivot][i].abs() {
                pivot = r;
            }
        }
        if m[pivot][i].abs() < 1e-15 {
            return None;
        }
        m.swap(pivot, i);
        let piv = m[i][i];
        for c in 0..2 * n {
            m[i][c] /= piv;
        }
        for r in 0..n {
            if r == i {
                continue;
            }
            let f = m[r][i];
            for c in 0..2 * n {
                m[r][c] -= f * m[i][c];
            }
        }
    }
    Some(m.iter().map(|row| row[n..].to_vec()).collect())
}

/// Moore-Penrose pseudoinverse from the SVD.
/// Channels with sigma below the cutoff cannot be uniquely determined; the
/// caller is told which output channels are unresolved.
pub fn pseudo_inverse(a: &[Vec<f64>]) -> (Vec<Vec<f64>>, Vec<usize>) {
    let s = svd(a);
    let n = a.len();
    let m = a[0].len();
    let cutoff = (s.sigma[m - 1] * 1e-9).max(1e-10);
    // A+ = V * Sigma+ * U^T
    let mut ap = vec![vec![0.0; n]; m];
    for i in 0..m {
        for j in 0..n {
            let mut sum = 0.0;
            for k in 0..m {
                if s.sigma[k] > cutoff {
                    sum += s.v[i][k] * (1.0 / s.sigma[k]) * s.u[j][k];
                }
            }
            ap[i][j] = sum;
        }
    }
    // Output channels dominated by null space are unresolved: mark rows of V
    // whose null-space mass exceeds 0.5.
    let mut unresolved = Vec::new();
    for i in 0..m {
        let mut null_mass = 0.0;
        for k in 0..m {
            if s.sigma[k] <= cutoff {
                null_mass += s.v[i][k] * s.v[i][k];
            }
        }
        if null_mass > 0.5 {
            unresolved.push(i);
        }
    }
    (ap, unresolved)
}

/// Tikhonov/ridge regularized inverse: (A^T A + lambda I)^-1 A^T.
/// It is a *candidate* interpretation, never an automatic replacement.
pub fn ridge_inverse(a: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let at = transpose(a);
    let ata = matmul(&at, a);
    let n = ata.len();
    let mut reg = ata;
    for i in 0..n {
        reg[i][i] += lambda;
    }
    let inv = invert(&reg).expect("regularized matrix is invertible");
    matmul(&inv, &at)
}

pub fn apply_matrix(m: &[Vec<f64>], v: &[Option<f64>]) -> Vec<Option<f64>> {
    let n = m.len();
    let mut out = vec![None; n];
    for i in 0..n {
        let mut sum = Some(0.0);
        for j in 0..m[0].len() {
            match v[j] {
                Some(x) => {
                    sum = sum.map(|s| s + m[i][j] * x);
                }
                None => {
                    sum = None;
                    break;
                }
            }
        }
        out[i] = sum;
    }
    out
}

pub fn asinh_transform(x: f64, cofactor: f64, gain: f64, offset: f64) -> f64 {
    (((x - offset) / gain) / cofactor).asinh() * cofactor
}

/// Point-in-convex-or-simple-polygon test with explicit boundary membership.
/// Returns (inside, on_boundary). Boundary points are included by convention.
pub fn point_in_polygon(pt: [f64; 2], poly: &[[f64; 2]], tol: f64) -> (bool, bool) {
    let n = poly.len();
    if n < 3 {
        return (false, false);
    }
    let [px, py] = pt;
    let mut on_edge = false;
    let mut inside = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let cross = (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0]);
        let on_line = cross.abs() <= tol;
        let in_box = px <= a[0].max(b[0]) + tol
            && px >= a[0].min(b[0]) - tol
            && py <= a[1].max(b[1]) + tol
            && py >= a[1].min(b[1]) - tol;
        if on_line && in_box {
            on_edge = true;
        }
        let cond = (a[1] > py) != (b[1] > py);
        if cond {
            let xint = a[0] + (py - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if xint < px {
                inside = !inside;
            }
        }
    }
    (inside || on_edge, on_edge)
}
