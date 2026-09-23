//! Timeline replay: fold the branch ancestry events into a Snapshot.

use crate::compute::{self, MatrixCandidate, MatrixAnalysis};
use crate::domain::*;
use crate::engine::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Per-gate membership record carried through replay.
#[derive(Clone)]
struct GateState {
    gate: Gate,
    included: Vec<String>,
    boundary: Vec<String>,
    numerator: usize,
    denominator: usize,
    percent: f64,
    /// gate/raw/lineage versions this membership was computed against
    gates_version: String,
    events_version: String,
    lineage_version: String,
    /// parent membership result this percentage denominator was pinned to
    denominator_parent_version: String,
    denominator_parent_event: String,
    denom_parent_gate_version: String,
    computed_event: String,
    fresh: bool,
    stale_since: Option<String>,
    // name bookkeeping
    name: String,
}

#[derive(Clone)]
struct ReplayState {
    artifacts: BTreeMap<String, ArtifactView>,
    gate_states: BTreeMap<String, GateState>,
    lineage: Option<Lineage>,
    decision: Option<String>, // chosen candidate_id for current matrix
    decided_event: Option<String>,
    judgments: Vec<JudgmentEvent>,
    transform_revisions: i64,
}

fn kind_key(k: ArtifactKind) -> &'static str {
    k.as_str()
}

impl ReplayState {
    fn artifact(&self, key: &str) -> Option<&ArtifactView> {
        self.artifacts.get(key)
    }
}

pub fn uid_point(event_id: &str, x_channel: &str, y_channel: &str) -> String {
    format!("pt:{x_channel}:{y_channel}:{event_id}")
}
pub fn uid_node(gate_id: &str) -> String {
    format!("gate:{gate_id}")
}
pub fn uid_pop(gate_id: &str) -> String {
    format!("pop:{gate_id}")
}

fn parse<T: serde::de::DeserializeOwned>(v: &ArtifactView) -> T {
    serde_json::from_value(v.content.clone()).expect("artifact content matches type")
}

/// Insert/replace one artifact version into the state.
fn put_artifact(
    state: &mut ReplayState,
    kind: ArtifactKind,
    content: Value,
    event_id: &str,
) {
    let artifact_id = content["artifact_id"].as_str().unwrap_or("art").to_string();
    let version = content["version"].as_str().unwrap_or("v?").to_string();
    state.artifacts.insert(
        kind_key(kind).to_string(),
        ArtifactView {
            kind: kind.as_str().into(),
            artifact_id,
            version,
            introducing_event_id: event_id.into(),
            fingerprint: crate::store::fingerprint(&crate::engine::canonical(content)),
            content,
        },
    );
}

