//! SQLite 持久化：版本化迁移、材料/诊断/批次/分支/事件/冲突。
//! 所有导入在单个事务内完成，失败只回滚本次变更，不留半份记录。

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

pub fn open(path: &str) -> anyhow::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> anyhow::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> anyhow::Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let version: i64 = conn
        .query_row(
            "SELECT value FROM _meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let files = [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_provenance.sql"),
        include_str!("../migrations/0003_heads.sql"),
    ];
    let mut current = version;
    for (i, sql) in files.iter().enumerate() {
        let target = (i + 1) as i64;
        if current < target {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO _meta(key,value) VALUES('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![target.to_string()],
            )?;
            tx.commit()?;
            current = target;
        }
    }
    Ok(current)
}

/// 从旧版本直接迁移（供状态迁移测试在 v1 老库上调用）。
pub fn open_and_migrate_from_v1(path: &str) -> anyhow::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(include_str!("../migrations/0001_init.sql"))?;
    conn.execute(
        "INSERT INTO _meta(key,value) VALUES('schema_version','1')
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [],
    )?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn schema_version(conn: &Connection) -> anyhow::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT value FROM _meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )?
        .parse()?)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[derive(Debug, Clone)]
pub struct MaterialRow {
    pub material_id: String,
    pub kind: String,
    pub name: String,
    pub media_type: String,
    pub raw_bytes: Vec<u8>,
    pub sha256: String,
    pub unit_or_basis: String,
    pub import_batch: String,
    pub ordinal: i64,
    pub supersedes: Option<String>,
}

pub fn insert_material(
    tx: &rusqlite::Transaction,
    row: &MaterialRow,
    now: &str,
) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO materials
           (material_id,kind,name,media_type,raw_bytes,sha256,imported_at,
            unit_or_basis,import_batch,ordinal,supersedes)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            row.material_id,
            row.kind,
            row.name,
            row.media_type,
            row.raw_bytes,
            row.sha256,
            now,
            row.unit_or_basis,
            row.import_batch,
            row.ordinal,
            row.supersedes,
        ],
    )?;
    Ok(())
}

pub fn insert_diagnostic(
    tx: &rusqlite::Transaction,
    material_id: &str,
    d: &crate::model::Diagnostic,
) -> anyhow::Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO material_diagnostics
           (material_id,code,message,location,severity)
         VALUES (?1,?2,?3,?4,?5)",
        params![material_id, d.code, d.message, d.location, d.severity],
    )?;
    Ok(())
}

pub fn begin_batch(conn: &mut Connection, batch_id: &str, now: &str) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO import_batches(batch_id,created_at,status,detail) VALUES(?1,?2,'open','')",
        params![batch_id, now],
    )?;
    Ok(())
}

pub fn finish_batch(conn: &mut Connection, batch_id: &str, status: &str, detail: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE import_batches SET status=?2, detail=?3 WHERE batch_id=?1",
        params![batch_id, status, detail],
    )?;
    Ok(())
}

pub fn insert_branch(
    conn: &Connection,
    branch_id: &str,
    parent_branch: Option<&str>,
    fork_seq: Option<i64>,
    name: &str,
    now: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO branches(branch_id,parent_branch,fork_seq,name,created_at)
         VALUES (?1,?2,?3,?4,?5)",
        params![branch_id, parent_branch, fork_seq, name, now],
    )?;
    Ok(conn.execute(
        "INSERT INTO branch_heads(branch_id,head_seq,updated_at) VALUES(?1,0,?2)
         ON CONFLICT(branch_id) DO UPDATE SET updated_at=excluded.updated_at",
        params![branch_id, now],
    )?)
}

