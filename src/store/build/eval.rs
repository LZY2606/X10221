//! Pure evaluation: compensation -> transform -> gate membership -> population
//! percentages with denominators pinned to exact parent result versions.

use super::Replay;
use crate::domain::{
    apply_transforms, channel_index, gate_contains, ChannelMeta, EventSetV, GateNode, GateV,
    PopulationResult, TransformV,
};
use crate::linalg::mat_vec;
use serde_json::Value;

pub const ACQ: &str = "eventset:acq";
pub const COMP: &str = "comp:matrix";
pub const TRANSFORM: &str = "transform:global";
pub const LINEAGE: &str = "lineage:tree";

pub fn typed<T: serde::de::DeserializeOwned>(v: &Value) -> T {
    serde_json::from_value(v.clone()).expect("resource body shape")
}

pub fn latest_eventset(state: &Replay) -> Option<(EventSetV, i64)> {
    let (body, ver) = state.latest(ACQ)?;
    Some((typed::<EventSetV>(body), ver))
}

pub fn latest_comp(state: &Replay) -> Option<(Value, i64)> {
    let ver = *state.latest_version.get(COMP)?;
    Some((state.resources.get(COMP)?.get(&ver)?.clone(), ver))
}

pub fn gate(state: &Replay, gate_id: &str, version: i64) -> Option<GateV> {
    state.resources.get(gate_id)?.get(&version).map(typed::<GateV>)
}

pub fn transforms_for(state: &Replay, transform_id: &str, version: i64) -> Option<Vec<TransformV>> {
    let body = state.resources.get(transform_id)?.get(&version)?;
    if let Some(list) = body.get("transforms") {
        return serde_json::from_value::<Vec<TransformV>>(list.clone()).ok();
    }
    serde_json::from_value::<TransformV>(body.clone()).ok().map(|t| vec![t])
}

pub fn lineage(state: &Replay) -> Option<(Vec<GateNode>, i64)> {
    let ver = *state.latest_version.get(LINEAGE)?;
    let body = state.resources.get(LINEAGE)?.get(&ver)?;
    let nodes = serde_json::from_value::<Vec<GateNode>>(body.get("nodes").cloned()?)?;
    Some((nodes, ver))
}

fn order(nodes: &[GateNode]) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = nodes
        .iter()
        .filter(|n| n.parent_gate_id.is_none())
        .map(|n| n.gate_id.clone())
        .collect();
    while let Some(id) = stack.pop() {
        out.push(id.clone());
        for child in nodes.iter().filter(|n| n.parent_gate_id.as_deref() == Some(id.as_str())) {
            stack.push(child.gate_id.clone());
        }
    }
    out
}

