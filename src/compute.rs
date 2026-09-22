//! 派生计算：补偿应用、坐标变换、门归属与群体计数。

use crate::domain::{
    CompensationVerdict, EntityKind, EntityVersion, GateLink, PolygonGate, TransformSet,
};
use crate::fcs::spill::{InverseMethod, SpillStatus, SpilloverMatrix};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFlag {
    pub event_index: i64,
    pub code: String,
    pub detail: String,
}

/// 单条事件的派生数据。所有极端/无法变换值保留并附诊断，不为画图丢弃。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedEvent {
    pub index: i64,
    pub raw: Vec<f64>,
    pub compensated: Vec<Option<f64>>,
    pub transformed: Vec<Option<f64>>,
    pub flags: Vec<EventFlag>,
}

#[derive(Debug, Clone)]
pub struct CompensationOutcome {
    pub verdict: CompensationVerdict,
    /// 采纳的逆矩阵（未裁定且非良态时为 None → 下游被阻断）。
    pub chosen_inverse: Option<Vec<Vec<f64>>>,
}

pub fn evaluate_compensation(
    spill: &SpilloverMatrix,
    spill_version_id: &str,
    chosen_index: Option<usize>,
) -> CompensationOutcome {
    let cands = spill.candidates();
    let (status, det, cond) = match spill.status() {
        SpillStatus::WellConditioned { det, cond } => ("well_conditioned", det, Some(cond)),
        SpillStatus::NearSingular { det, cond } => ("near_singular", det, Some(cond)),
        SpillStatus::Singular => ("singular", spill.determinant(), None),
    };
    let candidates_json = cands
        .iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect();
    let auto_chosen = if status == "well_conditioned" {
        cands.iter().position(|c| c.method == InverseMethod::Direct)
    } else {
        chosen_index
    };
    let chosen_inverse = auto_chosen.and_then(|i| cands.get(i)).map(|c| c.matrix.clone());
    CompensationOutcome {
        verdict: CompensationVerdict {
            spill_version_id: spill_version_id.into(),
            status: status.into(),
            det,
            cond,
            candidates: candidates_json,
            chosen_index: auto_chosen,
        },
        chosen_inverse,
    }
}

/// 应用补偿到荧光列（列索引 fluor_cols）。无可用逆时全部为 None 并标记。
pub fn compensate_events(
    raw_events: &[Vec<f64>],
    fluor_cols: &[usize],
    inverse: &Option<Vec<Vec<f64>>>,
    spill_status: &str,
) -> (Vec<Vec<Option<f64>>>, Vec<Vec<EventFlag>>) {
    let n = fluor_cols.len();
    let mut comp_all = Vec::with_capacity(raw_events.len());
    let mut flags_all = Vec::with_capacity(raw_events.len());
    for (idx, raw) in raw_events.iter().enumerate() {
        let mut comp: Vec<Option<f64>> = raw.iter().map(|v| Some(*v)).collect();
        let mut flags = Vec::new();
        match inverse {
            None => {
                for &c in fluor_cols {
                    comp[c] = None;
                }
                flags.push(EventFlag {
                    event_index: idx as i64,
                    code: "COMPENSATION_BLOCKED".into(),
                    detail: format!(
                        "补偿矩阵状态 {spill_status} 且未裁定，荧光值不参与派生，原始值保留"
                    ),
                });
            }
            Some(inv) => {
                let x: Vec<f64> = fluor_cols.iter().map(|&c| raw[c]).collect();
                for r in 0..n {
                    let mut s = 0.0;
                    for c in 0..n {
                        s += inv[r][c] * x[c];
                    }
                    comp[fluor_cols[r]] = Some(s);
                }
            }
        }
        comp_all.push(comp);
        flags_all.push(flags);
    }
    (comp_all, flags_all)
}

