//! 计算层：补偿矩阵求解、asinh 线性化、多边形门控与失效范围。
//! 关键规则：近奇异矩阵不默默求逆；无法转换的值留在原位置并附诊断；
//! 边界点采用“在边上即包含”的一致判定。

use serde_json::{json, Value};

/// 近奇异阈值：最小主元/最大主元低于该值即拒绝默默求逆。
pub const NEAR_SINGULAR_EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub struct RawEvent {
    pub id: String,
    /// 通道 -> 原始表示（字符串保留；数值为仪器线性单位 a.u.）
    pub values: Vec<(String, String)>,
}

/// 一个通道在一个事件上的取值：要么可转换为 f64，要么携带无法转换诊断。
#[derive(Debug, Clone)]
pub enum Cell {
    Number(f64),
    Unconvertible(String),
}

impl Cell {
    pub fn parse(raw: &str) -> Cell {
        let t = raw.trim();
        if let Ok(v) = t.parse::<f64>() {
            if v.is_finite() {
                return Cell::Number(v);
            }
        }
        Cell::Unconvertible(t.to_string())
    }
    pub fn as_opt(&self) -> Option<f64> {
        match self {
            Cell::Number(v) => Some(*v),
            Cell::Unconvertible(_) => None,
        }
    }
}

/// 求 det（带部分主元高斯消元），用于奇异/近奇异判定。
pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a = m.to_vec();
    let mut det = 1.0;
    for k in 0..n {
        let mut pivot = k;
        let mut best = a[k][k].abs();
        for i in (k + 1)..n {
            if a[i][k].abs() > best {
                best = a[i][k].abs();
                pivot = i;
            }
        }
        if best == 0.0 {
            return 0.0;
        }
        if pivot != k {
            a.swap(k, pivot);
            det = -det;
        }
        let piv = a[k][k];
        det *= piv;
        for i in (k + 1)..n {
            let factor = a[i][k] / piv;
            for j in (k + 1)..n {
                a[i][j] -= factor * a[k][j];
            }
        }
    }
    det
}

/// 矩阵健康度诊断。返回 (severity, code, evidence)。
pub fn matrix_health(m: &[Vec<f64>]) -> (String, String, Value) {
    let n = m.len();
    let square = !m.is_empty() && m.iter().all(|r| r.len() == n);
    let det = determinant(m);
    let max_scale = m
        .iter()
        .flatten()
        .fold(1f64, |acc, v| acc.max(v.abs()));
    let scaled = det.abs() / max_scale.powi(n as i32);
    let mut code = "MATRIX_OK";
    let mut severity = "ok";
    if !square {
        code = "MATRIX_NOT_SQUARE";
        severity = "error";
    } else if det == 0.0 || scaled < 1e-15 {
        code = "MATRIX_SINGULAR";
        severity = "error";
    } else if scaled < NEAR_SINGULAR_EPS {
        code = "MATRIX_NEAR_SINGULAR";
        severity = "warning";
    }
    (
        severity.to_string(),
        code.to_string(),
        json!({ "det": det, "scaled_det": scaled, "epsilon": NEAR_SINGULAR_EPS,
                "n": n, "max_scale": max_scale }),
    )
}

/// 高斯-若尔当求逆（仅在外部健康度通过后调用）。
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = m[i].clone();
        row.extend((0..n).map(|j| if i == j { 1.0 } else { 0.0 }));
        a.push(row);
    }
    for k in 0..n {
        let mut pivot = k;
        let mut best = a[k][k].abs();
        for i in (k + 1)..n {
            if a[i][k].abs() > best {
                best = a[i][k].abs();
                pivot = i;
            }
        }
        if best < 1e-15 {
            return None;
        }
        a.swap(k, pivot);
        let div = a[k][k];
        for c in a[k].iter_mut() {
            *c /= div;
        }
        for i in 0..n {
            if i != k {
                let factor = a[i][k];
                for j in 0..2 * n {
                    a[i][j] -= factor * a[k][j];
                }
            }
        }
    }
    Some(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

/// Tikhonov 正则化逆 (MᵀM + λI)⁻¹Mᵀ：近奇异情形的合法候选之一。
pub fn regularized_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mt = transpose(m);
    let mtm = matmul(&mt, m);
    let mut reg = mtm;
    for i in 0..n {
        reg[i][i] += lambda;
    }
    let reg_inv = inverse(&reg)?;
    Some(matmul(&reg_inv, &mt))
}