fn compensated_rows(
    rows: &[Vec<f64>],
    channels: &[ChannelMeta],
    comp: &Value,
) -> (Vec<Vec<f64>>, Vec<String>) {
    let detectors: Vec<String> = comp
        .get("detectors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let unmix: Vec<Vec<f64>> = comp
        .get("unmixing")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|r| {
                    r.as_array()
                        .map(|c| c.iter().filter_map(Value::as_f64).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let idxs: Vec<usize> = detectors
        .iter()
        .filter_map(|d| channels.iter().position(|c| &c.name == d))
        .collect();
    let mut diagnostics = Vec::new();
    if idxs.len() != detectors.len() {
        diagnostics.push(
            "compensation references channels absent from acquisition; affected values stay raw"
                .to_string(),
        );
    }
    let mut out = rows.to_vec();
    if unmix.is_empty() || idxs.is_empty() {
        return (out, diagnostics);
    }
    for (ri, row) in rows.iter().enumerate() {
        let sub: Vec<f64> = idxs.iter().map(|i| row[*i]).collect();
        if sub.iter().any(|v| !v.is_finite()) {
            diagnostics.push(format!(
                "event {ri}: non-finite compensated-channel input; outputs retained as NaN"
            ));
            for i in &idxs {
                out[ri][*i] = f64::NAN;
            }
            continue;
        }
        if let Some(cm) = mat_vec(&unmix, &sub) {
            for (k, i) in idxs.iter().enumerate() {
                out[ri][*i] = cm[k];
            }
        }
    }
    (out, diagnostics)
}

#[derive(Debug, Clone)]
pub struct EvalContext {
    pub comp_required: std::collections::HashSet<String>,
}

/// Evaluate every gate against the latest accepted inputs. Stored historical
/// results are never regenerated; callers persist a NEW result version.
pub fn evaluate(state: &Replay, ctx: &EvalContext) -> Vec<PopulationResult> {
    let (eventset, eventset_version) = match latest_eventset(state) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let channels = eventset.channels;
    let raw: Vec<Vec<f64>> = eventset
        .values
        .iter()
        .map(|row| row.iter().map(|v| v.0).collect())
        .collect();
    let total = raw.len();

    let (nodes, _lineage_version) = lineage(state).unwrap_or_default();
    let gate_order = order(&nodes);

    let comp = latest_comp(state);
    let (signal_rows, comp_diag) = match &comp {
        Some((body, _)) => {
            let (rows, diag) = compensated_rows(&raw, &channels, body);
            (rows, diag)
        }
        None => (raw.clone(), Vec::new()),
    };

    let mut membership: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    let mut prior_versions: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut results = Vec::new();

    for gid in gate_order {
        let node = nodes.iter().find(|n| n.gate_id == gid).unwrap();
        let gate_version = state.latest_version.get(&gid).copied().unwrap_or(1);
        let gate = match gate(state, &gid, gate_version) {
            Some(g) => g,
            None => continue,
        };
        let tversion = gate.transform_version;
        let transformed = match transforms_for(state, &gate.transform_id, tversion) {
            Some(list) => apply_transforms(&signal_rows, &channels, &list),
            None => signal_rows.clone(),
        };
        let x_idx = channel_index(&channels, &gate.x_channel);
        let y_idx = gate.y_channel.as_deref().and_then(|y| channel_index(&channels, y));

        let parent = node.parent_gate_id.clone();
        let parent_members: Vec<usize> = match &parent {
            Some(pid) => membership.get(pid).cloned().unwrap_or_default(),
            None => (0..total).collect(),
        };
        let (parent_result_id, parent_result_version) = match &parent {
            Some(pid) => (Some(format!("pop:{pid}")), prior_versions.get(pid).copied()),
            None => (None, None),
        };

        let needs_comp = ctx.comp_required.contains(&gid);
        let blocked = needs_comp && comp.is_none();

        let mut diagnostics = comp_diag.clone();
        let members: Vec<usize> = if blocked {
            diagnostics.push(
                "BLOCKED: compensation matrix near-singular and not adjudicated; \
                 no compensated population computed"
                    .to_string(),
            );
            Vec::new()
        } else {
            match x_idx {
                Some(xi) => parent_members
                    .iter()
                    .copied()
                    .filter(|i| {
                        let x = transformed[*i][xi];
                        let y = y_idx.map(|yi| transformed[*i][yi]);
                        gate_contains(&gate, x, y)
                    })
                    .collect(),
                None => {
                    diagnostics.push(format!("gate channel '{}' not found in acquisition", gate.x_channel));
                    Vec::new()
                }
            }
        };

        let denominator = parent_members.len();
        let numerator = members.len();
        let percent = if blocked {
            None
        } else if denominator == 0 {
            diagnostics.push("denominator parent population is empty; percent undefined".to_string());
            None
        } else {
            Some(100.0 * numerator as f64 / denominator as f64)
        };

        let comp_candidate_id = comp.and_then(|_| {
            state
                .candidates
                .values()
                .find(|c| c.status == "accepted" && c.scope == "compensation")
                .map(|c| c.candidate_id.clone())
        });

        membership.insert(gid.clone(), if blocked { Vec::new() } else { members.clone() });
        let result_version = prior_versions.get(&gid).map(|v| v + 1).unwrap_or(1);
        prior_versions.insert(gid.clone(), result_version);

        results.push(PopulationResult {
            status: if blocked { "blocked".to_string() } else { "computed".to_string() },
            gate_id: gid,
            gate_version,
            parent_result_id,
            parent_result_version,
            transform_id: gate.transform_id.clone(),
            transform_version: tversion,
            comp_candidate_id,
            denominator_scope: match &parent {
                Some(pid) => format!("parent population pop:{pid} (pinned result version)"),
                None => format!("acquisition {ACQ} v{eventset_version} (all events)"),
            },
            denominator_event_count: Some(denominator),
            numerator_event_count: Some(numerator),
            percent,
            member_event_indexes: Some(members),
            diagnostics,
        });
    }
    results
}

/// Direct + transitive affected results for a revision, based on the gate
/// lineage. Never silently replaces results; callers emit stale markers.
pub fn affected_scope(
    nodes: &[GateNode],
    direct: &[String],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack: Vec<(String, String)> =
        direct.iter().map(|g| (g.clone(), "direct".to_string())).collect();
    let mut seen = std::collections::HashSet::new();
    while let Some((gid, scope)) = stack.pop() {
        if !seen.insert((gid.clone(), scope.clone())) {
            continue;
        }
        out.push((gid.clone(), scope));
        for child in nodes
            .iter()
            .filter(|n| n.parent_gate_id.as_deref() == Some(gid.as_str()))
        {
            stack.push((child.gate_id.clone(), "transitive".to_string()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
