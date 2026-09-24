//! Replay engine: reconstructs the full review state of a branch at any event.
//!
//! State is a pure fold over events. Imported sources are never mutated; new
//! interpretations append new events. Revising an upstream gate marks dependent
//! downstream populations stale but never overwrites the old population rows.

use crate::domain::*;
use crate::store::Store;
use std::collections::{BTreeMap, HashMap};

pub const NEAR_SINGULAR_KAPPA: f64 = 1e6;
pub const EXTREME_MAGNITUDE: f64 = 1.0e6;
pub const BOUNDARY_TOL: f64 = 1e-7;

#[derive(serde::Serialize, Clone)]
pub struct DecisionState {
    pub decision_id: String,
    pub subject_kind: String, // "comp_run" | "population"
    pub subject_id: String,
    pub verdict: String,     // "accepted" | "rejected" | "adjusted"
    pub chosen_candidate_id: Option<String>,
    pub note: String,
    pub event_id: String,
    pub reverted_by: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct BranchState {
    pub branch: String,
    pub at_event: Option<String>,
    pub events: Vec<LedgerEvent>,
    /// key -> latest version artifact (per kind/key).
    pub artifacts: BTreeMap<String, ArtifactRec>,
    pub raw: BTreeMap<String, RawFile>,
    pub spill: BTreeMap<String, SpilloverMeta>,
    pub matrices: BTreeMap<String, CompMatrix>,
    pub matrix_verdicts: BTreeMap<String, MatrixVerdict>,
    pub transforms: BTreeMap<String, TransformSpec>,
    pub gates: BTreeMap<String, PolygonGate>,
    pub lineage: Option<Lineage>,
    pub runs: BTreeMap<String, CompRun>,
    pub pops: Vec<PopResult>,
    pub decisions: BTreeMap<String, DecisionState>,
    pub judgments: Vec<LedgerEvent>,
    pub children: BTreeMap<String, Vec<String>>,
}

impl BranchState {
    fn empty(branch: &str) -> Self {
        BranchState {
            branch: branch.to_string(),
            at_event: None,
            events: Vec::new(),
            artifacts: BTreeMap::new(),
            raw: BTreeMap::new(),
            spill: BTreeMap::new(),
            matrices: BTreeMap::new(),
            matrix_verdicts: BTreeMap::new(),
            transforms: BTreeMap::new(),
            gates: BTreeMap::new(),
            lineage: None,
            runs: BTreeMap::new(),
            pops: Vec::new(),
            decisions: BTreeMap::new(),
            judgments: Vec::new(),
            children: BTreeMap::new(),
        }
    }

    pub fn gate_order(&self) -> Vec<String> {
        let mut order = Vec::new();
        fn walk(node: &str, children: &BTreeMap<String, Vec<String>>, out: &mut Vec<String>) {
            for c in children.get(node).cloned().unwrap_or_default() {
                out.push(c.clone());
                walk(&c, children, out);
            }
        }
        walk("root", &self.children, &mut order);
        order
    }

