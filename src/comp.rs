//! 补偿矩阵与几何判定的纯函数层。
//!
//! 领域规则：
//! - 近奇异补偿矩阵不能被默默求逆：`try_invert` 在近奇异时返回 [`SingularReport`]，
//!   由上层记录机器可读事件并保留合法候选，绝不 fallback 到“差不多”的逆矩阵。
//! - 门边界点无法唯一决定归属：`classify` 显式返回 [`Membership::Boundary`]，
//!   由上层同时保留 inside/outside 两个候选。

/// 近奇异判定阈值（相对行列式）。
pub const SINGULAR_TOL: f64 = 1e-6;
/// 主元下限（相对最大元素）。
pub const PIVOT_TOL: f64 = 1e-9;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SingularReport {
    pub determinant: f64,
    pub min_pivot: f64,
    pub condition_estimate: f64,
}

/// 高斯消元（部分主元）求行列式与最小主元。
fn lu_stats(m: &[Vec<f64>]) -> (f64, f64, f64) {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0f64;
    let mut min_pivot = f64::INFINITY;
    let mut max_abs = 0.0f64;
    for row in &a {
        for v in row {
            max_abs = max_abs.max(v.abs());
        }
    }
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if piv != col {
            a.swap(piv, col);
            det = -det;
        }
        let p = a[col][col];
        min_pivot = min_pivot.min(p.abs());
        det *= p;
        if p.abs() < 1e-300 {
            return (det, min_pivot, max_abs);
        }
        for r in (col + 1)..n {
            let f = a[r][col] / p;
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    (det, min_pivot, max_abs)
}

/// 尝试求逆。近奇异（相对行列式过小或主元过小）时拒绝并返回诊断。
pub fn try_invert(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, SingularReport> {
    let n = m.len();
    assert!(n > 0 && m.iter().all(|r| r.len() == n), "matrix must be square");
    let (det, min_pivot, max_abs) = lu_stats(m);
    let scale = max_abs.powi(n as i32);
    let condition_estimate = if min_pivot > 0.0 {
        max_abs / min_pivot
    } else {
        f64::INFINITY
    };
    if det.abs() < SINGULAR_TOL * scale || min_pivot < PIVOT_TOL * max_abs {
        return Err(SingularReport {
            determinant: det,
            min_pivot,
            condition_estimate,
        });
    }
    // Gauss-Jordan with partial pivoting.
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
                if f != 0.0 {
                    for c in 0..n {
                        a[r][c] -= f * a[col][c];
                        inv[r][c] -= f * inv[col][c];
                    }
                }
            }
        }
    }
    Ok(inv)
}

/// 用逆矩阵补偿观测向量：true = inv * observed。
pub fn compensate(inv: &[Vec<f64>], observed: &[f64]) -> Vec<f64> {
    inv.iter()
        .map(|row| row.iter().zip(observed).map(|(a, b)| a * b).sum())
        .collect()
}

/// asinh 变换（流式常用 biexponential 的简化形式），坐标基准由调用方记录。
pub fn asinh_transform(v: f64, cofactor: f64) -> f64 {
    (v / cofactor).asinh()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Membership {
    Inside,
    Outside,
    /// 恰好落在多边形边上：无法唯一决定，需保留双候选。
    Boundary,
}

fn point_seg_dist(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// 射线法 + 边界检测。边界阈值取多边形包围盒对角线的 1e-6。
pub fn classify(x: f64, y: f64, poly: &[[f64; 2]]) -> Membership {
    let n = poly.len();
    if n < 3 {
        return Membership::Outside;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in poly {
        minx = minx.min(p[0]);
        miny = miny.min(p[1]);
        maxx = maxx.max(p[0]);
        maxy = maxy.max(p[1]);
    }
    let diag = ((maxx - minx).powi(2) + (maxy - miny).powi(2)).sqrt();
    let eps = 1e-6 * diag.max(1.0);
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if point_seg_dist(x, y, a[0], a[1], b[0], b[1]) <= eps {
            return Membership::Boundary;
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_well_conditioned() {
        let m = vec![
            vec![1.00, 0.02, 0.00],
            vec![0.12, 1.00, 0.04],
            vec![0.01, 0.35, 1.00],
        ];
        let inv = try_invert(&m).expect("well-conditioned matrix must invert");
        for i in 0..3 {
            for j in 0..3 {
                let s: f64 = (0..3).map(|k| inv[i][k] * m[k][j]).sum();
                let expect = if i == j { 1.0 } else { 0.0 };
                assert!((s - expect).abs() < 1e-9, "({i},{j}) = {s}");
            }
        }
    }

    #[test]
    fn near_singular_is_rejected_not_silently_inverted() {
        // PE 与 PerCP 探测行几乎共线：det ≈ -1e-7。
        let m = vec![
            vec![1.00, 0.02, 0.00],
            vec![0.12, 1.00, 0.5000001],
            vec![0.12, 1.00, 0.5],
        ];
        let report = try_invert(&m).expect_err("near-singular matrix must be rejected");
        assert!(report.determinant.abs() < 1e-6);
        assert!(report.condition_estimate > 1.0);
    }

    #[test]
    fn exactly_singular_is_rejected() {
        let m = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        assert!(try_invert(&m).is_err());
    }

    #[test]
    fn boundary_point_is_not_uniquely_decidable() {
        let poly = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(classify(5.0, 0.0, &poly), Membership::Boundary);
        assert_eq!(classify(5.0, 5.0, &poly), Membership::Inside);
        assert_eq!(classify(15.0, 5.0, &poly), Membership::Outside);
    }

    #[test]
    fn asinh_roundtrip_sign() {
        assert!(asinh_transform(-50.0, 150.0) < 0.0);
        assert!((asinh_transform(0.0, 150.0)).abs() < 1e-12);
    }
}
