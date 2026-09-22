//! 数值层：补偿矩阵分析、线性代数、asinh 变换、多边形门控。
//!
//! 所有领域量在此层显式携带坐标基准（raw counts / compensated counts / asinh units），
//! 无法转换的事件不在此层被丢弃，只在投影层做规范化并附诊断。

use serde::{Deserialize, Serialize};

/// 近奇异判定阈值（条件数上界）。超过该值的矩阵不会被默默求逆。
pub const CONDITION_LIMIT: f64 = 1.0e8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixAnalysis {
    pub n: usize,
    pub determinant: f64,
    pub condition: f64,
    pub singular: bool,
    pub near_singular: bool,
    /// 判定依据（机器可读规则 id），与事件 payload 中的 rule 对应。
    pub rule: String,
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCandidate {
    pub id: String,
    pub kind: String, // exact | ridge
    /// 补偿矩阵 M = S^{-1}（奇异时为空）。
    pub inverse: Vec<Vec<f64>>,
    pub ridge_lambda: Option<f64>,
    pub description: String,
}

pub fn is_finite_matrix(m: &[Vec<f64>]) -> bool {
    m.iter().all(|r| r.iter().all(|v| v.is_finite()))
}

/// LU 分解（部分选主元）。返回 (lu, piv, sign)。
fn lu_decompose(a: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, Vec<usize>, i32)> {
    let n = a.len();
    if n == 0 || a.iter().any(|r| r.len() != n) || !is_finite_matrix(a) {
        return None;
    }
    let mut lu = a.to_vec();
    let mut piv: Vec<usize> = (0..n).collect();
    let mut sign = 1i32;
    for k in 0..n {
        let mut max_row = k;
        let mut max_val = lu[k][k].abs();
        for i in (k + 1)..n {
            if lu[i][k].abs() > max_val {
                max_val = lu[i][k].abs();
                max_row = i;
            }
        }
        if max_val < 1e-300 {
            return None;
        }
        if max_row != k {
            lu.swap(k, max_row);
            piv.swap(k, max_row);
            sign = -sign;
        }
        let pivot = lu[k][k];
        for i in (k + 1)..n {
            lu[i][k] /= pivot;
            let factor = lu[i][k];
            for j in (k + 1)..n {
                lu[i][j] -= factor * lu[k][j];
            }
        }
    }
    Some((lu, piv, sign))
}

pub fn determinant(m: &[Vec<f64>]) -> Option<f64> {
    let (lu, _, sign) = lu_decompose(m)?;
    let mut det = sign as f64;
    for i in 0..lu.len() {
        det *= lu[i][i];
    }
    Some(det)
}

/// 严格求逆；奇异或非有限返回 None（不静默使用伪逆）。
pub fn inverse(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let (mut lu, piv, _) = lu_decompose(m)?;
    // 逐列解 m x = e_j（带行交换的前向/回代）。
    let mut inv = vec![vec![0.0f64; n]; n];
    for col in 0..n {
        let mut b = vec![0.0f64; n];
        b[col] = 1.0;
        // 应用主元置换
        let mut pb = vec![0.0f64; n];
        for i in 0..n {
            pb[i] = b[piv[i]];
        }
        // 前向代入 L y = Pb（L 单位对角）
        let mut y = vec![0.0f64; n];
        for i in 0..n {
            let mut s = pb[i];
            for j in 0..i {
                s -= lu[i][j] * y[j];
            }
            y[i] = s;
        }
        // 回代 U x = y
        for i in (0..n).rev() {
            let mut s = y[i];
            for j in (i + 1)..n {
                s -= lu[i][j] * inv[j][col];
            }
            let pivot = lu[i][i];
            if pivot.abs() < 1e-300 {
                return None;
            }
            inv[i][col] = s / pivot;
        }
    }
    // 数值清理（把 -0 归一）
    for row in inv.iter_mut() {
        for v in row.iter_mut() {
            if *v == 0.0 {
                *v = 0.0;
            }
        }
    }
    Some(inv)
}

/// 2-范数条件数估计：用逆阵与原阵的无穷范数乘积（保守上界）。
pub fn condition_number(m: &[Vec<f64>]) -> Option<f64> {
    let inv = inverse(m)?;
    let norm_inf = |x: &[Vec<f64>]| {
        x.iter()
            .map(|r| r.iter().map(|v| v.abs()).sum::<f64>())
            .fold(0.0f64, f64::max)
    };
    Some(norm_inf(m) * norm_inf(&inv))
}

/// (M + lambda I) 形式的岭回归候选，用于近奇异矩阵的“全部合法候选”。
pub fn ridge_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut reg = m.to_vec();
    for i in 0..n {
        reg[i][i] += lambda;
    }
    inverse(&reg)
}

