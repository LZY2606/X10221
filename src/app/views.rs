//! 审阅页只读视图：散点、门树、补偿前后差异、分母来源、事件流。
//! 每个显示值带稳定身份（view_value_id），两个草案并排时身份稳定可对齐。

use serde::Serialize;
use serde_json::{json, Value};

use crate::app::service::{Service, SrvResult};
use crate::domain::gate::{classify, Membership};
use crate::domain::matrix::apply_compensation;
use crate::domain::project::*;

#[derive(Debug, Serialize)]
pub struct ScatterPoint {
    /// 稳定显示身份：branch 无关，两个草案并排时一致。
    pub view_value_id: String,
    pub event_id: String,
    pub x_raw: Option<f64>,
    pub y_raw: Option<f64>,
    pub x_comp: Option<f64>,
    pub y_comp: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub membership: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GateNode {
    pub gate_id: String,
    pub name: String,
    pub rev: String,
    pub ordinal: u64,
    pub parent: Option<String>,
    pub stale: bool,
    pub stale_reasons: Vec<String>,
    pub count: Option<usize>,
    pub percent: Option<f64>,
    pub denominator: Option<Value>,
    pub boundary: Vec<String>,
    pub unknown: Vec<String>,
    pub basis: Value,
}

#[derive(Debug, Serialize)]
pub struct CompDiffRow {
    pub event_id: String,
    pub channel: String,
    pub raw: f64,
    pub compensated: f64,
    pub delta: f64,
    pub unit: String,
}

fn tf(state: &BranchState, ch: &str, v: f64) -> Option<f64> {
    let tp = state.transforms.values().find(|t| t.channel == ch)?;
    tp.spec.apply(v).0
}

impl Service {
    pub fn view_scatter(&self, branch: &str, gate_id: &str) -> SrvResult<Value> {
        let state = self.state(branch)?;
        let gate = state.gate(gate_id).ok_or_else(|| crate::app::service::err("NOT_FOUND", "门不存在"))?;
        let matrix = state.active_matrix();
        let parent_ids: Vec<String> = match gate.parent_gate_id.as_deref() {
            None | Some(ROOT_GATE) => state.event_order.clone(),
            Some(pid) => state.population_for_latest(pid).map(|p| p.inside_event_ids.clone()).unwrap_or_default(),
        };
        let mut points = Vec::new();
        for eid in &parent_ids {
            let row = match state.raw_events.get(eid) { Some(r) => r, None => continue };
            let raw_x = *row.values.get(&gate.basis.x_channel).unwrap_or(&f64::NAN);
            let raw_y = *row.values.get(&gate.basis.y_channel).unwrap_or(&f64::NAN);
            let mut comp_vals = row.values.clone();
            if let Some(m) = matrix {
                if let Some(comp) = m.comp_matrix() {
                    let ordered: Vec<f64> = m.channels.iter().map(|c| *row.values.get(c).unwrap_or(&0.0)).collect();
                    let out = apply_compensation(comp, &ordered);
                    for (c, v) in m.channels.iter().zip(out) {
                        comp_vals.insert(c.clone(), v);
                    }
                }
            }
            let cvx = *comp_vals.get(&gate.basis.x_channel).unwrap_or(&f64::NAN);
            let cvy = *comp_vals.get(&gate.basis.y_channel).unwrap_or(&f64::NAN);
            let xt = tf(&state, &gate.basis.x_channel, cvx);
            let yt = tf(&state, &gate.basis.y_channel, cvy);
            let membership = match classify(gate, xt, yt) {
                Membership::Inside => "inside",
                Membership::Outside => "outside",
                Membership::Boundary => "boundary",
                Membership::Unknown => "unknown",
            };
            points.push(ScatterPoint {
                view_value_id: format!("scatter#{gate_id}#{eid}"),
                event_id: eid.clone(),
                x_raw: if raw_x.is_finite() { Some(raw_x) } else { None },
                y_raw: if raw_y.is_finite() { Some(raw_y) } else { None },
                x_comp: if cvx.is_finite() { Some(cvx) } else { None },
                y_comp: if cvy.is_finite() { Some(cvy) } else { None },
                x: xt,
                y: yt,
                membership: membership.into(),
                diagnostics: row.diagnostics.clone(),
            });
        }
        Ok(json!({
            "branch": branch,
            "gate_id": gate_id,
            "gate_rev": gate.rev,
            "basis": serde_json::to_value(&gate.basis).unwrap(),
            "polygon": gate.polygon.iter().map(|p| json!({"x": p.x, "y": p.y})).collect::<Vec<_>>(),
            "points": points,
        }))
    }

