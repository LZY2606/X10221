//! 补偿矩阵线性代数：高斯-若当精确求逆 + 近奇异判定。
//! 规则 "comp.singularity.v1"：
//!   - 最大主元 < SINGULAR_TOL          => singular，禁止求逆；
//!   - SINGULAR_TOL..=NEAR_SINGULAR_TOL => near_singular，允许保存候选但不得默默采用。

pub const SINGULAR_TOL: f64 = 1e-12;
pub const NEAR_SINGULAR_TOL: f64 = 1e-2;

#[derive(Clone, Debug, PartialEq)]
pub enum InvertOutcome {
    /// 条件良好：唯一合法逆矩阵
    Invertible(Vec<Vec<f64>>, f64),
    /// 近奇异：精确逆存在但数值不可信；额外给出岭正则候选
    NearSingular(Vec<Vec<f64>>, f64, Vec<Vec<f64>>, f64),
    /// 奇异：不得求逆
    Singular(f64),
}

/// 带部分主元的高斯-若当消元；返回 (逆, 最小主元)。
fn gj_inverse(m: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, f64)> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let scale = m
        .iter()
        .flat_map(|r| r.iter())
        .fold(1.0_f64, |a, &x| a.max(x.abs()));
    if !scale.is_finite() || scale == 0.0 {
        return None;
    }
    let mut a = vec![vec![0.0_f64; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            a[i][j] = m[i][j];
        }
        a[i][n + i] = 1.0;
    }
    let mut min_pivot = f64::MAX;
    for col in 0..n {
        let pivot_row = (col..n)
            .max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())
            .unwrap();
        let pivot = a[pivot_row][col].abs();
        if !pivot.is_finite() {
            return None;
        }
        min_pivot = min_pivot.min(pivot / scale);
        a.swap(col, pivot_row);
        let p = a[col][col];
        for j in 0..2 * n {
            a[col][j] /= p;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            if factor == 0.0 {
                continue;
            }
            for j in 0..2 * n {
                a[r][j] -= factor * a[col][j];
            }
        }
    }
    let inv = a
        .iter()
        .map(|row| row[n..].to_vec())
        .collect::<Vec<_>>();
    Some((inv, min_pivot))
}

/// 解 (A + lambda I) x = b 的高斯消元（部分主元）
fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Vec<Vec<f64>> {
    let n = m.len();
    let mut a = vec![vec![0.0_f64; 2 * n]; n];
    for i in 0..n {
        for j in 0..n {
            a[i][j] = m[i][j] + if i == j { lambda } else { 0.0 };
        }
        a[i][n + i] = 1.0;
    }
    for col in 0..n {
        let pivot_row = (col..n)
            .max_by(|&x, &y| a[x][col].abs().partial_cmp(&a[y][col].abs()).unwrap())
            .unwrap();
        a.swap(col, pivot_row);
        let p = a[col][col];
        for j in 0..2 * n {
            a[col][j] /= p;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            for j in 0..2 * n {
                a[r][j] -= factor * a[col][j];
            }
        }
    }
    a.iter().map(|row| row[n..].to_vec()).collect()
}

pub fn classify_and_invert(m: &[Vec<f64>]) -> InvertOutcome {
    match gj_inverse(m) {
        None => InvertOutcome::Singular(0.0),
        Some((inv, min_pivot)) => {
            if min_pivot < SINGULAR_TOL {
                InvertOutcome::Singular(min_pivot)
            } else if min_pivot <= NEAR_SINGULAR_TOL {
                let ridge = ridge_inverse(m, 1e-3);
                InvertOutcome::NearSingular(inv, min_pivot, ridge, 1e-3)
            } else {
                InvertOutcome::Invertible(inv, min_pivot)
            }
        }
    }
}

pub fn mat_vec(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum())
        .collect()
}