/// 应用变换。极端/非法值（NaN、负对数等）保留原位置并附诊断。
pub fn transform_events(
    raw_events: &[Vec<f64>],
    compensated: &[Vec<Option<f64>>],
    flags: &mut [Vec<EventFlag>],
    transform: &TransformSet,
    channel_index: &[String],
) -> Vec<Vec<Option<f64>>> {
    let mut out: Vec<Vec<Option<f64>>> = raw_events
        .iter()
        .map(|row| row.iter().map(|_| None).collect())
        .collect();
    for spec in &transform.params {
        let Some(col) = channel_index.iter().position(|c| c == spec.channel()) else {
            continue;
        };
        for (i, _row) in raw_events.iter().enumerate() {
            let src = match spec {
                crate::domain::TransformSpec::Linear { .. } => Some(raw_events[i][col]),
                crate::domain::TransformSpec::Asinh { .. } => compensated[i][col],
            };
            match src {
                Some(v) if v.is_finite() => out[i][col] = Some(spec.apply(v)),
                Some(v) => {
                    flags[i].push(EventFlag {
                        event_index: i as i64,
                        code: "TRANSFORM_NONFINITE".into(),
                        detail: format!("通道 {} 值 {v} 非有限，保留原位置不参与绘图", spec.channel()),
                    });
                }
                None => {
                    flags[i].push(EventFlag {
                        event_index: i as i64,
                        code: "TRANSFORM_SOURCE_MISSING".into(),
                        detail: format!(
                            "通道 {} 上游补偿缺失（未裁定/阻断），不进行变换",
                            spec.channel()
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 半开边界点-多边形判定。
/// 规则：射线法；落在边上的点，按边方向归入“左/下边包含，右/上边排除”，
/// 由 boundary_rule="half_open" 指定，保证一个边界点唯一归属或明确排除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointClass {
    Inside,
    BoundaryIncluded,
    BoundaryExcluded,
    Outside,
}

pub fn classify_point(gate: &PolygonGate, x: f64, y: f64) -> PointClass {
    let pts = &gate.points;
    let n = pts.len();
    if n < 3 {
        return PointClass::Outside;
    }
    let eps = 1e-9;
    let mut on_edge: Option<(usize, f64, f64)> = None;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let dot = (x - a[0]) * (b[0] - a[0]) + (y - a[1]) * (b[1] - a[1]);
        let len2 = (b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2);
        if cross.abs() < eps * (1.0 + len2.sqrt()) && dot >= -eps && dot <= len2 + eps {
            // 边方向：从 a 到 b。半开规则——下降的竖直跨度排除，其余包含。
            on_edge = Some((i, a[1], b[1]));
            break;
        }
    }
    // 射线法（水平向右）。
    let mut inside = false;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        if (a[1] > y) != (b[1] > y) {
            let xcross = a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]);
            if xcross > x {
                inside = !inside;
            }
        }
    }
    if let Some((_, ay, by)) = on_edge {
        // 半开区间：边的上沿（by > ay）排除，下沿包含。
        if by > ay {
            PointClass::BoundaryExcluded
        } else {
            PointClass::BoundaryIncluded
        }
    } else if inside {
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

pub fn is_in_gate(c: PointClass) -> bool {
    matches!(c, PointClass::Inside | PointClass::BoundaryIncluded)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMembership {
    pub gate_version_id: String,
    pub included_event_indices: Vec<i64>,
    pub boundary_included: Vec<i64>,
    pub boundary_excluded: Vec<i64>,
}

/// 在给定父集合（事件索引）上计算门归属。
pub fn evaluate_gate(
    gate: &PolygonGate,
    gate_version_id: &str,
    transformed: &[Vec<Option<f64>>],
    channel_index: &[String],
    parent_member_indices: &[i64],
) -> Result<GateMembership, String> {
    let xc = channel_index
        .iter()
        .position(|c| c == &gate.basis.x_channel)
        .ok_or_else(|| format!("缺少 X 通道 {}", gate.basis.x_channel))?;
    let yc = channel_index
        .iter()
        .position(|c| c == &gate.basis.y_channel)
        .ok_or_else(|| format!("缺少 Y 通道 {}", gate.basis.y_channel))?;
    let mut inc = Vec::new();
    let mut bi = Vec::new();
    let mut be = Vec::new();
    for &idx in parent_member_indices {
        let row = &transformed[idx as usize];
        let (Some(x), Some(y)) = (row[xc], row[yc]) else {
            continue; // 无法变换/补偿的点保留在原位置，但不进入门。
        };
        match classify_point(gate, x, y) {
            PointClass::Inside => inc.push(idx),
            PointClass::BoundaryIncluded => {
                bi.push(idx);
                inc.push(idx);
            }
            PointClass::BoundaryExcluded => be.push(idx),
            PointClass::Outside => {}
        }
    }
    Ok(GateMembership {
        gate_version_id: gate_version_id.into(),
        included_event_indices: inc,
        boundary_included: bi,
        boundary_excluded: be,
    })
}

/// 父子关系索引（钉住门 id，门 id 在改名时保持稳定）。
pub fn parent_map(links: &[GateLink]) -> std::collections::HashMap<String, Option<String>> {
    links
        .iter()
        .map(|l| (l.gate_id.clone(), l.parent_gate_id.clone()))
        .collect()
}

/// 计算失效范围：被改动实体的传递下游门。
/// kind 为 spillover/transform 时影响所有使用荧光的门；gate/lineage 时影响其子孙门。
pub fn stale_scope(
    changed: EntityKind,
    changed_entity_id: &str,
    all_versions: &[EntityVersion],
    links: &[GateLink],
) -> Vec<String> {
    let current_gates: std::collections::HashSet<String> = all_versions
        .iter()
        .filter(|v| v.kind == EntityKind::Gate)
        .map(|v| v.entity_id.clone())
        .collect();
    match changed {
        EntityKind::Spillover | EntityKind::Transform => {
            // 所有门结果都引用补偿/变换坐标基准，整体失效（按需求显式计算范围）。
            current_gates.into_iter().collect()
        }
        EntityKind::Gate | EntityKind::Lineage => {
            let parents = parent_map(links);
            let mut affected = vec![changed_entity_id.to_string()];
            let mut frontier = vec![changed_entity_id.to_string()];
            while let Some(g) = frontier.pop() {
                for (child, parent) in parents.iter() {
                    if parent.as_deref() == Some(g.as_str()) && !affected.contains(child) {
                        affected.push(child.clone());
                        frontier.push(child.clone());
                    }
                }
            }
            affected
                .into_iter()
                .filter(|id| current_gates.contains(id))
                .collect()
        }
    }
}

/// 改名是否只改变名称而不改变几何（改名不影响数值，只影响显示版本）。
pub fn is_rename_only(old: &serde_json::Value, new: &PolygonGate) -> bool {
    let Ok(old_gate) = serde_json::from_value::<PolygonGate>(old.clone()) else {
        return false;
    };
    old_gate.points == new.points && old_gate.basis == new.basis
}
