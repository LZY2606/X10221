#[derive(Debug, Clone, PartialEq)]
pub struct SingularReport {
    pub determinant: f64,
    pub min_pivot: f64,
    pub message: String,
}

const PIVOT_TOL: f64 = 1e-9;
const DET_TOL: f64 = 1e-10;

/// Gauss-Jordan inversion with partial pivoting.
///
/// 近奇异矩阵不会被默默求逆：主元过小或行列式相对尺度过低时返回 `SingularReport`，
/// 由上层记录为机器可读取的 `compensation.near_singular_rejected` 事件并保留候选。
pub fn invert(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, SingularReport> {
    let n = matrix.len();
    if n == 0 || matrix.iter().any(|r| r.len() != n) {
        return Err(SingularReport {
            determinant: 0.0,
            min_pivot: 0.0,
            message: "matrix is not square".to_string(),
        });
    }
    let mut a: Vec<Vec<f64>> = Vec::with_capacity(n);
    for (i, row) in matrix.iter().enumerate() {
        let mut r = row.clone();
        r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
        a.push(r);
    }
    let mut det = 1.0_f64;
    let mut min_pivot = f64::MAX;
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&i, &j| {
                a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap()
            })
            .unwrap();
        let pivot_value = a[pivot][col].abs();
        min_pivot = min_pivot.min(pivot_value);
        if pivot_value < PIVOT_TOL {
            return Err(SingularReport {
                determinant: 0.0,
                min_pivot: pivot_value,
                message: format!("pivot {pivot_value:.3e} at column {col} below tolerance {PIVOT_TOL:.0e}"),
            });
        }
        if pivot != col {
            a.swap(pivot, col);
            det = -det;
        }
        let p = a[col][col];
        det *= p;
        for j in 0..2 * n {
            a[col][j] /= p;
        }
        for i in 0..n {
            if i == col {
                continue;
            }
            let factor = a[i][col];
            if factor == 0.0 {
                continue;
            }
            for j in 0..2 * n {
                a[i][j] -= factor * a[col][j];
            }
        }
    }
    let scale: f64 = matrix
        .iter()
        .map(|r| r.iter().map(|x| x * x).sum::<f64>().sqrt())
        .product::<f64>()
        .max(f64::MIN_POSITIVE);
    if det.abs() < DET_TOL * scale {
        return Err(SingularReport {
            determinant: det,
            min_pivot,
            message: format!(
                "|det| = {:.3e} below {:.0e} times row-norm scale {:.3e}",
                det.abs(),
                DET_TOL,
                scale
            ),
        });
    }
    Ok(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

/// `observed = spillover * true`，补偿量为 `inverse(spillover) * observed`。
pub fn compensate(inverse: &[Vec<f64>], observed: &[f64]) -> Vec<f64> {
    inverse
        .iter()
        .map(|row| row.iter().zip(observed).map(|(m, x)| m * x).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_healthy_spillover() {
        let spill = vec![vec![1.0, 0.15], vec![0.05, 1.0]];
        let inv = invert(&spill).expect("healthy matrix inverts");
        let back = compensate(&inv, &[1.0, 0.0]);
        assert!((back[0] - 1.0).abs() < 1e-9);
        assert!(back[1].abs() < 1e-9);
        let det = 1.0 * 1.0 - 0.15 * 0.05;
        assert!((inv[0][0] - 1.0 / det).abs() < 1e-9);
        assert!((inv[0][1] - -0.15 / det).abs() < 1e-9);
    }

    #[test]
    fn rejects_near_singular_matrix() {
        let near = vec![vec![1.0, 0.15], vec![0.15, 0.022_500_000_000_01]];
        let report = invert(&near).expect_err("near-singular matrix must be rejected");
        assert!(report.min_pivot < 1e-9 || report.determinant.abs() < 1e-8);
    }

    #[test]
    fn rejects_exactly_singular_matrix() {
        let singular = vec![vec![1.0, 0.5], vec![2.0, 1.0]];
        assert!(invert(&singular).is_err());
    }

    #[test]
    fn rejects_non_square_matrix() {
        assert!(invert(&[vec![1.0, 2.0]]).is_err());
    }
}
