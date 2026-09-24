//! 领域核心：补偿矩阵条件数、门多边形几何、信号变换。
//! 纯函数，不触碰存储，便于单元测试。

pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-6;
pub const BOUNDARY_EPS: f64 = 1e-9;

/// 高斯消元（部分主元）求行列式。
pub fn det(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return 0.0;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut sign = 1.0;
    let mut d = 1.0;
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return 0.0;
        }
        if piv != col {
            a.swap(piv, col);
            sign = -sign;
        }
        let p = a[col][col];
        d *= p;
        for r in (col + 1)..n {
            let f = a[r][col] / p;
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    sign * d
}

/// 归一化行列式：|det| / ∏行绝对值和。接近 0 表示近奇异。
pub fn normalized_det(m: &[Vec<f64>]) -> f64 {
    let d = det(m).abs();
    let scale: f64 = m
        .iter()
        .map(|r| r.iter().map(|x| x.abs()).sum::<f64>().max(1e-300))
        .product();
    d / scale
}

pub fn is_near_singular(m: &[Vec<f64>]) -> bool {
    normalized_det(m) < NEAR_SINGULAR_THRESHOLD
}

/// 高斯-若尔当求逆。仅在通过条件检查或显式正则化后调用。
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(piv, col);
        inv.swap(piv, col);
        let p = a[col][col];
        for c in 0..n {
            a[col][c] /= p;
            inv[col][c] /= p;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                for c in 0..n {
                    a[r][c] -= f * a[col][c];
                    inv[r][c] -= f * inv[col][c];
                }
            }
        }
    }
    Some(inv)
}

/// 岭正则化逆：(M + eps·I)^-1，作为近奇异时的显式候选之一。
pub fn ridge_inverse(m: &[Vec<f64>], eps: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let reg: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| m[i][j] + if i == j { eps } else { 0.0 })
                .collect()
        })
        .collect();
    inverse(&reg)
}

pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointPosition {
    Inside,
    Outside,
    Boundary,
}

/// 点与多边形关系；落在边上（容差内）判定为 Boundary，不默默归入任一侧。
pub fn classify_point(p: [f64; 2], poly: &[[f64; 2]]) -> PointPosition {
    let n = poly.len();
    if n < 3 {
        return PointPosition::Outside;
    }
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        let edge_len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let tol = BOUNDARY_EPS * (edge_len + 1.0);
        if cross.abs() <= tol {
            let dot = (p[0] - a[0]) * (p[0] - b[0]) + (p[1] - a[1]) * (p[1] - b[1]);
            if dot <= tol {
                return PointPosition::Boundary;
            }
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > p[1]) != (yj > p[1]) && p[0] < (xj - xi) * (p[1] - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        PointPosition::Inside
    } else {
        PointPosition::Outside
    }
}

/// arcsinh 变换（流式常用显示变换的简化形式），cofactor 携带坐标基准。
pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = vec![
            vec![1.0, 0.15, 0.02],
            vec![0.05, 1.0, 0.08],
            vec![0.01, 0.03, 1.0],
        ];
        assert!(!is_near_singular(&m));
        let inv = inverse(&m).unwrap();
        let prod = mat_vec(&inv, &[1.0, 2.0, 3.0]);
        let back = mat_vec(&m, &prod);
        for (a, b) in back.iter().zip([1.0, 2.0, 3.0].iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn near_singular_matrix_is_flagged_not_inverted_silently() {
        let m = vec![
            vec![1.0, 0.5, 0.25],
            vec![0.5, 1.0, 0.5],
            vec![1.5 + 1e-9, 1.5, 0.75],
        ];
        assert!(is_near_singular(&m));
        // 显式岭候选仍可用，但它是带参数的候选而非默认行为
        let ridge = ridge_inverse(&m, 1e-3).unwrap();
        assert_eq!(ridge.len(), 3);
    }

    #[test]
    fn boundary_point_is_not_silently_classified() {
        let poly = vec![[1.5, 0.5], [2.0, 0.5], [2.0, 3.0], [1.5, 3.0]];
        assert_eq!(classify_point([2.0, 1.8], &poly), PointPosition::Boundary);
        assert_eq!(classify_point([1.8, 1.8], &poly), PointPosition::Inside);
        assert_eq!(classify_point([2.5, 1.8], &poly), PointPosition::Outside);
    }

    #[test]
    fn asinh_roundtrip_scale() {
        let t = asinh_transform(150.0 * (2.0f64).sinh(), 150.0);
        assert!((t - 2.0).abs() < 1e-12);
    }
}