/// 分析 spillover 矩阵：严格奇异 -> 无逆候选；近奇异 -> 精确逆 + 多个岭候选并存。
pub fn analyze_spillover(m: &[Vec<f64>]) -> MatrixAnalysis {
    let n = m.len();
    let mut diagnostics = Vec::new();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return MatrixAnalysis {
            n,
            determinant: f64::NAN,
            condition: f64::NAN,
            singular: true,
            near_singular: false,
            rule: "RULE_COMP_MATRIX_SHAPE".into(),
            candidates: vec![],
            diagnostics: vec!["spillover 矩阵必须为方阵".into()],
        };
    }
    if !is_finite_matrix(m) {
        return MatrixAnalysis {
            n,
            determinant: f64::NAN,
            condition: f64::NAN,
            singular: true,
            near_singular: false,
            rule: "RULE_COMP_NONFINITE".into(),
            candidates: vec![],
            diagnostics: vec!["矩阵包含 NaN/Inf，无法求逆".into()],
        };
    }
    let det = determinant(m).unwrap_or(0.0);
    let cond = condition_number(m).unwrap_or(f64::INFINITY);
    let singular = det.abs() < 1e-12;
    let near = !singular && cond > CONDITION_LIMIT;

    if singular {
        diagnostics.push(format!(
            "spillover 矩阵奇异(det={:.3e})：不能默默求逆，也不生成补偿候选",
            det
        ));
        MatrixAnalysis {
            n,
            determinant: det,
            condition: cond,
            singular: true,
            near_singular: false,
            rule: "RULE_COMP_SINGULAR_NO_INVERSE".into(),
            candidates: vec![],
            diagnostics,
        }
    } else if near {
        let mut candidates = Vec::new();
        if let Some(inv) = inverse(m) {
            candidates.push(CompCandidate {
                id: "exact".into(),
                kind: "exact".into(),
                inverse: inv,
                ridge_lambda: None,
                description: "精确逆（条件数过大，数值可能不稳定）".into(),
            });
        }
        for (idx, lambda) in [1e-6_f64, 1e-4, 1e-2]
            .iter()
            .enumerate()
        {
            if let Some(inv) = ridge_inverse(m, *lambda) {
                candidates.push(CompCandidate {
                    id: format!("ridge-{}", idx + 1),
                    kind: "ridge".into(),
                    inverse: inv,
                    ridge_lambda: Some(*lambda),
                    description: format!("岭回归候选 (M + {:.0e}·I)^-1", lambda),
                });
            }
        }
        diagnostics.push(format!(
            "近奇异矩阵(cond={:.3e})：保留全部合法补偿候选，等待人工裁定",
            cond
        ));
        MatrixAnalysis {
            n,
            determinant: det,
            condition: cond,
            singular: false,
            near_singular: true,
            rule: "RULE_COMP_NEAR_SINGULAR_MULTI_CANDIDATE".into(),
            candidates,
            diagnostics,
        }
    } else {
        let candidates = inverse(m)
            .into_iter()
            .map(|inv| CompCandidate {
                id: "exact".into(),
                kind: "exact".into(),
                inverse: inv,
                ridge_lambda: None,
                description: "精确逆".into(),
            })
            .collect();
        MatrixAnalysis {
            n,
            determinant: det,
            condition: cond,
            singular: false,
            near_singular: false,
            rule: "RULE_COMP_WELL_CONDITIONED".into(),
            candidates,
            diagnostics,
        }
    }
}

/// 施加补偿：comp = M · raw。通道顺序由调用方保证一致。
pub fn compensate(inv: &[Vec<f64>], raw: &[f64]) -> Vec<f64> {
    inv.iter()
        .map(|row| {
            row.iter()
                .zip(raw.iter())
                .map(|(a, x)| a * x)
                .sum::<f64>()
        })
        .collect()
}

