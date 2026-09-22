//! Versioned object store + append-only event log on SQLite.
//!
//! Provenance split: raw channel values, spillover metadata, compensation
//! matrices, transform parameters, gate polygons and parent-child hierarchy
//! are SIX separately-versioned object kinds. Nothing is ever overwritten:
//! re-interpretation appends a new version on a branch, old versions stay
//! readable. Optimistic concurrency is per branch head (`expected_head`).

use rusqlite::{params, Connection};
use serde_json::Value;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct ObjectRow {
    pub id: String,
    pub version: i64,
    pub branch: String,
    pub kind: String,
    pub status: String,
    pub note: String,
    pub payload: Value,
    pub created_seq: i64,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub id: String,
    pub ts: String,
    pub branch: String,
    pub kind: String,
    pub payload: Value,
}

pub struct Store {
    pub conn: Mutex<Connection>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Conflict {
    pub branch: String,
    pub expected_head: i64,
    pub current_head: i64,
    pub events_since_base: Vec<EventRow>,
    pub your_ops: Value,
}

impl Store {
    pub fn new(conn: Connection) -> Self {
        let s = Store { conn: Mutex::new(conn) };
        {
            let c = s.conn.lock().unwrap();
            ensure_branch(&c, "main", None).ok();
        }
        s
    }
}

pub fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}ms", n.as_millis())
}

pub fn ensure_branch(conn: &Connection, name: &str, fork: Option<(String, i64)>) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO branches(name, head_seq, fork_from, fork_seq)
         VALUES (?1, COALESCE((SELECT seq FROM events WHERE branch=?1 ORDER BY seq DESC LIMIT 1), 0), ?2, ?3)",
        params![name, fork.as_ref().map(|f| f.0.clone()), fork.as_ref().map(|f| f.1)],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn branch_head(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE((SELECT seq FROM events WHERE branch=?1 ORDER BY seq DESC LIMIT 1), 0)",
        params![name],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

pub fn list_branches(conn: &Connection) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let mut stmt = conn
        .prepare("SELECT name, (SELECT COALESCE(MAX(seq),0) FROM events WHERE branch=branches.name) FROM branches ORDER BY name")
        .unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
    if let Ok(rows) = rows {
        for r in rows.flatten() {
            out.push(r);
        }
    }
    out
}

/// Append one event. Returns the stored row with assigned seq/id.
pub fn emit(conn: &Connection, branch: &str, kind: &str, payload: Value) -> Result<EventRow, String> {
    let ts = now_ts();
    conn.execute(
        "INSERT INTO events(id, ts, branch, kind, payload) VALUES ('evt_' || (COALESCE((SELECT MAX(seq) FROM events),0)+1), ?1, ?2, ?3, ?4)",
        params![ts, branch, kind, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let seq = conn.last_insert_rowid();
    Ok(EventRow {
        seq,
        id: format!("evt_{seq}"),
        ts,
        branch: branch.to_string(),
        kind: kind.to_string(),
        payload,
    })
}

/// Append a new immutable version of an object (emits object.created first so
/// created_seq points at a real event).
pub fn put_object(
    conn: &Connection,
    branch: &str,
    id: &str,
    kind: &str,
    payload: Value,
    note: &str,
) -> Result<ObjectRow, String> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM objects WHERE id=?1 AND branch=?2",
            params![id, branch],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let ev = emit(
        conn,
        branch,
        "object.created",
        serde_json::json!({"object_id": id, "version": version, "kind": kind, "note": note}),
    )?;
    conn.execute(
        "INSERT INTO objects(id, version, branch, kind, status, payload, created_seq, note)
         VALUES (?1,?2,?3,?4,'current',?5,?6,?7)",
        params![id, branch, version, kind, payload.to_string(), ev.seq, note],
    )
    .map_err(|e| e.to_string())?;
    Ok(ObjectRow {
        id: id.into(),
        version,
        branch: branch.into(),
        kind: kind.into(),
        status: "current".into(),
        note: note.into(),
        payload,
        created_seq: ev.seq,
    })
}

pub fn get_object(conn: &Connection, branch: &str, id: &str, version: i64) -> Option<ObjectRow> {
    conn.query_row(
        "SELECT id,version,branch,kind,status,note,payload,created_seq FROM objects WHERE id=?1 AND branch=?2 AND version=?3",
        params![id, branch, version],
        map_object,
    )
    .ok()
}

pub fn latest_version(conn: &Connection, branch: &str, id: &str) -> Option<ObjectRow> {
    conn.query_row(
        "SELECT id,version,branch,kind,status,note,payload,created_seq FROM objects
         WHERE id=?1 AND branch=?2 ORDER BY version DESC LIMIT 1",
        params![id, branch],
        map_object,
    )
    .ok()
}

pub fn list_versions(conn: &Connection, id: &str, branch: &str) -> Vec<ObjectRow> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id,version,branch,kind,status,note,payload,created_seq FROM objects
         WHERE id=?1 AND branch=?2 ORDER BY version",
    ) {
        if let Ok(rows) = stmt.query_map(params![id, branch], map_object) {
            out = rows.flatten().collect();
        }
    }
    out
}

pub fn list_latest_by_kind(conn: &Connection, branch: &str, kind: &str) -> Vec<ObjectRow> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT o.id,o.version,o.branch,o.kind,o.status,o.note,o.payload,o.created_seq
         FROM objects o
         JOIN (SELECT id, MAX(version) mv FROM objects WHERE branch=?1 AND kind=?2 GROUP BY id) m
           ON m.id=o.id AND o.version=m.mv
         WHERE o.branch=?1 ORDER BY o.id",
    ) {
        if let Ok(rows) = stmt.query_map(params![branch, kind], map_object) {
            out = rows.flatten().collect();
        }
    }
    out
}