/// 追加事件并校验乐观并发基序号。返回 Ok(新 seq)；并发冲突返回 Err("conflict")。
pub fn append_event(
    conn: &mut Connection,
    branch_id: &str,
    event_type: &str,
    payload: &serde_json::Value,
    actor: &str,
    now: &str,
    base_seq: i64,
    event_id: &str,
) -> Result<i64, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let head: i64 = tx
        .query_row(
            "SELECT head_seq FROM branch_heads WHERE branch_id=?1",
            params![branch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if base_seq != head {
        let theirs = head;
        tx.rollback().map_err(|e| e.to_string())?;
        return Err(format!(
            "conflict: base_seq={base_seq} 但分支头为 {theirs}"
        ));
    }
    let seq = head + 1;
    tx.execute(
        "INSERT INTO events(event_id,seq,branch_id,event_type,payload,created_at,actor,base_seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            event_id,
            seq,
            branch_id,
            event_type,
            payload.to_string(),
            now,
            actor,
            base_seq
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE branch_heads SET head_seq=?2, updated_at=?3 WHERE branch_id=?1",
        params![branch_id, seq, now],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(seq)
}

#[derive(Debug, Clone)]
pub struct StoredEvent {
    pub event_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub event_type: String,
    pub payload: String,
    pub created_at: String,
    pub actor: String,
    pub base_seq: Option<i64>,
}

pub fn list_events(conn: &Connection) -> anyhow::Result<Vec<StoredEvent>> {
    let mut stmt = conn.prepare(
        "SELECT event_id,seq,branch_id,event_type,payload,created_at,actor,base_seq
         FROM events ORDER BY created_at, rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(StoredEvent {
            event_id: r.get(0)?,
            seq: r.get(1)?,
            branch_id: r.get(2)?,
            event_type: r.get(3)?,
            payload: r.get(4)?,
            created_at: r.get(5)?,
            actor: r.get(6)?,
            base_seq: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_materials(conn: &Connection) -> anyhow::Result<Vec<(MaterialRow, Vec<crate::model::Diagnostic>)>> {
    let mut stmt = conn.prepare(
        "SELECT material_id,kind,name,media_type,raw_bytes,sha256,unit_or_basis,
                import_batch,ordinal,supersedes
         FROM materials ORDER BY imported_at, ordinal",
    )?;
    let mats: Vec<MaterialRow> = stmt
        .query_map([], |r| {
            Ok(MaterialRow {
                material_id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                media_type: r.get(3)?,
                raw_bytes: r.get(4)?,
                sha256: r.get(5)?,
                unit_or_basis: r.get(6)?,
                import_batch: r.get(7)?,
                ordinal: r.get(8)?,
                supersedes: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for m in mats {
        let mut dstmt = conn.prepare(
            "SELECT code,message,location,severity FROM material_diagnostics WHERE material_id=?1",
        )?;
        let diags: Vec<crate::model::Diagnostic> = dstmt
            .query_map(params![m.material_id], |r| {
                Ok(crate::model::Diagnostic {
                    code: r.get(0)?,
                    message: r.get(1)?,
                    location: r.get(2)?,
                    severity: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push((m, diags));
    }
    Ok(out)
}

pub fn list_branches(
    conn: &Connection,
) -> anyhow::Result<Vec<(String, Option<String>, Option<i64>, String)>> {
    let mut stmt =
        conn.prepare("SELECT branch_id,parent_branch,fork_seq,name FROM branches ORDER BY created_at")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn head_seq(conn: &Connection, branch_id: &str) -> anyhow::Result<i64> {
    Ok(conn.query_row(
        "SELECT head_seq FROM branch_heads WHERE branch_id=?1",
        params![branch_id],
        |r| r.get(0),
    )?)
}

pub fn reset(conn: &mut Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "DELETE FROM material_diagnostics;
         DELETE FROM materials;
         DELETE FROM events;
         DELETE FROM branch_heads;
         DELETE FROM branches;
         DELETE FROM import_batches;
         DELETE FROM conflict_audits;",
    )?;
    Ok(())
}

pub fn record_conflict(
    conn: &mut Connection,
    conflict_id: &str,
    branch_id: &str,
    attempted_by: &str,
    base_seq: i64,
    server_head: i64,
    attempted_event_type: &str,
    attempted_payload: &serde_json::Value,
    now: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO conflict_audits
           (conflict_id,branch_id,attempted_by,base_seq,server_head,
            attempted_event_type,attempted_payload,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            conflict_id,
            branch_id,
            attempted_by,
            base_seq,
            server_head,
            attempted_event_type,
            attempted_payload.to_string(),
            now
        ],
    )?;
    Ok(())
}

pub fn list_conflicts(conn: &Connection) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT conflict_id,branch_id,attempted_by,base_seq,server_head,
                attempted_event_type,attempted_payload,created_at
         FROM conflict_audits ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(serde::json::json!({
            "conflict_id": r.get::<_, String>(0)?,
            "branch_id": r.get::<_, String>(1)?,
            "attempted_by": r.get::<_, String>(2)?,
            "base_seq": r.get::<_, i64>(3)?,
            "server_head": r.get::<_, i64>(4)?,
            "attempted_event_type": r.get::<_, String>(5)?,
            "attempted_payload": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(6)?).unwrap_or(serde_json::Value::Null),
            "created_at": r.get::<_, String>(7)?,
        }))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
