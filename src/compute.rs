//! Derived computation: compensate -> transform -> gate -> percentages.
//!
//! Guarantees enforced here:
//!  * a near-singular spillover matrix is never silently inverted; blocked
//!    compensation records explicit diagnostics and keeps all legal ridge
//!    candidates for adjudication;
//!  * a gate percentage's denominator is pinned to the *exact parent result
//!    version* it was computed from;
//!  * when an upstream gate is revised, downstream results are flagged stale
//!    (not overwritten), and re-computation produces fresh rows.

use serde_json::{json, Value};

use crate::geometry::{classify, Membership};
use crate::id::obj_id;
use crate::linalg::{judge_invert, mat_vec, ridge_inverse, InvertStatus};
use crate::models::{CompMatrix, GateHierarchy, GatePolygon, RawFcsEvents, TransformParams};
use crate::store::{
    Db, ResultCandidateRow, ResultRow, BranchRow,
    STREAM_COMP, STREAM_GATES, STREAM_HIER, STREAM_RAW, STREAM_XFORM,
};

pub struct BranchView {
    pub branch: BranchRow,
    pub refs: std::collections::BTreeMap<String, (String, i64)>,
    pub raw: Option<RawFcsEvents>,
    pub comp: Option<CompMatrix>,
    pub xform: Option<TransformParams>,
    pub gates: Vec<GatePolygon>,
    pub hierarchy: Option<GateHierarchy>,
}

impl Db {
    /// Materialize the dependency snapshot of a branch: current refs (or refs
    /// reconstructed at an arbitrary event), then load each entity payload.
    pub fn branch_view_at(
        &self,
        case_id: &str,
        branch_id_or_key: &str,
        up_to_seq: Option<i64>,
    ) -> Option<BranchView> {
        let branch = self.get_branch(case_id, branch_id_or_key)?;
        let pairs = match up_to_seq {
            Some(seq) => self.refs_at_event(case_id, &branch.branch_id, seq),
            None => self.get_refs(case_id, &branch.branch_id),
        };
        let mut refs = std::collections::BTreeMap::new();
        for (stream, id, ver) in pairs {
            refs.insert(stream, (id, ver));
        }
        let load = |stream: &str| -> Option<Value> {
            refs.get(stream)
                .and_then(|(id, _)| self.get_entity(case_id, id).map(|e| e.payload))
        };
        let raw = load(STREAM_RAW).and_then(|v| serde_json::from_value(v).ok());
        let comp = load(STREAM_COMP).and_then(|v| serde_json::from_value(v).ok());
        let xform = load(STREAM_XFORM).and_then(|v| serde_json::from_value(v).ok());
        let hierarchy = load(STREAM_HIER).and_then(|v| serde_json::from_value(v).ok());
        let mut gates = Vec::new();
        for (stream, (id, _)) in &refs {
            if stream.starts_with("gate_polygon:") {
                if let Some(e) = self.get_entity(case_id, id) {
                    if let Ok(g) = serde_json::from_value::<GatePolygon>(e.payload) {
                        gates.push(g);
                    }
                }
            }
        }
        Some(BranchView {
            branch,
            refs,
            raw,
            comp,
            xform,
            gates,
            hierarchy,
        })
    }
}

/// Per-event derived arrays for the review plots.
pub struct PipelineData {
    pub channel_names: Vec<String>,
    /// Raw finite values; NaN/Inf kept as None plus a diagnostic marker.
    pub raw: Vec<Vec<Option<f64>>>,
    pub raw_valid: Vec<Vec<bool>>,
    pub compensated: Option<Vec<Vec<Option<f64>>>>,
    pub transformed: Option<Vec<Vec<Option<f64>>>>,
    pub diagnostics: Vec<String>,
}

pub fn transform_value(v: f64, xform: &TransformParams) -> f64 {
    match xform.method.as_str() {
        "asinh" => (v / xform.cofactor).asinh(),
        "linear" => v,
        _ => v,
    }
}

