//! Replay engine: rebuild read models from the append-only event log and
//! evaluate gate-lineage results with pinned denominators and stale markers.

use super::Store;
use crate::domain::{
    CompMatrixV, EventSetV, GateV, LineageV, PopulationResult, SpilloverMetaV, TransformV,
};
use rusqlite::params;
use serde_json::Value;

pub mod eval;

pub type ResourceMap = std::collections::BTreeMap<String, std::collections::BTreeMap<i64, Value>>;

#[derive(Debug, Clone)]
pub struct CandidateRow {
    pub candidate_id: String,
    pub scope: String,
    pub status: String,
    pub option_key: String,
    pub payload: Value,
    pub rationale: String,
    pub created_seq: i64,
    pub decided_seq: Option<i64>,
}

#[derive(Debug, Default)]
pub struct Replay {
    pub resources: ResourceMap,
    pub latest_version: std::collections::HashMap<String, i64>,
    pub candidates: std::collections::BTreeMap<String, CandidateRow>,
    /// seq -> invalidation payload (affects stale scope computation)
    pub invalidations: Vec<(i64, Value)>,
    /// seq -> populations evaluated payload, in evaluation order
    pub evaluations: Vec<(i64, String, Value)>,
    pub rules: Vec<(i64, Value)>,
    pub names: std::collections::HashMap<String, String>,
}

impl Replay {
    pub fn latest(&self, resource_id: &str) -> Option<(&Value, i64)> {
        let v = *self.latest_version.get(resource_id)?;
        Some((self.resources.get(resource_id)?.get(&v)?, v))
    }
}

fn version_of(payload: &Value) -> i64 {
    payload
        .get("version")
        .and_then(Value::as_i64)
        .or_else(|| payload.get("resource_version").and_then(Value::as_i64))
        .unwrap_or(1)
}

fn resource_slot(kind: &str, payload: &Value) -> Option<(String, i64, Value)> {
    let map = [
        ("eventset_imported", "eventset:acq"),
        ("single_stains_imported", "eventset:single-stains"),
        ("spillover_registered", "spillover:instrument"),
        ("comp_matrix_registered", "comp:matrix"),
        ("transform_registered", "transform:global"),
        ("gate_registered", ""),
        ("lineage_registered", "lineage:tree"),
        ("comp_revised", "comp:matrix"),
        ("transform_revised", ""),
        ("gate_geometry_revised", ""),
    ];
    for (k, fixed) in map {
        if k == kind {
            let id = if fixed.is_empty() {
                payload
                    .get("gate_id")
                    .or_else(|| payload.get("resource_id"))
                    .or_else(|| payload.get("transform_id"))?
                    .as_str()?
                    .to_string()
            } else if kind == "transform_registered" {
                match payload.get("transform_id").and_then(Value::as_str) {
                    Some(id) => id.to_string(),
                    None => format!(
                        "transform:{}",
                        payload.get("channel").and_then(Value::as_str).unwrap_or("global")
                    ),
                }
            } else if kind == "transform_revised" {
                payload.get("transform_id")?.as_str()?.to_string()
            } else if kind == "gate_geometry_revised" {
                payload.get("gate_id")?.as_str()?.to_string()
            } else {
                fixed.to_string()
            };
            let prefix = match kind {
                "comp_matrix_registered" | "comp_revised" => "comp:matrix",
                "gate_registered" | "gate_geometry_revised" => id.as_str(),
                "transform_registered" | "transform_revised" => id.as_str(),
                _ => id.as_str(),
            };
            let body = payload.get("body").cloned().unwrap_or_else(|| payload.clone());
            return Some((prefix.to_string(), version_of(payload), body));
        }
    }
    None
}

