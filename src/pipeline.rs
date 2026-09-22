//! Domain pipeline: compensate -> transform -> gate -> population results.
//!
//! Hard rules enforced here and recorded as machine-readable events:
//!  * near-singular compensation matrices are rejected, never silently inverted;
//!  * percentage denominators are pinned to a concrete parent result version;
//!  * revising an upstream gate marks downstream results stale, never overwrites;
//!  * ambiguous boundary points keep ALL legal candidates pending adjudication;
//!  * unconvertible extreme values stay in place with a diagnostic.

use crate::comp;
use crate::gate::{self, PointClass};
use crate::store::{self, ObjectRow};
use crate::transform;
use rusqlite::Connection;
use serde_json::{json, Value};

pub const BOUNDARY_EPS: f64 = 1e-6;

#[derive(Debug)]
pub enum PipeError {
    Db(String),
    /// Structured rejection, e.g. near-singular matrix (event already emitted).
    Rejected(Value),
}

impl From<String> for PipeError {
    fn from(e: String) -> Self {
        PipeError::Db(e)
    }
}

fn latest(conn: &Connection, branch: &str, id: &str) -> Result<ObjectRow, PipeError> {
    store::latest_version(conn, branch, id)
        .ok_or_else(|| PipeError::Db(format!("object {id} not found on branch {branch}")))
}

