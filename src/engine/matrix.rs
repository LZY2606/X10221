//! 方阵数值工具：行列式、2-范数条件数、逆与伪逆。不使用外部 BLAS。

#[derive(Debug, Clone)]
pub struct MatrixStats {
    pub determinant: f64,
    pub condition_number: f64,
    pub invertible: bool,
}

/// 带部分选主元 Gauss-Jordan 求逆，顺带得到行列式；条件数用 2-范数幂迭代估算。
pub fn analyze(a: &[Vec<f64>]) -> Option<MatrixStats> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return None;
    }
    // 行列式：先做一次 LU。
    let mut lu = a.to_vec();
    let mut swaps: i64 = 0;
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|i, j| lu[*i][col].abs().partial_cmp(&lu[*j][col].abs()).unwrap())?;
        if pivot != col {
            lu.swap(pivot, col);
            swaps += 1;
        }
        let piv = lu[col][col];
        if !piv.is_finite() {
            return None;
        }
        if piv.abs() < 1e-15 {
            return Some(MatrixStats {
                determinant: 0.0,
                condition_number: f64::INFINITY,
                invertible: false,
            });
        }
        for row in (col + 1)..n {
            let factor = lu[row][col] / piv;
            lu[row][col] = factor;
            for k in (col + 1)..n {
                lu[row][k] -= factor * lu[col][k];
            }
        }
    }
    let mut det: f64 = if swaps % 2 == 0 { 1.0 } else { -1.0 };
    for i in 0..n {
        det *= lu[i][i];
    }
    let inv = inverse(a).ok()?;
    let cn = norm2(a) * norm2(&inv);
    Some(MatrixStats {
        determinant: det,
        condition_number: cn,
        invertible: det.abs() >= 1e-12 && cn.is_finite(),
    })
}

/// 2-范数：幂迭代求 A^T A 最大特征值后开方。
fn norm2(x: &[Vec<f64>]) -> f64 {
    let n = x.len();
    let m = x.first().map(|r| r.len()).unwrap_or(0);
    let mut v = vec![1.0 / (m as f64).sqrt(); m];
    let mut eig = 0.0;
    for _ in 0..300 {
        let av: Vec<f64> = (0..n)
            .map(|i| (0..m).map(|j| x[i][j] * v[j]).sum())
            .collect();
        let w: Vec<f64> = (0..m)
            .map(|j| (0..n).map(|i| x[i][j] * av[i]).sum())
            .collect();
        let norm = w.iter().map(|z| z * z).sum::<f64>().sqrt();
        if norm < 1e-18 {
            return 0.0;
        }
        v = w.into_iter().map(|z| z / norm).collect();
        eig = norm;
    }
    eig.sqrt()
}

/// Gauss-Jordan 带部分选主元求逆。
pub fn inverse(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, ()> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return Err(());
    }
    let mut aug: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = Vec::with_capacity(2 * n);
        row.extend_from_slice(&a[i]);
        row.extend(std::iter::repeat(0.0).take(n));
        row[n + i] = 1.0;
        aug.push(row);
    }
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|i, j| aug[*i][col].abs().partial_cmp(&aug[*j][col].abs()).unwrap())
            .ok_or(())?;
        if aug[pivot][col].abs() < 1e-12 {
            return Err(());
        }
        aug.swap(pivot, col);
        let piv = aug[col][col];
        for k in 0..2 * n {
            aug[col][k] /= piv;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = aug[row][col];
            for k in 0..2 * n {
                aug[row][k] -= factor * aug[col][k];
            }
        }
    }
    Ok(aug.iter().map(|r| r[n..].to_vec()).collect())
}

/// 显式标注的伪逆候选路径（A^+ = A^T (A A^T)^-1），绝不是默认静默回退。
pub fn pseudoinverse(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, ()> {
    let at = transpose(a);
    let aat = matmul(a, &at);
    let inv_aat = inverse(&aat)?;
    Ok(matmul(&at, &inv_aat))
}

pub fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = a.first().map(|r| r.len()).unwrap_or(0);
    (0..m)
        .map(|j| (0..n).map(|i| a[i][j]).collect())
        .collect()
}

pub fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b.first().map(|r| r.len()).unwrap_or(0);
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

/// 近奇异阈值：条件数超过该值则阻断并给出全部合法候选，不允许默默求逆。
pub const NEAR_SINGULAR_COND: f64 = 1e6;
