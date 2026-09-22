//! Read models: branch state, historical state at an event, branch compare.
//! Statuses are folded from the event log — objects themselves are immutable.

use crate::pipeline;
use crate::store::{self, EventRow, ObjectRow};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashSet;

fn obj_json(o: &ObjectRow) -> Value {
    json!({
        "id": o.id, "version": o.version, "branch": o.branch, "kind": o.kind,
        "status": o.status, "note": o.note, "created_seq": o.created_seq,
        "payload": o.payload,
    })
}

fn ev_json(e: &EventRow) -> Value {
    json!({"seq": e.seq, "id": e.id, "ts": e.ts, "branch": e.branch, "kind": e.kind, "payload": e.payload})
}

/// Latest objects per id created at or before `at_seq` (None = now).
fn latest_at(conn: &Connection, branch: &str, at_seq: Option<i64>) -> Vec<ObjectRow> {
    let limit = at_seq.unwrap_or(i64::MAX);
    store::list_events(conn, Some(branch)); // touch for symmetry; objects queried directly
    let mut out = Vec::new();
    let sql = "SELECT o.id,o.version,o.branch,o.kind,o.status,o.note,o.payload,o.created_seq
               FROM objects o
               JOIN (SELECT id, MAX(version) mv FROM objects
                     WHERE branch=?1 AND created_seq<=?2 GROUP BY id) m
                 ON m.id=o.id AND o.version=m.mv
               WHERE o.branch=?1 ORDER BY o.kind, o.id";
    if let Ok(mut stmt) = conn.prepare(sql) {
        if let Ok(rows) = stmt.query_map(rusqlite::params![branch, limit], |r| {
            let payload: String = r.get(6)?;
            Ok(ObjectRow {
                id: r.get(0)?,
                version: r.get(1)?,
                branch: r.get(2)?,
                kind: r.get(3)?,
                status: r.get(4)?,
                note: r.get(5)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                created_seq: r.get(7)?,
            })
        }) {
            out = rows.flatten().collect();
        }
    }
    out
}

fn stale_set(conn: &Connection, branch: &str, at_seq: Option<i64>) -> HashSet<(String, i64)> {
    let mut set = HashSet::new();
    for e in store::list_events(conn, Some(branch)) {
        if let Some(s) = at_seq {
            if e.seq > s {
                break;
            }
        }
        if e.kind == "population.stale_marked" {
            if let (Some(id), Some(v)) = (
                e.payload["result_id"].as_str(),
                e.payload["result_version"].as_i64(),
            ) {
                set.insert((id.to_string(), v));
            }
        }
    }
    set
}

/// Full review state for a branch, optionally rewound to an event seq.
pub fn branch_state(conn: &Connection, branch: &str, at_seq: Option<i64>) -> Value {
    let objects = latest_at(conn, branch, at_seq);
    let stale = stale_set(conn, branch, at_seq);
    let decisions = pipeline::active_decisions(conn, branch);

    let mut by_kind: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut populations = Vec::new();
    for o in &objects {
        let mut j = obj_json(o);
        if o.kind == "population_result" {
            let is_stale = stale.contains(&(o.id.clone(), o.version));
            j["effective_status"] = json!(if is_stale { "stale" } else { "current" });
            let sample = o.payload["sample"].as_str().unwrap_or("").to_string();
            let gate = o.payload["gate_id"].as_str().unwrap_or("").to_string();
            let decided = decisions.get(&(sample.clone(), gate.clone()));
            j["adjudication"] = json!(decided);
            j["pending_adjudication"] =
                json!(o.payload["pending_adjudication"].as_bool().unwrap_or(false) && decided.is_none());
            populations.push(j.clone());
        }
        by_kind.entry(o.kind.clone()).or_insert_with(|| json!([])).as_array_mut().unwrap().push(j);
    }

    let events: Vec<Value> = store::list_events(conn, Some(branch))
        .into_iter()
        .filter(|e| at_seq.map(|s| e.seq <= s).unwrap_or(true))
        .map(|e| ev_json(&e))
        .collect();
    let boundary_events: Vec<Value> = events
        .iter()
        .filter(|e| e["kind"] == "gate.boundary_candidates")
        .cloned()
        .collect();
    let adjudications: Vec<Value> = events
        .iter()
        .filter(|e| e["kind"] == "adjudication.accepted" || e["kind"] == "adjudication.reverted")
        .cloned()
        .collect();

    json!({
        "branch": branch,
        "at_seq": at_seq,
        "head": store::branch_head(conn, branch),
        "branches": store::list_branches(conn).iter()
            .map(|(n, h)| json!({"name": n, "head": h})).collect::<Vec<_>>(),
        "objects": by_kind,
        "populations": populations,
        "boundary_events": boundary_events,
        "adjudications": adjudications,
        "diagnostics": store::list_diagnostics(conn, branch),
        "events": events,
    })
}

/// Side-by-side comparison of two branches/drafts: populations matched by
/// gate id, plus per-kind version tables. Every value keeps its stable id.
pub fn compare(conn: &Connection, left: &str, right: &str) -> Value {
    let lpops = latest_at(conn, left, None);
    let rpops = latest_at(conn, right, None);
    let lmap: std::collections::HashMap<String, &ObjectRow> = lpops
        .iter()
        .filter(|o| o.kind == "population_result")
        .map(|o| (o.payload["gate_id"].as_str().unwrap_or("").to_string(), o))
        .collect();
    let rmap: std::collections::HashMap<String, &ObjectRow> = rpops
        .iter()
        .filter(|o| o.kind == "population_result")
        .map(|o| (o.payload["gate_id"].as_str().unwrap_or("").to_string(), o))
        .collect();
    let mut gates: Vec<String> = lmap.keys().chain(rmap.keys()).cloned().collect();
    gates.sort();
    gates.dedup();
    let rows: Vec<Value> = gates
        .iter()
        .map(|g| {
            let l = lmap.get(g);
            let r = rmap.get(g);
            let changed = match (l, r) {
                (Some(a), Some(b)) => {
                    a.payload["count"] != b.payload["count"]
                        || a.payload["percent"] != b.payload["percent"]
                        || a.payload["gate_version"] != b.payload["gate_version"]
                }
                _ => true,
            };
            json!({
                "gate_id": g,
                "left": l.map(|o| json!({"id": o.id, "version": o.version,
                    "count": o.payload["count"], "percent": o.payload["percent"],
                    "denominator": o.payload["denominator"]})),
                "right": r.map(|o| json!({"id": o.id, "version": o.version,
                    "count": o.payload["count"], "percent": o.payload["percent"],
                    "denominator": o.payload["denominator"]})),
                "changed": changed,
            })
        })
        .collect();
    json!({"left": left, "right": right, "populations": rows})
}
