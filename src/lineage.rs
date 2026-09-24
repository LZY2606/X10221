use std::collections::{HashMap, HashSet};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub const KIND_RAW: &str = "raw_channels";
pub const KIND_SPILL: &str = "spillover";
pub const KIND_COMP: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate_polygon";
pub const KIND_HIER: &str = "gate_hierarchy";

pub const EVT_ARTIFACT: &str = "artifact.versioned";
pub const EVT_STALED: &str = "result.staled";
pub const EVT_COMPUTED: &str = "result.computed";
pub const EVT_DENOMINATOR: &str = "denominator.pinned";
pub const EVT_NEAR_SINGULAR: &str = "compensation.near_singular_rejected";
pub const EVT_DIAGNOSTIC: &str = "normalization.diagnostic";
pub const EVT_BOUNDARY: &str = "gate.boundary_ambiguous";
pub const EVT_RENAMED: &str = "gate.renamed";
pub const EVT_REPARENTED: &str = "hierarchy.reparented";
pub const EVT_RULING: &str = "ruling.made";
pub const EVT_UNDO: &str = "ruling.undone";
pub const EVT_CONFLICT: &str = "draft.conflict";
pub const EVT_DRAFT_SAVED: &str = "draft.saved";
pub const EVT_LOADED: &str = "casepack.loaded";
pub const EVT_SCENARIO: &str = "scenario.replayed";

pub type EventRow = (i64, String, String, Value);

pub fn record_event(conn: &Connection, kind: &str, payload: Value) -> (i64, String) {
    conn.execute(
        "INSERT INTO events(id, kind, payload) VALUES ('', ?1, ?2)",
        params![kind, payload.to_string()],
    )
    .expect("insert event");
    let seq = conn.last_insert_rowid();
    let id = format!("evt:{seq:06}");
    conn.execute("UPDATE events SET id = ?1 WHERE seq = ?2", params![id, seq])
        .expect("set event id");
    (seq, id)
}

pub fn head_seq(conn: &Connection) -> i64 {
    conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| {
        r.get(0)
    })
    .unwrap_or(0)
}

pub fn events(conn: &Connection, at: Option<i64>) -> Vec<EventRow> {
    let limit = at.unwrap_or(i64::MAX);
    let mut stmt = conn
        .prepare("SELECT seq, id, kind, payload FROM events WHERE seq <= ?1 ORDER BY seq")
        .unwrap();
    let rows = stmt
        .query_map(params![limit], |r| {
            let payload_str: String = r.get(3)?;
            let payload = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, payload))
        })
        .unwrap();
    rows.flatten().collect()
}

#[derive(Debug, Clone)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub payload: Value,
    pub unit: Option<String>,
    pub basis: Option<String>,
    pub created_seq: i64,
}

fn artifact_from_row(r: &rusqlite::Row) -> rusqlite::Result<Artifact> {
    let payload_str: String = r.get(4)?;
    Ok(Artifact {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        version: r.get(3)?,
        payload: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
        unit: r.get(5)?,
        basis: r.get(6)?,
        created_seq: r.get(7)?,
    })
}

pub fn all_artifacts(conn: &Connection, at: Option<i64>) -> Vec<Artifact> {
    let limit = at.unwrap_or(i64::MAX);
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, name, version, payload, unit, basis, created_seq \
             FROM artifacts WHERE created_seq <= ?1 ORDER BY created_seq, id",
        )
        .unwrap();
    let rows = stmt
        .query_map(params![limit], artifact_from_row)
        .unwrap();
    rows.flatten().collect()
}

/// 每个 (kind, name) 的最新版本（可按历史事件 seq 截断，用于回到旧事件）。
pub fn latest_artifacts(
    conn: &Connection,
    at: Option<i64>,
) -> HashMap<(String, String), Artifact> {
    let mut out = HashMap::new();
    for art in all_artifacts(conn, at) {
        let key = (art.kind.clone(), art.name.clone());
        match out.get(&key) {
            Some(existing) if existing.version >= art.version => {}
            _ => {
                out.insert(key, art);
            }
        }
    }
    out
}

/// 分开版本化写入一个来源产物；事件先落盘，产物用其 seq 标记创建时间。
pub fn put_artifact(
    conn: &Connection,
    kind: &str,
    name: &str,
    payload: &Value,
    unit: Option<&str>,
    basis: Option<&str>,
) -> Artifact {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM artifacts WHERE kind = ?1 AND name = ?2",
            params![kind, name],
            |r| r.get(0),
        )
        .unwrap();
    let (seq, id) = record_event(
        conn,
        EVT_ARTIFACT,
        json!({"artifact": format!("art:{kind}:{name}:v{version}"), "kind": kind, "name": name, "version": version}),
    );
    let art = Artifact {
        id,
        kind: kind.to_string(),
        name: name.to_string(),
        version,
        payload: payload.clone(),
        unit: unit.map(|s| s.to_string()),
        basis: basis.map(|s| s.to_string()),
        created_seq: seq,
    };
    conn.execute(
        "INSERT INTO artifacts(id, kind, name, version, payload, unit, basis, created_seq) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            art.id,
            art.kind,
            art.name,
            art.version,
            art.payload.to_string(),
            art.unit,
            art.basis,
            art.created_seq
        ],
    )
    .expect("insert artifact");
    art
}