/// Attempt compensation of a sample with a matrix version. Near-singular
/// matrices produce `compensation.rejected_near_singular` + diagnostic and the
/// derived layer is left untouched.
pub fn apply_compensation(
    conn: &Connection,
    branch: &str,
    sample_id: &str,
    matrix_id: &str,
    matrix_version: Option<i64>,
) -> Result<ObjectRow, PipeError> {
    let raw = latest(conn, branch, sample_id)?;
    let m = match matrix_version {
        Some(v) => store::get_object(conn, branch, matrix_id, v)
            .ok_or_else(|| PipeError::Db(format!("{matrix_id} v{v} not found")))?,
        None => latest(conn, branch, matrix_id)?,
    };
    let channels: Vec<String> = m.payload["channels"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let matrix: Vec<Vec<f64>> = serde_json::from_value(m.payload["matrix"].clone())
        .map_err(|e| PipeError::Db(e.to_string()))?;

    let inverse = match comp::try_invert(&matrix) {
        Ok(inv) => inv,
        Err(report) => {
            let previous: Vec<Value> = store::list_versions(conn, matrix_id, branch)
                .iter()
                .filter(|o| o.version != m.version)
                .map(|o| json!({"candidate":"use_matrix_version","matrix_id":matrix_id,"version":o.version}))
                .collect();
            let mut candidates = previous;
            candidates.push(json!({"candidate":"re_acquire_spillover","detail":"re-import single-stain controls and rebuild matrix"}));
            candidates.push(json!({"candidate":"manual_regularized_matrix","detail":"adjudicator supplies an explicit new matrix version"}));
            let ev = store::emit(
                conn,
                branch,
                "compensation.rejected_near_singular",
                json!({
                    "sample": sample_id, "raw_version": raw.version,
                    "matrix_id": matrix_id, "matrix_version": m.version,
                    "report": report,
                    "legal_candidates": candidates,
                    "rule": "near-singular compensation matrices must not be silently inverted"
                }),
            )?;
            let _ = store::add_diagnostic(
                conn, branch, matrix_id, m.version, Some(ev.seq), "error",
                &format!("compensation blocked: {}", report.reason),
                json!({"event": ev.id, "determinant": report.determinant}),
            );
            return Err(PipeError::Rejected(json!({"event": ev.id, "seq": ev.seq, "report": report})));
        }
    };

    let params = raw.payload["params"].as_array().cloned().unwrap_or_default();
    let pnames: Vec<String> = params.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect();
    let idxs: Vec<usize> = channels
        .iter()
        .map(|c| pnames.iter().position(|n| n == c).unwrap_or(0))
        .collect();
    let events_raw: Vec<Vec<f64>> =
        serde_json::from_value(raw.payload["events"].clone()).unwrap_or_default();
    let comp_events: Vec<Vec<f64>> = events_raw
        .iter()
        .map(|ev| comp::compensate_event(ev, &inverse, &idxs))
        .collect();
    let mut diffs = Vec::new();
    for (i, ev) in events_raw.iter().enumerate() {
        for &ci in &idxs {
            let d = comp_events[i][ci] - ev[ci];
            if d.abs() > 1e-9 {
                diffs.push(json!({"event_index": i, "channel": pnames[ci],
                    "raw": ev[i][ci], "compensated": comp_events[i][ci], "delta": d,
                    "unit": raw.payload["unit"]}));
            }
        }
    }
    let obj = store::put_object(
        conn,
        branch,
        &format!("derived:comp:{sample_id}"),
        "derived_compensated",
        json!({
            "sample": sample_id, "raw_version": raw.version,
            "matrix_id": matrix_id, "matrix_version": m.version,
            "channels": channels, "events": comp_events,
            "diff": diffs,
            "unit": raw.payload["unit"],
            "basis": "compensated = raw * inverse(spillover matrix)"
        }),
        &format!("compensate with {matrix_id} v{}", m.version),
    )?;
    Ok(obj)
}

/// Build the display (asinh) layer from the compensated layer. Extreme values
/// stay raw in place and emit `transform.out_of_range` + a diagnostic.
pub fn apply_transforms(
    conn: &Connection,
    branch: &str,
    sample_id: &str,
    transform_id: &str,
) -> Result<ObjectRow, PipeError> {
    let raw = latest(conn, branch, sample_id)?;
    let comp_obj = latest(conn, branch, &format!("derived:comp:{sample_id}"))?;
    let tr = latest(conn, branch, transform_id)?;
    let params = raw.payload["params"].as_array().cloned().unwrap_or_default();
    let comp_events: Vec<Vec<f64>> =
        serde_json::from_value(comp_obj.payload["events"].clone()).unwrap_or_default();
    let spec = &tr.payload["params"];
    let mut out_events: Vec<Vec<f64>> = Vec::with_capacity(comp_events.len());
    let mut flags: Vec<Value> = Vec::new();
    let existing_oor: std::collections::HashSet<(String, usize)> = store::list_events(conn, Some(branch))
        .iter()
        .filter(|e| e.kind == "transform.out_of_range")
        .filter_map(|e| Some((
            e.payload["param"].as_str()?.to_string(),
            e.payload["event_index"].as_u64()? as usize,
        )))
        .collect();
    for (ei, ev) in comp_events.iter().enumerate() {
        let mut row = Vec::with_capacity(ev.len());
        for (ci, p) in params.iter().enumerate() {
            let pname = p["name"].as_str().unwrap_or_default();
            let range = p["range"].as_f64().unwrap_or(262144.0);
            let tp = transform::TransformParams {
                kind: spec[pname]["kind"].as_str().unwrap_or("asinh").to_string(),
                cofactor: spec[pname]["cofactor"].as_f64().unwrap_or(150.0),
                range,
                unit: spec[pname]["unit"].as_str().unwrap_or("ADC channels").to_string(),
            };
            let (v, diag) = transform::apply(ev[ci], pname, ei, &tp);
            row.push(v);
            if let Some(d) = diag {
                if existing_oor.contains(&(d.param.clone(), d.event_index)) {
                    continue; // value already pushed above; skip duplicate event
                }
                let ev_row = store::emit(
                    conn,
                    branch,
                    "transform.out_of_range",
                    json!({"sample": sample_id, "param": d.param, "event_index": d.event_index,
                           "raw_value": d.raw_value, "unit": d.unit, "range": d.range,
                           "message": d.message,
                           "rule": "extreme signals stay in place with diagnostics, never clamped"}),
                )?;
                let did = store::add_diagnostic(
                    conn, branch, sample_id, raw.version, Some(ev_row.seq), "warning",
                    &d.message,
                    json!({"event": ev_row.id, "param": d.param, "event_index": d.event_index}),
                )?;
                flags.push(json!({"event": ev_row.id, "diagnostic_id": did, "param": d.param,
                    "event_index": d.event_index, "raw_value": d.raw_value,
                    "unit": d.unit, "range": d.range}));
            }
        }
        out_events.push(row);
    }
    let obj = store::put_object(
        conn,
        branch,
        &format!("derived:display:{sample_id}"),
        "derived_display",
        json!({
            "sample": sample_id, "raw_version": raw.version,
            "source": {"id": comp_obj.id, "version": comp_obj.version},
            "transform_id": transform_id, "transform_version": tr.version,
            "events": out_events, "flags": flags,
            "unit": "asinh-scaled ADC channels",
            "basis": format!("asinh(x/cofactor), cofactors from {transform_id} v{}", tr.version)
        }),
        &format!("display transform with {transform_id} v{}", tr.version),
    )?;
    Ok(obj)
}

/// Fold adjudication events into the active boundary decisions:
/// (sample, gate) -> "inside" | "outside". Reverted adjudications are ignored.
pub fn active_decisions(
    conn: &Connection,
    branch: &str,
) -> std::collections::HashMap<(String, String), String> {
    let mut decided: Vec<(i64, Value)> = Vec::new();
    let mut reverted: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for ev in store::list_events(conn, Some(branch)) {
        match ev.kind.as_str() {
            "adjudication.accepted" => decided.push((ev.seq, ev.payload.clone())),
            "adjudication.reverted" => {
                if let Some(t) = ev.payload.get("reverts_event_seq").and_then(|v| v.as_i64()) {
                    reverted.insert(t);
                }
            }
            _ => {}
        }
    }
    let mut map = std::collections::HashMap::new();
    for (seq, p) in decided {
        if reverted.contains(&seq) {
            continue;
        }
        if let (Some(s), Some(g), Some(c)) = (
            p.get("sample").and_then(|v| v.as_str()),
            p.get("gate_id").and_then(|v| v.as_str()),
            p.get("choice").and_then(|v| v.as_str()),
        ) {
            map.insert((s.to_string(), g.to_string()), c.to_string());
        }
    }
    map
}

fn layer_events(obj: &ObjectRow) -> Vec<Vec<f64>> {
    serde_json::from_value(obj.payload["events"].clone()).unwrap_or_default()
}

/// Run the full gate tree on the current derived layers and store immutable
/// population results with pinned denominators.
pub fn run_gating(
    conn: &Connection,
    branch: &str,
    sample_id: &str,
    hierarchy_id: Option<&str>,
) -> Result<Vec<ObjectRow>, PipeError> {
    let raw = latest(conn, branch, sample_id)?;
    let comp_obj = latest(conn, branch, &format!("derived:comp:{sample_id}"))?;
    let disp_obj = latest(conn, branch, &format!("derived:display:{sample_id}"))?;
    let hid = hierarchy_id.unwrap_or("hierarchy:main");
    let hier = latest(conn, branch, hid)?;
    let nodes: Vec<Value> = hier.payload["nodes"].as_array().cloned().unwrap_or_default();
    let mut gates: std::collections::HashMap<String, ObjectRow> = std::collections::HashMap::new();
    for n in &nodes {
        let gid = n["gate_id"].as_str().unwrap_or("");
        gates.insert(gid.to_string(), latest(conn, branch, gid)?);
    }
    let decisions = active_decisions(conn, branch);

    // deterministic traversal: root first, then nodes in declared order
    let mut order: Vec<Value> = Vec::new();
    let mut remaining: Vec<Value> = nodes.clone();
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut rest = Vec::new();
        for n in remaining.drain(..) {
            let parent = n["parent"].as_str();
            let ready = parent.map(|p| placed.contains(p) || p == "root" || !gates.contains_key(p))
                .unwrap_or(true);
            if ready {
                placed.insert(n["gate_id"].as_str().unwrap_or("").to_string());
                order.push(n);
            } else {
                rest.push(n);
            }
        }
        remaining = rest;
        if remaining.len() == before {
            // cycle guard
            order.extend(remaining.drain(..));
            break;
        }
    }

    let raw_events = layer_events(&raw);
    let layers: std::collections::HashMap<&str, Vec<Vec<f64>>> = [
        ("raw", raw_events.clone()),
        ("compensated", layer_events(&comp_obj)),
        ("display", layer_events(&disp_obj)),
    ]
    .into_iter()
    .collect();
    let params = raw.payload["params"].as_array().cloned().unwrap_or_default();
    let pnames: Vec<String> = params.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect();

    let mut parent_version: std::collections::HashMap<String, (String, i64, usize)> =
        std::collections::HashMap::new();
    let mut produced = Vec::new();

    for n in order {
        let gid = n["gate_id"].as_str().unwrap_or("").to_string();
        let g = &gates[&gid];
        let axis_x = g.payload["axis_x"].as_str().unwrap_or("raw");
        let axis_y = g.payload["axis_y"].as_str().unwrap_or("raw");
        let layer_x = layers.get(axis_x).ok_or_else(|| PipeError::Db(format!("layer {axis_x} missing")))?;
        let layer_y = layers.get(axis_y).ok_or_else(|| PipeError::Db(format!("layer {axis_y} missing")))?;
        let xname = g.payload["x_channel"].as_str().unwrap_or("");
        let yname = g.payload["y_channel"].as_str().unwrap_or("");
        let xi = pnames.iter().position(|p| p == xname).unwrap_or(0);
        let yi = pnames.iter().position(|p| p == yname).unwrap_or(0);
        let poly: Vec<(f64, f64)> = g.payload["polygon"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|p| (p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)))
            .collect();

        let root = n["parent"].as_str().map(|p| p == "root").unwrap_or(true);
        let pool: Vec<usize> = if root {
            (0..raw_events.len()).collect()
        } else if let Some(pid) = n["parent"].as_str() {
            // members of parent version were recorded in its result payload
            let pv = parent_version.get(pid).unwrap();
            let parent_obj = store::get_object(conn, branch, &pv.0, pv.1).unwrap();
            serde_json::from_value(parent_obj.payload["members"].clone()).unwrap_or_default()
        } else {
            (0..raw_events.len()).collect()
        };

        let mut members: Vec<usize> = Vec::new();
        let mut boundaries: Vec<Value> = Vec::new();
        let mut pending = false;
        let mut boundary_indices: Vec<usize> = Vec::new();
        for ei in &pool {
            if poly.len() < 3 {
                members.push(*ei);
                continue;
            }
            let (cls, dist) = gate::classify(layer_x[*ei][xi], layer_y[*ei][yi], &poly, BOUNDARY_EPS);
            match cls {
                PointClass::Inside => members.push(*ei),
                PointClass::Outside => {}
                PointClass::Boundary => {
                    boundary_indices.push(*ei);
                    let chosen = decisions
                        .get(&(sample_id.to_string(), gid.clone()))
                        .map(|s| s == "inside")
                        .unwrap_or(true); // provisional default
                    if chosen {
                        members.push(*ei);
                    }
                    if !decisions.contains_key(&(sample_id.to_string(), gid.clone())) {
                        pending = true;
                    }
                    boundaries.push(json!({
                        "event_index": ei,
                        "distance": dist,
                        "legal_candidates": ["inside", "outside"],
                        "provisional_default": "inside",
                        "unit_basis": format!("{xname}({axis_x}) vs {yname}({axis_y})"),
                        "resolved_by_decision": decisions
                            .get(&(sample_id.to_string(), gid.clone()))
                            .cloned(),
                    }));
                }
            }
        }
        if !boundary_indices.is_empty() && !boundary_event_exists(conn, branch, &gid, g.version, hier.version, &boundary_indices) {
            store::emit(
                conn,
                branch,
                "gate.boundary_candidates",
                json!({
                    "sample": sample_id, "gate_id": gid, "gate_version": g.version,
                    "hierarchy_version": hier.version,
                    "event_indices": boundary_indices,
                    "candidates": ["inside", "outside"],
                    "rule": "boundary points keep all legal candidates until human adjudication"
                }),
            )?;
        }

        let count = members.len();
        let parent = n["parent"].as_str().and_then(|p| if p == "root" { None } else { Some(p) });
        let (percent, denominator) = match parent {
            None => (100.0, Value::Null),
            Some(pid) => {
                let pv = parent_version.get(pid).unwrap();
                let denom = json!({"population_id": pv.0, "version": pv.1, "count": pv.2,
                    "parent_gate_id": pid, "rule": "pinned at computation time"});
                store::emit(
                    conn,
                    branch,
                    "denominator.pinned",
                    json!({"sample": sample_id, "population_gate_id": gid,
                           "numerator": count, "denominator": denom,
                           "rule": "gate percentage denominator is pinned to a concrete parent population version"}),
                )?;
                (count as f64 * 100.0 / pv.2.max(1) as f64, denom)
            }
        };

        let dep_gates: Vec<Value> = collect_dep_gates(&gid, &nodes, &gates);
        let result = store::put_object(
            conn,
            branch,
            &format!("pop:{sample_id}:{gid}"),
            "population_result",
            json!({
                "sample": sample_id,
                "gate_id": gid,
                "gate_name": g.payload["name"],
                "gate_version": g.version,
                "parent": parent,
                "count": count,
                "percent": percent,
                "denominator": denominator,
                "members": members,
                "boundary": boundaries,
                "pending_adjudication": pending,
                "deps": {
                    "raw_channels": {"id": raw.id, "version": raw.version},
                    "comp_matrix": comp_obj.payload["matrix_id"].as_str().map(|mid|
                        json!({"id": mid, "version": comp_obj.payload["matrix_version"]})).unwrap_or(Value::Null),
                    "transform": {"id": tr_id(&disp_obj), "version": disp_obj.payload["transform_version"]},
                    "gate_polygons": dep_gates,
                    "hierarchy": {"id": hid, "version": hier.version},
                    "parent_result": parent.and_then(|pid| parent_version.get(pid).map(|pv|
                        json!({"id": pv.0, "version": pv.1})))
                }
            }),
            "gating result",
        )?;
        parent_version.insert(gid.clone(), (result.id.clone(), result.version, count));
        produced.push(result);
    }
    Ok(produced)
}