/// Fold a single event into the mutable replay state.
fn apply_event(state: &mut ReplayState, ev: &EventView) {
    let eid = &ev.event_id;
    match ev.event_type.as_str() {
        "import_baseline" => { /* artifacts already inserted from DB rows */ }
        "revise_artifact" => {
            let kind_s = ev.payload["kind"].as_str().unwrap_or("");
            if let Some(kind) = parse_kind(kind_s) {
                if let Some(content) = ev.payload.get("content") {
                    put_artifact(state, kind, content.clone(), eid);
                    if kind == ArtifactKind::Transform {
                        state.transform_revisions += 1;
                    }
                    // new matrix resets any prior adjudication of it
                    if kind == ArtifactKind::Matrix {
                        state.decision = None;
                        state.decided_event = None;
                    }
                }
            }
        }
        "patch_raw_event" => {
            // surgical source change: only one event/channel moves
            if let Some(av) = state.artifacts.get_mut("raw_events") {
                let event_id = ev.payload["event_id"].as_str().unwrap_or("");
                let channel_id = ev.payload["channel_id"].as_str().unwrap_or("");
                if let Some(value) = ev.payload.get("value").and_then(|v| v.as_f64()) {
                    if let Some(arr) = av.content.get_mut("events").and_then(|v| v.as_array_mut()) {
                        for evv in arr.iter_mut() {
                            if evv["event_id"] == event_id {
                                evv["values"][channel_id] = json!(value);
                            }
                        }
                    }
                    av.version = ev.payload["new_version"].as_str().unwrap_or(&av.version).to_string();
                    av.introducing_event_id = eid.clone();
                    av.fingerprint = crate::store::fingerprint(&crate::engine::canonical(&av.content));
                }
            }
        }
        "rename_gate" => {
            if let Some(av) = state.artifacts.get_mut("gates") {
                let gate_id = ev.payload["gate_id"].as_str().unwrap_or("");
                let new_name = ev.payload["new_name"].as_str().unwrap_or("");
                if let Some(arr) = av.content.get_mut("gates").and_then(|v| v.as_array_mut()) {
                    for g in arr.iter_mut() {
                        if g["gate_id"] == gate_id {
                            g["name"] = json!(new_name);
                        }
                    }
                }
                av.introducing_event_id = eid.clone();
                av.fingerprint = crate::store::fingerprint(&crate::engine::canonical(&av.content));
                if let Some(gs) = state.gate_states.get_mut(gate_id) {
                    gs.name = new_name.to_string();
                }
            }
        }
        "adjudicate_matrix" => {
            state.decision = ev.payload["candidate_id"].as_str().map(|s| s.to_string());
            state.decided_event = Some(eid.clone());
        }
        "revert" => {
            // handled by truncating the timeline before replay; nothing here
        }
        _ => {}
    }
}

fn parse_kind(s: &str) -> Option<ArtifactKind> {
    ArtifactKind::all().into_iter().find(|k| k.as_str() == s)
}

fn gate_order(gates: &Gates, lineage: Option<&Lineage>) -> Vec<String> {
    // parents before children (topological)
    let mut ids: Vec<String> = gates.gates.iter().map(|g| g.gate_id.clone()).collect();
    let mut ordered = Vec::new();
    while ordered.len() < ids.len() {
        let before = ordered.len();
        for gid in ids.clone() {
            if ordered.contains(&gid) {
                continue;
            }
            let parent = lineage.and_then(|l| compute::parent_of(l, &gid));
            let ready = match &parent {
                None => true,
                Some(p) => !ids.contains(p) || ordered.contains(p),
            };
            if ready {
                ordered.push(gid);
            }
        }
        if ordered.len() == before {
            // cycle guard: flush remainder
            for g in &ids {
                if !ordered.contains(g) {
                    ordered.push(g.clone());
                }
            }
            break;
        }
    }
    let _ = &mut ids;
    ordered
}

struct CompContext<'a> {
    analysis: MatrixAnalysis,
    decided: Option<String>,
    inverse: Option<Vec<Vec<f64>>>,
    dropped: Option<String>,
    raw: &'a RawEvents,
    matrix: &'a Matrix,
}

fn comp_context<'a>(raw: &'a RawEvents, matrix: &'a Matrix, decision: Option<&str>)
    -> CompContext<'a>
{
    let analysis = compute::analyze_matrix(matrix);
    let mut inverse = None;
    let mut dropped = None;
    if analysis.near_singular {
        if let Some(cid) = decision {
            if let Some(cand) = analysis.candidates.iter().find(|c| c.candidate_id == cid) {
                inverse = cand.inverse.clone();
                dropped = cand.dropped_channel.clone();
            }
        }
    } else if let Some(cand) = analysis.candidates.iter().find(|c| c.kind == "direct") {
        inverse = cand.inverse.clone();
    }
    CompContext { analysis, decided: decision.map(|s| s.to_string()), inverse, dropped, raw, matrix }
}

