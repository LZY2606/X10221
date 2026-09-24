//! 派生层：补偿、变换、门控求值、百分比与失效范围计算。
//! 规则：近奇异补偿矩阵不能被默默求逆；分母钉住父群版本；上游修订只标记过期。

use crate::model::*;
use std::collections::BTreeMap;

/// 相对主元阈值：低于它即视为近奇异，拒绝求逆。
pub const SINGULAR_EPS: f64 = 1e-8;
/// 门边界判定容差（变换后坐标）。
pub const BOUNDARY_EPS: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq)]
pub enum DeriveError {
    NearSingular { stage: usize, pivot: f64 },
    DimensionMismatch,
}

/// 高斯-约当消元求逆；任何主元近零即拒绝，绝不默默继续。
pub fn invert_matrix(m: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, DeriveError> {
    let n = m.len();
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return Err(DeriveError::DimensionMismatch);
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let mut best = col;
        for row in (col + 1)..n {
            if a[row][col].abs() > a[best][col].abs() {
                best = row;
            }
        }
        let pivot = a[best][col];
        if pivot.abs() < SINGULAR_EPS {
            return Err(DeriveError::NearSingular { stage: col, pivot });
        }
        a.swap(col, best);
        inv.swap(col, best);
        let p = a[col][col];
        for j in 0..n {
            a[col][j] /= p;
            inv[col][j] /= p;
        }
        for row in 0..n {
            if row != col {
                let f = a[row][col];
                for j in 0..n {
                    a[row][j] -= f * a[col][j];
                    inv[row][j] -= f * inv[col][j];
                }
            }
        }
    }
    Ok(inv)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
}