pub fn replay(events: &[crate::store::StoredEvent]) -> Replay {
    let mut state = Replay::default();
    for ev in events {
        let p = &ev.payload;
        match ev.kind.as_str() {
            "rules_registered" => state.rules.push((ev.seq, p.clone())),
            "eventset_imported"
            | "single_stains_imported"
            | "spillover_registered"
            | "comp_matrix_registered"
            | "comp_revised"
            | "transform_registered"
            | "transform_revised"
            | "gate_registered"
            | "gate_geometry_revised"
            | "lineage_registered" => {
                if let Some((id, ver, body)) = resource_slot(&ev.kind, p) {
                    state
                        .resources
                        .entry(id.clone())
                        .or_default()
                        .insert(ver, body);
                    let prev = state.latest_version.get(&id).copied().unwrap_or(0);
                    if ver > prev {
                        state.latest_version.insert(id, ver);
                    }
                }
            }
            "candidate_proposed" => {
                let id = p.get("candidate_id").and_then(Value::as_str).unwrap_or("").to_string();
                state.candidates.insert(
                    id.clone(),
                    CandidateRow {
                        candidate_id: id,
                        scope: p.get("scope").and_then(Value::as_str).unwrap_or("").to_string(),
                        status: "open".to_string(),
                        option_key: p.get("option_key").and_then(Value::as_str).unwrap_or("").to_string(),
                        payload: p.get("payload").cloned().unwrap_or_else(|| p.clone()),
                        rationale: p.get("rationale").and_then(Value::as_str).unwrap_or("").to_string(),
                        created_seq: ev.seq,
                        decided_seq: None,
                    },
                );
            }
            "candidate_decided" => {
                if let Some(id) = p.get("candidate_id").and_then(Value::as_str) {
                    if let Some(c) = state.candidates.get_mut(id) {
                        c.status = p.get("decision").and_then(Value::as_str).unwrap_or("decided").to_string();
                        c.decided_seq = Some(ev.seq);
                        if let Some(rat) = p.get("rationale").and_then(Value::as_str) {
                            c.rationale = rat.to_string();
                        }
                    }
                }
            }
            "gate_renamed" => {
                if let (Some(gid), Some(name)) = (
                    p.get("gate_id").and_then(Value::as_str),
                    p.get("new_name").and_then(Value::as_str),
                ) {
                    state.names.insert(gid.to_string(), name.to_string());
                }
            }
            "dependencies_invalidated" => state.invalidations.push((ev.seq, p.clone())),
            "populations_evaluated" => {
                state.evaluations.push((ev.seq, ev.actor.clone(), p.clone()));
            }
            _ => {}
        }
    }
    state
}

