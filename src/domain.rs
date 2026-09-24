//! 领域推导规则：补偿、变换、门控、失效传播。
//! 规则红线：
//! 1. 近奇异补偿矩阵不能被默默求逆 —— 拒绝并给出候选，记录事件。
//! 2. 门的百分比分母必须钉住父群版本。
//! 3. 修订上游门后下游结果标记过期而不覆盖。
use serde::{Deserialize, Serialize};

pub const SINGULAR_EPS: f64 = 1e-8;
pub const BOUNDARY_EPS: f64 = 1e-9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompOutcome {
    /// 求逆成功，返回逆矩阵。
    Inverted { inverse: Vec<Vec<f64>>, condition: f64 },
    /// 近奇异：拒绝默默求逆，附全部合法候选。
    Rejected { determinant: f64, candidates: Vec<CompCandidate> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCandidate {
    pub strategy: String, // "pseudoinverse" | "manual_identity" | "reject_compensation"
    pub note: String,
}

/// 行列式（高斯消元，部分主元）。
pub fn determinant(m: &[Vec<f64>]) -> f64 {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut det = 1.0;
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
            det = -det;
        }
        det *= a[col][col];
        for r in (col + 1)..n {
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
        }
    }
    det
}

/// 高斯-约当求逆；奇异时返回 None。
pub fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
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
        if a[piv][col].abs() < SINGULAR_EPS {
            return None;
        }
        a.swap(piv, col);
        inv.swap(piv, col);
        let d = a[col][col];
        for c in 0..n {
            a[col][c] /= d;
            inv[col][c] /= d;
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

/// 补偿入口：近奇异（|det| 相对量级过小或直接求逆失败）绝不默默求逆。
pub fn compensate_matrix(m: &[Vec<f64>]) -> CompOutcome {
    let det = determinant(m);
    let scale: f64 = m
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0f64, |acc, v| acc.max(v.abs()))
        .max(1.0);
    let threshold = SINGULAR_EPS * scale.powi(m.len() as i32);
    let near_singular = det.abs() <= threshold;
    match invert(m) {
        Some(inverse) if !near_singular => {
            let inv_det = determinant(&inverse);
            let condition = if det.abs() > 0.0 { (inv_det.abs() * det.abs()).max(1.0 / det.abs().max(1e-300)) } else { f64::INFINITY };
            CompOutcome::Inverted { inverse, condition }
        }
        _ => CompOutcome::Rejected {
            determinant: det,
            candidates: vec![
                CompCandidate {
                    strategy: "pseudoinverse".into(),
                    note: "摩尔-彭罗斯伪逆，需人工确认后采纳".into(),
                },
                CompCandidate {
                    strategy: "manual_identity".into(),
                    note: "退回单位阵（不补偿），保留原始信号".into(),
                },
                CompCandidate {
                    strategy: "reject_compensation".into(),
                    note: "拒绝本次补偿，保持未补偿视图".into(),
                },
            ],
        },
    }
}

/// 应用补偿矩阵到单事件通道向量。
pub fn apply_matrix(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GateHit {
    Inside,
    Outside,
    /// 边界点：落在多边形边上，无法唯一决定，两个候选都保留。
    OnBoundary,
}

/// 射线法点在多边形内判定；|点-边距离| < eps 判为边界。
pub fn classify_point(poly: &[(f64, f64)], x: f64, y: f64) -> GateHit {
    if poly.len() < 3 {
        return GateHit::Outside;
    }
    let n = poly.len();
    for i in 0..n {
        let (x1, y1) = poly[i];
        let (x2, y2) = poly[(i + 1) % n];
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len2 = dx * dx + dy * dy;
        if len2 > 0.0 {
            let t = ((x - x1) * dx + (y - y1) * dy) / len2;
            if (0.0..=1.0).contains(&t) {
                let px = x1 + t * dx;
                let py = y1 + t * dy;
                let dist = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                if dist < BOUNDARY_EPS {
                    return GateHit::OnBoundary;
                }
            }
        }
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        GateHit::Inside
    } else {
        GateHit::Outside
    }
}

/// 百分比：分母钉住父群版本（parent_pop_ref），调用方必须显式传入。
pub fn percent_of(child_count: usize, parent_count: usize) -> Option<f64> {
    if parent_count == 0 {
        None
    } else {
        Some(100.0 * child_count as f64 / parent_count as f64)
    }
}

/// 依赖图失效传播：给定被修订的门 id，返回应标记 stale 的群体 id 集。
/// edges: (child_pop, parent_pop)。只影响下游局部依赖，不做全局替换。
pub fn stale_scope(revised_gate_pops: &[String], edges: &[(String, String)]) -> Vec<String> {
    let mut stale: Vec<String> = revised_gate_pops.to_vec();
    let mut i = 0;
    while i < stale.len() {
        let cur = stale[i].clone();
        for (child, parent) in edges {
            if *parent == cur && !stale.contains(child) {
                stale.push(child.clone());
            }
        }
        i += 1;
    }
    stale
}

/// 极端信号检测：超出检测器量程的值保留原始表示并给出诊断，不做截断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignalCheck {
    pub value_raw: String,
    pub value: Option<f64>,
    pub diagnostic: Option<String>,
}

