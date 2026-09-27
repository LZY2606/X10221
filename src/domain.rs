//! 领域逻辑：补偿矩阵、门多边形、失效范围、百分比分母。
//! 所有函数为纯函数，便于单元测试。

use std::collections::{BTreeSet, HashMap, HashSet};

/// 近奇异判定阈值：主元相对量级低于该值即拒绝求逆。
pub const SINGULAR_PIVOT_RATIO: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq)]
pub enum MatrixError {
    NotSquare,
    NearSingular { pivot_ratio: f64, det: f64 },
}

/// 高斯-约当消元求逆；近奇异时拒绝并返回诊断，绝不默默给出结果。
pub fn invert_matrix(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, MatrixError> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(MatrixError::NotSquare);
    }
    let max_abs = m
        .iter()
        .flat_map(|r| r.iter())
        .fold(0.0_f64, |a, &b| a.max(b.abs()));
    let scale = if max_abs > 0.0 { max_abs } else { 1.0 };

    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let mut det = 1.0_f64;
    let mut min_pivot_ratio = f64::INFINITY;

    for col in 0..n {
        // 选主元
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        let pivot_val = a[piv][col];
        let ratio = pivot_val.abs() / scale;
        if ratio < min_pivot_ratio {
            min_pivot_ratio = ratio;
        }
        if ratio < SINGULAR_PIVOT_RATIO {
            return Err(MatrixError::NearSingular {
                pivot_ratio: ratio,
                det,
            });
        }
        if piv != col {
            a.swap(piv, col);
            inv.swap(piv, col);
            det = -det;
        }
        det *= pivot_val;
        let pv = a[col][col];
        for j in 0..n {
            a[col][j] /= pv;
            inv[col][j] /= pv;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f != 0.0 {
                for j in 0..n {
                    a[r][j] -= f * a[col][j];
                    inv[r][j] -= f * inv[col][j];
                }
            }
        }
    }
    let _ = min_pivot_ratio;
    Ok(inv)
}

