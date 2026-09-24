//! SQLite 模式与迁移。user_version 驱动，迁移只增不改。

use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 2;

const MIGRATION_V1: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE counters(name TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE artifacts(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  payload TEXT NOT NULL,
  raw TEXT NOT NULL,
  seq INTEGER NOT NULL,
  UNIQUE(kind, version)
);
CREATE TABLE branches(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  base_seq INTEGER NOT NULL,
  head_seq INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  created_seq INTEGER NOT NULL
);
CREATE TABLE branch_pins(
  branch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  PRIMARY KEY(branch_id, kind)
);
CREATE TABLE results(
  id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  version INTEGER NOT NULL,
  parent_result_id TEXT,
  parent_result_version INTEGER,
  count INTEGER NOT NULL,
  pct REAL,
  pct_denominator_source TEXT,
  status TEXT NOT NULL,
  stale_since_seq INTEGER,
  seq INTEGER NOT NULL,
  UNIQUE(branch_id, gate_id, version)
);
CREATE TABLE candidates(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  options TEXT NOT NULL,
  status TEXT NOT NULL,
  seq INTEGER NOT NULL
);
CREATE TABLE decisions(
  id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  subject TEXT NOT NULL,
  action TEXT NOT NULL,
  params TEXT NOT NULL,
  expected_head INTEGER NOT NULL,
  undone_seq INTEGER,
  seq INTEGER NOT NULL
);
CREATE TABLE conflicts(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  base_version INTEGER NOT NULL,
  ours TEXT NOT NULL,
  theirs TEXT NOT NULL,
  status TEXT NOT NULL,
  seq INTEGER NOT NULL
);
"#;

const MIGRATION_V2: &str = r#"
CREATE TABLE diagnostics(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  severity TEXT NOT NULL,
  message TEXT NOT NULL,
  detail TEXT NOT NULL,
  seq INTEGER NOT NULL
);
"#;

/// 迁移到指定版本（含）。返回应用了哪些迁移。
pub fn migrate_to(conn: &Connection, target: i64) -> rusqlite::Result<Vec<i64>> {
    let mut applied = Vec::new();
    let mut v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    while v < target {
        v += 1;
        match v {
            1 => conn.execute_batch(MIGRATION_V1)?,
            2 => conn.execute_batch(MIGRATION_V2)?,
            _ => unreachable!(),
        }
        conn.pragma_update(None, "user_version", v)?;
        applied.push(v);
    }
    Ok(applied)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<Vec<i64>> {
    migrate_to(conn, SCHEMA_VERSION)
}