    pub fn current_artifacts(&self) -> Vec<&ArtifactRec> {
        self.artifacts.values().collect()
    }
}

fn art_key(kind: &str, key: &str) -> String {
    format!("{}::{}", kind, key)
}

/// Resolve a branch event chain: the branch's own events (in seq order)
/// preceded by the events of its base branch up to the fork point.
pub fn event_chain(store: &Store, branch: &str, up_to_event: Option<&str>) -> Vec<LedgerEvent> {
    let branches = store.list_branches();
    let bmap: HashMap<String, crate::store::BranchRow> =
        branches.into_iter().map(|b| (b.id.clone(), b)).collect();
    let mut chain_ids: Vec<String> = Vec::new();
    let mut cur = branch.to_string();
    loop {
        let row = match bmap.get(&cur) {
            Some(r) => r.clone(),
            None => break,
        };
        let mut events: Vec<LedgerEvent> = Vec::new();
        // events directly recorded on this branch, in seq order.
        {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT id, seq, branch_id, kind, payload, parent_event_id, author, created_at
                     FROM events WHERE branch_id = ?1 ORDER BY seq",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![cur], crate::store::map_event_pub)
                .unwrap();
            for r in rows.flatten() {
                events.push(r);
            }
        }
        // Truncate at the fork point if this branch has a base.
        if let Some(base_evt) = &row.base_event_id {
            let pos = events.iter().position(|e| &e.id == base_evt);
            if let Some(p) = pos {
                events.truncate(p + 1);
            }
        }
        let own: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
        chain_ids.splice(0..0, own);
        match row.base_branch {
            Some(base) => cur = base,
            None => break,
        }
    }
    let mut out = Vec::new();
    let mut last_ok = true;
    for id in &chain_ids {
        if let Some(ev) = store.get_event(id) {
            out.push(ev);
        }
        if let Some(upto) = up_to_event {
            if id == upto {
                last_ok = false;
                break;
            }
        }
    }
    let _ = last_ok;
    out
}

pub fn replay_at(store: &Store, branch: &str, at_event: Option<&str>) -> BranchState {
    let chain = event_chain(store, branch, at_event);
    let mut state = BranchState::empty(branch);
    for ev in chain {
        apply_event(store, &mut state, ev);
    }
    state
}

pub fn replay(store: &Store, branch: &str) -> BranchState {
    replay_at(store, branch, None)
}

fn take_artifact<T: serde::de::DeserializeOwned>(state: &mut BranchState, kind: &str, key: &str, payload: serde_json::Value, event: &LedgerEvent) -> T {
    let rec = ArtifactRec {
        id: crate::ids::artifact_id(kind, &canonical(&payload)),
        kind: kind.to_string(),
        key: key.to_string(),
        event_id: event.id.clone(),
        seq: event.seq,
        payload: payload.clone(),
    };
    state.artifacts.insert(art_key(kind, key), rec);
    serde_json::from_value(payload).unwrap()
}

pub fn canonical(v: &serde_json::Value) -> String {
    // serde_json preserves insertion order, which the builder keeps stable.
    serde_json::to_string(v).unwrap()
}

fn apply_event(store: &Store, state: &mut BranchState, event: LedgerEvent) {
    state.at_event = Some(event.id.clone());
    match event.kind.as_str() {
        "raw.imported" => {
            let raw: RawFile = serde_json::from_value(event.payload.clone()).unwrap();
            let key = raw.sha256.clone();
            let _ = take_artifact::<RawFile>(state, "raw", &key, event.payload.clone(), &event);
            state.raw.insert(key, raw);
        }
        "spillover.recorded" => {
            let sp: SpilloverMeta = serde_json::from_value(event.payload.clone()).unwrap();
            let key = sp.name.clone();
            let _ = take_artifact::<SpilloverMeta>(state, "spillover", &key, event.payload.clone(), &event);
            state.spill.insert(key, sp);
        }
        "comp_matrix.versioned" => {
            let m: CompMatrix = serde_json::from_value(event.payload.clone()).unwrap();
            let v = crate::mathx::verdict(&m.rows, NEAR_SINGULAR_KAPPA);
            let key = m.note.clone();
            let payload2 = serde_json::to_value(&m).unwrap();
            let _ = take_artifact::<CompMatrix>(state, "comp_matrix", &key, payload2, &event);
            state.matrices.insert(key, m);
            state.matrix_verdicts.insert(key, v);
        }
        "transform.versioned" => {
            let t: TransformSpec = serde_json::from_value(event.payload.clone()).unwrap();
            let key = t.channel.clone();
            let _ = take_artifact::<TransformSpec>(state, "transform", &key, event.payload.clone(), &event);
            state.transforms.insert(key, t);
        }
        "gate.versioned" => {
            let g: PolygonGate = serde_json::from_value(event.payload.clone()).unwrap();
            let key = g.gate_uid.clone();
            let _ = take_artifact::<PolygonGate>(state, "gate", &key, event.payload.clone(), &event);
            state.gates.insert(key, g);
        }
        "lineage.versioned" => {
            let l: Lineage = serde_json::from_value(event.payload.clone()).unwrap();
            let _ = take_artifact::<Lineage>(state, "lineage", "tree", event.payload.clone(), &event);
            state.children.clear();
            for e in &l.edges {
                state.children.entry(e.parent.clone()).or_default().push(e.gate_uid.clone());
            }
            state.lineage = Some(l);
        }
        "comp.computed" => {
            let run: CompRun = serde_json::from_value(event.payload.clone()).unwrap();
            let _ = take_artifact::<CompRun>(state, "comp_run", &run.run_id, event.payload.clone(), &event);
            state.runs.insert(run.run_id.clone(), run);
        }
        "pop.materialized" => {
            let pop: PopResult = serde_json::from_value(event.payload.clone()).unwrap();
            let _ = take_artifact::<PopResult>(state, "pop", &pop.pop_id, event.payload.clone(), &event);
            state.pops.push(pop);
        }
        "pop.stale" => {
            let p = &event.payload;
            let pop_id = p["pop_id"].as_str().unwrap();
            let reason = p["reason"].as_str().unwrap_or("upstream revised").to_string();
            if let Some(pop) = state.pops.iter_mut().find(|x| x.pop_id == pop_id) {
                pop.stale = true;
                pop.stale_reason = Some(reason);
            }
        }
        "judgment.machine" => {
            state.judgments.push(event.clone());
        }
        "review.decision" => {
            let d = DecisionState {
                decision_id: p_str(event.payload.get("decision_id")),
                subject_kind: p_str(event.payload.get("subject_kind")),
                subject_id: p_str(event.payload.get("subject_id")),
                verdict: p_str(event.payload.get("verdict")),
                chosen_candidate_id: event
                    .payload
                    .get("chosen_candidate_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                note: event.payload.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                event_id: event.id.clone(),
                reverted_by: None,
            };
            state.decisions.insert(d.decision_id.clone(), d);
        }
        "review.decision_reverted" => {
            let did = event.payload.get("decision_id").and_then(|v| v.as_str()).unwrap();
            if let Some(d) = state.decisions.get_mut(did) {
                d.reverted_by = Some(event.id.clone());
            }
        }
        _ => {}
    }
    state.events.push(event);
}

fn p_str(v: Option<&serde_json::Value>) -> String {
    v.and_then(|x| x.as_str()).unwrap_or("").to_string()
}
