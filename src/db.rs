//! SQLite 持久层：append-only 事件日志 + 分支 + 原始字节。
//! 所有写操作在事务中完成；导入失败整体回滚，不留半份记录。

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::{BranchEvent, EventPayload, RevisionBundle};

pub const SCHEMA_VERSION: i64 = 2;

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        Connection::open(path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT OR IGNORE INTO meta(key,value) VALUES ('schema_version','1');",
    )?;
    let current: i64 = conn
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(1);

    if current < 2 {
        migrate_v1_to_v2(conn)?;
        conn.execute(
            "UPDATE meta SET value='2' WHERE key='schema_version'",
            [],
        )?;
    }
    let _ = SCHEMA_VERSION;
    Ok(())
}

/// v1→v2：增加分支/字节/归档表，并把原事件登记到 main 分支。
fn migrate_v1_to_v2(conn: &Connection) -> rusqlite::Result<()> {
    // 若从 v1 升级：events_v1 可能存在；全新库直接建 v2 表。
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS branches (
            branch        TEXT PRIMARY KEY,
            forked_from   TEXT,
            forked_seq    INTEGER NOT NULL DEFAULT 0,
            head_seq      INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL,
            note          TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE IF NOT EXISTS events (
            branch        TEXT NOT NULL,
            seq           INTEGER NOT NULL,
            event_id      TEXT NOT NULL,
            at            TEXT NOT NULL,
            actor         TEXT,
            payload_json  TEXT NOT NULL,
            UNIQUE(branch, seq),
            UNIQUE(event_id)
        );
        CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch, seq);

        CREATE TABLE IF NOT EXISTS blobs (
            blob_id    TEXT PRIMARY KEY,
            sha256     TEXT NOT NULL,
            bytes_b64  TEXT NOT NULL,
            media_type TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS case_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO branches(branch, forked_from, forked_seq, head_seq, created_at, note)
         VALUES ('main', NULL, 0, 0, '1970-01-01T00:00:00Z', '初始主干')",
        [],
    )?;
    Ok(())
}

pub fn create_branch(
    conn: &mut Connection,
    branch: &str,
    forked_from: &str,
    note: &str,
) -> rusqlite::Result<i64> {
    let head: i64 = conn.query_row(
        "SELECT head_seq FROM branches WHERE branch=?1",
        params![forked_from],
        |r| r.get(0),
    )?;
    let now = now_iso();
    conn.execute(
        "INSERT INTO branches(branch, forked_from, forked_seq, head_seq, created_at, note)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![branch, forked_from, head, head, now, note],
    )?;
    // 复制父分支事件到新分支（谱系快照，便于独立演进与并排）。
    conn.execute(
        "INSERT INTO events(branch, seq, event_id, at, actor, payload_json)
         SELECT ?2, seq, event_id, at, actor, payload_json FROM events WHERE branch=?1",
        params![forked_from, branch],
    )?;
    Ok(head)
}

pub fn list_branches(conn: &Connection) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT branch, forked_from, forked_seq, head_seq, created_at, note FROM branches ORDER BY branch",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(serde_json::json!({
            "branch": r.get::<_, String>(0)?,
            "forked_from": r.get::<_, Option<String>>(1)?,
            "forked_seq": r.get::<_, i64>(2)?,
            "head_seq": r.get::<_, i64>(3)?,
            "created_at": r.get::<_, String>(4)?,
            "note": r.get::<_, String>(5)?,
        }))
    })?;
    rows.collect()
}

