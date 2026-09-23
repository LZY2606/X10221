//! 补偿矩阵线性代数。
//!
//! 关键规则：近奇异补偿矩阵不能被默默求逆。
//! * 精确奇异（枢轴为 0）→ 无逆，阻断。
//! * 近奇异（最小/最大枢轴比低于阈值）→ 不静默返回普通逆，只给出带正则化
//!   的显式候选（Tikhonov / ridge），并报告放大因子，等待人工裁定。

#[derive(Debug, Clone, PartialEq)]
pub struct MatrixAnalysis {
    pub n: usize,
    pub determinant: f64,
    /// 部分主元 LU 的最小/最大非零枢轴之比；越小越接近奇异。
    pub pivot_ratio: f64,
    pub singular: bool,
    pub near_singular: bool,
    pub threshold: f64,
}

/// 部分主元 LU 分解，返回 (analysis, lu, perm)。
pub fn analyze(matrix: &[Vec<f64>], near_threshold: f64) -> MatrixAnalysis {
    let n = matrix.len();
    let mut m: Vec<Vec<f64>> = matrix.to_vec();
    let mut pivots: Vec<f64> = Vec::with_capacity(n);
    let mut det_sign = 1.0_f64;

    for col in 0..n {
        let mut max_row = col;
        let mut max_val = m[col][col].abs();
        for row in (col + 1)..n {
            if m[row][col].abs() > max_val {
                max_val = m[row][col].abs();
                max_row = row;
            }
        }
        if max_row != col {
            m.swap(col, max_row);
            det_sign = -det_sign;
        }
        let pivot = m[col][col];
        if pivot.abs() == 0.0 {
            return MatrixAnalysis {
                n,
                determinant: 0.0,
                pivot_ratio: 0.0,
                singular: true,
                near_singular: true,
                threshold: near_threshold,
            };
        }
        pivots.push(pivot.abs());
        for row in (col + 1)..n {
            let factor = m[row][col] / pivot;
            for k in col..n {
                m[row][k] -= factor * m[col][k];
            }
        }
    }

    let det: f64 =
        det_sign * pivots.iter().product::<f64>();
    let max_p = *pivots.iter().fold(&f64::MIN, |a, b| if b > a { b } else { a });
    let min_p = *pivots.iter().fold(&f64::MAX, |a, b| if b < a { b } else { a });
    let ratio = min_p / max_p;
    MatrixAnalysis {
        n,
        determinant: det,
        pivot_ratio: ratio,
        singular: false,
        near_singular: ratio < near_threshold,
        threshold: near_threshold,
    }
}

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut id = vec![vec![0.0; n]; n];
    for i in 0..n {
        id[i][i] = 1.0;
    }
    id
}

fn lup_solve(lu: &mut [Vec<f64>], perm: &[usize], rhs: &[f64]) -> Vec<f64> {
    let n = lu.len();
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = rhs[perm[i]];
        for j in 0..i {
            s -= lu[i][j] * y[j];
        }
        y[i] = s;
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for j in (i + 1)..n {
            s -= lu[i][j] * x[j];
        }
        x[i] = s / lu[i][i];
    }
    x
}

fn lup_factor(matrix: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>)> {
    let n = matrix.len();
    let mut m = matrix.to_vec();
    let mut perm: Vec<usize> = (0..n).collect();
    for col in 0..n {
        let mut max_row = col;
        let mut max_val = m[col][col].abs();
        for row in (col + 1)..n {
            if m[row][col].abs() > max_val {
                max_val = m[row][col].abs();
                max_row = row;
            }
        }
        if max_val == 0.0 {
            return None;
        }
        if max_row != col {
            m.swap(col, max_row);
            perm.swap(col, max_row);
        }
        let pivot = m[col][col];
        for row in (col + 1)..n {
            let factor = m[row][col] / pivot;
            m[row][col] = factor;
            for k in (col + 1)..n {
                m[row][k] -= factor * m[col][k];
            }
        }
    }
    Some((m, perm))
}

/// 普通数值逆（仅供条件良好的矩阵使用；调用前必须检查 analysis）。
pub fn inverse(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    if matrix.iter().any(|r| r.len() != n) {
        return None;
    }
    let (mut lu, perm) = lup_factor(matrix)?;
    let id = identity(n);
    let mut inv = vec![vec![0.0; n]; n];
    for col in 0..n {
        let col_vec: Vec<f64> = id.iter().map(|r| r[col]).collect();
        let sol = lup_solve(&mut lu, &perm, &col_vec);
        for row in 0..n {
            inv[row][col] = sol[row];
        }
    }
    Some(inv)
}

/// (S^T S + eps^2 I)^-1 S^T —— 显式正则化逆；用于近奇异矩阵的候选。
pub fn ridge_inverse(matrix: &[Vec<f64>], epsilon: f64) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    let mut sts = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut acc = 0.0;
            for k in 0..n {
                acc += matrix[k][i] * matrix[k][j];
            }
            sts[i][j] = acc;
        }
        sts[i][i] += epsilon * epsilon;
    }
    let mut st = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            st[i][j] = matrix[j][i];
        }
    }
    let (mut lu, perm) = lup_factor(&sts)?;
    let mut res = vec![vec![0.0; n]; n];
    for col in 0..n {
        let rhs: Vec<f64> = st.iter().map(|r| r[col]).collect();
        let sol = lup_solve(&mut lu, &perm, &rhs);
        for row in 0..n {
            res[row][col] = sol[row];
        }
    }
    Some(res)
}

/// 放大因子 ||M x|| 与 ||x|| 的最大比值（对标准基向量取最大值）。
pub fn amplification(matrix: &[Vec<f64>]) -> f64 {
    let n = matrix.len();
    let mut worst = 0.0;
    for col in 0..n {
        let norm: f64 = (0..n).map(|row| matrix[row][col].powi(2)).sum::<f64>().sqrt();
        worst = worst.max(norm);
    }
    worst
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v).map(|(a, b)| a * b).sum())
        .collect()
}