fn channel_value(cc: &CompContext<'_>, event_id: &str, channel: &str) -> Option<f64> {
    let e = cc.raw.events.iter().find(|e| e.event_id == event_id)?;
    let raw = *e.values.get(channel)?;
    if cc.matrix.channels.contains(&channel.to_string()) {
        if !cc.analysis.near_singular || cc.inverse.is_some() {
            if let Some(inv) = &cc.inverse {
                if cc.dropped.as_deref() == Some(channel) {
                    return Some(raw);
                }
                let vals: Vec<f64> = cc
                    .matrix
                    .channels
                    .iter()
                    .map(|c| e.values.get(c).copied().unwrap_or(0.0))
                    .collect();
                let idx = cc.matrix.channels.iter().position(|c| c == channel)?;
                return Some((0..cc.matrix.channels.len())
                    .map(|j| inv[idx][j] * vals[j])
                    .sum());
            }
        }
    }
    Some(raw)
}

fn boundary_events(gate: &Gate, included: &[String], raw: &RawEvents) -> Vec<String> {
    let mut out = Vec::new();
    for eid in included {
        if let Some(e) = raw.events.iter().find(|e| &e.event_id == eid) {
            let x = e.values[&gate.x_channel];
            let y = e.values[&gate.y_channel];
            for v in &gate.polygon {
                if (v[0] - x).abs() < 1e-9 && (v[1] - y).abs() < 1e-9 {
                    out.push(eid.clone());
                    break;
                }
            }
        }
    }
    out
}

/// Full top-down membership recomputation. `upstream_revision_gate` makes
/// descendants stale-only (kept, not overwritten) and skips their recompute.
fn recompute_memberships(
    state: &mut ReplayState,
    event_id: &str,
    upstream_revision_gate: Option<&str>,
) {
    let raw: RawEvents = parse(state.artifacts.get("raw_events").expect("raw present"));
    let gates: Gates = parse(state.artifacts.get("gates").expect("gates present"));
    let lineage: Option<Lineage> = state
        .artifacts
        .get("lineage")
        .map(parse::<Lineage>);
    let matrix: Matrix = parse(state.artifacts.get("matrix").expect("matrix present"));
    let cc = comp_context(&raw, &matrix, state.decision.as_deref());

    let events_version = state.artifacts["raw_events"].version.clone();
    let gates_version = state.artifacts["gates"].version.clone();
    let lineage_version = state
        .artifacts
        .get("lineage")
        .map(|a| a.version.clone())
        .unwrap_or_else(|| "none".into());
    let blocked = cc.analysis.near_singular && cc.inverse.is_none();

    let order = gate_order(&gates, lineage.as_ref());
    let stale_set: Vec<String> = upstream_revision_gate
        .map(|g| compute::descendants(lineage.as_ref().unwrap_or(&Lineage {
            artifact_id: "l".into(), version: "x".into(), edges: vec![]
        }), g))
        .unwrap_or_default();

    // membership of the root population (all acquired events)
    let root_denom = raw.events.len();
    let mut new_states: BTreeMap<String, GateState> = BTreeMap::new();
    for gid in &order {
        let gate = gates.gates.iter().find(|g| &g.gate_id == gid).unwrap().clone();
        let parent = lineage.as_ref().and_then(|l| compute::parent_of(l, gid));
        let denom_pool: Vec<String> = match &parent {
            None => raw.events.iter().map(|e| e.event_id.clone()).collect(),
            Some(p) => {
                let ps = state
                    .gate_states
                    .get(p)
                    .or_else(|| new_states.get(p))
                    .expect("parent gate present");
                ps.included.clone()
            }
        };

        if upstream_revision_gate.is_some() && stale_set.contains(gid) {
            // keep old conclusion, pin its denominator, mark stale: no overwrite
            if let Some(old) = state.gate_states.get(gid) {
                let mut kept = old.clone();
                kept.fresh = false;
                kept.stale_since.get_or_insert_with(|| event_id.to_string());
                new_states.insert(gid.clone(), kept);
                continue;
            }
        }

        // fluorescent gates blocked while near-singular matrix is undecided
        let uses_fluor = [&gate.x_channel, &gate.y_channel]
            .iter()
            .any(|c| matrix.channels.contains(&c.to_string()));
        let mut included = Vec::new();
        if !(blocked && uses_fluor) {
            for eid in &denom_pool {
                if let (Some(x), Some(y)) = (
                    channel_value(&cc, eid, &gate.x_channel),
                    channel_value(&cc, eid, &gate.y_channel),
                ) {
                    // gating coordinates are declared raw linear basis
                    let e = raw.events.iter().find(|e| &e.event_id == eid).unwrap();
                    let rx = e.values[&gate.x_channel];
                    let ry = e.values[&gate.y_channel];
                    if compute::point_in_polygon(rx, ry, &gate.polygon) {
                        let _ = (x, y);
                        included.push(eid.clone());
                    }
                }
            }
        }

        let denominator = denom_pool.len();
        let numerator = included.len();
        let percent = if denominator == 0 {
            0.0
        } else {
            100.0 * numerator as f64 / denominator as f64
        };
        let boundary = boundary_events(&gate, &included, &raw);
        let (denom_parent_version, denom_parent_event, denom_gate_version) = match &parent {
            None => (
                "root".to_string(),
                state
                    .artifacts
                    .get("raw_events")
                    .map(|a| a.introducing_event_id.clone())
                    .unwrap_or_default(),
                events_version.clone(),
            ),
            Some(p) => {
                let ps = state
                    .gate_states
                    .get(p)
                    .or_else(|| new_states.get(p))
                    .expect("parent present");
                (
                    ps.gates_version.clone(),
                    ps.computed_event.clone(),
                    ps.gates_version.clone(),
                )
            }
        };
        let prev = state.gate_states.get(gid);
        let fresh = if blocked && uses_fluor {
            false
        } else {
            true
        };
        let gs = GateState {
            gate: gate.clone(),
            included: included.clone(),
            boundary,
            numerator,
            denominator,
            percent,
            gates_version: gates_version.clone(),
            events_version: events_version.clone(),
            lineage_version: lineage_version.clone(),
            denominator_parent_version: denom_parent_version,
            denominator_parent_event: denom_parent_event,
            denom_parent_gate_version: denom_gate_version,
            computed_event: event_id.to_string(),
            fresh,
            stale_since: if fresh { None } else { Some(event_id.to_string()) },
            name: gate.name,
        };
        let _ = prev;
        let _ = root_denom;
        new_states.insert(gid.clone(), gs);
    }
    state.gate_states = new_states;
}

