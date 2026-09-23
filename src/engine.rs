//! Recompute / invalidation engine.
//!
//! Streams changed by a revision define an *invalidation scope*: only results
//! whose dependency edges touch a changed entity are affected. Downstream gate
//! results are marked stale and left in place; fresh results are written as new
//! rows with the new dependency set.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::compute::{gate_membership, run_pipeline, BranchView};
use crate::geometry::Membership;
use crate::id::obj_id;
use crate::linalg::{judge_invert, ridge_inverse, InvertStatus};
use crate::models::{CompMatrix, GateHierarchy};
use crate::store::{
    Db, ResultCandidateRow, ResultRow, BranchRow,
    STREAM_COMP, STREAM_GATES, STREAM_HIER, STREAM_RAW, STREAM_XFORM,
};

/// Result kinds.
pub const KIND_COMPENSATION: &str = "compensation";
pub const KIND_GATE: &str = "gate_membership";
pub const KIND_POP_PCT: &str = "population_pct";

/// Result statuses.
pub const ST_OK: &str = "ok";
pub const ST_BLOCKED: &str = "blocked";
pub const ST_PENDING_RULING: &str = "pending_ruling";
pub const ST_STALE: &str = "stale";

pub struct RecomputeOutcome {
    pub stale_marked: Vec<String>,
    pub created: Vec<String>,
    pub blocking: Vec<String>,
}

fn changed_streams(changed_entities: &[(String, i64)]) -> BTreeSet<String> {
    // entity streams are resolved by the caller into stream ids; here we use
    // the stream prefix grouping so a new gate version invalidates that gate.
    changed_entities.iter().map(|(s, _)| s.clone()).collect()
}

fn result_touches(result: &ResultRow, changed: &BTreeSet<String>) -> bool {
    result.deps.iter().any(|(stream, _, _)| {
        changed.iter().any(|c| {
            // Exact stream match, or same polygon key under a new version.
            stream == c || (stream.starts_with("gate_polygon:") && stream == c)
        })
    })
}

/// Recursively expand invalidation along the gate parent/child chain: a stale
/// parent gate invalidates the child percentages even if the child's own deps
/// did not change.
pub fn invalidate_scope(
    db: &Db,
    case_id: &str,
    branch: &BranchRow,
    changed_entities: &[(String, i64)],
    reason_event: &str,
) -> Vec<String> {
    let mut changed = changed_streams(changed_entities);
    // Changing raw or comp affects every gate/pct downstream.
    let global = changed.contains(STREAM_RAW)
        || changed.contains(STREAM_COMP)
        || changed.contains(STREAM_XFORM)
        || changed.contains(STREAM_HIER);
    let existing = db.list_results_for_branch(case_id, &branch.branch_id);
    let mut marked = Vec::new();

    // Fixed-point expansion over parent edges.
    let mut progress = true;
    while progress {
        progress = false;
        for r in &existing {
            if r.stale {
                continue;
            }
            let touches = global
                || result_touches(r, &changed)
                || r.denominator_result_id
                    .as_ref()
                    .map(|d| marked.contains(d) || existing.iter().any(|x| &x.result_id == d && x.stale))
                    .unwrap_or(false);
            if touches {
                db.mark_result_stale(case_id, &r.result_id, reason_event);
                db.record_result_timeline(case_id, &r.result_id, reason_event, ST_STALE);
                marked.push(r.result_id.clone());
                // Gate stale -> its population percentage must also be stale.
                progress = true;
            }
        }
    }
    marked
}