fn dist_to_segment(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
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

/// 射线法 + 边界容差：落在边上的点返回 Boundary，由上层保留多候选。
pub fn classify(x: f64, y: f64, poly: &[(f64, f64)]) -> Membership {
    let n = poly.len();
    if n < 3 {
        return Membership::Outside;
    }
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        if dist_to_segment(x, y, ax, ay, bx, by) < BOUNDARY_EPS {
            return Membership::Boundary;
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
        Membership::Inside
    } else {
        Membership::Outside
    }
}

/// 单个事件的规范化结果：无法转换的值留在原位置并附诊断。
#[derive(Debug, Clone)]
pub struct NormEvent {
    pub id: String,
    pub values: BTreeMap<String, f64>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub object_id: String,
    pub severity: String,
    pub code: String,
    pub message: String,
}

/// 规范化原始通道值：解析 raw、校验单位、标记极端信号；不做任何不可逆清洗。
pub fn normalize_events(raw: &RawChannelsPayload) -> Vec<NormEvent> {
    let mut out = Vec::new();
    for ev in &raw.events {
        let mut values = BTreeMap::new();
        let mut diagnostics = Vec::new();
        for (ch, cv) in &ev.values {
            let parsed = cv.value.or_else(|| cv.raw.trim().parse::<f64>().ok());
            match parsed {
                Some(v) if cv.unit == raw.detector.unit => {
                    if v.abs() > raw.detector.linear_max {
                        diagnostics.push(Diagnostic {
                            object_id: ev.id.clone(),
                            severity: "warn".into(),
                            code: "extreme-signal".into(),
                            message: format!(
                                "事件 {} 通道 {} 值 {} 超出线性范围 ±{}，原值保留不截断",
                                ev.id, ch, v, raw.detector.linear_max
                            ),
                        });
                    }
                    values.insert(ch.clone(), v);
                }
                Some(_) => {
                    diagnostics.push(Diagnostic {
                        object_id: ev.id.clone(),
                        severity: "error".into(),
                        code: "unit-mismatch".into(),
                        message: format!(
                            "事件 {} 通道 {} 单位 {} 与检测器单位 {} 不一致，值留在原位置",
                            ev.id, ch, cv.unit, raw.detector.unit
                        ),
                    });
                }
                None => {
                    diagnostics.push(Diagnostic {
                        object_id: ev.id.clone(),
                        severity: "error".into(),
                        code: "unconvertible-value".into(),
                        message: format!(
                            "事件 {} 通道 {} 原始表示 {:?} 无法转换为数值，留在原位置",
                            ev.id, ch, cv.raw
                        ),
                    });
                }
            }
        }
        out.push(NormEvent { id: ev.id.clone(), values, diagnostics });
    }
    out
}

/// 补偿：true = S^-1 * observed。矩阵近奇异时整体拒绝，由上层保留候选。
pub fn compensate(
    events: &[NormEvent],
    order: &[String],
    inv: &[Vec<f64>],
) -> Vec<BTreeMap<String, f64>> {
    events
        .iter()
        .map(|ev| {
            let mut out = ev.values.clone();
            let obs: Vec<Option<f64>> = order.iter().map(|c| ev.values.get(c).copied()).collect();
            if obs.iter().all(|v| v.is_some()) {
                for (i, ch) in order.iter().enumerate() {
                    let mut acc = 0.0;
                    for (j, v) in obs.iter().enumerate() {
                        acc += inv[i][j] * v.unwrap();
                    }
                    out.insert(ch.clone(), acc);
                }
            }
            out
        })
        .collect()
}

/// 变换：log10(v / cofactor)；非正值留诊断、不进入变换空间。
pub fn transform(
    values: &BTreeMap<String, f64>,
    spec: &TransformParamsPayload,
    event_id: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (ch, v) in values {
        match spec.transforms.get(ch) {
            Some(ts) if ts.kind == "log" => {
                if *v > 0.0 {
                    out.insert(ch.clone(), (*v / ts.cofactor).log10());
                } else {
                    diagnostics.push(Diagnostic {
                        object_id: event_id.into(),
                        severity: "warn".into(),
                        code: "non-positive-transform".into(),
                        message: format!("事件 {event_id} 通道 {ch} 值 {v} 非正，无法取对数，原值保留"),
                    });
                }
            }
            _ => {
                out.insert(ch.clone(), *v);
            }
        }
    }
    out
}

/// 群体求值结果（单次派生运行）。
#[derive(Debug, Clone)]
pub struct PopulationResult {
    pub gate_id: String,
    pub count_inclusive: usize,
    pub count_exclusive: usize,
    pub boundary_events: Vec<String>,
    pub member_events: Vec<String>,
}

pub fn evaluate_populations(
    event_ids: &[String],
    transformed: &[BTreeMap<String, f64>],
    gates: &[GateDef],
    hierarchy: &GateHierarchyPayload,
) -> Vec<PopulationResult> {
    let mut results = Vec::new();
    for node in &hierarchy.nodes {
        let Some(_gate) = gates.iter().find(|g| g.gate_id == node.gate_id) else {
            continue;
        };
        // 祖先链：自身 + 全部祖先门都必须通过
        let mut chain = vec![node.gate_id.clone()];
        let mut cur = node.parent.clone();
        while let Some(p) = cur {
            chain.push(p.clone());
            cur = hierarchy.nodes.iter().find(|n| n.gate_id == p).and_then(|n| n.parent.clone());
        }
        let mut inclusive = 0usize;
        let mut exclusive = 0usize;
        let mut boundary_events = Vec::new();
        let mut member_events = Vec::new();
        for (idx, eid) in event_ids.iter().enumerate() {
            let mut pass = true;
            let mut on_boundary = false;
            for gid in &chain {
                let Some(g) = gates.iter().find(|g| &g.gate_id == gid) else {
                    pass = false;
                    break;
                };
                let (Some(x), Some(y)) = (
                    transformed[idx].get(&g.x_channel),
                    transformed[idx].get(&g.y_channel),
                ) else {
                    pass = false;
                    break;
                };
                match classify(*x, *y, &g.points) {
                    Membership::Inside => {}
                    Membership::Outside => {
                        pass = false;
                        break;
                    }
                    Membership::Boundary => on_boundary = true,
                }
            }
            if pass {
                inclusive += 1;
                member_events.push(eid.clone());
                if on_boundary {
                    boundary_events.push(eid.clone());
                } else {
                    exclusive += 1;
                }
            }
        }
        results.push(PopulationResult {
            gate_id: node.gate_id.clone(),
            count_inclusive: inclusive,
            count_exclusive: exclusive,
            boundary_events,
            member_events,
        });
    }
    results
}

/// 失效范围：给定新材料版本，按候选的来源链精确计算受影响对象。
pub fn invalidation_scope<'a>(
    candidates: &'a [CandidateRow],
    artifact_id: &str,
) -> Vec<&'a CandidateRow> {
    candidates
        .iter()
        .filter(|c| {
            c.status != status::STALE
                && c.provenance
                    .iter()
                    .any(|p| p.artifact_id == artifact_id)
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProvenanceRef {
    pub artifact_id: String,
    pub version_seq: i64,
}

#[derive(Debug, Clone)]
pub struct CandidateRow {
    pub id: String,
    pub status: String,
    pub provenance: Vec<ProvenanceRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_singular_matrix_is_refused() {
        let m = vec![vec![1.0, 1.0], vec![1.0, 1.0 + 1e-12]];
        match invert_matrix(&m) {
            Err(DeriveError::NearSingular { stage, .. }) => assert_eq!(stage, 1),
            other => panic!("expected refusal, got {other:?}"),
        }
    }

    #[test]
    fn well_conditioned_matrix_inverts() {
        let m = vec![vec![1.0, 0.15], vec![0.02, 1.0]];
        let inv = invert_matrix(&m).unwrap();
        // inv * m ≈ I
        let mut p = vec![vec![0.0; 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                p[i][j] = inv[i][0] * m[0][j] + inv[i][1] * m[1][j];
            }
        }
        assert!((p[0][0] - 1.0).abs() < 1e-9);
        assert!((p[1][1] - 1.0).abs() < 1e-9);
        assert!(p[0][1].abs() < 1e-9);
    }

    #[test]
    fn boundary_point_is_not_forced_in_or_out() {
        let poly = vec![(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)];
        assert_eq!(classify(1.0, 2.0, &poly), Membership::Boundary);
        assert_eq!(classify(2.0, 2.0, &poly), Membership::Inside);
        assert_eq!(classify(0.5, 2.0, &poly), Membership::Outside);
    }

    #[test]
    fn invalidation_only_touches_dependents() {
        let rows = vec![
            CandidateRow {
                id: "c1".into(),
                status: status::PENDING.into(),
                provenance: vec![ProvenanceRef { artifact_id: "art-raw".into(), version_seq: 1 }],
            },
            CandidateRow {
                id: "c2".into(),
                status: status::PENDING.into(),
                provenance: vec![
                    ProvenanceRef { artifact_id: "art-raw".into(), version_seq: 1 },
                    ProvenanceRef { artifact_id: "art-comp".into(), version_seq: 1 },
                ],
            },
        ];
        let scope = invalidation_scope(&rows, "art-comp");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope[0].id, "c2");
    }
}