    pub fn view_tree(&self, branch: &str) -> SrvResult<Value> {
        let state = self.state(branch)?;
        let mut nodes = Vec::new();
        for gid in &state.gate_order {
            let g = match state.gate(gid) { Some(g) => g, None => continue };
            let pop = state.population_for_latest(gid);
            let stale_marks = state.gate_stale_marks(gid);
            let stale = !stale_marks.is_empty();
            nodes.push(GateNode {
                gate_id: gid.clone(),
                name: g.name.clone(),
                rev: g.rev.clone(),
                ordinal: g.ordinal,
                parent: g.parent_gate_id.clone(),
                stale,
                stale_reasons: stale_marks.iter().map(|m| format!("{}@{}", m.because_kind, &m.because_source_rev[..12.min(m.because_source_rev.len())])).collect(),
                count: pop.map(|p| p.count),
                percent: pop.map(|p| p.percent),
                denominator: pop.map(|p| json!({
                    "parent_gate_id": p.denominator.parent_gate_id,
                    "parent_gate_rev": p.denominator.parent_gate_rev,
                    "compensation_rev": p.denominator.compensation_rev,
                    "parent_membership_rev": p.denominator.parent_membership_rev,
                    "parent_count": p.denominator.parent_count,
                })),
                boundary: pop.map(|p| p.boundary_event_ids.clone()).unwrap_or_default(),
                unknown: pop.map(|p| p.unknown_event_ids.clone()).unwrap_or_default(),
                basis: serde_json::to_value(&g.basis).unwrap(),
            });
        }
        Ok(json!({"branch": branch, "nodes": nodes}))
    }

    pub fn view_comp_diff(&self, branch: &str) -> SrvResult<Value> {
        let state = self.state(branch)?;
        let m = match state.active_matrix() { Some(m) => m, None => {
            return Ok(json!({"branch": branch, "available": false, "reason": "补偿矩阵未就绪（近奇异待裁定或被拒绝）", "rows": []}));
        }};
        let comp = match m.comp_matrix() { Some(c) => c, None => return Ok(json!({"branch": branch, "available": false, "rows": []})) };
        let mut rows = Vec::new();
        for eid in &state.event_order {
            let row = match state.raw_events.get(eid) { Some(r) => r, None => continue };
            let ordered: Vec<f64> = m.channels.iter().map(|c| *row.values.get(c).unwrap_or(&0.0)).collect();
            let out = apply_compensation(comp, &ordered);
            for (c, (raw, cv)) in m.channels.iter().zip(ordered.iter().zip(out)) {
                let unit = state.channels.get(c).map(|ch| ch.unit.clone()).unwrap_or_default();
                rows.push(CompDiffRow {
                    event_id: eid.clone(), channel: c.clone(),
                    raw: *raw, compensated: cv, delta: cv - raw, unit,
                });
            }
        }
        Ok(json!({"branch": branch, "available": true, "matrix_id": m.stable_id,
                  "matrix_rev": m.rev, "status": m.status, "rcond": m.rcond,
                  "chosen_candidate": m.chosen_candidate,
                  "candidates": m.candidates.iter().map(|c| c.kind.clone()).collect::<Vec<_>>(),
                  "diagnostics": m.diagnostics, "rows": rows}))
    }

    pub fn view_overview(&self, branch: &str) -> SrvResult<Value> {
        let state = self.state(branch)?;
        Ok(json!({
            "branch": branch,
            "head_seq": state.head_seq,
            "head_event_id": state.head_event_id,
            "head_hash": state.head_hash,
            "channels": state.channel_order,
            "event_count": state.event_order.len(),
            "blobs": state.blobs.values().map(|b| json!({"blob_id": b.blob_id, "kind": b.kind, "filename": b.filename, "byte_len": b.byte_len, "sha256": b.sha256})).collect::<Vec<_>>(),
            "forked_from": state.forked_from,
        }))
    }

    /// 并排两个草案：每个显示值以稳定身份对齐。
    pub fn view_diff(&self, branch_a: &str, branch_b: &str) -> SrvResult<Value> {
        let ta = self.view_tree(branch_a)?;
        let tb = self.view_tree(branch_b)?;
        let index = |v: &Value| -> std::collections::BTreeMap<String, Value> {
            let mut map = std::collections::BTreeMap::new();
            if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                for n in nodes {
                    if let Some(id) = n.get("gate_id").and_then(|x| x.as_str()) {
                        map.insert(id.to_string(), n.clone());
                    }
                }
            }
            map
        };
        let ia = index(&ta);
        let ib = index(&tb);
        let mut rows = Vec::new();
        let mut all: Vec<String> = ia.keys().chain(ib.keys()).cloned().collect();
        all.sort();
        all.dedup();
        for gid in all {
            rows.push(json!({
                "gate_id": gid,
                "a": ia.get(&gid),
                "b": ib.get(&gid),
            }));
        }
        Ok(json!({"a": branch_a, "b": branch_b, "rows": rows}))
    }
}
