//! SQLite 持久化：不可变 blob、追加事件日志、分支头与乐观并发。
//!
//! 导入失败只回滚本次变更（单一事务），不留半份记录。

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::domain::events::EventRecord;

#[derive(Debug)]
pub struct DbError(pub String);
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "db: {}", self.0)
    }
}
impl std::error::Error for DbError {}
impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError(e.to_string())
    }
}

pub fn open(path: &str) -> Result<Connection, DbError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// 内存库（测试用）。
pub fn open_memory() -> Result<Connection, DbError> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn user_version(conn: &Connection) -> Result<i64, DbError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?)
}

/// 迁移：v1 建表，v2 增加分支草案所需列（状态迁移测试覆盖 v1→v2）。
pub fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    let v = user_version(conn)?;
    if v < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            -- 原始字节，只追加：FCS、单染对照、spillover 描述、门控草案、导出包
            CREATE TABLE IF NOT EXISTS blobs (
                blob_id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                filename TEXT NOT NULL,
                bytes BLOB NOT NULL,
                sha256 TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            -- 追加事件日志（每个分支一条哈希链）
            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER NOT NULL,
                branch TEXT NOT NULL,
                event_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                hash TEXT NOT NULL,
                at TEXT NOT NULL,
                payload TEXT NOT NULL,
                judgements TEXT NOT NULL,
                PRIMARY KEY (branch, seq)
            );
            -- 分支头 + 乐观并发版本
            CREATE TABLE IF NOT EXISTS branch_heads (
                branch TEXT PRIMARY KEY,
                head_seq INTEGER NOT NULL,
                head_event_id TEXT NOT NULL,
                head_hash TEXT NOT NULL
            );
            "#,
        )?;
        tx.execute("PRAGMA user_version = 1", [])?;
        tx.commit()?;
    }
    let v = user_version(conn)?;
    if v < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS drafts (
                draft_id TEXT PRIMARY KEY,
                branch TEXT NOT NULL,
                base_seq INTEGER NOT NULL,
                label TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_branch ON events(branch, seq);
            "#,
        )?;
        // 给 v1 已有的事件表补列（若历史版本曾缺少）
        let cols: Vec<String> = tx
            .prepare("PRAGMA table_info(events)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|c| c.ok())
            .collect();
        if !cols.contains(&"actor".to_string()) {
            tx.execute_batch("ALTER TABLE events ADD COLUMN actor TEXT NOT NULL DEFAULT 'reviewer';")?;
        }
        tx.execute("PRAGMA user_version = 2", [])?;
        tx.commit()?;
    }
    Ok(())
}

/// 在事务中追加事件并推进分支头（CAS：expected_head 必须匹配，否则版本冲突）。
pub struct AppendOutcome {
    pub seq: i64,
    pub event_id: String,
    pub hash: String,
}

pub fn append_event(
    conn: &mut Connection,
    branch: &str,
    kind: &str,
    at: &str,
    payload: &Value,
    judgements: &Value,
    expected_head_hash: Option<&str>,
    actor: &str,
    make_hash: impl Fn(i64, &str, &str, &str, &Value) -> String,
) -> Result<AppendOutcome, DbError> {
    let tx = conn.transaction()?;
    let head: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT head_seq, head_event_id, head_hash FROM branch_heads WHERE branch=?1",
            params![branch],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
        )
        .optional()?;

    if let Some(expected) = expected_head_hash {
        let current = head.as_ref().map(|h| h.2.as_str()).unwrap_or("");
        if current != expected {
            return Err(DbError(format!(
                "VERSION_CONFLICT: 分支 {branch} 已从 {expected} 推进到 {current}"
            )));
        }
    }

    let (seq, prev_hash) = match &head {
        Some((s, _, h)) => (s + 1, h.clone()),
        None => (1, String::new()),
    };
    let event_id = format!("e{seq}");
    let hash = make_hash(seq, &event_id, &prev_hash, at, payload);

    tx.execute(
        "INSERT INTO events (seq, branch, event_id, kind, prev_hash, hash, at, payload, judgements, actor)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            seq, branch, event_id, kind, prev_hash, hash, at,
            payload.to_string(), judgements.to_string(), actor
        ],
    )?;
    tx.execute(
        "INSERT INTO branch_heads (branch, head_seq, head_event_id, head_hash)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(branch) DO UPDATE SET head_seq=excluded.head_seq,
             head_event_id=excluded.head_event_id, head_hash=excluded.head_hash",
        params![branch, seq, event_id, hash],
    )?;
    tx.commit()?;
    Ok(AppendOutcome { seq, event_id, hash })
}

pub fn list_events(conn: &Connection, branch: &str) -> Result<Vec<EventRecord>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT seq, event_id, kind, prev_hash, hash, at, payload, judgements
         FROM events WHERE branch=?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch], |r| {
        let payload: String = r.get(6)?;
        let judgements: String = r.get(7)?;
        Ok(EventRecord {
            seq: r.get(0)?,
            event_id: r.get(1)?,
            branch: branch.to_string(),
            kind: r.get(2)?,
            prev_hash: r.get(3)?,
            hash: r.get(4)?,
            at: r.get(5)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            judgements: serde_json::from_str(&judgements).unwrap_or_default(),
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn list_branches(conn: &Connection) -> Result<Vec<(String, i64, String)>, DbError> {
    let mut stmt = conn.prepare("SELECT branch, head_seq, head_hash FROM branch_heads ORDER BY branch")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn head_hash(conn: &Connection, branch: &str) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row("SELECT head_hash FROM branch_heads WHERE branch=?1", params![branch], |r| {
            r.get::<_, String>(0)
        })
        .optional()?)
}

pub fn insert_blob(
    conn: &mut Connection,
    blob_id: &str,
    kind: &str,
    filename: &str,
    bytes: &[u8],
    sha: &str,
    at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO blobs (blob_id, kind, filename, bytes, sha256, created_at)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![blob_id, kind, filename, bytes, sha, at],
    )?;
    Ok(())
}

pub fn get_blob(conn: &Connection, blob_id: &str) -> Result<Option<(String, Vec<u8>)>, DbError> {
    Ok(conn
        .query_row("SELECT filename, bytes FROM blobs WHERE blob_id=?1", params![blob_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .optional()?)
}

pub fn list_blob_meta(conn: &Connection) -> Result<Vec<(String, String, String, usize, String)>, DbError> {
    let mut stmt = conn.prepare("SELECT blob_id, kind, filename, length(bytes), sha256 FROM blobs ORDER BY blob_id")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, usize>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn create_draft(conn: &mut Connection, draft_id: &str, branch: &str, base_seq: i64, label: &str, at: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR REPLACE INTO drafts (draft_id, branch, base_seq, label, created_at) VALUES (?1,?2,?3,?4,?5)",
        params![draft_id, branch, base_seq, label, at],
    )?;
    Ok(())
}

pub fn list_drafts(conn: &Connection) -> Result<Vec<(String, String, i64, String)>, DbError> {
    let mut stmt = conn.prepare("SELECT draft_id, branch, base_seq, label FROM drafts ORDER BY draft_id")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}
