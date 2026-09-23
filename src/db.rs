//! SQLite 模式与状态迁移。
//! 迁移按版本号递增应用，`schema_migrations` 记录已应用版本，支持迁移测试逐版本重放。

use rusqlite::Connection;

pub const LATEST: i64 = 3;

pub fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

const M1: &str = r#"
CREATE TABLE datasets(
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  source TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE docs(
  dataset_id TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  PRIMARY KEY(dataset_id, doc_id)
);
CREATE TABLE doc_versions(
  dataset_id TEXT NOT NULL,
  doc_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  payload TEXT NOT NULL,
  rationale TEXT NOT NULL DEFAULT '',
  created_seq INTEGER NOT NULL,
  superseded_seq INTEGER,
  PRIMARY KEY(dataset_id, doc_id, branch_id, version)
);
"#;

const M2: &str = r#"
CREATE TABLE events(
  dataset_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(dataset_id, seq)
);
CREATE TABLE branches(
  dataset_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  name TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(dataset_id, branch_id)
);
CREATE TABLE populations(
  dataset_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_gate_id TEXT,
  parent_version INTEGER,
  comp_version INTEGER,
  transform_version INTEGER,
  boundary_policy TEXT NOT NULL DEFAULT 'pending',
  member_event_ids TEXT NOT NULL,
  boundary_event_ids TEXT NOT NULL,
  count INTEGER NOT NULL,
  denominator INTEGER NOT NULL,
  pct REAL,
  status TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  stale_since INTEGER,
  superseded_seq INTEGER,
  PRIMARY KEY(dataset_id, branch_id, gate_id, gate_version, created_seq)
);
"#;

const M3: &str = r#"
CREATE TABLE diagnostics(
  dataset_id TEXT NOT NULL,
  diag_id TEXT NOT NULL,
  subject TEXT NOT NULL,
  severity TEXT NOT NULL,
  message TEXT NOT NULL,
  data TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY(dataset_id, diag_id)
);
CREATE TABLE candidates(
  dataset_id TEXT NOT NULL,
  group_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,
  options TEXT NOT NULL,
  status TEXT NOT NULL,
  decision TEXT,
  basis TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  decided_seq INTEGER,
  PRIMARY KEY(dataset_id, group_id, branch_id)
);
"#;

pub fn migrate_to(conn: &Connection, target: i64) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY,
           applied_at TEXT NOT NULL
         );",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for v in (current + 1)..=target {
        match v {
            1 => conn.execute_batch(M1)?,
            2 => conn.execute_batch(M2)?,
            3 => conn.execute_batch(M3)?,
            _ => panic!("unknown migration version {v}"),
        }
        conn.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![v, now()],
        )?;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, LATEST)
}
