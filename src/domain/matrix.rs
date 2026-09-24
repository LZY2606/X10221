/// 近奇异判定阈值（1-范数条件数倒数 rcond）。
pub const NEAR_RCOND: f64 = 1e-4;
/// 精确奇异的主元阈值。
pub const SINGULAR_PIVOT: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq)]
pub struct MatrixAnalysis {
    pub n: usize,
    pub determinant: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub condition_number: Option<f64>,
    pub reciprocal_condition: Option<f64>,
    pub singular: bool,
    pub near_singular: bool,
    pub diagnostics: Vec<String>,
}

pub fn is_square(m: &[Vec<f64>]) -> bool {
    let n = m.len();
    n > 0 && m.iter().all(|r| r.len() == n)
}

fn norm1(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    (0..n)
        .map(|j| (0..n).map(|i| m[i][j].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// 部分主元 Gauss-Jordan：同时求行列式与逆；零主元标记精确奇异。
pub fn analyze(m: &[Vec<f64>]) -> MatrixAnalysis {
    let mut diagnostics = Vec::new();
    if !is_square(m) {
        return MatrixAnalysis {
            n: m.len(),
            determinant: f64::NAN,
            inverse: None,
            condition_number: None,
            reciprocal_condition: None,
            singular: true,
            near_singular: false,
            diagnostics: vec!["补偿矩阵不是方阵".to_string()],
        };
    }
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    let mut det = 1.0_f64;
    let mut singular = false;

    for col in 0..n {
        let pivot = (col..n).max_by(|x, y| a[*x][col].abs().partial_cmp(&a[*y][col].abs()).unwrap()).unwrap();
        if a[pivot][col].abs() < SINGULAR_PIVOT {
            singular = true;
            diagnostics.push(format!("第 {} 列无有效主元（|pivot|<{:.0e}），矩阵奇异，不能求逆", col, SINGULAR_PIVOT));
            break;
        }
        if pivot != col {
            a.swap(pivot, col);
            inv.swap(pivot, col);
            det = -det;
        }
        let piv = a[col][col];
        det *= piv;
        for j in 0..n {
            a[col][j] /= piv;
            inv[col][j] /= piv;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            for j in 0..n {
                a[row][j] -= factor * a[col][j];
                inv[row][j] -= factor * inv[col][j];
            }
        }
    }

    let inverse = if singular { None } else { Some(inv) };
    let mut condition_number = None;
    let mut reciprocal_condition = None;
    let mut near_singular = false;
    if let Some(inv) = &inverse {
        let cond = norm1(m) * norm1(inv);
        condition_number = Some(cond);
        if cond.is_finite() && cond > 0.0 {
            let rcond = 1.0 / cond;
            reciprocal_condition = Some(rcond);
            if rcond < NEAR_RCOND {
                near_singular = true;
                diagnostics.push(format!(
                    "近奇异补偿矩阵：rcond={:.3e} < {:.0e}，不得默默求逆，必须显式给出全部合法候选并等待裁定",
                    rcond, NEAR_RCOND
                ));
            }
        }
    }
    MatrixAnalysis { n, determinant: det, inverse, condition_number, reciprocal_condition, singular, near_singular, diagnostics }
}

/// Tikhonov 脊逆 (S^T S + λI)^-1 S^T，近奇异时的合法稳定候选之一。
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut sts = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += m[k][i] * m[k][j];
            }
            sts[i][j] = s;
        }
    }
    for i in 0..n {
        sts[i][i] += lambda;
    }
    let sts_inv = analyze(&sts).inverse?;
    // inv = sts_inv * S^T
    let mut out = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += sts_inv[i][k] * m[j][k];
            }
            out[i][j] = s;
        }
    }
    Some(out)
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter().map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum()).collect()
}

pub fn max_abs(m: &[Vec<f64>]) -> f64 {
    m.iter().flat_map(|r| r.iter().map(|x| x.abs())).fold(0.0, f64::max)
}