/// Replay the branch ancestry timeline up to `up_to_event` (None = head).
pub fn replay(
    store: &Store,
    branch_id: &str,
    up_to_event: Option<&str>,
) -> rusqlite::Result<Snapshot> {
    let ancestors = store.branch_ancestors(branch_id)?;
    // collect events parent-first
    let conn = store.conn.lock().unwrap();
    let mut timeline: Vec<EventView> = Vec::new();
    for (bid, _) in ancestors.iter().rev() {
        let mut stmt = conn.prepare(
            "SELECT event_id,seq,parent_event_id,event_type,payload FROM events
             WHERE branch_id=?1 ORDER BY seq",
        )?;
        let mut rows = stmt.query(rusqlite::params![bid])?;
        while let Some(r) = rows.next()? {
            let payload: String = r.get(4)?;
            timeline.push(EventView {
                event_id: r.get(0)?,
                seq: r.get(1)?,
                parent_event_id: r.get(2)?,
                event_type: r.get(3)?,
                label: None,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            });
        }
    }
    drop(conn);

    // initial artifacts as present at baseline import: query the artifact rows
    // introduced on ancestry in event order, then let revision events override.
    let mut state = ReplayState {
        artifacts: load_initial_artifacts(store, branch_id)?,
        gate_states: BTreeMap::new(),
        lineage: None,
        decision: None,
        decided_event: None,
        judgments: Vec::new(),
        transform_revisions: 0,
    };

    let mut head = timeline
        .first()
        .map(|e| e.event_id.clone())
        .unwrap_or_default();
    let mut computed_at = head.clone();
    for ev in &timeline {
        if let Some(stop) = up_to_event {
            if ev.event_id == stop {
                head = ev.event_id.clone();
                apply_and_recompute(store, &mut state, ev);
                computed_at = ev.event_id.clone();
                break;
            }
        }
        head = ev.event_id.clone();
        apply_and_recompute(store, &mut state, ev);
        computed_at = ev.event_id.clone();
    }

    build_snapshot(store, branch_id, state, &head, up_to_event, &computed_at)
}

