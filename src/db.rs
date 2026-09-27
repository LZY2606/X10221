//! SQLite schema 与状态迁移。user_version 记录迁移进度。
use rusqlite::Connection;

pub const MIGRATION_V1: &str = r#"
CREATE TABLE source_materials(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  version INTEGER NOT NULL,
  unit TEXT,
  coord TEXT,
  payload TEXT NOT NULL,
  created_seq INTEGER NOT NULL DEFAULT 0,
  supersedes TEXT
);
CREATE UNIQUE INDEX idx_sm_knv ON source_materials(kind,name,version);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT UNIQUE,
  ts_ms INTEGER NOT NULL,
  branch TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  basis TEXT NOT NULL,
  inputs TEXT NOT NULL
);
CREATE TABLE derived_results(
  id TEXT PRIMARY KEY,
  branch TEXT NOT NULL,
  kind TEXT NOT NULL,
  population TEXT,
  version INTEGER NOT NULL,
  deps TEXT NOT NULL,
  denominator TEXT,
  value TEXT NOT NULL,
  status TEXT NOT NULL,
  created_seq INTEGER NOT NULL
);
"#;

pub const MIGRATION_V2: &str = r#"
CREATE TABLE branches(
  name TEXT PRIMARY KEY,
  parent TEXT,
  base_seq INTEGER NOT NULL,
  head_seq INTEGER NOT NULL
);
CREATE TABLE decisions(
  id TEXT PRIMARY KEY,
  branch TEXT NOT NULL,
  target_event TEXT,
  action TEXT NOT NULL,
  candidate TEXT,
  params TEXT,
  prev_head_seq INTEGER NOT NULL,
  event_seq INTEGER NOT NULL
);
INSERT OR IGNORE INTO branches(name,parent,base_seq,head_seq) VALUES('main',NULL,0,0);
"#;

pub const MIGRATION_V3: &str = r#"
CREATE TABLE conflicts(
  id TEXT PRIMARY KEY,
  branch TEXT NOT NULL,
  base_version INTEGER NOT NULL,
  ours TEXT NOT NULL,
  theirs TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  event_seq INTEGER NOT NULL
);
ALTER TABLE decisions ADD COLUMN undone_by TEXT;
"#;

pub const MIGRATIONS: &[&str] = &[MIGRATION_V1, MIGRATION_V2, MIGRATION_V3];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        if (i as i64) >= v {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", (i as i64) + 1)?;
        }
    }
    Ok(())
}

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    migrate(&conn)?;
    Ok(conn)
}
