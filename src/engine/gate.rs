//! 门几何与门评估。边界点按门的 `boundary_policy` 处理；
//! `candidate` 策略下保留 inside/outside 全部合法候选，绝不私自二选一。

use crate::domain::{
    DenominatorRef, EventsArtifact, GateLinkArtifact, GatePolygonArtifact, GateResult,
    ResultStatus, TransformArtifact,
};
use crate::engine::transform_value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointClass {
    Inside,
    Boundary,
    Outside,
}

/// 射线法 + 边距离判定边界。返回 (分类)。
pub fn classify_point(point: [f64; 2], verts: &[[f64; 2]]) -> PointClass {
    if point.iter().any(|v| !v.is_finite()) {
        return PointClass::Outside;
    }
    if on_boundary(point, verts) {
        return PointClass::Boundary;
    }
    let mut inside = false;
    let mut j = verts.len() - 1;
    for i in 0..verts.len() {
        let (xi, yi) = (verts[i][0], verts[i][1]);
        let (xj, yj) = (verts[j][0], verts[j][1]);
        let intersect = (yi > point[1]) != (yj > point[1])
            && point[0] < (xj - xi) * (point[1] - yi) / (yj - yi) + xi;
        if intersect {
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

fn on_boundary(p: [f64; 2], verts: &[[f64; 2]]) -> bool {
    if verts.len() < 2 {
        return false;
    }
    let eps = 1e-9;
    for i in 0..verts.len() {
        let a = verts[i];
        let b = verts[(i + 1) % verts.len()];
        let abx = b[0] - a[0];
        let aby = b[1] - a[1];
        let apx = p[0] - a[0];
        let apy = p[1] - a[1];
        let len2 = abx * abx + aby * aby;
        if len2 < eps {
            if (apx * apx + apy * apy).sqrt() < eps {
                return true;
            }
            continue;
        }
        let t = ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0);
        let cx = a[0] + t * abx;
        let cy = a[1] + t * aby;
        let dist = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt();
        // 坐标是 asinh 空间浮点，1e-7 视为落在边上。
        if dist < 1e-7 {
            return true;
        }
    }
    false
}

/// 把门轴检测器在补偿后矩阵中的列映射出来并做变换，返回每个事件的 (x,y)。
fn gate_coordinates(
    events: &EventsArtifact,
    gate: &GatePolygonArtifact,
    transform: &TransformArtifact,
    compensated: &[Vec<f64>],
    comp_detectors: &[String],
) -> Vec<Option<[f64; 2]>> {
    let xi = comp_detectors.iter().position(|d| *d == gate.x_detector);
    let yi = comp_detectors.iter().position(|d| *d == gate.y_detector);
    let (xi, yi) = match (xi, yi) {
        (Some(a), Some(b)) => (a, b),
        _ => return vec![None; events.events.len()],
    };
    let xparam = transform.params.iter().find(|p| p.detector == gate.x_detector);
    let yparam = transform.params.iter().find(|p| p.detector == gate.y_detector);
    events
        .events
        .iter()
        .enumerate()
        .map(|(row, ev)| {
            // 补偿跳过的事件（含哨兵/不可解析）没有坐标，不画图造假。
            let row_vals = compensated.get(row)?;
            if row_vals.is_empty() {
                let _ = ev;
                return None;
            }
            let (xv, yv) = (row_vals[xi], row_vals[yi]);
            let xt = match xparam {
                Some(p) => transform_value(xv, p.cofactor, &p.method),
                None => xv,
            };
            let yt = match yparam {
                Some(p) => transform_value(yv, p.cofactor, &p.method),
                None => yv,
            };
            Some([xt, yt])
        })
        .collect()
}

/// 评估单个门。`parent_members` 为父群在**当前事件版本**下的成员事件 index；
/// 百分比的分母钉住 `denom`（父群版本快照），而不是临时重算的父群。
#[allow(clippy::too_many_arguments)]
pub fn evaluate_gate(
    gate: &GatePolygonArtifact,
    _link: &GateLinkArtifact,
    events: &EventsArtifact,
    transform: &TransformArtifact,
    compensated: &[Vec<f64>],
    comp_detectors: &[String],
    parent_members: &[u64],
    denom: Option<DenominatorRef>,
) -> GateResult {
    let coords = gate_coordinates(events, gate, transform, compensated, comp_detectors);
    let parent_set: std::collections::BTreeSet<u64> = parent_members.iter().copied().collect();

    let mut inside_idx = Vec::new();
    let mut boundary_idx = Vec::new();
    let mut diagnostics = Vec::new();

    for (row, ev) in events.events.iter().enumerate() {
        if !parent_set.contains(&ev.index) {
            continue;
        }
        let Some(pt) = coords[row] else {
            diagnostics.push(format!(
                "事件#{} 在父群内但缺少可用变换坐标（哨兵/不可解析），门控跳过且不丢弃",
                ev.index
            ));
            continue;
        };
        match classify_point(pt, &gate.vertices) {
            PointClass::Inside => inside_idx.push(ev.index),
            PointClass::Boundary => boundary_idx.push(ev.index),
            PointClass::Outside => {}
        }
    }

    diagnostics.sort();
    diagnostics.dedup();

    // 严格内外集合：boundary_inside = inside + boundary，boundary_outside = inside。
    let mut with_boundary = inside_idx.clone();
    with_boundary.extend(boundary_idx.iter().copied());
    with_boundary.sort();
    inside_idx.sort();
    boundary_idx.sort();

    let mut result = GateResult {
        gate_id: String::new(),
        status: ResultStatus::Ok,
        member_indices: Vec::new(),
        boundary_indices: boundary_idx,
        candidates: Vec::new(),
        denominator: denom.clone(),
        percent: None,
        diagnostics,
    };

    match gate.boundary_policy.as_str() {
        "inside" => {
            result.member_indices = with_boundary.clone();
        }
        "outside" => {
            result.member_indices = inside_idx.clone();
        }
        _ => {
            // candidate：无法唯一决定时保留全部合法候选，结果 blocked 等待裁定。
            result.status = ResultStatus::Blocked;
            result.member_indices = Vec::new();
            result.candidates = vec![
                crate::domain::GateCandidate {
                    label: "boundary-inside".into(),
                    member_indices: with_boundary,
                },
                crate::domain::GateCandidate {
                    label: "boundary-outside".into(),
                    member_indices: inside_idx,
                },
            ];
        }
    }

    if let Some(d) = &denom {
        if d.parent_count > 0 {
            let count = result.member_indices.len() as f64;
            result.percent = Some(100.0 * count / d.parent_count as f64);
        }
    }
    result
}
