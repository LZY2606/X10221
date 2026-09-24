//! 派生层：补偿 → 变换 → 门成员 → 群百分比。
//!
//! 只有派生层做规范化；无法转换的事件值保留在原位并以 Unknown/Boundary 显式呈现。

use std::collections::BTreeMap;

use crate::domain::gate::{classify, Membership, PolygonGate};
use crate::domain::ids::content_rev_parts;
use crate::domain::matrix::apply_compensation;
use crate::domain::model::{
    CompensationMatrix, DenominatorRef, PopulationResult,
};
use crate::domain::project::{BranchState, RawEventRow, ROOT_GATE};

/// 某事件在给定补偿矩阵下的荧光值（非荧光通道保持原值）。
fn compensated_values(
    state: &BranchState,
    event: &RawEventRow,
    matrix: Option<&CompensationMatrix>,
) -> BTreeMap<String, f64> {
    let mut vals = event.values.clone();
    if let Some(m) = matrix {
        if let Some(comp) = m.comp_matrix() {
            let ordered: Vec<f64> = m.channels.iter().map(|c| *vals.get(c).unwrap_or(&0.0)).collect();
            let out = apply_compensation(comp, &ordered);
            for (c, v) in m.channels.iter().zip(out) {
                vals.insert(c.clone(), v);
            }
        }
    }
    vals
}

fn transformed(state: &BranchState, channel: &str, v: f64, transform_rev: Option<&str>) -> Option<f64> {
    if let Some(rev) = transform_rev {
        let tp = state
            .transforms
            .values()
            .find(|t| t.rev == rev && t.channel == channel)?;
        tp.spec.apply(v).0
    } else if v.is_finite() {
        Some(v)
    } else {
        None
    }
}

fn transform_rev_for(state: &BranchState, channel: &str) -> Option<String> {
    state.transform_order.iter().find_map(|id| {
        let t = state.transforms.get(id)?;
        if t.channel == channel {
            Some(t.rev.clone())
        } else {
            None
        }
    })
}

fn event_set_rev(ids: &[String]) -> String {
    let parts: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    content_rev_parts(&parts)
}

/// 计算某门在指定父群成员集合上的一次群结果。
///
/// `boundary_decisions` 为边界事件的显式裁定（event_id → include）。
/// 未裁定的边界点不进入 inside，单独列出。
pub fn derive_population(
    state: &BranchState,
    gate: &PolygonGate,
    parent_event_ids: &[String],
    parent_result_rev: String,
    parent_count: usize,
    boundary_decisions: &BTreeMap<String, bool>,
) -> PopulationResult {
    let matrix = state.active_matrix();
    let comp_rev = matrix.map(|m| m.rev.clone()).unwrap_or_else(|| "matrix#none".into());

    let x_rev = gate.basis.x_transform_rev.clone().or_else(|| transform_rev_for(state, &gate.basis.x_channel));
    let y_rev = gate.basis.y_transform_rev.clone().or_else(|| transform_rev_for(state, &gate.basis.y_channel));

    let mut basis_gate = gate.clone();
    basis_gate.basis.x_transform_rev = x_rev.clone();
    basis_gate.basis.y_transform_rev = y_rev.clone();

    let mut inside = Vec::new();
    let mut boundary = Vec::new();
    let mut unknown = Vec::new();

    for eid in parent_event_ids {
        let row = match state.raw_events.get(eid) {
            Some(r) => r,
            None => continue,
        };
        let comp_vals = compensated_values(state, row, matrix);
        let x_raw = comp_vals.get(&gate.basis.x_channel).copied();
        let y_raw = comp_vals.get(&gate.basis.y_channel).copied();
        let x = x_raw.and_then(|v| transformed(state, &gate.basis.x_channel, v, x_rev.as_deref()));
        let y = y_raw.and_then(|v| transformed(state, &gate.basis.y_channel, v, y_rev.as_deref()));
        match classify(&basis_gate, x, y) {
            Membership::Inside => inside.push(eid.clone()),
            Membership::Boundary => match boundary_decisions.get(eid) {
                Some(true) => inside.push(eid.clone()),
                Some(false) => {}
                None => boundary.push(eid.clone()),
            },
            Membership::Outside => {}
            Membership::Unknown => unknown.push(eid.clone()),
        }
    }

    inside.sort();
    let membership_rev = event_set_rev(&inside);
    let denom = DenominatorRef {
        parent_gate_id: gate.parent_gate_id.clone().unwrap_or_else(|| ROOT_GATE.into()),
        parent_gate_rev: parent_result_rev.clone(),
        compensation_rev: comp_rev.clone(),
        parent_membership_rev: parent_result_rev,
        parent_count,
    };
    let count = inside.len();
    let percent = if parent_count == 0 {
        0.0
    } else {
        100.0 * count as f64 / parent_count as f64
    };
    let result_rev = content_rev_parts(&[
        &gate.stable_id,
        &gate.rev,
        &comp_rev,
        &membership_rev,
        &denom.parent_gate_rev,
    ]);
    PopulationResult {
        gate_id: gate.stable_id.clone(),
        gate_rev: gate.rev.clone(),
        gate_name: gate.name.clone(),
        parent_gate_id: gate.parent_gate_id.clone(),
        compensation_rev: comp_rev,
        inside_event_ids: inside,
        boundary_event_ids: boundary,
        unknown_event_ids: unknown,
        count,
        percent,
        denominator: denom,
        result_rev,
    }
}

/// 按门树从根向下重算全部群结果；每个分母钉住刚刚算出的父群版本。
pub fn derive_tree(
    state: &BranchState,
    boundary_decisions: &BTreeMap<String, BTreeMap<String, bool>>,
) -> Vec<PopulationResult> {
    let mut results: Vec<PopulationResult> = Vec::new();
    let root_ids: Vec<String> = state.event_order.clone();
    let root_rev = event_set_rev(&root_ids);

    for id in &state.gate_order {
        let gate = match state.gate(id) {
            Some(g) => g.clone(),
            None => continue,
        };
        let (parent_ids, parent_rev, parent_count) = match gate.parent_gate_id.as_deref() {
            None => (root_ids.clone(), root_rev.clone(), root_ids.len()),
            Some(pid) if pid == ROOT_GATE => {
                (root_ids.clone(), root_rev.clone(), root_ids.len())
            }
            Some(pid) => match results.iter().rev().find(|r| r.gate_id == pid) {
                Some(pr) => (pr.inside_event_ids.clone(), pr.result_rev.clone(), pr.count),
                None => continue,
            },
        };
        let decisions = boundary_decisions.get(&gate.stable_id).cloned().unwrap_or_default();
        let r = derive_population(state, &gate, &parent_ids, parent_rev, parent_count, &decisions);
        results.push(r);
    }
    results
}

/// 计算某上游版本变化影响的门范围（沿父子边传播）。
pub fn invalidation_scope(state: &BranchState, changed_gate_id: &str) -> Vec<String> {
    state.descendants_of(changed_gate_id)
}
