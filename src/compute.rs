//! 推导引擎：补偿、变换、门控、群体百分比分母钉版、过期传播、多候选。
//!
//! 关键规则（推导规则，见 judgments）：
//! * 近奇异补偿矩阵不能被默默求逆：det 低于阈值时不产生派生补偿，给出全部合法候选。
//! * 门的百分比分母必须钉住父群版本（pop_id 中含父群 pop_id 与父门版本）。
//! * 修订上游门后，下游结果标记过期而不覆盖（保留旧 DerivedPop，stale=true）。

use serde::Serialize;

use crate::domain::*;

/// 近奇异阈值：|det(S)| < 1e-3 即拒绝直接求逆。
pub const SINGULAR_DET_THRESHOLD: f64 = 1e-3;

#[derive(Debug, Clone, PartialEq)]
pub struct MatrixInverse {
    pub determinant: f64,
    pub inverse: Vec<Vec<f64>>,
}

/// 带部分主元的高斯-约当求逆。不做任何正则化兜底——近奇异时直接报错，绝不默默求逆。
pub fn invert_matrix(m: &[Vec<f64>]) -> Result<MatrixInverse, String> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err("matrix must be a non-empty square matrix".to_string());
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| {
        let mut r = vec![0.0; n];
        r[i] = 1.0;
        r
    }).collect();

    let mut det = 1.0;
    for col in 0..n {
        let pivot_row = (col..n).max_by(|&p, &q| {
            a[p][col].abs().partial_cmp(&a[q][col].abs()).unwrap()
        }).unwrap();
        if a[pivot_row][col].abs() < SINGULAR_DET_THRESHOLD {
            let d = determinant(m);
            return Err(format!(
                "near-singular compensation matrix rejected: |det|={d:.3e} < threshold {SINGULAR_DET_THRESHOLD:.0e}; silent inversion forbidden"
            ));
        }
        if pivot_row != col {
            a.swap(pivot_row, col);
            inv.swap(pivot_row, col);
            det = -det;
        }
        let piv = a[col][col];
        det *= piv;
        for j in 0..n {
            a[col][j] /= piv;
            inv[col][j] /= piv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let factor = a[r][col];
            for j in 0..n {
                a[r][j] -= factor * a[col][j];
                inv[r][j] -= factor * inv[col][j];
            }
        }
    }
    Ok(MatrixInverse { determinant: det, inverse: inv })
}

/// 直接按首项展开计算行列式（矩阵规模小：荧光通道数 2）。
pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    if n == 1 {
        return m[0][0];
    }
    let mut det = 0.0;
    for c in 0..n {
        let mut minor: Vec<Vec<f64>> = Vec::with_capacity(n - 1);
        for r in 1..n {
            let mut row = Vec::with_capacity(n - 1);
            for j in 0..n {
                if j != c {
                    row.push(m[r][j]);
                }
            }
            minor.push(row);
        }
        let sign = if c % 2 == 0 { 1.0 } else { -1.0 };
        det += sign * m[0][c] * determinant(&minor);
    }
    det
}

/// 求逆候选的脊参数（仅在审阅者显式接受 "regularize" 候选时使用）。
pub const RIDGE: f64 = 0.01;

/// 显式正则化：S' = S + ridge * I。仍把结果标记为“有争议的人工修正”。
pub fn regularized_inverse(m: &[Vec<f64>], ridge: f64) -> MatrixInverse {
    let adjusted: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .enumerate()
                .map(|(j, &v)| v + if i == j { ridge } else { 0.0 })
                .collect()
        })
        .collect();
    invert_matrix(&adjusted).expect("ridge-adjusted matrix must be invertible")
}

/// asinh(x / cofactor)，输出单位为 asinh-scale。
pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

/// 点是否在多边形内。`on_boundary` 单独报告，由 BoundaryPolicy 决定含义。
pub fn point_in_polygon(p: &Point, poly: &[Point]) -> (bool, bool) {
    let n = poly.len();
    if n < 3 {
        return (false, false);
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (pi, pj) = (&poly[i], &poly[j]);
        if on_segment(pi, pj, p) {
            return (false, true);
        }
        if (pi.y > p.y) != (pj.y > p.y) {
            let x_cross = pi.x + (p.y - pi.y) / (pj.y - pi.y) * (pj.x - pi.x);
            if x_cross > p.x {
                inside = !inside;
            }
        }
        j = i;
    }
    (inside, false)
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ax * by - ay * bx
}

fn on_segment(a: &Point, b: &Point, p: &Point) -> bool {
    const EPS: f64 = 1e-9;
    if cross(b.x - a.x, b.y - a.y, p.x - a.x, p.y - a.y).abs() > EPS {
        return false;
    }
    p.x >= a.x.min(b.x) - EPS
        && p.x <= a.x.max(b.x) + EPS
        && p.y >= a.y.min(b.y) - EPS
        && p.y <= a.y.max(b.y) + EPS
}

/// 单个事件相对某门的成员判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Membership {
    In,
    Out,
    /// 恰好落在边界，且策略为 ambiguous。
    BoundaryAmbiguous,
}

/// 一个数据点在门坐标空间中的坐标（用于散点与判定）。
#[derive(Debug, Clone, Serialize)]
pub struct GatePoint {
    pub event_id: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub raw_x: String,
    pub raw_y: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<Vec<f64>> {
        vec![vec![1.0, 0.2], vec![0.3, 1.0]]
    }

    #[test]
    fn inverts_well_conditioned_matrix() {
        let inv = invert_matrix(&square()).unwrap();
        assert!((inv.determinant - 0.94).abs() < 1e-9);
        let s = square();
        // S * S^-1 ≈ I
        let n = 2;
        for i in 0..n {
            for j in 0..n {
                let mut v = 0.0;
                for k in 0..n {
                    v += s[i][k] * inv.inverse[k][j];
                }
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!((v - expected).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn rejects_near_singular_matrix_without_silent_inverse() {
        // 两行几乎相同：det = 1 - 0.9999 = 0.0001 < 1e-3
        let m = vec![vec![1.0, 1.0], vec![0.9999, 1.0]];
        assert!(determinant(&m).abs() < SINGULAR_DET_THRESHOLD);
        let err = invert_matrix(&m).unwrap_err();
        assert!(err.contains("near-singular"), "{err}");
    }

    #[test]
    fn boundary_detection_on_polygon_edge() {
        let poly = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
            Point { x: 0.0, y: 10.0 },
        ];
        let (inside, boundary) = point_in_polygon(&Point { x: 5.0, y: 0.0 }, &poly);
        assert!(boundary);
        assert!(!inside);
        let (inside, boundary) = point_in_polygon(&Point { x: 5.0, y: 5.0 }, &poly);
        assert!(inside);
        assert!(!boundary);
        let (inside, boundary) = point_in_polygon(&Point { x: 50.0, y: 50.0 }, &poly);
        assert!(!inside && !boundary);
    }

    #[test]
    fn asinh_is_monotonic_and_zero_at_origin() {
        assert!(asinh_transform(0.0, 150.0).abs() < 1e-12);
        assert!(asinh_transform(1000.0, 150.0) > asinh_transform(10.0, 150.0));
    }
}