fn tr_id(disp: &ObjectRow) -> String {
    disp.payload["transform_id"].as_str().unwrap_or("transform:default").to_string()
}

fn collect_dep_gates(gid: &str, nodes: &[Value], gates: &std::collections::HashMap<String, ObjectRow>) -> Vec<Value> {
    let mut chain = vec![gid.to_string()];
    let mut cur = gid;
    for _ in 0..64 {
        let parent = nodes.iter()
            .find(|n| n["gate_id"].as_str() == Some(cur))
            .and_then(|n| n["parent"].as_str());
        match parent {
            Some(p) if p != "root" => { chain.push(p.to_string()); cur = p; }
            _ => break,
        }
    }
    chain.iter().map(|id| {
        let g = &gates[id];
        json!({"id": g.id, "version": g.version})
    }).collect()
}

fn boundary_event_exists(
    conn: &Connection,
    branch: &str,
    gate_id: &str,
    gate_version: i64,
    hierarchy_version: i64,
    indices: &[usize],
) -> bool {
    store::list_events(conn, Some(branch)).iter().any(|e| {
        e.kind == "gate.boundary_candidates"
            && e.payload["gate_id"].as_str() == Some(gate_id)
            && e.payload["gate_version"].as_i64() == Some(gate_version)
            && e.payload["hierarchy_version"].as_i64() == Some(hierarchy_version)
            && e.payload["event_indices"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_u64()).eq(indices.iter().map(|i| *i as u64)))
                .unwrap_or(false)
    })
}

