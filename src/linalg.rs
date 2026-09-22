//! 补偿矩阵线性代数：近奇异判定、显式正则逆候选。
//! 近奇异矩阵不会被默默求逆——调用方必须基于 rcond 诊断做裁定。

#[derive(Debug, Clone)]
pub struct MatrixReport {
    pub n: usize,
    pub singular: bool,
    pub near_singular: bool,
    pub rcond: Option<f64>,
    pub cond1: Option<f64>,
    pub determinant: Option<f64>,
    pub diagnose: Vec<String>,
}

fn lup_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, usize, f64)> {
    let n = a.len();
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut swaps = 0usize;
    for k in 0..n {
        let mut max = k;
        for i in (k + 1)..n {
            if lu[i][k].abs() > lu[max][k].abs() {
                max = i;
            }
        }
        let pivot = lu[max][k];
        if pivot.abs() < 1e-15 {
            return None;
        }
        if max != k {
            lu.swap(max, k);
            piv.swap(max, k);
            swaps += 1;
        }
        for i in (k + 1)..n {
            lu[i][k] /= lu[k][k];
            let factor = lu[i][k];
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }
    let mut det = if swaps % 2 == 0 { 1.0 } else { -1.0 };
    for i in 0..n {
        det *= lu[i][i];
    }
    Some((lu, piv, swaps, det))
}

fn lup_solve(lu: &[Vec<f64>], piv: &[usize], b: &[f64]) -> Vec<f64> {
    let n = lu.len();
    let mut x = vec![0.0; n];
    for i in 0..n {
        let mut s = b[piv[i]];
        for j in 0..i {
            s -= lu[i][j] * x[j];
        }
        x[i] = s;
    }
    for i in (0..n).rev() {
        for j in (i + 1)..n {
            x[i] -= lu[i][j] * x[j];
        }
        x[i] /= lu[i][i];
    }
    x
}

fn norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    (0..n)
        .map(|j| (0..n).map(|i| a[i][j].abs()).sum::<f64>())
        .fold(0.0f64, f64::max)
}

/// Hager/LAPACK dgecon 风格的 1-范数倒数条件数估计。
/// 返回 None 表示完全奇异。
pub fn rcond1(a: &[Vec<f64>]) -> Option<f64> {
    let n = a.len();
    let (lu, piv, _, _) = lup_decompose(a)?;
    let anorm = norm1(a);
    if anorm == 0.0 {
        return Some(0.0);
    }
    // x := A^-1 e / n  (e 为全 1)
    let mut x = lup_solve(&lu, &piv, &vec![1.0; n]);
    let scale = n as f64;
    for v in x.iter_mut() {
        *v /= scale;
    }
    let mut gamma = 0.0;
    for _ in 0..2 {
        let xi: Vec<f64> = x.iter().map(|v| if *v >= 0.0 { 1.0 } else { -1.0 }).collect();
        let z = lup_solve(&lu, &piv, &xi);
        let z1: f64 = z.iter().map(|v| v.abs()).sum();
        if z1 <= gamma {
            break;
        }
        gamma = z1;
        let jmax = (0..n)
            .max_by(|a, b| z[*a].abs().partial_cmp(&z[*b].abs()).unwrap())
            .unwrap();
        let mut e = vec![0.0; n];
        e[jmax] = 1.0;
        x = lup_solve(&lu, &piv, &e);
        let x1: f64 = x.iter().map(|v| v.abs()).sum::<f64>();
        if x1 < gamma {
            // 估计不再增长
            break;
        }
    }
    let est = gamma.max(norm_of_inverse_direct(&lu, &piv));
    Some(1.0 / (anorm * est.max(1.0)))
}

fn norm_of_inverse_direct(lu: &[Vec<f64>], piv: &[usize]) -> f64 {
    let n = lu.len();
    let mut best = 0.0f64;
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        let col = lup_solve(lu, piv, &e);
        best = best.max(col.iter().map(|v| v.abs()).sum::<f64>());
    }
    best
}

/// 高斯-若尔当求逆。仅在调用方已确认非奇异且条件数可接受时使用。
pub fn invert(a: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let n = a.len();
    if a.iter().any(|r| r.len() != n) {
        return Err("矩阵不是方阵".into());
    }
    let mut m: Vec<Vec<f64>> = a
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            let mut id = vec![0.0; n];
            id[i] = 1.0;
            r.extend(id);
            r
        })
        .collect();
    for col in 0..n {
        let mut pivot = col;
        for r in (col + 1)..n {
            if m[r][col].abs() > m[col][col].abs() {
                pivot = r;
            }
        }
        let p = m[pivot][col];
        if p.abs() < 1e-12 {
            return Err("矩阵奇异，无法求逆".into());
        }
        m.swap(pivot, col);
        for j in 0..2 * n {
            m[col][j] /= p;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = m[r][col];
            for j in 0..2 * n {
                m[r][j] -= factor * m[col][j];
            }
        }
    }
    Ok(m.iter().map(|r| r[n..].to_vec()).collect())
}

/// Tikhonov 正则逆：(A^T A + lambda I)^-1 A^T。
/// 这是显式候选，不自动替换任何结果。
pub fn regularized_inverse(a: &[Vec<f64>], lambda: f64) -> Result<Vec<Vec<f64>>, String> {
    let n = a.len();
    let mut ata = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += a[k][i] * a[k][j];
            }
            ata[i][j] = s;
        }
        ata[i][i] += lambda;
    }
    let inv = invert(&ata)?;
    let mut out = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += inv[i][k] * a[j][k];
            }
            out[i][j] = s;
        }
    }
    Ok(out)
}

pub fn report(a: &[Vec<f64>], near_threshold: f64) -> MatrixReport {
    let n = a.len();
    let mut diagnose = Vec::new();
    let determinant = lup_decompose(a).map(|(_, _, _, d)| d);
    let singular = match invert(a) {
        Ok(_) => false,
        Err(e) => {
            diagnose.push(e);
            true
        }
    };
    let rc = rcond1(a);
    let near = !singular && matches!(rc, Some(v) if v < near_threshold);
    if near {
        diagnose.push(format!(
            "近奇异：rcond={:.3e} < 阈值 {:.0e}，拒绝默默求逆，必须人工裁定",
            rc.unwrap(),
            near_threshold
        ));
    }
    let cond1 = rc.map(|v| if v > 0.0 { 1.0 / v } else { f64::INFINITY });
    MatrixReport {
        n,
        singular,
        near_singular: near,
        rcond: rc,
        cond1,
        determinant,
        diagnose,
    }
}

pub fn apply(inv: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    let n = inv.len();
    (0..n)
        .map(|i| (0..n).map(|j| inv[i][j] * v[j]).sum())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_well_conditioned() {
        let a = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let r = report(&a, 1e-8);
        assert!(!r.singular && !r.near_singular);
        assert!(r.rcond.unwrap() > 0.5);
        assert_eq!(invert(&a).unwrap(), a);
    }

    #[test]
    fn exact_singular_detected() {
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let r = report(&a, 1e-8);
        assert!(r.singular);
        assert!(!r.near_singular);
        assert!(invert(&a).is_err());
    }

    #[test]
    fn near_singular_flagged_and_regularized_available() {
        // 条件数约 2e10
        let a = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 5e-11]];
        let r = report(&a, 1e-8);
        assert!(!r.singular);
        assert!(r.near_singular, "rcond={:?}", r.rcond);
        assert!(regularized_inverse(&a, 1e-6).is_ok());
    }
}
