use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 2;

/// v1 初始结构（保留常量以便迁移测试搭建旧库）。
pub const MIGRATE_V1: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS versions(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  line_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  base_id TEXT,
  payload TEXT NOT NULL,
  raw BLOB,
  created_ms INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'fresh',
  UNIQUE(line_id, seq)
);
CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_ms INTEGER NOT NULL
);
PRAGMA user_version = 1;
";

/// v2：引入分支列（同版本并发修改按分支判定）与事件索引。
/// SQLite 不能改约束，重建 versions 表并保留全部旧行。
pub const MIGRATE_V2: &str = "
CREATE TABLE versions_v2(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  line_id TEXT NOT NULL,
  branch TEXT NOT NULL DEFAULT 'main',
  seq INTEGER NOT NULL,
  base_id TEXT,
  payload TEXT NOT NULL,
  raw BLOB,
  created_ms INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'fresh',
  UNIQUE(line_id, branch, seq)
);
INSERT INTO versions_v2(id, kind, line_id, branch, seq, base_id, payload, raw, created_ms, status)
  SELECT id, kind, line_id, 'main', seq, base_id, payload, raw, created_ms, status FROM versions;
DROP TABLE versions;
ALTER TABLE versions_v2 RENAME TO versions;
CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind);
CREATE INDEX IF NOT EXISTS idx_versions_line ON versions(line_id, branch, seq);
PRAGMA user_version = 2;
";

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        conn.execute_batch(MIGRATE_V1)?;
    }
    if v < 2 {
        conn.execute_batch(MIGRATE_V2)?;
    }
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    Ok(())
}