/// Flatten a population result's dependency set into (id, version) pairs.
pub fn flat_deps(result: &ObjectRow) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let deps = &result.payload["deps"];
    for key in ["raw_channels", "comp_matrix", "transform", "hierarchy", "parent_result"] {
        if let (Some(id), Some(v)) = (deps[key]["id"].as_str(), deps[key]["version"].as_i64()) {
            out.push((id.to_string(), v));
        }
    }
    if let Some(arr) = deps["gate_polygons"].as_array() {
        for g in arr {
            if let (Some(id), Some(v)) = (g["id"].as_str(), g["version"].as_i64()) {
                out.push((id.to_string(), v));
            }
        }
    }
    out
}

/// Mark every population result whose dependency closure contains
/// `changed_id` at an older version as stale (event-sourced; results are
/// never mutated or overwritten). Returns the affected result ids.
pub fn mark_stale(
    conn: &Connection,
    branch: &str,
    changed_id: &str,
    changed_version: i64,
    cause: &str,
) -> Result<Vec<String>, String> {
    let results = store::list_latest_by_kind(conn, branch, "population_result");
    let mut stale_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for r in &results {
            if stale_ids.contains(&r.id) {
                continue;
            }
            let hit = flat_deps(r).iter().any(|(id, v)| {
                (id == changed_id && *v < changed_version) || stale_ids.contains(id)
            });
            if hit {
                stale_ids.insert(r.id.clone());
                changed = true;
            }
        }
    }
    let mut affected = Vec::new();
    for r in &results {
        if !stale_ids.contains(&r.id) {
            continue;
        }
        let already = store::list_events(conn, Some(branch)).iter().any(|e| {
            e.kind == "population.stale_marked"
                && e.payload["result_id"].as_str() == Some(r.id.as_str())
                && e.payload["result_version"].as_i64() == Some(r.version)
        });
        if already {
            continue;
        }
        store::emit(
            conn,
            branch,
            "population.stale_marked",
            json!({
                "result_id": r.id,
                "result_version": r.version,
                "population_gate_id": r.payload["gate_id"],
                "cause": {"changed_id": changed_id, "changed_version": changed_version, "kind": cause},
                "rule": "revising an upstream gate marks downstream results stale; old results are never overwritten"
            }),
        )?;
        affected.push(r.id.clone());
    }
    Ok(affected)
}

