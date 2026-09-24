//! 补偿矩阵求逆。核心规则：近奇异矩阵不能被默默求逆，
//! 必须返回结构化诊断，由上层生成候选与事件，等待人工裁定。

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NearSingular {
    pub det: f64,
    pub min_pivot: f64,
    pub threshold: f64,
    pub size: usize,
}

pub const PIVOT_THRESHOLD: f64 = 1e-9;
pub const DET_THRESHOLD: f64 = 1e-8;

/// 高斯消元（部分主元）求逆；主元或行列式低于阈值时拒绝并给出依据。
pub fn try_invert(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, NearSingular> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
            r
        })
        .collect();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut best = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[best][col].abs() {
                best = r;
            }
        }
        let pivot = a[best][col].abs();
        if pivot < min_pivot {
            min_pivot = pivot;
        }
        det *= if best == col { a[best][col] } else { -a[best][col] };
        if pivot < PIVOT_THRESHOLD {
            return Err(NearSingular { det, min_pivot, threshold: PIVOT_THRESHOLD, size: n });
        }
        if best != col {
            a.swap(best, col);
        }
        let pv = a[col][col];
        for j in 0..(2 * n) {
            a[col][j] /= pv;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..(2 * n) {
                        a[r][j] -= f * a[col][j];
                    }
                }
            }
        }
    }
    if det.abs() < DET_THRESHOLD {
        return Err(NearSingular { det, min_pivot, threshold: DET_THRESHOLD, size: n });
    }
    Ok(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

/// y = M * x
pub fn mat_vec(m: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    m.iter().map(|row| row.iter().zip(x).map(|(a, b)| a * b).sum()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverts_well_conditioned() {
        let m = vec![vec![1.0, 0.2], vec![0.05, 1.0]];
        let inv = try_invert(&m).expect("well conditioned");
        let prod = mat_vec(&inv, &[1.0, 1.0]);
        let orig = mat_vec(&m, &prod);
        assert!((orig[0] - 1.0).abs() < 1e-9);
        assert!((orig[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_near_singular_with_basis() {
        // 两行几乎相同：det 接近 0
        let m = vec![vec![1.0, 0.5], vec![1.0, 0.500000001]];
        let err = try_invert(&m).expect_err("must refuse");
        assert!(err.det.abs() < DET_THRESHOLD);
        assert!(err.threshold > 0.0);
    }

    #[test]
    fn rejects_exactly_singular() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(try_invert(&m).is_err());
    }
}