/// 矩阵乘向量：out = M * v
pub fn mat_vec(m: &[Vec<f64>], v: &[f64]) -> Vec<f64> {
    m.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointClass {
    Inside,
    Outside,
    OnBoundary,
}

fn on_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> bool {
    let cross = (bx - ax) * (py - ay) - (by - ay) * (px - ax);
    let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
    let tol = 1e-9 * len.max(1.0);
    if cross.abs() > tol {
        return false;
    }
    let dot = (px - ax) * (bx - ax) + (py - ay) * (by - ay);
    dot >= -tol && dot <= len * len + tol
}

/// 射线法 + 边界精确判定。边界点必须显式暴露，不得默默归入任一侧。
pub fn classify_point(x: f64, y: f64, poly: &[(f64, f64)]) -> PointClass {
    let n = poly.len();
    if n < 3 {
        return PointClass::Outside;
    }
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        if on_segment(x, y, ax, ay, bx, by) {
            return PointClass::OnBoundary;
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
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

/// 门系谱：child_gate -> parent_gate。根门的 parent 为 None（分母为总事件数）。
pub type Hierarchy = HashMap<String, Option<String>>;

/// 计算某门修订后的失效范围：该门及其全部下游子孙门。
pub fn invalidation_scope(hier: &Hierarchy, revised_gate: &str) -> BTreeSet<String> {
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for (child, parent) in hier {
        if let Some(p) = parent {
            children.entry(p.as_str()).or_default().push(child.as_str());
        }
    }
    let mut scope = BTreeSet::new();
    let mut stack = vec![revised_gate.to_string()];
    while let Some(g) = stack.pop() {
        if scope.insert(g.clone()) {
            if let Some(kids) = children.get(g.as_str()) {
                for k in kids {
                    stack.push(k.to_string());
                }
            }
        }
    }
    scope
}

/// 门的祖先链（含自身），用于链式过滤。
pub fn gate_chain(hier: &Hierarchy, gate: &str) -> Vec<String> {
    let mut chain = vec![gate.to_string()];
    let mut seen: HashSet<String> = chain.iter().cloned().collect();
    let mut cur = gate.to_string();
    while let Some(Some(parent)) = hier.get(&cur) {
        if !seen.insert(parent.clone()) {
            break; // 环保护
        }
        chain.push(parent.clone());
        cur = parent.clone();
    }
    chain
}

/// 百分比：分母必须钉住父群某一具体版本的计数。
#[derive(Debug, Clone, PartialEq)]
pub struct Percentage {
    pub count: u64,
    pub denominator_count: u64,
    pub percent: f64,
    /// 形如 "pop:lymph@v2" 或 "total@v1" 的稳定来源标识。
    pub denominator_source: String,
}

pub fn percentage(count: u64, denom_count: u64, denom_source: &str) -> Option<Percentage> {
    if denom_count == 0 {
        return None;
    }
    Some(Percentage {
        count,
        denominator_count: denom_count,
        percent: 100.0 * count as f64 / denom_count as f64,
        denominator_source: denom_source.to_string(),
    })
}

/// 极端信号检测：超出坐标基准 [lo, hi] 的值保留在原位置并给出诊断。
#[derive(Debug, Clone, PartialEq)]
pub struct SignalDiagnostic {
    pub event_id: String,
    pub channel: String,
    pub raw_value: f64,
    pub message: String,
}

pub fn check_signal_range(
    event_id: &str,
    channel: &str,
    value: f64,
    lo: f64,
    hi: f64,
) -> Option<SignalDiagnostic> {
    if value < lo || value > hi {
        Some(SignalDiagnostic {
            event_id: event_id.to_string(),
            channel: channel.to_string(),
            raw_value: value,
            message: format!(
                "值 {value} 超出通道 {channel} 的坐标基准 [{lo}, {hi}]，保留原始表示，不参与派生统计"
            ),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_conditioned_matrix_inverts() {
        let s = vec![vec![1.0, 0.12], vec![0.35, 1.0]];
        let inv = invert_matrix(&s).expect("应可求逆");
        // S * S^-1 ≈ I
        for (i, row) in inv.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                let expect = if i == j { 1.0 } else { 0.0 };
                let got: f64 = (0..2).map(|k| s[i][k] * inv[k][j]).sum();
                let _ = v;
                assert!((got - expect).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn near_singular_matrix_is_rejected_not_inverted() {
        let s = vec![vec![1.0, 1.0], vec![1.0, 1.0000000001]];
        match invert_matrix(&s) {
            Err(MatrixError::NearSingular { pivot_ratio, .. }) => {
                assert!(pivot_ratio < SINGULAR_PIVOT_RATIO);
            }
            other => panic!("近奇异矩阵必须被拒绝, got {other:?}"),
        }
    }

    #[test]
    fn boundary_point_is_explicit() {
        let poly = vec![
            (20000.0, 10000.0),
            (120000.0, 10000.0),
            (120000.0, 90000.0),
            (20000.0, 90000.0),
        ];
        assert_eq!(classify_point(20000.0, 50000.0, &poly), PointClass::OnBoundary);
        assert_eq!(classify_point(50000.0, 50000.0, &poly), PointClass::Inside);
        assert_eq!(classify_point(150000.0, 50000.0, &poly), PointClass::Outside);
    }

    #[test]
    fn invalidation_scope_is_subtree_only() {
        let mut h = Hierarchy::new();
        h.insert("lymph".to_string(), None);
        h.insert("pe_pos".to_string(), Some("lymph".to_string()));
        h.insert("pe_hi".to_string(), Some("pe_pos".to_string()));
        h.insert("debris".to_string(), None);
        let scope = invalidation_scope(&h, "lymph");
        assert_eq!(
            scope.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["lymph", "pe_hi", "pe_pos"]
        );
        let scope2 = invalidation_scope(&h, "pe_pos");
        assert!(!scope2.contains("lymph"));
        assert!(!scope2.contains("debris"));
    }

    #[test]
    fn percentage_pins_denominator_source() {
        let p = percentage(250, 1000, "pop:lymph@v2").unwrap();
        assert_eq!(p.percent, 25.0);
        assert_eq!(p.denominator_source, "pop:lymph@v2");
        assert!(percentage(10, 0, "pop:x@v1").is_none());
    }

    #[test]
    fn extreme_signal_stays_with_diagnostic() {
        let d = check_signal_range("e9", "FL2-A", 300000.0, 0.0, 262143.0).unwrap();
        assert_eq!(d.raw_value, 300000.0);
        assert!(check_signal_range("e1", "FL1-A", 100.0, 0.0, 262143.0).is_none());
    }
}