/// Rename a gate (identity is the stable id; name is metadata). Emits
/// `gate.renamed` with the affected children, then marks dependents stale.
pub fn rename_gate(conn: &Connection, branch: &str, gate_id: &str, new_name: &str) -> Result<ObjectRow, String> {
    let g = store::latest_version(conn, branch, gate_id)
        .ok_or_else(|| format!("gate {gate_id} not found"))?;
    let old_name = g.payload["name"].as_str().unwrap_or("").to_string();
    let mut payload = g.payload.clone();
    payload["name"] = json!(new_name);
    let new_gate = store::put_object(conn, branch, gate_id, "gate_polygon", payload, "rename")?;

    let hier = store::latest_version(conn, branch, "hierarchy:main")
        .ok_or_else(|| "hierarchy:main not found".to_string())?;
    let children: Vec<String> = hier.payload["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|n| n["parent"].as_str() == Some(gate_id))
        .filter_map(|n| n["gate_id"].as_str().map(String::from))
        .collect();
    store::emit(
        conn,
        branch,
        "gate.renamed",
        json!({
            "gate_id": gate_id,
            "old_name": old_name,
            "new_name": new_name,
            "gate_version": new_gate.version,
            "affected_children": children,
            "hierarchy_version": hier.version,
            "rule": "gate identity is the stable id; renaming never breaks parent-child relations"
        }),
    )?;
    mark_stale(conn, branch, gate_id, new_gate.version, "gate_renamed")?;
    Ok(new_gate)
}

/// Record a human adjudication of a boundary-candidates event and re-run
/// gating so dependent results are rebuilt as NEW versions.
pub fn adjudicate(
    conn: &Connection,
    branch: &str,
    boundary_event_seq: i64,
    choice: &str,
) -> Result<Value, String> {
    let events = store::list_events(conn, Some(branch));
    let target = events
        .iter()
        .find(|e| e.seq == boundary_event_seq && e.kind == "gate.boundary_candidates")
        .ok_or_else(|| format!("boundary event seq {boundary_event_seq} not found"))?;
    let sample = target.payload["sample"].as_str().unwrap_or("sample_events").to_string();
    let gate_id = target.payload["gate_id"].as_str().unwrap_or("").to_string();
    let ev = store::emit(
        conn,
        branch,
        "adjudication.accepted",
        json!({
            "boundary_event_seq": boundary_event_seq,
            "sample": sample,
            "gate_id": gate_id,
            "choice": choice,
            "rule": "human adjudication is an event; it never rewrites raw data"
        }),
    )?;
    let results = run_gating(conn, branch, &sample, None).map_err(|e| match e {
        PipeError::Db(s) => s,
        PipeError::Rejected(v) => v.to_string(),
    })?;
    Ok(json!({
        "adjudication_event": ev.id,
        "seq": ev.seq,
        "rebuilt_results": results.iter().map(|r| json!({"id": r.id, "version": r.version})).collect::<Vec<_>>()
    }))
}

/// Undo an adjudication: the decision is reverted (event-sourced) and gating
/// is re-run, restoring the pre-adjudication branch of interpretation.
pub fn revert_adjudication(conn: &Connection, branch: &str, adjudication_seq: i64) -> Result<Value, String> {
    let events = store::list_events(conn, Some(branch));
    let target = events
        .iter()
        .find(|e| e.seq == adjudication_seq && e.kind == "adjudication.accepted")
        .ok_or_else(|| format!("adjudication seq {adjudication_seq} not found"))?;
    let sample = target.payload["sample"].as_str().unwrap_or("sample_events").to_string();
    let ev = store::emit(
        conn,
        branch,
        "adjudication.reverted",
        json!({
            "reverts_event_seq": adjudication_seq,
            "sample": sample,
            "gate_id": target.payload["gate_id"],
            "rule": "any adjudication can be reverted; the previous interpretation branch is restored"
        }),
    )?;
    let results = run_gating(conn, branch, &sample, None).map_err(|e| match e {
        PipeError::Db(s) => s,
        PipeError::Rejected(v) => v.to_string(),
    })?;
    Ok(json!({
        "revert_event": ev.id,
        "seq": ev.seq,
        "rebuilt_results": results.iter().map(|r| json!({"id": r.id, "version": r.version})).collect::<Vec<_>>()
    }))
}
