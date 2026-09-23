//! Derived layer: compensation -> transform -> gate membership -> populations.
//!
//! Every derived artifact carries a content id and an explicit dependency set.
//! A source change invalidates *only* the artifacts whose dependencies moved,
//! which is how stale propagation stays local instead of globally replacing
//! old conclusions.

use crate::cid::{cid_of, cid_of_ids};
use crate::domain::*;
use crate::gate::{classify, BoundaryRule};
use crate::matrix::{self, InversionReport};
use crate::transform::apply_channel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

impl Raw {
    pub fn is_unconvertible(&self) -> bool {
        matches!(self, Raw::Unconvertible { .. })
    }
}

/// Stable identity of a single displayed point: the event set doc id plus the
/// event index. Survives export/import and branch side-by-side views.
pub fn point_id(events_doc_id: &str, event_index: usize) -> String {
    format!("pt_{events_doc_id}_e{event_index}")
}

/// Explicit dependency set of a derived artifact. Only the cids actually used
/// by the computation are listed, so a change in an unrelated channel/version
/// does not invalidate it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DepSet {
    pub events_doc_cid: Option<String>,
    pub compensation_result_cid: Option<String>,
    pub transform_doc_cid: Option<String>,
    pub gate_doc_cid: Option<String>,
    pub lineage_doc_cid: Option<String>,
    pub parent_membership_cid: Option<String>,
}

