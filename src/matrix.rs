//! Compensation matrix diagnostics.
//!
//! Rule (machine-readable, enforced here): a near-singular compensation matrix
//! must never be silently inverted. We compute the SVD with the one-sided
//! Jacobi algorithm, inspect sigma_min and the condition number, and either
//! return a single exact inverse or return *all legal regularized candidates*
//! with residuals, leaving the decision to the reviewer.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InversionReport {
    pub n: usize,
    pub singular_values: Vec<f64>,
    pub condition_number: f64,
    pub near_singular: bool,
    pub exact_singular: bool,
    /// Present only when the matrix is safely invertible.
    pub inverse: Option<Vec<Vec<f64>>>,
    /// All legal candidates when the matrix cannot be uniquely inverted.
    pub candidates: Vec<RegularizedCandidate>,
    pub diagnostic_code: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegularizedCandidate {
    pub id: String,
    pub method: String,
    pub alpha: f64,
    /// alpha * I + S^T S based pseudo-inverse, i.e. (S^T S + aI)^-1 S^T.
    pub matrix: Vec<Vec<f64>>,
    /// ||S * S_inv - I||_F (residual against identity).
    pub residual: f64,
    /// Norm of the inverse rows (amplification indicator).
    pub amplification: f64,
}

const CONDITION_LIMIT: f64 = 1e6;
const SIGMA_MIN_LIMIT: f64 = 1e-8;

pub fn analyze(matrix: &[Vec<f64>]) -> InversionReport {
    let n = matrix.len();
    let sv = singular_values(matrix);
    let smax = sv.iter().cloned().fold(0.0_f64, f64::max);
    let smin = sv.iter().cloned().fold(f64::INFINITY, f64::min);
    let cond = if smin > 0.0 && smin.is_finite() { smax / smin } else { f64::INFINITY };
    let exact_singular = smin <= SIGMA_MIN_LIMIT || !smin.is_finite();
    let near_singular = !exact_singular && cond >= CONDITION_LIMIT;

    let (inverse, code, candidates) = if exact_singular {
        (
            None,
            "exact_singular".into(),
            regularized_candidates(matrix, &[1e-2, 1e-1, 1.0]),
        )
    } else if near_singular {
        (
            None,
            "near_singular".into(),
            regularized_candidates(matrix, &[1e-3, 1e-2, 1e-1]),
        )
    } else {
        match invert(matrix) {
            Ok(inv) => (Some(inv), "inverted".into(), vec![]),
            Err(_) => (
                None,
                "inversion_failed".into(),
                regularized_candidates(matrix, &[1e-2, 1e-1, 1.0]),
            ),
        }
    };

    InversionReport {
        n,
        singular_values: sv,
        condition_number: if cond.is_finite() { cond } else { f64::INFINITY },
        near_singular,
        exact_singular,
        inverse,
        candidates,
        diagnostic_code: code,
    }
}

fn regularized_candidates(matrix: &[Vec<f64>], alphas: &[f64]) -> Vec<RegularizedCandidate> {
    let n = matrix.len();
    let st = transpose(matrix);
    let mut out = Vec::new();
    for (i, &alpha) in alphas.iter().enumerate() {
        // (S^T S + alpha I)^-1 S^T
        let sts = matmul(&st, matrix);
        let mut reg = sts.clone();
        for r in 0..n {
            reg[r][r] += alpha;
        }
        let reg_inv = match invert(&reg) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let cand = matmul(&reg_inv, &st);
        let residual = residual_identity(&matmul(matrix, &cand));
        let amplification = frob(&cand);
        out.push(RegularizedCandidate {
            id: format!("regcand_{i}_{}", (alpha * 1e6) as i64),
            method: "tikhonov".into(),
            alpha,
            matrix: cand,
            residual,
            amplification,
        });
    }
    out
}