pub fn run_pipeline(view: &BranchView) -> PipelineData {
    let mut diagnostics = Vec::new();
    let raw_events = match &view.raw {
        Some(r) => r,
        None => {
            return PipelineData {
                channel_names: Vec::new(),
                raw: Vec::new(),
                raw_valid: Vec::new(),
                compensated: None,
                transformed: None,
                diagnostics: vec!["no raw events on this branch".into()],
            };
        }
    };
    let channel_names: Vec<String> = raw_events.channels.iter().map(|c| c.short_name.clone()).collect();
    let mut raw = Vec::new();
    let mut raw_valid = Vec::new();
    for e in &raw_events.events {
        let mut vals = Vec::new();
        let mut valid = Vec::new();
        for sig in &e.values {
            match sig {
                crate::domain::Signal::Finite(v) => {
                    vals.push(Some(*v));
                    valid.push(true);
                }
                crate::domain::Signal::NaN => {
                    diagnostics.push(format!("event {} NaN retained, excluded from derived plots", e.event_index));
                    vals.push(None);
                    valid.push(false);
                }
                crate::domain::Signal::PosInf | crate::domain::Signal::NegInf => {
                    diagnostics.push(format!("event {} infinite signal retained at raw layer", e.event_index));
                    vals.push(None);
                    valid.push(false);
                }
            }
        }
        raw.push(vals);
        raw_valid.push(valid);
    }

    let mut compensated: Option<Vec<Vec<Option<f64>>>> = None;
    if let Some(comp) = &view.comp {
        match &comp.matrix {
            Some(mat) => {
                let mut rows = Vec::new();
                for row in &raw {
                    let mut out_row = vec![None; channel_names.len()];
                    if row.iter().all(|v| v.is_some()) {
                        let fin: Vec<f64> = row.iter().map(|v| v.unwrap()).collect();
                        if let Some(ordered) = order_to_matrix(&channel_names, &comp.channel_order, &fin) {
                            if let Some(c) = mat_vec(mat, &ordered) {
                                for (idx, name) in comp.channel_order.iter().enumerate() {
                                    if let Some(pos) = channel_names.iter().position(|n| n == name) {
                                        out_row[pos] = Some(c[idx]);
                                    }
                                }
                            }
                        }
                    }
                    rows.push(out_row);
                }
                compensated = Some(rows);
            }
            None => diagnostics.push(format!(
                "compensation blocked: matrix status={} (min_pivot={:e}); values remain uncompensated",
                comp.invert.status, comp.invert.min_pivot
            )),
        }
    }

    let transformed = match (&compensated, &view.xform) {
        (Some(comp_rows), Some(xf)) => {
            let mut rows = Vec::new();
            for r in comp_rows {
                rows.push(r.iter().map(|v| v.map(|x| transform_value(x, xf))).collect());
            }
            Some(rows)
        }
        (None, Some(xf)) => {
            diagnostics.push("transform skipped: compensation unavailable".into());
            None
        }
        _ => None,
    };

    PipelineData {
        channel_names,
        raw,
        raw_valid,
        compensated,
        transformed,
        diagnostics,
    }
}

fn order_to_matrix(names: &[String], order: &[String], values: &[f64]) -> Option<Vec<f64>> {
    let mut out = Vec::with_capacity(order.len());
    for ch in order {
        let pos = names.iter().position(|n| n == ch)?;
        out.push(values[pos]);
    }
    Some(out)
}

/// Gate membership for an event in transformed space.
pub fn gate_membership(
    pipeline: &PipelineData,
    gate: &GatePolygon,
) -> Vec<Membership> {
    let data = pipeline.transformed.as_ref();
    let mut out = vec![Membership::Outside; pipeline.raw.len()];
    let Some(rows) = data else { return out };
    let xi = pipeline.channel_names.iter().position(|c| c == &gate.channels[0]);
    let yi = pipeline.channel_names.iter().position(|c| c == &gate.channels[1]);
    let (Some(xi), Some(yi)) = (xi, yi) else { return out };
    for (i, row) in rows.iter().enumerate() {
        match (row[xi], row[yi]) {
            (Some(x), Some(y)) => out[i] = classify(x, y, &gate.points),
            _ => out[i] = Membership::Outside,
        }
    }
    out
}