/// 通道变换参数。basis 记录坐标基准："linear_counts" 或 "asinh_compensated"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub channel: String,
    pub basis: String,
    /// asinh: x' = asinh(x / cofactor) * scale
    pub cofactor: f64,
    pub scale: f64,
}

impl TransformParams {
    pub fn apply(&self, x: f64) -> f64 {
        if self.basis == "linear_counts" || self.cofactor == 0.0 {
            x
        } else {
            (x / self.cofactor).asinh() * self.scale
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 射线法点在多边形内。edge_inclusive=true 时把落在边上的点判为内（“边界内”约定）；
/// false 为“边界外”约定。两种约定都会作为候选保留。
pub fn point_in_polygon(p: Point, poly: &[Point], edge_inclusive: bool) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut on_edge = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x, poly[i].y);
        let (xj, yj) = (poly[j].x, poly[j].y);
        let cross = (p.x - xi) * (yj - yi) - (p.y - yi) * (xj - xi);
        if cross.abs() < 1e-9
            && p.x <= xi.max(xj) + 1e-9
            && p.x >= xi.min(xj) - 1e-9
            && p.y <= yi.max(yj) + 1e-9
            && p.y >= yi.min(yj) - 1e-9
        {
            on_edge = true;
        }
        let intersect = (yi > p.y) != (yj > p.y)
            && p.x < (xj - xi) * (p.y - yi) / (yj - yi) + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    if on_edge {
        edge_inclusive
    } else {
        inside

#[cfg(test)]
mod tests {
    use super::*;

    fn mat3(v: &[f64; 9]) -> Vec<Vec<f64>> {
        vec![
            vec![v[0], v[1], v[2]],
            vec![v[3], v[4], v[5]],
            vec![v[6], v[7], v[8]],
        ]
    }

    #[test]
    fn identity_like_spillover_inverts() {
        let s = mat3(&[1.0, 0.1, 0.0, 0.2, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let a = analyze_spillover(&s);
        assert!(!a.singular && !a.near_singular);
        assert_eq!(a.candidates.len(), 1);
        // S * S^-1 = I
        let inv = &a.candidates[0].inverse;
        let prod = |r: usize, c: usize| {
            (0..3).map(|k| s[r][k] * inv[k][c]).sum::<f64>()
        };
        for r in 0..3 {
            for c in 0..3 {
                let expect = if r == c { 1.0 } else { 0.0 };
                assert!((prod(r, c) - expect).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn singular_matrix_has_no_candidate() {
        // 第三行 = 第一行 + 第二行
        let s = mat3(&[1.0, 0.2, 0.0, 0.1, 1.0, 0.0, 1.1, 1.2, 0.0]);
        let a = analyze_spillover(&s);
        assert!(a.singular);
        assert!(a.candidates.is_empty());
        assert_eq!(a.rule, "RULE_COMP_SINGULAR_NO_INVERSE");
    }

    #[test]
    fn near_singular_keeps_all_legal_candidates() {
        // 三列荧光通道中两列几乎共线
        let s = mat3(&[1.0, 0.9999, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let a = analyze_spillover(&s);
        assert!(a.near_singular, "cond={}", a.condition);
        assert!(a.candidates.len() >= 2, "应保留精确逆+岭候选");
        assert_eq!(a.rule, "RULE_COMP_NEAR_SINGULAR_MULTI_CANDIDATE");
    }

    #[test]
    fn boundary_point_two_conventions_differ() {
        let square = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
            Point { x: 0.0, y: 10.0 },
        ];
        let edge = Point { x: 5.0, y: 0.0 };
        assert!(point_in_polygon(edge, &square, true));
        assert!(!point_in_polygon(edge, &square, false));
        let inner = Point { x: 5.0, y: 5.0 };
        assert!(point_in_polygon(inner, &square, true));
        assert!(point_in_polygon(inner, &square, false));
    }

    #[test]
    fn asinh_transform_monotonic_with_cofactor() {
        let t = TransformParams {
            channel: "FITC".into(),
            basis: "asinh_compensated".into(),
            cofactor: 150.0,
            scale: 5.0,
        };
        assert!((t.apply(0.0)).abs() < 1e-12);
        assert!(t.apply(1000.0) > t.apply(100.0));
        assert!(t.apply(-500.0) < 0.0);
    }
}