fn residual_identity(product: &[Vec<f64>]) -> f64 {
    let n = product.len();
    let mut sum = 0.0;
    for r in 0..n {
        for c in 0..n {
            let target = if r == c { 1.0 } else { 0.0 };
            let d = product[r][c] - target;
            sum += d * d;
        }
    }
    sum.sqrt()
}

fn frob(m: &[Vec<f64>]) -> f64 {
    m.iter()
        .flat_map(|r| r.iter())
        .map(|x| x * x)
        .sum::<f64>()
        .sqrt()
}

/// Solve/invert via Gauss-Jordan partial pivot.
pub fn invert(m: &[Vec<f64>]) -> std::result::Result<Vec<Vec<f64>>, String> {
    let n = m.len();
    if m.iter().any(|r| r.len() != n) {
        return Err("not square".into());
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv = identity(n);
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&i, &j| {
                a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap()
            })
            .unwrap();
        if a[pivot][col].abs() < 1e-14 {
            return Err("singular".into());
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let p = a[col][col];
        for k in 0..n {
            a[col][k] /= p;
            inv[col][k] /= p;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            for k in 0..n {
                a[r][k] -= factor * a[col][k];
                inv[r][k] -= factor * inv[col][k];
            }
        }
    }
    Ok(inv)
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = 1.0;
    }
    m
}

pub fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b[0].len();
    let mut out = vec![vec![0.0; m]; n];
    for i in 0..n {
        for j in 0..m {
            let mut s = 0.0;
            for t in 0..k {
                s += a[i][t] * b[t][j];
            }
            out[i][j] = s;
        }
    }
    out
}

pub fn transpose(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = m.len();
    let k = m[0].len();
    let mut out = vec![vec![0.0; n]; k];
    for i in 0..n {
        for j in 0..k {
            out[j][i] = m[i][j];
        }
    }
    out
}

/// One-sided Jacobi SVD singular values (columns of A orthogonalized).
pub fn singular_values(a: &[Vec<f64>]) -> Vec<f64> {
    let rows = a.len();
    if rows == 0 {
        return vec![];
    }
    let cols = a[0].len();
    let mut v = identity(cols);
    let mut aa: Vec<Vec<f64>> = a.to_vec();
    for _sweep in 0..60 {
        let mut off = 0.0;
        for p in 0..cols {
            for q in (p + 1)..cols {
                let mut alpha = 0.0;
                let mut beta = 0.0;
                let mut gamma = 0.0;
                for r in 0..rows {
                    alpha += aa[r][p] * aa[r][p];
                    beta += aa[r][q] * aa[r][q];
                    gamma += aa[r][p] * aa[r][q];
                }
                off += gamma * gamma;
                if gamma.abs() < 1e-15 {
                    continue;
                }
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = if zeta >= 0.0 {
                    1.0 / (zeta + (1.0 + zeta * zeta).sqrt())
                } else {
                    -1.0 / (-zeta + (1.0 + zeta * zeta).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for r in 0..rows {
                    let ap = aa[r][p];
                    let aq = aa[r][q];
                    aa[r][p] = c * ap - s * aq;
                    aa[r][q] = s * ap + c * aq;
                }
                for r in 0..cols {
                    let vp = v[r][p];
                    let vq = v[r][q];
                    v[r][p] = c * vp - s * vq;
                    v[r][q] = s * vp + c * vq;
                }
            }
        }
        if off < 1e-28 {
            break;
        }
    }
    let mut sv: Vec<f64> = (0..cols)
        .map(|j| {
            (0..rows).map(|r| aa[r][j] * aa[r][j]).sum::<f64>().sqrt()
        })
        .collect();
    sv.sort_by(|a, b| b.partial_cmp(a).unwrap());
    sv
}

/// Apply an inverse/compensation matrix to one event vector.
pub fn apply(matrix_inv: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    let n = matrix_inv.len();
    let mut out = vec![0.0; n];
    for i in 0..n {
        let mut s = 0.0;
        for j in 0..n {
            s += matrix_inv[i][j] * v[j];
        }
        out[i] = s;
    }
    out
}