#[allow(clippy::too_many_arguments)]
fn make_result_id(
    case_id: &str,
    branch_id: &str,
    kind: &str,
    target: &str,
    deps: &[(String, String, i64)],
    value: &Value,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(case_id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(branch_id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(kind.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(target.as_bytes());
    let mut sorted = deps.to_vec();
    sorted.sort();
    for (s, id, v) in &sorted {
        bytes.push(0);
        bytes.extend_from_slice(s.as_bytes());
        bytes.extend_from_slice(id.as_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes.extend_from_slice(crate::store::canonical(value).as_slice());
    obj_id("result", &bytes)
}

impl Db {
    /// Build current compensation / gate / percentage results for the branch.
    /// Creates a fresh row for every result whose dependency set is new; never
    /// overwrites existing rows.
    pub fn populate_results(
        &self,
        case_id: &str,
        branch: &BranchRow,
        at_event: &str,
        up_to_seq: i64,
    ) -> (Vec<String>, Vec<String>) {
        let view = match self.branch_view_at(case_id, &branch.branch_id, Some(up_to_seq)) {
            Some(v) => v,
            None => return (Vec::new(), Vec::new()),
        };
        let pipeline = run_pipeline(&view);
        let decisions: BTreeMap<String, String> = self
            .decisions_at(case_id, &branch.branch_id, up_to_seq)
            .into_iter()
            .collect();
        let mut created = Vec::new();
        let mut blocking = Vec::new();
        let existing = self.list_results_for_branch(case_id, &branch.branch_id);
        let has_depset = |rid_kind: &str, target: &str, deps: &[(String, String, i64)]| {
            existing.iter().any(|r| {
                r.kind == rid_kind
                    && r.target_key == target
                    && deps_same(&r.deps, deps)
            })
        };

        // ---- compensation ----
        let comp_ref = view.refs.get(STREAM_COMP).cloned();
        let raw_ref = view.refs.get(STREAM_RAW).cloned();
        if let (Some(comp_ref), Some(raw_ref)) = (comp_ref, raw_ref) {
            let deps = vec![
                (STREAM_RAW.to_string(), raw_ref.0.clone(), raw_ref.1),
                (STREAM_COMP.to_string(), comp_ref.0.clone(), comp_ref.1),
            ];
            if let Some(comp) = &view.comp {
                let spill = view.refs.get(crate::store::STREAM_SPILL_MATRIX)
                    .and_then(|(id, _)| self.get_entity(case_id, id))
                    .and_then(|e| serde_json::from_value::<crate::models::SpilloverMatrix>(e.payload).ok());
                if !has_depset(KIND_COMPENSATION, "channels", &deps) {
                    let (status, value, candidates) =
                        comp_status_and_candidates(comp, spill.as_ref(), &pipeline, decisions.get("comp:inverse").map(String::as_str));
                    if status != ST_OK {
                        blocking.push(format!("compensation: {}", comp.invert.message));
                    }
                    let value = json!({
                        "status": status,
                        "summary": value,
                        "diagnostics": pipeline.diagnostics,
                    });
                    let rid = make_result_id(case_id, &branch.branch_id, KIND_COMPENSATION, "channels", &deps, &value);
                    self.insert_result(case_id, &ResultRow {
                        result_id: rid.clone(),
                        branch_id: branch.branch_id.clone(),
                        kind: KIND_COMPENSATION.into(),
                        target_key: "channels".into(),
                        status: status.into(),
                        value: Some(value),
                        denominator_result_id: None,
                        computed_at_event: at_event.into(),
                        stale: false,
                        stale_reason: None,
                        ruling_id: None,
                        deps,
                        candidates,
                    });
                    self.record_result_timeline(case_id, &rid, at_event, status);
                    created.push(rid);
                }
            }
        }

        // ---- gates ----
        let mut gate_result_ids: BTreeMap<String, String> = BTreeMap::new();
        for gate in &view.gates {
            let gate_stream = format!("{}:{}", STREAM_GATES, gate.gate_key);
            let gate_ref = view.refs.get(&gate_stream).cloned();
            let (Some(gate_ref), Some(raw_ref), Some(xf_ref)) = (
                gate_ref,
                view.refs.get(STREAM_RAW).cloned(),
                view.refs.get(STREAM_XFORM).cloned(),
            ) else { continue };
            let mut deps = vec![
                (STREAM_RAW.to_string(), raw_ref.0.clone(), raw_ref.1),
                (STREAM_XFORM.to_string(), xf_ref.0.clone(), xf_ref.1),
                (gate_stream, gate_ref.0.clone(), gate_ref.1),
            ];
            if let Some(c) = view.refs.get(STREAM_COMP).cloned() {
                deps.push((STREAM_COMP.to_string(), c.0, c.1));
            }
            let membership = gate_membership(&pipeline, gate);
            let inside = membership.iter().filter(|m| **m == Membership::Inside).count();
            let boundary = membership.iter().filter(|m| **m == Membership::Boundary).count();
            let outside = membership.iter().filter(|m| **m == Membership::Outside).count();

            // Boundary points -> two legal candidates if no edge decision yet.
            let slot = format!("gate_edge:{}", gate.gate_key);
            let decided = decisions.get(&slot).cloned();
            let mut candidates = Vec::new();
            let (status, effective_inside) = if boundary > 0 && decided.is_none() {
                candidates.push(ResultCandidateRow {
                    candidate_id: format!("{}_edge_exclude", gate.gate_key),
                    label: "boundary excluded (proposed policy)".into(),
                    accepted: None,
                    value: json!({"included_inside": inside}),
                    rationale: "points exactly on a polygon edge are excluded until adjudicated".into(),
                });
                candidates.push(ResultCandidateRow {
                    candidate_id: format!("{}_edge_include", gate.gate_key),
                    label: "boundary included".into(),
                    accepted: None,
                    value: json!({"included_inside": inside + boundary}),
                    rationale: "points exactly on a polygon edge are treated as inside".into(),
                });
                (ST_PENDING_RULING, inside)
            } else {
                let include = match decided.as_deref() {
                    Some(d) if d.ends_with("_edge_include") => true,
                    Some(d) if d.ends_with("_edge_exclude") => false,
                    _ => gate.edge_policy_proposal == "include",
                };
                let chosen = if include {
                    format!("{}_edge_include", gate.gate_key)
                } else {
                    format!("{}_edge_exclude", gate.gate_key)
                };
                candidates.push(ResultCandidateRow {
                    candidate_id: chosen.clone(),
                    label: if include { "boundary included".into() } else { "boundary excluded".into() },
                    accepted: Some(true),
                    value: json!({"included_inside": if include { inside + boundary } else { inside }}),
                    rationale: "edge policy pinned by branch decision ledger".into(),
                });
                (ST_OK, if include { inside + boundary } else { inside })
            };

            let total = membership.len();
            let value = json!({
                "gate_key": gate.gate_key,
                "label": gate.label,
                "channels": gate.channels,
                "inside": effective_inside,
                "boundary": boundary,
                "outside": outside,
                "total": total,
                "membership": membership.iter().map(|m| match m {
                    Membership::Inside => "inside",
                    Membership::Outside => "outside",
                    Membership::Boundary => "boundary",
                }).collect::<Vec<_>>(),
            });
            if !has_depset(KIND_GATE, &gate.gate_key, &deps) {
                let rid = make_result_id(case_id, &branch.branch_id, KIND_GATE, &gate.gate_key, &deps, &value);
                self.insert_result(case_id, &ResultRow {
                    result_id: rid.clone(),
                    branch_id: branch.branch_id.clone(),
                    kind: KIND_GATE.into(),
                    target_key: gate.gate_key.clone(),
                    status: status.into(),
                    value: Some(value),
                    denominator_result_id: None,
                    computed_at_event: at_event.into(),
                    stale: false,
                    stale_reason: None,
                    ruling_id: None,
                    deps: deps.clone(),
                    candidates,
                });
                self.record_result_timeline(case_id, &rid, at_event, status);
                gate_result_ids.insert(gate.gate_key.clone(), rid.clone());
                created.push(rid);
            } else {
                if let Some(r) = existing.iter().find(|r| {
                    r.kind == KIND_GATE && r.target_key == gate.gate_key && deps_same(&r.deps, &deps)
                }) {
                    gate_result_ids.insert(gate.gate_key.clone(), r.result_id.clone());
                }
            }
        }

        // ---- population percentages (denominator pinned to parent version) ----
        if let Some(hier) = &view.hierarchy {
            for node in &hier.nodes {
                let own = match gate_result_ids.get(&node.gate_key) {
                    Some(id) => id.clone(),
                    None => continue,
                };
                // Parent: nearest ancestor with an existing gate result on this
                // same dependency snapshot. Root denominator = raw events count.
                let parent_key = ancestor_gate(hier, node);
                let (denom_result, denom_count) = match parent_key {
                    Some(pk) => match gate_result_ids.get(&pk) {
                        Some(prid) => {
                            let prow = self.get_result(case_id, prid).unwrap();
                            let count = prow.value.as_ref().and_then(|v| v["inside"].as_i64()).unwrap_or(0);
                            (Some(prid.clone()), count as usize)
                        }
                        None => continue,
                    },
                    None => (
                        None,
                        view.raw.as_ref().map(|r| r.events.len()).unwrap_or(0),
                    ),
                };
                let own_row = self.get_result(case_id, &own).unwrap();
                let inside = own_row
                    .value
                    .as_ref()
                    .and_then(|v| v["inside"].as_i64())
                    .unwrap_or(0) as usize;
                let pct = if denom_count == 0 {
                    None
                } else {
                    Some((inside as f64) / (denom_count as f64) * 100.0)
                };
                // Deps include the denominator result's own deps implicitly via
                // denominator_result_id, which is the pinned parent version.
                let mut deps = own_row.deps.clone();
                if let Some(dr) = &denom_result {
                    if !deps.iter().any(|(_, id, _)| id == dr) {
                        if let Some(parent_result) = self.get_result(case_id, dr) {
                            for d in parent_result.deps {
                                if !deps.contains(&d) {
                                    deps.push(d);
                                }
                            }
                        }
                    }
                }
                let value = json!({
                    "population_key": node.population_key,
                    "gate_key": node.gate_key,
                    "parent_population_key": node.parent_population_key,
                    "numerator": inside,
                    "denominator": denom_count,
                    "denominator_pinned_result": denom_result,
                    "percent": pct,
                });
                if !has_depset(KIND_POP_PCT, &node.population_key, &deps) {
                    let rid = make_result_id(case_id, &branch.branch_id, KIND_POP_PCT, &node.population_key, &deps, &value);
                    self.insert_result(case_id, &ResultRow {
                        result_id: rid.clone(),
                        branch_id: branch.branch_id.clone(),
                        kind: KIND_POP_PCT.into(),
                        target_key: node.population_key.clone(),
                        status: own_row.status.clone(),
                        value: Some(value),
                        denominator_result_id: denom_result,
                        computed_at_event: at_event.into(),
                        stale: false,
                        stale_reason: None,
                        ruling_id: None,
                        deps,
                        candidates: Vec::new(),
                    });
                    self.record_result_timeline(case_id, &rid, at_event, &own_row.status);
                    created.push(rid);
                }
            }
        }

        (created, blocking)
    }
}

fn deps_same(a: &[(String, String, i64)], b: &[(String, String, i64)]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

fn ancestor_gate(hier: &GateHierarchy, node: &crate::models::HierarchyNode) -> Option<String> {
    let parent_pop = node.parent_population_key.clone()?;
    hier.nodes
        .iter()
        .find(|n| n.population_key == parent_pop)
        .map(|n| n.gate_key.clone())
}

fn comp_status_and_candidates(
    comp: &CompMatrix,
    spill: Option<&crate::models::SpilloverMatrix>,
    _pipeline: &crate::compute::PipelineData,
    decision: Option<&str>,
) -> (&'static str, Value, Vec<ResultCandidateRow>) {
    let _ = (pipeline_is_used(_pipeline), decision.is_some());
    match comp.invert.status.as_str() {
        "ok" => (
            ST_OK,
            json!({"applied": true, "note": "compensation matrix applied"}),
            Vec::new(),
        ),
        "near_singular" => {
            // All legal ridge candidates are computed from the real spillover
            // matrix and retained. None is applied silently.
            let mut candidates = Vec::new();
            if let Some(sp) = spill {
                for lambda in [1e-6_f64, 1e-4] {
                    let slot = format!("ridge_{:e}", lambda);
                    let value_candidate = ridge_inverse(&sp.matrix, lambda)
                        .map(|m| json!({
                            "lambda": lambda,
                            "matrix": m,
                            "min_pivot": comp.invert.min_pivot,
                            "condition_estimate": comp.invert.condition_estimate,
                        }))
                        .unwrap_or_else(|| json!({"lambda": lambda, "matrix": null}));
                    candidates.push(ResultCandidateRow {
                        candidate_id: format!("comp_{}", slot).replace('.', "_"),
                        label: format!("ridge regression lambda={:e}", lambda),
                        accepted: decision.map(|d| d == slot),
                        value: value_candidate,
                        rationale: "regularized (S+lambda I)^-1; near-singular spillover cannot be inverted without an explicit human ruling".into(),
                    });
                }
            }
            (
                ST_PENDING_RULING,
                json!({
                    "applied": false,
                    "reason": "near_singular",
                    "min_pivot": comp.invert.min_pivot,
                    "threshold": comp.invert.threshold,
                    "condition_estimate": comp.invert.condition_estimate,
                    "legal_candidates": candidates.len(),
                }),
                candidates,
            )
        }
        _ => (
            ST_BLOCKED,
            json!({
                "applied": false,
                "reason": comp.invert.status,
                "min_pivot": comp.invert.min_pivot,
                "message": comp.invert.message,
            }),
            Vec::new(),
        ),
    }
}

// Silence unused-import guard for InvertStatus used by callers/tests.
#[allow(dead_code)]
fn _use_status(s: InvertStatus) -> &'static str {
    match s {
        InvertStatus::Ok => "ok",
        InvertStatus::Singular => "singular",
        InvertStatus::NearSingular => "near_singular",
    }
}

fn pipeline_is_used<T>(_: &T) -> bool { true }
