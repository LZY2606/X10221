//! Small dense linear algebra for compensation.

#[derive(Debug)]
pub struct LinError(pub String);

pub fn mat_zeros(n: usize, m: usize) -> Vec<Vec<f64>> {
    vec![vec![0.0; m]; n]
}

pub fn mat_identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = mat_zeros(n, n);
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

pub fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let k = a.first().map(|r| r.len())?;
    let k2 = b.len();
    let m = b.first().map(|r| r.len())?;
    if k != k2 {
        return None;
    }
    let mut out = mat_zeros(n, m);
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            out[i][j] = s;
        }
    }
    Some(out)
}

pub fn mat_vec(a: &[Vec<f64>], x: &[f64]) -> Option<Vec<f64>> {
    let n = a.len();
    let k = a.first()?.len();
    if k != x.len() {
        return None;
    }
    let mut out = vec![0.0; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..k {
            s += a[i][j] * x[j];
        }
        out[i] = s;
    }
    Some(out)
}

pub fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = a.first().map(|r| r.len()).unwrap_or(0);
    let mut out = mat_zeros(m, n);
    for i in 0..n {
        for j in 0..m {
            out[j][i] = a[i][j];
        }
    }
    out
}

pub fn mat_add(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    a.iter()
        .zip(b)
        .map(|(r, s)| r.iter().zip(s).map(|(x, y)| x + y).collect())
        .collect()
}

pub fn scale(a: &[Vec<f64>], s: f64) -> Vec<Vec<f64>> {
    a.iter().map(|r| r.iter().map(|x| x * s).collect()).collect()
}

/// LU decomposition with partial pivoting. Returns (L, U, permutation), Err
/// when the matrix is singular up to a tiny tolerance.
pub fn lu(a: &[Vec<f64>]) -> Result<(Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<usize>), LinError> {
    let n = a.len();
    if a.iter().any(|r| r.len() != n) {
        return Err(LinError("non-square matrix".into()));
    }
    let mut u = a.to_vec();
    let mut l = mat_identity(n);
    let mut perm: Vec<usize> = (0..n).collect();
    for k in 0..n {
        let mut pivot = k;
        let mut best = u[k][k].abs();
        for i in k + 1..n {
            if u[i][k].abs() > best {
                best = u[i][k].abs();
                pivot = i;
            }
        }
        if best < 1e-14 {
            return Err(LinError("matrix is singular to working precision".into()));
        }
        if pivot != k {
            u.swap(k, pivot);
            // swap recorded multipliers in columns < k of L
            for j in 0..k {
                let tmp = l[k][j];
                l[k][j] = l[pivot][j];
                l[pivot][j] = tmp;
            }
            perm.swap(k, pivot);
        }
        for i in k + 1..n {
            let f = u[i][k] / u[k][k];
            l[i][k] = f;
            for j in k..n {
                u[i][j] -= f * u[k][j];
            }
        }
    }
    Ok((l, u, perm))
}

fn solve_lu(l: &[Vec<f64>], u: &[Vec<f64>], perm: &[usize], b: &[f64]) -> Vec<f64> {
    let n = l.len();
    let pb: Vec<f64> = perm.iter().map(|&i| b[i]).collect();
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = pb[i];
        for j in 0..i {
            s -= l[i][j] * y[j];
        }
        y[i] = s;
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for j in i + 1..n {
            s -= u[i][j] * x[j];
        }
        x[i] = s / u[i][i];
    }
    x
}

pub fn inverse(a: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let (l, u, p) = lu(a).ok()?;
    let n = a.len();
    let mut inv = mat_zeros(n, n);
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        let col = solve_lu(&l, &u, &p, &e);
        for i in 0..n {
            inv[i][j] = col[i];
        }
    }
    if inv.iter().flatten().any(|x| !x.is_finite()) {
        return None;
    }
    Some(inv)
}

/// Tikhonov ridge solution: (S^T S + lambda I)^-1 S^T.
pub fn ridge_inverse(s: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let st = transpose(s);
    let sts = mat_mul(&st, s)?;
    let n = sts.len();
    let mut reg = sts;
    for i in 0..n {
        reg[i][i] += lambda;
    }
    let inv = inverse(&reg)?;
    Some(mat_mul(&inv, &st)?)
}

fn inf_norm(a: &[Vec<f64>]) -> f64 {
    a.iter().map(|r| r.iter().map(|x| x.abs()).sum::<f64>()).fold(0.0, f64::max)
}

/// Crude reciprocal condition estimate rcond = 1/(||A||inf * ||A^-1||inf).
/// Small rcond => near singular.
pub fn reciprocal_condition(a: &[Vec<f64>]) -> f64 {
    let na = inf_norm(a);
    if na == 0.0 {
        return 0.0;
    }
    match inverse(a) {
        Some(inv) => 1.0 / (na * inf_norm(&inv)),
        None => 0.0,
    }
}

/// Cyclic Jacobi SVD singular values for square symmetric matrices
/// (used on S^T S only if ever needed for diagnostics).
pub fn svd_singular_values_symmetric(a: &[Vec<f64>], sweeps: u32) -> Vec<f64> {
    let n = a.len();
    let mut m = a.to_vec();
    for _ in 0..sweeps {
        let mut off = 0.0f64;
        for p in 0..n {
            for q in p + 1..n {
                off += m[p][q] * m[p][q];
            }
        }
        if off < 1e-24 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if m[p][q].abs() < 1e-15 {
                    continue;
                }
                let tau = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = m[k][p];
                    let akq = m[k][q];
                    m[k][p] = c * akp - s * akq;
                    m[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = m[p][k];
                    let aqk = m[q][k];
                    m[p][k] = c * apk - s * aqk;
                    m[q][k] = s * apk + c * aqk;
                }
            }
        }
    }
    (0..n).map(|i| m[i][i].abs().sqrt()).collect()
}
