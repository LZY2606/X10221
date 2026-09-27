//! SQLite 存储。append-only 事件日志 + 分支头指针；结果行可被标记 stale 但数值不覆盖。
use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

pub struct Db {
    pub conn: Arc<Mutex<Connection>>,
}

const V1: &str = r#"
CREATE TABLE meta(
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE materials(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  source_uri TEXT NOT NULL,
  source_sha256 TEXT NOT NULL,
  raw_repr TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  superseded_by INTEGER,
  PRIMARY KEY(id, version)
);
CREATE TABLE branches(
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  parent_branch TEXT,
  head_event_seq INTEGER,
  created_at INTEGER NOT NULL
);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  base_seq INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
CREATE TABLE artifacts(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  material_ref TEXT NOT NULL,
  content_json TEXT NOT NULL,
  source_event_seq INTEGER NOT NULL,
  superseded_by INTEGER,
  PRIMARY KEY(id, version, branch_id)
);
CREATE TABLE results(
  id TEXT NOT NULL,
  version INTEGER NOT NULL,
  branch_id TEXT NOT NULL,
  gate_stable_id TEXT NOT NULL,
  candidate_label TEXT NOT NULL,
  parent_result_ref TEXT,
  numerator_count INTEGER NOT NULL,
  denominator_count INTEGER,
  percent REAL,
  member_ids_json TEXT NOT NULL,
  deps_json TEXT NOT NULL,
  source_event_seq INTEGER NOT NULL,
  status TEXT NOT NULL,
  stale_reason TEXT,
  PRIMARY KEY(id, version, branch_id)
);
CREATE INDEX idx_results_branch ON results(branch_id, gate_stable_id, version);
"#;

/// v2：决策/裁定分支表（状态迁移测试覆盖 v1->v2）
const V2: &str = r#"
CREATE TABLE decisions(
  event_seq INTEGER PRIMARY KEY,
  branch_id TEXT NOT NULL,
  verdict TEXT NOT NULL,
  target_result_ref TEXT,
  chosen_candidate TEXT,
  note TEXT,
  reversed_at INTEGER
);
CREATE INDEX idx_decisions_branch ON decisions(branch_id);
"#;

pub fn open(path: &str) -> AppResult<Db> {
    let conn = if path == ":memory:" {
        Connection::open_in_memory()?
    } else {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Connection::open(path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 10_000i64)?;
    let db = Db {
        conn: Arc::new(Mutex::new(conn)),
    };
    db.migrate()?;
    Ok(db)
}

impl Db {
    pub fn migrate(&self) -> AppResult<()> {
        let mut c = self.conn.lock().unwrap();
        let version: i64 =
            c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        let tx = c.transaction()?;
        let mut v = version as u32;
        if v == 0 {
            tx.execute_batch(V1)?;
            v = 1;
            tx.pragma_update(None, "user_version", 1)?;
        }
        if v == 1 {
            tx.execute_batch(V2)?;
            v = 2;
            tx.pragma_update(None, "user_version", 2)?;
        }
        if v != CURRENT_SCHEMA_VERSION {
            return Err(AppError::BadInput(format!(
                "未知数据库 schema 版本 {v}，本程序为 {CURRENT_SCHEMA_VERSION}"
            )));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn user_version(&self) -> AppResult<u32> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32)
    }
}

/// 迁移测试用：直接在给定连接上把 v1 schema 落地。
pub fn fresh_v1(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(V1)?;
    conn.pragma_update(None, "user_version", 1)?;
    Ok(())
}
