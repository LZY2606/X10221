//! 流式领域数学：补偿矩阵分析、信号变换、多边形门控。
//!
//! 约定：
//! - 所有领域量的单位/坐标基准在案例材料的 `units` 字段中携带，本模块只在派生层做规范化。
//! - 近奇异矩阵显式拒绝求逆，由调用方生成候选，绝不默默给出一个逆矩阵。

pub const COND_MAX: f64 = 1.0e6;
pub const DET_MIN: f64 = 1.0e-9;

/// 矩阵分析结论。`inverse` 为 None 时表示不可（或不应）直接求逆。
#[derive(Debug, Clone)]
pub struct MatrixAnalysis {
    pub n: usize,
    pub det: f64,
    pub cond_one: f64,
    pub inverse: Option<Vec<Vec<f64>>>,
    pub near_singular: bool,
    pub reason: Option<String>,
}

/// Gauss-Jordan 带部分主元消元求逆，同时返回行列式。
pub fn invert(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, f64)> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut m: Vec<Vec<f64>> = a.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let mut det = 1.0_f64;
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&i, &j| {
                m[i][col]
                    .abs()
                    .partial_cmp(&m[j][col].abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(col);
        if m[pivot][col].abs() < 1.0e-300 {
            return None;
        }
        if pivot != col {
            m.swap(pivot, col);
            inv.swap(pivot, col);
            det = -det;
        }
        let pv = m[col][col];
        det *= pv;
        for j in 0..n {
            m[col][j] /= pv;
            inv[col][j] /= pv;
        }
        for i in 0..n {
            if i != col {
                let factor = m[i][col];
                for j in 0..n {
                    m[i][j] -= factor * m[col][j];
                    inv[i][j] -= factor * inv[col][j];
                }
            }
        }
    }
    Some((inv, det))
}

fn norm_one(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    (0..n)
        .map(|j| (0..n).map(|i| m[i][j].abs()).sum())
        .fold(0.0_f64, f64::max)
}

/// 分析补偿矩阵是否允许直接求逆。
pub fn analyze_matrix(a: &[Vec<f64>]) -> MatrixAnalysis {
    let n = a.len();
    match invert(a) {
        None => MatrixAnalysis {
            n,
            det: 0.0,
            cond_one: f64::INFINITY,
            inverse: None,
            near_singular: true,
            reason: Some("exact_singular".into()),
        },
        Some((inv, det)) => {
            let cond = norm_one(a) * norm_one(&inv);
            let mut near = false;
            let mut reason = None;
            if det.abs() < DET_MIN {
                near = true;
                reason = Some(format!("det|={:.3e}<{}", det.abs(), DET_MIN));
            } else if cond > COND_MAX || !cond.is_finite() {
                near = true;
                reason = Some(format!("cond1={:.3e}>{}", cond, COND_MAX));
            }
            MatrixAnalysis {
                n,
                det,
                cond_one: cond,
                inverse: if near { None } else { Some(inv) },
                near_singular: near,
                reason,
            }
        }
    }
}

/// 正则化伪逆 (AᵀA + λI)⁻¹ Aᵀ。只允许在人工裁定显式选择该候选后调用。
pub fn regularized_inverse(a: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let at = transpose(a);
    let ata = multiply(&at, a);
    let mut sys = ata;
    for i in 0..n {
        sys[i][i] += lambda;
    }
    let (s_inv, _) = invert(&sys)?;
    Some(multiply(&s_inv, &at))
}

pub fn transpose(a: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let m = a.first().map(|r| r.len()).unwrap_or(0);
    (0..m)
        .map(|j| (0..n).map(|i| a[i][j]).collect())
        .collect()
}

pub fn multiply(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = a.len();
    let k = b.len();
    let m = b.first().map(|r| r.len()).unwrap_or(0);
    (0..n)
        .map(|i| {
            (0..m)
                .map(|j| (0..k).map(|t| a[i][t] * b[t][j]).sum())
                .collect()
        })
        .collect()
}