fn apply_and_recompute(store: &Store, state: &mut ReplayState, ev: &EventView) {
    let upstream = match ev.event_type.as_str() {
        "revise_artifact" if ev.payload["kind"] == "gates" => {
            ev.payload["upstream_gate_id"].as_str()
        }
        _ => None,
    };
    apply_event(state, ev);
    match ev.event_type.as_str() {
        "import_baseline" => recompute_memberships(state, &ev.event_id, None),
        "revise_artifact" => {
            match ev.payload["kind"].as_str() {
                Some("gates") => recompute_memberships(state, &ev.event_id, upstream),
                Some("lineage") | Some("raw_events") | Some("matrix") => {
                    recompute_memberships(state, &ev.event_id, None)
                }
                Some("transform") => {} // gates are raw-coordinate; only transform layer changes
                _ => {}
            }
            maybe_judge_matrix(state, ev);
        }
        "patch_raw_event" => recompute_memberships(state, &ev.event_id, None),
        "rename_gate" => {} // identity/polygon unchanged -> no staleness
        "adjudicate_matrix" => {
            record_decision_judgment(state, ev);
            recompute_memberships(state, &ev.event_id, None);
        }
        "recompute_gates" => recompute_memberships(state, &ev.event_id, None),
        _ => {}
    }
    let _ = store;
}

fn maybe_judge_matrix(state: &mut ReplayState, ev: &EventView) {
    if ev.payload["kind"] != "matrix" {
        return;
    }
    if let Some(content) = ev.payload.get("content") {
        if let Ok(matrix) = serde_json::from_value::<Matrix>(content.clone()) {
            let a = compute::analyze_matrix(&matrix);
            if a.near_singular {
                state.judgments.push(JudgmentEvent {
                    code: "near_singular_no_silent_inverse".into(),
                    event_id: ev.event_id.clone(),
                    subject: format!("{}/{}", matrix.artifact_id, matrix.version),
                    evidence: json!({
                        "determinant": a.determinant,
                        "threshold": a.threshold,
                        "condition_estimate": a.candidates.first()
                            .map(|c| c.condition_estimate).unwrap_or(0.0),
                        "candidate_ids": a.candidates.iter()
                            .map(|c| c.candidate_id.clone()).collect::<Vec<_>>(),
                        "rule": "near-singular compensation matrix must not be silently inverted"
                    }),
                });
            }
        }
    }
}

fn record_decision_judgment(state: &mut ReplayState, ev: &EventView) {
    state.judgments.push(JudgmentEvent {
        code: "matrix_candidate_adjudicated".into(),
        event_id: ev.event_id.clone(),
        subject: ev.payload["matrix_version"].as_str().unwrap_or("matrix").to_string(),
        evidence: json!({
            "candidate_id": ev.payload["candidate_id"],
            "action": ev.payload["action"],
            "rule": "all legal candidates retained until an explicit decision"
        }),
    });
}

fn load_initial_artifacts(store: &Store, branch_id: &str)
    -> rusqlite::Result<BTreeMap<String, ArtifactView>>
{
    let ancestors = store.branch_ancestors(branch_id)?;
    let conn = store.conn.lock().unwrap();
    let mut out = BTreeMap::new();
    for (bid, _) in ancestors.iter().rev() {
        let mut stmt = conn.prepare(
            "SELECT kind,artifact_id,version,event_id,content,fingerprint FROM artifacts
             WHERE branch_id=?1 ORDER BY row_id",
        )?;
        let mut rows = stmt.query(rusqlite::params![bid])?;
        while let Some(r) = rows.next()? {
            let content_s: String = r.get(4)?;
            let content: Value = serde_json::from_str(&content_s).unwrap_or(Value::Null);
            let fp: Option<String> = r.get(5).ok();
            out.insert(
                r.get::<_, String>(0)?,
                ArtifactView {
                    kind: r.get(0)?,
                    artifact_id: r.get(1)?,
                    version: r.get(2)?,
                    introducing_event_id: r.get(3)?,
                    fingerprint: fp.unwrap_or_else(|| crate::store::fingerprint(&crate::engine::canonical(&content))),
                    content,
                },
            );
        }
    }
    Ok(out)
}

