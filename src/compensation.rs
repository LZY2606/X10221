//! 补偿内核：纯 Rust 的对称特征分解（一维 Jacobi）+ SVD，
//! 条件数判定，以及“近奇异不默默求逆”的多候选协议。
//!
//! 流式约定：spillover 矩阵 S 满足 measured = S * true，
//! 因此 true = S^{-1} * measured。S 近奇异时，
//! 系统不允许默默给出某个逆：必须保留全部合法候选并等待人工裁定。

use serde::{Deserialize, Serialize};

use crate::model::{MatrixConditioning, MatrixDoc};

pub const NEAR_SINGULAR_COND: f64 = 30.0;
pub const TINY_SV_RATIO: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Svd {
    pub u: Vec<Vec<f64>>,
    pub s: Vec<f64>,
    pub vt: Vec<Vec<f64>>,
}

impl Svd {
    pub fn condition_number(&self) -> Option<f64> {
        let mx = self.s.iter().cloned().fold(0.0f64, f64::max);
        let mn = self
            .s
            .iter()
            .cloned()
            .filter(|v| *v > TINY_SV_RATIO * mx.max(1.0))
            .fold(f64::INFINITY, f64::min);
        if mn.is_finite() && mx > 0.0 {
            Some(mx / mn)
        } else {
            None
        }
    }
}

fn mat_zeros(n: usize) -> Vec<Vec<f64>> {
    vec![vec![0.0; n]; n]
}

fn mat_mul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut c = mat_zeros(n);
    for i in 0..n {
        for k in 0..n {
            let aik = a[i][k];
            if aik == 0.0 {
                continue;
            }
            for j in 0..n {
                c[i][j] += aik * b[k][j];
            }
        }
    }
    c
}

fn mat_t(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let mut t = mat_zeros(n);
    for i in 0..n {
        for j in 0..n {
            t[j][i] = a[i][j];
        }
    }
    t
}

/// 对称矩阵 A 的特征分解 A = V diag(w) V^T（一维循环 Jacobi）。
fn jacobi_eigen(mut a: Vec<Vec<f64>>) -> (Vec<Vec<f64>>, Vec<f64>) {
    let n = a.len();
    let mut v = mat_zeros(n);
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
        if off < 1e-24 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = a[p][q];
                if apq.abs() < 1e-15 {
                    continue;
                }
                let app = a[p][p];
                let aqq = a[q][q];
                let tau = (aqq - app) / (2.0 * apq);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let sn = t * c;
                for k in 0..n {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - sn * akq;
                    a[p][k] = a[k][p];
                    a[k][q] = sn * akp + c * akq;
                    a[q][k] = a[k][q];
                }
                for k in 0..n {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = c * vkp - sn * vkq;
                    v[k][q] = sn * vkp + c * vkq;
                }
            }
        }
    }
    let w = (0..n).map(|i| a[i][i]).collect();
    (v, w)
}

/// 方阵 SVD：A = U S V^T（经由 A^T A 的 Jacobi 特征分解）。
/// 案例包矩阵规模 <= 8x8，精度足够；不引入 BLAS 依赖。
pub fn svd(a: &[Vec<f64>]) -> Svd {
    let n = a.len();
    let at = mat_t(a);
    let ata = mat_mul(&at, a);
    let (v, w) = jacobi_eigen(ata);
    // 特征值降序
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|i, j| w[*j].partial_cmp(&w[*i]).unwrap());
    let mut s = vec![0.0; n];
    let mut vs = mat_zeros(n);
    for (col, &idx) in order.iter().enumerate() {
        s[col] = if w[idx] > 0.0 { w[idx].sqrt() } else { 0.0 };
        for row in 0..n {
            vs[row][col] = v[row][idx];
        }
    }
    // U = A V S^{-1}；零奇异值列用正交补全。
    let av = mat_mul(a, &vs);
    let mut u = mat_zeros(n);
    for col in 0..n {
        if s[col] > 1e-14 {
            for row in 0..n {
                u[row][col] = av[row][col] / s[col];
            }
        } else {
            u[row_placeholder(); 0] = 0.0;
        }
    }
    u = complete_orthogonal(u, &s);
    let vt = mat_t(&vs);
    Svd { u, s, vt }
}

fn row_placeholder() -> usize {
    0
}

fn complete_orthogonal(mut u: Vec<Vec<f64>>, s: &[f64]) -> Vec<Vec<f64>> {
    let n = u.len();
    for col in 0..n {
        if s[col] > 1e-14 {
            continue;
        }
        // 找一个与已填列正交的向量
        let mut cand = vec![0.0; n];
        for trial in 0..n {
            let mut x = vec![0.0; n];
            x[trial] = 1.0;
            for k in 0..col {
                if s[k] <= 1e-14 {
                    continue;
                }
                let dot: f64 = (0..n).map(|i| x[i] * u[i][k]).sum();
                for i in 0..n {
                    x[i] -= dot * u[i][k];
                }
            }
            let norm: f64 = x.iter().map(|v| v * v).sum::<f64>().sqrt();
            if norm > 1e-9 {
                for i in 0..n {
                    x[i] /= norm;
                }
                cand = x;
                break;
            }
        }
        for i in 0..n {
            u[i][col] = cand[i];
        }
    }
    u
}