pub fn list_events(conn: &Connection, branch: Option<&str>) -> Vec<EventRow> {
    let mut out = Vec::new();
    let sql = match branch {
        Some(_) => "SELECT seq,id,ts,branch,kind,payload FROM events WHERE branch=?1 ORDER BY seq",
        None => "SELECT seq,id,ts,branch,kind,payload FROM events ORDER BY seq",
    };
    if let Ok(mut stmt) = conn.prepare(sql) {
        let rows = if let Some(b) = branch {
            stmt.query_map(params![b], map_event)
        } else {
            stmt.query_map([], map_event)
        };
        if let Ok(rows) = rows {
            out = rows.flatten().collect();
        }
    }
    out
}

pub fn events_since(conn: &Connection, branch: &str, since_seq: i64) -> Vec<EventRow> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT seq,id,ts,branch,kind,payload FROM events WHERE branch=?1 AND seq>?2 ORDER BY seq",
    ) {
        if let Ok(rows) = stmt.query_map(params![branch, since_seq], map_event) {
            out = rows.flatten().collect();
        }
    }
    out
}

pub fn add_diagnostic(
    conn: &Connection,
    branch: &str,
    object_id: &str,
    object_version: i64,
    event_seq: Option<i64>,
    severity: &str,
    message: &str,
    payload: Value,
) -> Result<String, String> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*)+1 FROM diagnostics", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let id = format!("diag_{n}");
    conn.execute(
        "INSERT INTO diagnostics(id,branch,object_id,object_version,event_seq,severity,message,payload)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id, branch, object_id, object_version, event_seq, severity, message, payload.to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

pub fn list_diagnostics(conn: &Connection, branch: &str) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT id,object_id,object_version,event_seq,severity,message,payload FROM diagnostics
         WHERE branch=?1 ORDER BY rowid",
    ) {
        if let Ok(rows) = stmt.query_map(params![branch], |r| {
            let payload: String = r.get(6)?;
            Ok(serde_json::json!({
                "id": r.get::<_,String>(0)?,
                "object_id": r.get::<_,String>(1)?,
                "object_version": r.get::<_,i64>(2)?,
                "event_seq": r.get::<_,Option<i64>>(3)?,
                "severity": r.get::<_,String>(4)?,
                "message": r.get::<_,String>(5)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
            }))
        }) {
            out = rows.flatten().collect();
        }
    }
    out
}

pub fn store_blob(conn: &Connection, filename: &str, bytes: &[u8]) -> Result<(String, String), String> {
    let checksum = fnv1a_hex(bytes);
    let id = format!("blob_{checksum}");
    conn.execute(
        "INSERT OR IGNORE INTO blobs(id, filename, checksum, bytes) VALUES (?1,?2,?3,?4)",
        params![id, filename, checksum, bytes],
    )
    .map_err(|e| e.to_string())?;
    Ok((id, checksum))
}

pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// Runs `ops` inside a transaction guarded by the branch head. On mismatch the
/// transaction is rolled back, a `draft.conflict` event is appended in a fresh
/// transaction, and the structured `Conflict` is returned.
pub fn with_optimistic<F>(
    conn: &Connection,
    branch: &str,
    expected_head: i64,
    ops_desc: Value,
    ops: F,
) -> Result<i64, Conflict>
where
    F: FnOnce(&Connection) -> Result<(), String>,
{
    ensure_branch(conn, branch, None).map_err(|e| fake_conflict(branch, expected_head, ops_desc.clone(), e))?;
    let head = branch_head(conn, branch);
    if head != expected_head {
        return Err(build_conflict(conn, branch, expected_head, ops_desc));
    }
    {
        let tx = conn.unchecked_transaction().expect("tx");
        ops(&tx).map_err(|e| fake_conflict(branch, expected_head, ops_desc.clone(), e))?;
        tx.commit().map_err(|e| fake_conflict(branch, expected_head, ops_desc.clone(), e.to_string()))?;
    }
    let new_head = branch_head(conn, branch);
    Ok(new_head)
}

fn build_conflict(conn: &Connection, branch: &str, expected_head: i64, ops_desc: Value) -> Conflict {
    let current = branch_head(conn, branch);
    let since = events_since(conn, branch, expected_head);
    // Persist the conflict itself; it is not part of the rejected transaction.
    let _ = {
        let tx = conn.unchecked_transaction().expect("tx");
        let r = emit(
            &tx,
            branch,
            "draft.conflict",
            serde_json::json!({
                "expected_head": expected_head,
                "current_head": current,
                "committed_events": since.iter().map(|e| e.seq).collect::<Vec<_>>(),
                "rejected_ops": ops_desc,
            }),
        );
        tx.commit().ok().and(r.ok())
    };
    Conflict {
        branch: branch.into(),
        expected_head,
        current_head: current,
        events_since_base: since,
        your_ops: ops_desc,
    }
}

fn fake_conflict(branch: &str, expected_head: i64, ops_desc: Value, err: String) -> Conflict {
    Conflict {
        branch: branch.into(),
        expected_head,
        current_head: -1,
        events_since_base: vec![],
        your_ops: serde_json::json!({"error": err, "ops": ops_desc}),
    }
}

fn map_object(r: &rusqlite::Row) -> rusqlite::Result<ObjectRow> {
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
}

fn map_event(r: &rusqlite::Row) -> rusqlite::Result<EventRow> {
    let payload: String = r.get(5)?;
    Ok(EventRow {
        seq: r.get(0)?,
        id: r.get(1)?,
        ts: r.get(2)?,
        branch: r.get(3)?,
        kind: r.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
    })
}
