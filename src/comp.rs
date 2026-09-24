//! 补偿矩阵规则：近奇异矩阵不能被默默求逆。
//!
//! 判定策略（机器可读）：Gauss-Jordan 带部分主元；若最小/最大主元比 < PIVOT_TOL
//! 或 1-范数条件数 > COND_TOL，则给出 `NearSingular`，并保留全部合法候选
//! （Tikhonov 正则化 (MᵀM + λI)⁻¹Mᵀ，多个 λ）。

use serde::Serialize;

pub const COND_TOL: f64 = 1e6;
pub const PIVOT_TOL: f64 = 1e-10;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Matrix {
    pub n: usize,
    pub a: Vec<Vec<f64>>,
}

impl Matrix {
    pub fn new(a: Vec<Vec<f64>>) -> Self {
        let n = a.len();
        debug_assert!(a.iter().all(|r| r.len() == n));
        Self { n, a }
    }
    pub fn norm1(&self) -> f64 {
        (0..self.n)
            .map(|j| (0..self.n).map(|i| self.a[i][j].abs()).sum::<f64>())
            .fold(0.0, f64::max)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub method: String,
    pub param: Option<f64>,
    pub inverse: Matrix,
    pub residual: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum InverseVerdict {
    Ok { inverse: Matrix, condition: f64 },
    NearSingular {
        condition: f64,
        min_pivot_ratio: f64,
        threshold: f64,
        policy: &'static str,
        candidates: Vec<Candidate>,
    },
}

fn gauss_jordan(m: &Matrix) -> Option<(Matrix, f64)> {
    let n = m.n;
    let mut aug = vec![vec![0.0; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            aug[i][j] = m.a[i][j];
        }
        aug[i][n + i] = 1.0;
    }
    let mut max_pivot = 0.0f64;
    let mut min_pivot = f64::MAX;
    for col in 0..n {
        let pivot = (col..n)
            .map(|r| (r, aug[r][col].abs()))
            .fold((col, 0.0f64), |a, b| if b.1 > a.1 { b } else { a });
        if pivot.1 < PIVOT_TOL {
            return None;
        }
        max_pivot = max_pivot.max(pivot.1);
        min_pivot = min_pivot.min(pivot.1);
        aug.swap(col, pivot.0);
        let p = aug[col][col];
        for j in 0..2 * n {
            aug[col][j] /= p;
        }
        for r in 0..n {
            if r != col {
                let f = aug[r][col];
                if f != 0.0 {
                    for j in 0..2 * n {
                        aug[r][j] -= f * aug[col][j];
                    }
                }
            }
        }
    }
    let inv = Matrix::new(
        (0..n)
            .map(|i| aug[i][n..2 * n].to_vec())
            .collect(),
    );
    Some((inv, min_pivot / max_pivot))
}

fn transpose(m: &Matrix) -> Matrix {
    Matrix::new((0..m.n).map(|i| (0..m.n).map(|j| m.a[j][i]).collect()).collect())
}

fn matmul(a: &Matrix, b: &Matrix) -> Matrix {
    Matrix::new(
        (0..a.n)
            .map(|i| {
                (0..b.n)
                    .map(|j| (0..a.n).map(|k| a.a[i][k] * b.a[k][j]).sum())
                    .collect()
            })
            .collect(),
    )
}

fn tikhonov(m: &Matrix, lambda: f64) -> Option<Matrix> {
    let mt = transpose(m);
    let mtm = matmul(&mt, m);
    let reg = Matrix::new(
        (0..m.n)
            .map(|i| {
                (0..m.n)
                    .map(|j| mtm.a[i][j] + if i == j { lambda } else { 0.0 })
                    .collect()
            })
            .collect(),
    );
    let (inv, _) = gauss_jordan(&reg)?;
    Some(matmul(&inv, &mt))
}

fn residual(m: &Matrix, inv: &Matrix) -> f64 {
    let prod = matmul(inv, m);
    (0..m.n)
        .flat_map(|i| (0..m.n).map(move |j| (i, j)))
        .map(|(i, j)| prod.a[i][j] - if i == j { 1.0 } else { 0.0 })
        .map(f64::abs)
        .fold(0.0, f64::max)
}

pub fn invert_checked(m: &Matrix) -> InverseVerdict {
    match gauss_jordan(m) {
        Some((inv, ratio)) => {
            let cond = m.norm1() * inv.norm1();
            if ratio < PIVOT_TOL || cond > COND_TOL {
                let lambdas = [1e-4, 1e-3, 1e-2];
                let candidates = lambdas
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &l)| {
                        tikhonov(m, l).map(|c| Candidate {
                            id: format!("cand-tikhonov-{}", i + 1),
                            label: format!("Tikhonov 正则化 λ={l}"),
                            method: "tikhonov".to_string(),
                            param: Some(l),
                            residual: residual(m, &c),
                            inverse: c,
                        })
                    })
                    .collect();
                InverseVerdict::NearSingular {
                    condition: cond,
                    min_pivot_ratio: ratio,
                    threshold: COND_TOL,
                    policy: "refuse-silent-inversion",
                    candidates,
                }
            } else {
                InverseVerdict::Ok { inverse: inv, condition: cond }
            }
        }
        None => {
            let lambdas = [1e-4, 1e-3, 1e-2];
            let candidates = lambdas
                .iter()
                .enumerate()
                .filter_map(|(i, &l)| {
                    tikhonov(m, l).map(|c| Candidate {
                        id: format!("cand-tikhonov-{}", i + 1),
                        label: format!("Tikhonov 正则化 λ={l}"),
                        method: "tikhonov".to_string(),
                        param: Some(l),
                        residual: residual(m, &c),
                        inverse: c,
                    })
                })
                .collect();
            InverseVerdict::NearSingular {
                condition: f64::INFINITY,
                min_pivot_ratio: 0.0,
                threshold: COND_TOL,
                policy: "refuse-silent-inversion",
                candidates,
            }
        }
    }
}

/// 用补偿矩阵把一行通道值做线性补偿：x_comp = M⁻¹ · x。
pub fn apply_row(inv: &Matrix, row: &[f64]) -> Vec<f64> {
    (0..inv.n)
        .map(|i| (0..inv.n).map(|j| inv.a[i][j] * row[j]).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_inverts() {
        let m = Matrix::new(vec![vec![1.0, 0.1], vec![0.2, 1.0]]);
        match invert_checked(&m) {
            InverseVerdict::Ok { condition, .. } => assert!(condition < COND_TOL),
            other => panic!("期望 Ok，得到 {other:?}"),
        }
    }

    #[test]
    fn near_singular_refuses_and_offers_candidates() {
        // det ≈ 5e-7
        let m = Matrix::new(vec![vec![1.0, 0.8], vec![0.8, 0.6400005]]);
        let v = invert_checked(&m);
        match v {
            InverseVerdict::NearSingular {
                condition,
                policy,
                candidates,
                ..
            } => {
                assert!(condition > COND_TOL);
                assert_eq!(policy, "refuse-silent-inversion");
                assert_eq!(candidates.len(), 3);
                // 正则化候选残差可控
                assert!(candidates.iter().all(|c| c.residual < 1.0));
            }
            other => panic!("期望 NearSingular，得到 {other:?}"),
        }
    }

    #[test]
    fn exactly_singular_returns_candidates() {
        let m = Matrix::new(vec![vec![1.0, 0.5], vec![2.0, 1.0]]);
        match invert_checked(&m) {
            InverseVerdict::NearSingular { candidates, .. } => assert_eq!(candidates.len(), 3),
            other => panic!("期望 NearSingular，得到 {other:?}"),
        }
    }
}