impl Store {
    /// Wipe derived tables and rebuild them from the full event log.
    pub fn rebuild(&mut self, case_id: &str) -> rusqlite::Result<()> {
        let events = self.all_events(case_id)?;
        let state = replay(&events);

        let tx = self.conn.transaction()?;
        for table in [
            "stale_marker",
            "result_dependency",
            "result_version",
            "candidate",
            "resource_version",
        ] {
            tx.execute(format!("DELETE FROM {table} WHERE case_id=?1").as_str(), params![case_id])?;
        }

        for (id, versions) in &state.resources {
            for (ver, body) in versions {
                let kind = if id.starts_with("eventset") {
                    "eventset"
                } else if id.starts_with("spillover") {
                    "spillover"
                } else if id.starts_with("comp") {
                    "comp_matrix"
                } else if id.starts_with("transform") {
                    "transform"
                } else if id == "lineage:tree" {
                    "lineage"
                } else {
                    "gate"
                };
                // Find the introducing event for seq/branch provenance.
                let (seq, branch): (i64, String) = events
                    .iter()
                    .find(|e| {
                        let p = &e.payload;
                        let pid = p.get("resource_id").and_then(Value::as_str)
                            .or_else(|| p.get("gate_id")).and_then(Value::as_str)
                            .or_else(|| p.get("transform_id")).and_then(Value::as_str);
                        let pver = version_of(p);
                        match kind {
                            "eventset" => p.get("resource_id").and_then(Value::as_str) == Some(id.as_str()) && pver == *ver,
                            "spillover" | "comp_matrix" | "lineage" => pver == *ver
                                && matches!(e.kind.as_str(),
                                    "spillover_registered" | "comp_matrix_registered" | "comp_revised" | "lineage_registered"),
                            "transform" => pid == Some(id.as_str()) && pver == *ver,
                            "gate" => pid == Some(id.as_str()) && pver == *ver,
                            _ => false,
                        }
                    })
                    .map(|e| (e.seq, e.branch_id.clone()))
                    .unwrap_or((0, String::new()));
                tx.execute(
                    "INSERT INTO resource_version(case_id, resource_id, version, kind, seq, branch_id, body, replaces_version)
                     VALUES (?1,?2,?3,?4,?5,?6,?7, NULL)",
                    params![case_id, id, ver, kind, seq, branch, serde_json::to_string(body).unwrap()],
                )?;
            }
        }

        for c in state.candidates.values() {
            tx.execute(
                "INSERT INTO candidate(case_id, candidate_id, scope, created_seq, decided_seq,
                                       status, option_key, payload, rationale)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    case_id, c.candidate_id, c.scope, c.created_seq, c.decided_seq,
                    c.status, c.option_key, serde_json::to_string(&c.payload).unwrap(), c.rationale
                ],
            )?;
        }

        // Re-evaluate the branch view: every `populations_evaluated` event stored
        // its full results (computed against pinned inputs at decision time). We
        // persist the latest stored result per gate id, preserving its pinned
        // dependency versions, then apply stale markers from invalidation events.
        let mut latest_eval: std::collections::HashMap<String, (i64, &Value)> = Default::default();
        for (seq, _, payload) in &state.evaluations {
            if let Some(results) = payload.get("results").and_then(Value::as_array) {
                for r in results {
                    if let Some(gid) = r.get("gate_id").and_then(Value::as_str) {
                        latest_eval.insert(gid.to_string(), (*seq, r));
                    }
                }
            }
        }
        for (gid, (seq, body)) in &latest_eval {
            let gv = body.get("gate_version").and_then(Value::as_i64).unwrap_or(1);
            let result_id = format!("pop:{}", gid);
            let version = body.get("result_version").and_then(Value::as_i64).unwrap_or(1);
            tx.execute(
                "INSERT INTO result_version(case_id, result_id, version, gate_id, gate_version,
                        parent_result_id, parent_result_version, transform_id, transform_version,
                        comp_candidate_id, seq, branch_id, body)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    case_id, result_id, version, gid, gv,
                    body.get("parent_result_id").and_then(Value::as_str),
                    body.get("parent_result_version").and_then(Value::as_i64),
                    body.get("transform_id").and_then(Value::as_str),
                    body.get("transform_version").and_then(Value::as_i64),
                    body.get("comp_candidate_id").and_then(Value::as_str),
                    seq,
                    body.get("branch_id").and_then(Value::as_str).unwrap_or(""),
                    serde_json::to_string(body).unwrap()
                ],
            )?;
            if let Some(deps) = body.get("dependencies").and_then(Value::as_array) {
                for d in deps {
                    tx.execute(
                        "INSERT INTO result_dependency(case_id, result_id, result_version,
                                dep_kind, dep_resource_id, dep_version)
                         VALUES (?1,?2,?3,?4,?5,?6)",
                        params![
                            case_id, result_id, version,
                            d.get("kind").and_then(Value::as_str).unwrap_or(""),
                            d.get("resource_id").and_then(Value::as_str).unwrap_or(""),
                            d.get("version").and_then(Value::as_i64).unwrap_or(1)
                        ],
                    )?;
                }
            }
        }
        for (seq, payload) in &state.invalidations {
            let affected = payload.get("affected").and_then(Value::as_array).cloned().unwrap_or_default();
            for item in affected {
                let rid = item.get("result_id").and_then(Value::as_str).unwrap_or("");
                let rv = item.get("result_version").and_then(Value::as_i64).unwrap_or(1);
                tx.execute(
                    "INSERT OR IGNORE INTO stale_marker(case_id, result_id, result_version, seq,
                            branch_id, reason_kind, reason_resource_id, reason_version, affected_scope)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        case_id, rid, rv, seq,
                        payload.get("branch_id").and_then(Value::as_str).unwrap_or(""),
                        payload.get("reason_kind").and_then(Value::as_str).unwrap_or(""),
                        payload.get("reason_resource_id").and_then(Value::as_str).unwrap_or(""),
                        payload.get("reason_version").and_then(Value::as_i64).unwrap_or(1),
                        item.get("scope").and_then(Value::as_str).unwrap_or("")
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[allow(dead_code)]
fn type_anchors(
    _e: &EventSetV,
    _s: &SpilloverMetaV,
    _c: &CompMatrixV,
    _t: &TransformV,
    _g: &GateV,
    _l: &LineageV,
    _p: &PopulationResult,
) {
}