fn build_snapshot(
    store: &Store,
    branch_id: &str,
    state: ReplayState,
    head: &str,
    up_to: Option<&str>,
    computed_at: &str,
) -> rusqlite::Result<Snapshot> {
    let raw: RawEvents = serde_json::from_value(state.artifacts["raw_events"].content.clone()).unwrap();
    let matrix: Matrix = serde_json::from_value(state.artifacts["matrix"].content.clone()).unwrap();
    let gates: Gates = serde_json::from_value(state.artifacts["gates"].content.clone()).unwrap();
    let lineage: Lineage = state
        .artifacts
        .get("lineage")
        .map(|a| serde_json::from_value(a.content.clone()).unwrap())
        .unwrap_or(Lineage {
            artifact_id: "l".into(),
            version: "none".into(),
            edges: vec![],
        });
    let cc = comp_context(&raw, &matrix, state.decision.as_deref());
    let diags = compute::value_diagnostics(&raw, &matrix);

    // scatter: FSC vs SSC (with compensated FITC shown as overlay value)
    let mut scatter = Vec::new();
    for e in &raw.events {
        let x = e.values[&gates.gates[0].x_channel];
        let y = e.values[&gates.gates[0].y_channel];
        if !x.is_finite() || !y.is_finite() {
            continue; // never dropped from storage, only excluded from plot
        }
        let x_comp = channel_value(&cc, &e.event_id, "ch_fitc").filter(|v| v.is_finite());
        let mut flags: Vec<String> = diags
            .iter()
            .filter(|d| d.event_id == e.event_id)
            .map(|d| d.code.clone())
            .collect();
        flags.sort();
        flags.dedup();
        let in_gates = state
            .gate_states
            .iter()
            .filter(|(_, gs)| gs.included.contains(&e.event_id))
            .map(|(id, _)| id.clone())
            .collect();
        scatter.push(ScatterPoint {
            uid: uid_point(&e.event_id, "ch_fsc", "ch_ssc"),
            event_id: e.event_id.clone(),
            x,
            y,
            x_compensated: x_comp,
            flags,
            in_gates,
        });
    }

    // tree in gate topological order
    let order = gate_order(&gates, Some(&lineage));
    let mut tree = Vec::new();
    for gid in order {
        let gs = state
            .gate_states
            .get(&gid)
            .cloned()
            .unwrap_or_else(|| GateState {
                gate: gates.gates.iter().find(|g| g.gate_id == gid).cloned().unwrap(),
                included: vec![], boundary: vec![], numerator: 0, denominator: 0, percent: 0.0,
                gates_version: "?".into(), events_version: "?".into(), lineage_version: "?".into(),
                denominator_parent_version: "?".into(), denominator_parent_event: "?".into(),
                denom_parent_gate_version: "?".into(), computed_event: head.into(),
                fresh: false, stale_since: None,
                name: gid.clone(),
            });
        let parent = compute::parent_of(&lineage, &gid);
        let status = if gs.fresh {
            "fresh".to_string()
        } else if gs.stale_since.is_some() {
            "stale".to_string()
        } else {
            "blocked".to_string()
        };
        let denominator_ref = DenominatorRef {
            parent_gate_id: parent.clone(),
            parent_gate_version: gs.denom_parent_gate_version.clone(),
            parent_result_event_id: gs.denominator_parent_event.clone(),
            scope_event_id: computed_at.to_string(),
            denominator: gs.denominator,
        };
        tree.push(TreeNode {
            uid: uid_node(&gid),
            gate_id: gid,
            name: gs.name,
            parent_gate_id: parent,
            percent_of_parent: Some(gs.percent),
            numerator: gs.numerator,
            denominator: gs.denominator,
            denominator_ref: Some(denominator_ref),
            status: status.clone(),
            stale: gs.stale_since.is_some(),
        });

        // machine-readable denominator pinning judgment whenever stale
        if gs.stale_since.is_some() {
            // appended below from state.judgments copy if needed
        }
    }

    // denom-pinned and upstream-stale judgments persisted for replay scope
    let mut judgments = state.judgments.clone();
    for (gid, gs) in &state.gate_states {
        if let Some(since) = &gs.stale_since {
            judgments.push(JudgmentEvent {
                code: "denominator_pinned_to_parent_version".into(),
                event_id: since.clone(),
                subject: format!("pop:{gid}"),
                evidence: json!({
                    "parent_gate_id": compute::parent_of(&lineage, gid),
                    "parent_gate_version": gs.denom_parent_gate_version,
                    "parent_result_event_id": gs.denominator_parent_event,
                    "denominator": gs.denominator,
                    "rule": "percentage denominator must be pinned to the parent population version"
                }),
            });
            judgments.push(JudgmentEvent {
                code: "downstream_marked_stale_not_overwritten".into(),
                event_id: since.clone(),
                subject: format!("pop:{gid}"),
                evidence: json!({
                    "stale_since_event_id": since,
                    "kept_numerator": gs.numerator,
                    "kept_denominator": gs.denominator,
                    "rule": "revising an upstream gate marks downstream results stale without overwrite"
                }),
            });
        }
    }

    let comp = CompensationView {
        determinant: cc.analysis.determinant,
        near_singular: cc.analysis.near_singular,
        threshold: cc.analysis.threshold,
        decided: state.decided_event.clone().map(|_| {
            state.decision.clone().unwrap_or_else(|| "cand_reject".into())
        }),
        candidates: cc.analysis.candidates.clone(),
        rows_before: matrix.rows.clone(),
        rows_after: cc.inverse.clone(),
    };

    let (label,): (String,) = {
        let c = store.conn.lock().unwrap();
        c.query_row(
            "SELECT label FROM branches WHERE branch_id=?1",
            rusqlite::params![branch_id],
            |r| Ok((r.get(0)?,)),
        )?
    };

    // visible event list restricted to the chosen scope
    let events = {
        let ancestors = store.branch_ancestors(branch_id)?;
        let c = store.conn.lock().unwrap();
        let mut evs = Vec::new();
        for (bid, _) in ancestors.iter().rev() {
            let mut stmt = c.prepare(
                "SELECT event_id,seq,parent_event_id,event_type,payload FROM events
                 WHERE branch_id=?1 ORDER BY seq",
            )?;
            let mut rows = stmt.query(rusqlite::params![bid])?;
            while let Some(r) = rows.next()? {
                let eid: String = r.get(0)?;
                if let Some(stop) = up_to {
                    // parent-first ordering: stop collecting after the scope event.
                }
                let payload_s: String = r.get(4)?;
                let label = serde_json::from_str::<Value>(&payload_s).ok()
                    .and_then(|p| p.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()));
                evs.push(EventView {
                    event_id: eid,
                    seq: r.get(1)?,
                    parent_event_id: r.get(2)?,
                    event_type: r.get(3)?,
                    label,
                    payload: serde_json::from_str(&payload_s).unwrap_or(Value::Null),
                });
            }
        }
        let mut truncated = Vec::new();
        for e in evs {
            truncated.push(e.clone());
            if let Some(stop) = up_to {
                if e.event_id == stop {
                    break;
                }
            }
        }
        truncated
    };

    Ok(Snapshot {
        branch_id: branch_id.into(),
        branch_label: label,
        head_event_id: head.to_string(),
        scope_event_id: up_to.map(|s| s.to_string()),
        artifacts: state.artifacts,
        events,
        scatter,
        tree,
        compensation: comp,
        diagnostics: diags,
        judgments,
        transform_revision_count: state.transform_revisions,
    })
}