pub fn check_signal(raw: &str, detector_max: f64) -> SignalCheck {
    match raw.parse::<f64>() {
        Ok(v) if v.abs() > detector_max => SignalCheck {
            value_raw: raw.to_string(),
            value: Some(v),
            diagnostic: Some(format!("signal_out_of_range: |{v}| > detector_max {detector_max}")),
        },
        Ok(v) => SignalCheck { value_raw: raw.to_string(), value: Some(v), diagnostic: None },
        Err(_) => SignalCheck {
            value_raw: raw.to_string(),
            value: None,
            diagnostic: Some(format!("unconvertible_value: '{raw}' 保留原位置，不参与规范化")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_singular_matrix_is_rejected_not_inverted() {
        let m = vec![
            vec![1.0, 0.30, 0.0],
            vec![0.30, 1.0, 0.0],
            vec![1.0, 0.30, 1e-12], // 第三行几乎与第一行共线
        ];
        match compensate_matrix(&m) {
            CompOutcome::Rejected { candidates, .. } => {
                assert!(candidates.iter().any(|c| c.strategy == "pseudoinverse"));
                assert!(candidates.iter().any(|c| c.strategy == "reject_compensation"));
            }
            other => panic!("近奇异矩阵必须被拒绝，得到 {other:?}"),
        }
    }

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = vec![vec![1.0, 0.2], vec![0.05, 1.0]];
        match compensate_matrix(&m) {
            CompOutcome::Inverted { inverse, .. } => {
                let prod = apply_matrix(&inverse, &[m[0][0], m[1][0]]);
                assert!((prod[0] - 1.0).abs() < 1e-9 && prod[1].abs() < 1e-9);
            }
            other => panic!("应可求逆: {other:?}"),
        }
    }

    #[test]
    fn boundary_point_is_flagged() {
        let poly = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert_eq!(classify_point(&poly, 5.0, 5.0), GateHit::Inside);
        assert_eq!(classify_point(&poly, 20.0, 5.0), GateHit::Outside);
        assert_eq!(classify_point(&poly, 5.0, 0.0), GateHit::OnBoundary);
    }

    #[test]
    fn stale_scope_is_local_not_global() {
        let edges = vec![
            ("pop:b".to_string(), "pop:a".to_string()),
            ("pop:c".to_string(), "pop:b".to_string()),
            ("pop:d".to_string(), "pop:x".to_string()),
        ];
        let stale = stale_scope(&["pop:a".to_string()], &edges);
        assert!(stale.contains(&"pop:a".to_string()));
        assert!(stale.contains(&"pop:b".to_string()));
        assert!(stale.contains(&"pop:c".to_string()));
        assert!(!stale.contains(&"pop:d".to_string()), "不相关分支不得失效");
    }

    #[test]
    fn extreme_signal_kept_raw_with_diagnostic() {
        let c = check_signal("262143.5", 262144.0);
        assert!(c.diagnostic.is_none());
        let c = check_signal("300000", 262144.0);
        assert!(c.diagnostic.unwrap().contains("signal_out_of_range"));
        assert_eq!(c.value_raw, "300000");
        let c = check_signal("N/A", 262144.0);
        assert!(c.value.is_none());
        assert!(c.diagnostic.unwrap().contains("unconvertible_value"));
    }

    #[test]
    fn percent_requires_nonzero_parent() {
        assert_eq!(percent_of(50, 200), Some(25.0));
        assert_eq!(percent_of(1, 0), None);
    }
}