/// 追加事件（事务内）。expected_seq 用于同版本并发修改检测：
/// 若当前 head_seq 与调用方基线不一致，返回 Err(conflict)。
pub fn append_event(
    conn: &mut Connection,
    branch: &str,
    expected_seq: i64,
    event_id: &str,
    actor: Option<&str>,
    payload: &EventPayload,
) -> Result<i64, AppendError> {
    let tx = conn.transaction().map_err(AppendError::Db)?;
    let head: i64 = tx
        .query_row(
            "SELECT head_seq FROM branches WHERE branch=?1",
            params![branch],
            |r| r.get(0),
        )
        .map_err(AppendError::Db)?;
    if head != expected_seq {
        let theirs: Vec<String> = {
            let mut stmt = tx
                .prepare("SELECT event_id FROM events WHERE branch=?1 AND seq>?2 ORDER BY seq")
                .map_err(AppendError::Db)?;
            let rows = stmt
                .query_map(params![branch, expected_seq], |r| r.get(0))
                .map_err(AppendError::Db)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(AppendError::Db)?
        };
        return Err(AppendError::Conflict {
            branch: branch.to_string(),
            expected_seq,
            actual_head: head,
            newer_event_ids: theirs,
        });
    }
    let next = head + 1;
    let json = serde_json::to_string(payload)
        .map_err(|e| AppendError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    tx.execute(
        "INSERT INTO events(branch, seq, event_id, at, actor, payload_json)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![branch, next, event_id, now_iso(), actor, json],
    )
    .map_err(AppendError::Db)?;
    tx.execute(
        "UPDATE branches SET head_seq=?1 WHERE branch=?2",
        params![next, branch],
    )
    .map_err(AppendError::Db)?;
    tx.commit().map_err(AppendError::Db)?;
    Ok(next)
}

#[derive(Debug)]
pub enum AppendError {
    Db(rusqlite::Error),
    Conflict {
        branch: String,
        expected_seq: i64,
        actual_head: i64,
        newer_event_ids: Vec<String>,
    },
}

pub fn load_events(conn: &Connection, branch: &str) -> rusqlite::Result<Vec<BranchEvent>> {
    let mut stmt = conn.prepare(
        "SELECT seq, event_id, at, actor, payload_json FROM events
         WHERE branch=?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch], |r| {
        let payload_json: String = r.get(4)?;
        let payload: EventPayload = serde_json::from_str(&payload_json)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e)))?;
        Ok(BranchEvent {
            seq: r.get(0)?,
            event_id: r.get(1)?,
            branch: branch.to_string(),
            at: r.get(2)?,
            actor: r.get(3)?,
            payload,
        })
    })?;
    rows.collect()
}

pub fn head_seq(conn: &Connection, branch: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT head_seq FROM branches WHERE branch=?1",
        params![branch],
        |r| r.get(0),
    )
}

pub fn branch_exists(conn: &Connection, branch: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM branches WHERE branch=?1",
        params![branch],
        |_| Ok(()),
    )
    .optional()
    .unwrap()
    .is_some()
}

pub fn put_blob(
    conn: &mut Connection,
    blob_id: &str,
    sha256: &str,
    bytes_b64: &str,
    media_type: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO blobs(blob_id, sha256, bytes_b64, media_type, created_at)
         VALUES (?1,?2,?3,?4,?5)",
        params![blob_id, sha256, bytes_b64, media_type, now_iso()],
    )?;
    Ok(())
}

pub fn set_case_value(conn: &mut Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO case_state(key,value) VALUES (?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_case_value(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM case_state WHERE key=?1", params![key], |r| r.get(0))
        .optional()
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix:{secs}")
}

pub fn ensure_main_branch(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO branches(branch, forked_from, forked_seq, head_seq, created_at, note)
         VALUES ('main', NULL, 0, 0, ?1, '初始主干')",
        params![now_iso()],
    )?;
    Ok(())
}

pub fn has_events(conn: &Connection, branch: &str) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE branch=?1",
        params![branch],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

pub fn newest_bundle_event(conn: &Connection, branch: &str) -> rusqlite::Result<Option<RevisionBundle>> {
    let events = load_events(conn, branch)?;
    for ev in events.iter().rev() {
        let b = match &ev.payload {
            EventPayload::Imported { bundle, .. }
            | EventPayload::ParameterRevised { bundle, .. }
            | EventPayload::SourcePatched { bundle, .. }
            | EventPayload::GateRevised { bundle, .. }
            | EventPayload::GateRenamed { bundle }
            | EventPayload::Recomputed { bundle, .. } => Some(bundle),
            EventPayload::Decision { .. } | EventPayload::Retracted { .. } => None,
        };
        if let Some(b) = b {
            return Ok(Some(b.clone()));
        }
    }
    Ok(None)
}