pub fn transpose(m: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = m.len();
    let p = m.first().map(|r| r.len()).unwrap_or(0);
    (0..p)
        .map(|j| (0..n).map(|i| m[i][j]).collect())
        .collect()
}

pub fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let p = b.len();
    let q = b.first().map(|r| r.len()).unwrap_or(0);
    let mut out = vec![vec![0.0; q]; n];
    for i in 0..n {
        for k in 0..p {
            for j in 0..q {
                out[i][j] += a[i][k] * b[k][j];
            }
        }
    }
    out
}

/// 解 M x = y（补偿：x = M⁻¹ y）。
pub fn solve(m: &[Vec<f64>], y: &[f64]) -> Option<Vec<f64>> {
    let inv = inverse(m)?;
    Some(
        inv.iter()
            .map(|row| row.iter().zip(y).map(|(a, b)| a * b).sum())
            .collect(),
    )
}

/// asinh 线性化：x' = asinh(x / cofactor)；单位为 asinh 单位（坐标基准随门携带）。
pub fn asinh_transform(x: f64, cofactor: f64) -> f64 {
    ((x / cofactor) + ((x / cofactor).powi(2) + 1.0).sqrt()).ln()
}

/// 射线法点在多边形内；点恰好落在边上时判为内部（边界点包含）。
pub fn point_in_polygon(px: f64, py: f64, poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        // 边包含（含端点）
        if on_segment(px, py, xi, yi, xj, yj) {
            return true;
        }
        let intersect = (yi > py) != (yj > py)
            && px < (xj - xi) * (py - yi) / (yj - yi) + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn on_segment(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> bool {
    const EPS: f64 = 1e-9;
    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    if cross.abs() > EPS * (1.0 + (x2 - x1).abs() + (y2 - y1).abs()) {
        return false;
    }
    let within = |v: f64, a: f64, b: f64| v <= a.max(b) + EPS && v + EPS >= a.min(b);
    within(px, x1, x2) && within(py, y1, y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = vec![vec![1.0, 0.1, 0.02], vec![0.0, 1.0, 0.08], vec![0.0, 0.0, 1.0]];
        let (sev, code, _) = matrix_health(&m);
        assert_eq!(sev, "ok");
        assert_eq!(code, "MATRIX_OK");
        let x = solve(&m, &[1000.0, 800.0, 500.0]).unwrap();
        assert!((x[2] - 500.0).abs() < 1e-9);
        assert!((x[1] - (800.0 - 0.08 * 500.0)).abs() < 1e-8);
    }

    #[test]
    fn exact_singular_detected() {
        let m = vec![vec![1.0, 0.5], vec![2.0, 1.0]];
        let (sev, code, ev) = matrix_health(&m);
        assert_eq!(code, "MATRIX_SINGULAR");
        assert_eq!(sev, "error");
        assert_eq!(ev["det"].as_f64().unwrap(), 0.0);
    }

    #[test]
    fn near_singular_detected_and_not_silently_inverted() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-11]];
        let (sev, code, _) = matrix_health(&m);
        assert_eq!(code, "MATRIX_NEAR_SINGULAR");
        assert_eq!(sev, "warning");
        // 引擎仍能数学求逆，但应用层必须依据 health 拒绝：这里给出正则化候选
        let reg = regularized_inverse(&m, 1e-6).unwrap();
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn boundary_points_included_consistently() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(point_in_polygon(5.0, 0.0, &square)); // 底边
        assert!(point_in_polygon(10.0, 5.0, &square)); // 右边
        assert!(point_in_polygon(0.0, 0.0, &square)); // 顶点
        assert!(point_in_polygon(4.0, 4.0, &square));
        assert!(!point_in_polygon(10.1, 5.0, &square));
    }

    #[test]
    fn unconvertible_stays_marked() {
        assert!(matches!(Cell::parse("OVF"), Cell::Unconvertible(_)));
        assert!(matches!(Cell::parse("NaN"), Cell::Unconvertible(_)));
        match Cell::parse("1.25e3") {
            Cell::Number(v) => assert!((v - 1250.0).abs() < 1e-9),
            _ => panic!("should parse"),
        }
    }

    #[test]
    fn asinh_monotonic() {
        let a = asinh_transform(100.0, 150.0);
        let b = asinh_transform(1000.0, 150.0);
        assert!(b > a);
    }
}