impl DepSet {
    fn cids(&self) -> Vec<String> {
        [
            self.events_doc_cid.clone(),
            self.compensation_result_cid.clone(),
            self.transform_doc_cid.clone(),
            self.gate_doc_cid.clone(),
            self.lineage_doc_cid.clone(),
            self.parent_membership_cid.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompensationResult {
    pub channels: Vec<String>,
    pub report: InversionReport,
    /// Compensated vector per event; `None` for unconvertible events or when
    /// the matrix was not invertible (no silent inversion).
    pub rows: Vec<Option<Vec<f64>>>,
    pub cid: String,
}

pub fn run_compensation(
    events: &EventsDoc,
    events_cid: &str,
    comp: &CompensationDoc,
    comp_cid: &str,
) -> CompensationResult {
    let report = matrix::analyze(&comp.matrix);
    let mut rows = vec![None; events.rows.len()];
    if let Some(inv) = &report.inverse {
        let ch_index: Vec<Option<usize>> = comp
            .channels
            .iter()
            .map(|cn| events.channels.iter().position(|c| &c.short_name == cn))
            .collect();
        for (ei, row) in events.rows.iter().enumerate() {
            let vals: Option<Vec<f64>> = ch_index
                .iter()
                .map(|o| o.and_then(|i| row.get(i)).and_then(|r| r.as_f64()))
                .collect();
            if let Some(vals) = vals {
                rows[ei] = Some(matrix::apply(inv, &vals));
            }
        }
    }
    let cid = cid_of(
        "comp_result",
        &serde_json::json!({
            "events": events_cid, "comp": comp_cid,
            "sv": report.singular_values, "code": report.diagnostic_code,
            "rows": rows,
        }),
    );
    CompensationResult {
        channels: comp.channels.clone(),
        report,
        rows,
        cid,
    }
}

/// Coordinate value of an event on a channel in a given coordinate basis.
pub fn coordinate(
    basis: Basis,
    channel: &str,
    event_index: usize,
    events: &EventsDoc,
    comp_result: Option<&CompensationResult>,
    transform: Option<&TransformDoc>,
) -> Option<f64> {
    let ch = events.channels.iter().position(|c| c.short_name == channel)?;
    let raw = events.rows.get(event_index)?.get(ch)?;
    match basis {
        Basis::RawSignal | Basis::ScaleLinear => raw.as_f64(),
        Basis::Compensated => {
            let cr = comp_result?;
            let col = cr.channels.iter().position(|c| c == channel)?;
            cr.rows.get(event_index)?.as_ref()?.get(col).copied()
        }
        Basis::Transformed => {
            let base = if let Some(cr) = comp_result {
                if let Some(col) = cr.channels.iter().position(|c| c == channel) {
                    cr.rows
                        .get(event_index)
                        .and_then(|r| r.as_ref())
                        .and_then(|v| v.get(col).copied())
                } else {
                    raw.as_f64()
                }
            } else {
                raw.as_f64()
            }?;
            let t = transform?.channels.iter().find(|t| t.channel == channel)?;
            Some(apply_channel(t, &Raw::number(base)).unwrap_or(base))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub gate_id: String,
    pub rule: BoundaryRule,
    pub membership_cid: String,
    /// Per-event status: in/out/boundary/excluded(NaN)/outside_parent.
    pub statuses: Vec<String>,
    pub count_in: usize,
    pub count_boundary: usize,
    pub count_excluded: usize,
    pub dep: DepSet,
}

#[allow(clippy::too_many_arguments)]
pub fn run_gate(
    gate: &GateDoc,
    rule: BoundaryRule,
    events: &EventsDoc,
    events_cid: &str,
    transform: Option<(&TransformDoc, &str)>,
    comp_result: Option<&CompensationResult>,
    parent_membership_cid: Option<String>,
    parent_includes: &[bool],
    gate_doc_cid: String,
    lineage_doc_cid: String,
) -> GateResult {
    let n = events.rows.len();
    let mut statuses = Vec::with_capacity(n);
    let mut count_in = 0;
    let mut count_boundary = 0;
    let mut count_excluded = 0;
    let tdoc = transform.map(|(t, _)| t);
    for ei in 0..n {
        if !parent_includes.get(ei).copied().unwrap_or(false) {
            statuses.push("outside_parent".into());
            continue;
        }
        let x = coordinate(gate.x_basis, &gate.x_channel, ei, events, comp_result, tdoc);
        let y = coordinate(gate.y_basis, &gate.y_channel, ei, events, comp_result, tdoc);
        let (x, y) = match (x, y) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                count_excluded += 1;
                statuses.push("excluded".into());
                continue;
            }
        };
        let m = classify(gate, x, y, rule);
        if m.on_boundary {
            count_boundary += 1;
            statuses.push("boundary".into());
        } else if m.status == "in" {
            count_in += 1;
            statuses.push("in".into());
        } else {
            statuses.push("out".into());
        }
    }
    let included: Vec<String> = statuses
        .iter()
        .enumerate()
        .filter(|(_, s)| is_included(s, rule))
        .map(|(ei, _)| point_id(events_cid, ei))
        .collect();
    let membership_cid = cid_of_ids("membership", &included);
    let uses_comp = gate.x_basis == Basis::Compensated
        || gate.y_basis == Basis::Compensated
        || gate.x_basis == Basis::Transformed
        || gate.y_basis == Basis::Transformed;
    GateResult {
        gate_id: gate.gate_id.clone(),
        rule,
        membership_cid,
        statuses,
        count_in,
        count_boundary,
        count_excluded,
        dep: DepSet {
            events_doc_cid: Some(events_cid.into()),
            compensation_result_cid: if uses_comp {
                comp_result.map(|c| c.cid.clone())
            } else {
                None
            },
            transform_doc_cid: if gate.x_basis == Basis::Transformed
                || gate.y_basis == Basis::Transformed
            {
                transform.map(|(_, c)| c.to_string())
            } else {
                None
            },
            gate_doc_cid: Some(gate_doc_cid),
            lineage_doc_cid: Some(lineage_doc_cid),
            parent_membership_cid,
        },
    }
}

pub fn is_included(status: &str, rule: BoundaryRule) -> bool {
    match status {
        "in" => true,
        "boundary" => matches!(rule, BoundaryRule::Inclusive),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopResult {
    pub gate_id: String,
    pub membership_cid: String,
    /// Pinned denominator: parent population membership cid at computation
    /// time (root = all-convertible-events membership). Percentages never
    /// silently follow a newer parent version.
    pub denominator_membership_cid: String,
    pub denominator_gate_id: Option<String>,
    pub numerator_count: usize,
    pub denominator_count: usize,
    pub percent: f64,
}

pub fn root_membership(events_cid: &str, events: &EventsDoc) -> (String, Vec<bool>, usize) {
    let included: Vec<bool> = events
        .rows
        .iter()
        .map(|row| !row.iter().any(Raw::is_unconvertible))
        .collect();
    let ids: Vec<String> = included
        .iter()
        .enumerate()
        .filter(|(_, b)| **b)
        .map(|(ei, _)| point_id(events_cid, ei))
        .collect();
    let count = ids.len();
    (cid_of_ids("root_membership", &ids), included, count)
}

/// Transitively collect gate ids downstream of `roots` through lineage edges.
pub fn downstream(roots: &[String], lineage: &LineageDoc) -> Vec<String> {
    let mut stale: BTreeSet<String> = roots.iter().cloned().collect();
    loop {
        let before = stale.len();
        for (child, parent) in &lineage.edges {
            if let Some(p) = parent {
                if stale.contains(p) {
                    stale.insert(child.clone());
                }
            }
        }
        if stale.len() == before {
            break;
        }
    }
    stale.into_iter().collect()
}
