//! Deterministic replay of the boundary scenarios after initial import.
//!
//! Every operation is an optimistic-concurrency save: it checks the branch
//! revision inside one transaction, so the second of two simultaneous saves
//! against the same revision fails with a conflict and both-side diff data.

use std::path::Path;

use serde_json::{json, Value};

use crate::domain::Signal;
use crate::engine::invalidate_scope;
use crate::id::obj_id;
use crate::linalg::{judge_invert, NEAR_SINGULAR_THRESHOLD};
use crate::models::{CompMatrix, InvertRecord, RawFcsEvents, SpilloverMatrix, SpilloverMeta};
use crate::spill::parse_spill_csv;
use crate::{
    fcs::parse_fcs,
    importer::{fcs_to_raw_events_pub, read_bytes_pub},
    store::{
        stream_for, BranchRow,
        STREAM_COMP, STREAM_GATES, STREAM_HIER, STREAM_RAW, STREAM_SPILL_MATRIX, STREAM_SPILL_META,
        STREAM_XFORM,
    },
    db::Db,
};

#[derive(Debug)]
pub struct ConflictError {
    pub branch_id: String,
    pub server_revision: i64,
    pub your_revision: i64,
    pub server_tip: String,
    pub diff: Value,
}

#[allow(dead_code)]
fn seq_ts(seq: i64) -> i64 {
    1_700_000_000_000 + seq
}

fn make_event_id(case_id: &str, branch: &str, seq: i64, etype: &str, payload: &Value) -> String {
    let mut bytes = case_id.as_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(branch.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&seq.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(etype.as_bytes());
    bytes.extend_from_slice(&crate::store::canonical(payload));
    obj_id("event", &bytes)
}

fn insert_entity_on(
    db: &Db,
    case_id: &str,
    stream: &str,
    kind: &str,
    payload: &Value,
) -> (String, i64) {
    let version = db.current_entity_version(case_id, stream) + 1;
    let bytes = crate::store::canonical(payload);
    let id = obj_id(
        stream.split(':').next().unwrap_or("ent"),
        &[version.to_le_bytes().as_slice(), bytes.as_slice()].concat(),
    );
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO entities(case_id,id,stream_id,kind,version,payload_json,created_event)
         VALUES(?1,?2,?3,?4,?5,?6,NULL)",
        rusqlite::params![case_id, id, stream, kind, version, payload.to_string()],
    )
    .unwrap();
    (id, version)
}

fn next_seq(db: &Db, case_id: &str, branch: &str) -> i64 {
    db.next_event_seq(case_id, branch)
}

fn emit(db: &Db, case_id: &str, branch: &str, seq: i64, etype: &str, payload: Value) -> String {
    let eid = make_event_id(case_id, branch, seq, etype, &payload);
    db.append_event(case_id, branch, &eid, seq, etype, &payload, seq_ts(seq));
    eid
}

/// Atomic optimistic-concurrency revision of one stream.
/// On revision mismatch returns a ConflictError carrying the server-side diff.
pub fn revise_layer(
    db: &Db,
    case_id: &str,
    branch_id: &str,
    expected_revision: i64,
    stream_id: &str,
    kind: &str,
    payload: &Value,
    author_note: &str,
) -> Result<String, ConflictError> {
    let branch = db.get_branch(case_id, branch_id).expect("branch");
    if branch.revision != expected_revision {
        return Err(conflict(db, case_id, &branch, expected_revision, stream_id));
    }
    // 1. create new entity version (outside branch tx)
    let (new_id, new_ver) = insert_entity_on(db, case_id, stream_id, kind, payload);
    // 2. revision + ref move + event, one transaction
    let bump = db.bump_revision(case_id, &branch.branch_id, expected_revision);
    if bump.is_err() {
        return Err(conflict(db, case_id, &branch, expected_revision, stream_id));
    }
    db.set_ref(case_id, &branch.branch_id, stream_id, &new_id, new_ver);
    let seq = next_seq(db, case_id, &branch.branch_id);
    let old_ref = old_ref_before(db, case_id, &branch.branch_id, stream_id, seq - 1);
    let payload_event = json!({
        "kind": "layer_revision",
        "stream_id": stream_id,
        "old": old_ref,
        "new": {"entity_id": new_id, "version": new_ver},
        "author_note": author_note,
        "scope": invalidation_scope_for(db, case_id, &branch, &[(stream_id.to_string(), new_ver)], seq, "layer"),
    });
    let eid = emit(db, case_id, &branch.branch_id, seq, "ref_updated", json!({
        "stream_id": stream_id, "entity_id": new_id, "version": new_ver,
    }));
    let _ = eid;
    let seq2 = seq + 1;
    let rev_eid = emit(db, case_id, &branch.branch_id, seq2, "layer_revised", payload_event);

    // 3. invalidate dependent results, then recompute fresh rows.
    invalidate_scope(
        db,
        case_id,
        &branch,
        &[(stream_id.to_string(), new_ver)],
        &rev_eid,
    );
    let (created, _blocking) = db.populate_results(case_id, &branch, &rev_eid, seq2);
    let _ = created;
    Ok(rev_eid)
}

fn old_ref_before(db: &Db, case_id: &str, branch: &str, stream: &str, _prev_seq: i64) -> Value {
    db.refs_at_event(case_id, branch, i64::MAX)
        .into_iter()
        .find(|(s, _, _)| s == stream)
        .map(|(_, id, v)| json!({"entity_id": id, "version": v}))
        .unwrap_or(Value::Null)
}

fn invalidation_scope_for(
    db: &Db,
    case_id: &str,
    branch: &BranchRow,
    changed: &[(String, i64)],
    _seq: i64,
    _tag: &str,
) -> Value {
    let before: std::collections::BTreeSet<String> = db
        .list_results_for_branch(case_id, &branch.branch_id)
        .into_iter()
        .map(|r| r.result_id)
        .collect();
    // predict affected results from current deps without mutating
    let mut affected = Vec::new();
    for r in db.list_results_for_branch(case_id, &branch.branch_id) {
        if r.deps.iter().any(|(s, id, _)| {
            changed.iter().any(|(cs, cv)| {
                let same_stream = s == cs;
                let older = same_stream
                    && db.get_entity(case_id, id).map(|e| e.version < *cv).unwrap_or(false);
                same_stream || older
            })
        }) || (changed.iter().any(|(s, _)| {
            s == crate::store::STREAM_RAW
                || s == crate::store::STREAM_COMP
                || s == crate::store::STREAM_XFORM
                || s == crate::store::STREAM_HIER
        }) && r.kind != "compensation")
        {
            affected.push(json!({"result_id": r.result_id, "kind": r.kind, "target": r.target_key}));
        }
    }
    let _ = before;
    json!({"affected": affected})
}

fn conflict(
    db: &Db,
    case_id: &str,
    branch: &BranchRow,
    your_revision: i64,
    stream_id: &str,
) -> ConflictError {
    let server_ref = db
        .get_refs(case_id, &branch.branch_id)
        .into_iter()
        .find(|(s, _, _)| s == stream_id)
        .map(|(_, id, v)| json!({"entity_id": id, "version": v}))
        .unwrap_or(Value::Null);
    ConflictError {
        branch_id: branch.branch_id.clone(),
        server_revision: branch.revision,
        your_revision,
        server_tip: branch.branch_id.clone(),
        diff: json!({
            "stream_id": stream_id,
            "your_base_revision": your_revision,
            "server_revision": branch.revision,
            "server_ref": server_ref,
        }),
    }
}