pub fn mat_vec(a: &[Vec<f64>], x: &[f64]) -> Vec<f64> {
    a.iter()
        .map(|row| row.iter().zip(x).map(|(v, x)| v * x).sum())
        .collect()
}

pub fn default_lambda(a: &[Vec<f64>]) -> f64 {
    1.0e-6 * norm_one(a).max(1.0)
}

/// asinh(x / cofactor)，坐标基准由调用方在 units 中记录。
pub fn transform_value(raw: f64, cofactor: f64) -> f64 {
    (raw / cofactor.max(1.0e-12)).asinh()
}

/// 从派生（显示）坐标反算原始值，用于构造恰好落在门边上的固定案例。
pub fn untransform_value(display: f64, cofactor: f64) -> f64 {
    display.sinh() * cofactor
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contain {
    In,
    Out,
    Boundary,
}

fn dist_point_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let qx = ax + t * dx;
    let qy = ay + t * dy;
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

/// 点对多边形的分类：边界点（距离边小于相对容差）单独返回 Boundary。
pub fn classify_point(x: f64, y: f64, poly: &[(f64, f64)]) -> Contain {
    if poly.len() < 3 {
        return Contain::Out;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[j];
        let edge_len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let tol = 1.0e-9 * (1.0 + edge_len);
        if dist_point_segment(x, y, ax, ay, bx, by) <= tol {
            return Contain::Boundary;
        }
        if (ay > y) != (by > y) && x < ((bx - ax) * (y - ay) / (by - ay) + ax) {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Contain::In
    } else {
        Contain::Out
    }
}

/// 提取 f64 矩阵；形状不一致返回错误。
pub fn matrix_from_json(v: &serde_json::Value) -> Result<Vec<Vec<f64>>, String> {
    v.as_array()
        .ok_or("matrix must be an array")?
        .iter()
        .map(|row| {
            row.as_array()
                .ok_or("matrix row must be an array")?
                .iter()
                .map(|c| c.as_f64().ok_or_else(|| format!("not a number: {c}")))
                .collect::<Result<Vec<f64>, String>>()
        })
        .collect::<Result<Vec<Vec<f64>>, String>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_matrix_is_well_conditioned() {
        let a = vec![vec![1.0, 0.2], vec![0.1, 1.0]];
        let r = analyze_matrix(&a);
        assert!(!r.near_singular);
        assert!(r.inverse.is_some());
        let inv = r.inverse.unwrap();
        let prod = multiply(&a, &inv);
        for i in 0..2 {
            for j in 0..2 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((prod[i][j] - want).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn exact_singular_matrix_is_not_inverted() {
        let a = vec![vec![1.0, 0.5], vec![2.0, 1.0]];
        let r = analyze_matrix(&a);
        assert!(r.near_singular);
        assert!(r.inverse.is_none());
    }

    #[test]
    fn near_singular_matrix_is_not_silently_inverted() {
        let a = vec![vec![1.0, 0.5], vec![1.9999999, 1.0]];
        let r = analyze_matrix(&a);
        assert!(r.near_singular, "det={} cond={}", r.det, r.cond_one);
        assert!(r.inverse.is_none(), "must refuse inverse, no silent math");
        let lam = default_lambda(&a);
        let pinv = regularized_inverse(&a, lam).expect("candidate pseudoinverse");
        assert_eq!(pinv.len(), 2);
    }

    #[test]
    fn boundary_point_is_detected() {
        let square = vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)];
        assert_eq!(classify_point(2.0, 0.0, &square), Contain::Boundary);
        assert_eq!(classify_point(1.0, 1.0, &square), Contain::In);
        assert_eq!(classify_point(5.0, 5.0, &square), Contain::Out);
        assert_eq!(classify_point(4.0, 2.0, &square), Contain::Boundary);
    }

    #[test]
    fn transform_roundtrip() {
        let raw = 1234.567_f64;
        let d = transform_value(raw, 1000.0);
        let back = untransform_value(d, 1000.0);
        assert!((back - raw).abs() < 1e-6 * raw);
    }
}
