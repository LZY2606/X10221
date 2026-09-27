//! SQLite 存储：迁移 + 追加式表结构。除分支头指针外一切只增不改。

use anyhow::Result;
use rusqlite::Connection;

pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "core",
        r#"
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  branch_id TEXT NOT NULL DEFAULT 'main',
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE artifacts(
  artifact_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  base_version INTEGER,
  payload TEXT NOT NULL,
  branch_id TEXT NOT NULL DEFAULT 'main',
  source_event_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(artifact_id, version, branch_id)
);
CREATE TABLE populations(
  population_id TEXT NOT NULL,
  gate_artifact_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_population_id TEXT,
  parent_population_version INTEGER,
  count INTEGER NOT NULL,
  denominator_count INTEGER,
  percent REAL,
  denominator_source TEXT,
  branch_id TEXT NOT NULL DEFAULT 'main',
  source_event_seq INTEGER NOT NULL,
  PRIMARY KEY(population_id, gate_version, branch_id)
);
CREATE TABLE candidates(
  candidate_id TEXT NOT NULL,
  subject_kind TEXT NOT NULL,
  subject_id TEXT NOT NULL,
  option_json TEXT NOT NULL,
  branch_id TEXT NOT NULL DEFAULT 'main',
  source_event_seq INTEGER NOT NULL,
  PRIMARY KEY(candidate_id, branch_id)
);
CREATE TABLE marks(
  mark_id INTEGER PRIMARY KEY AUTOINCREMENT,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  mark TEXT NOT NULL,
  detail TEXT NOT NULL,
  source_event_seq INTEGER NOT NULL
);
CREATE TABLE conflicts(
  conflict_id TEXT NOT NULL PRIMARY KEY,
  artifact_id TEXT NOT NULL,
  base_version INTEGER NOT NULL,
  seen_version INTEGER NOT NULL,
  incoming_json TEXT NOT NULL,
  current_json TEXT NOT NULL,
  diff_json TEXT NOT NULL,
  resolved INTEGER NOT NULL DEFAULT 0,
  source_event_seq INTEGER NOT NULL
);
CREATE TABLE branches(
  branch_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  base_event_seq INTEGER NOT NULL,
  head_seq INTEGER NOT NULL,
  created_event_seq INTEGER NOT NULL
);
CREATE TABLE rulings(
  ruling_id TEXT NOT NULL PRIMARY KEY,
  candidate_id TEXT,
  decision TEXT NOT NULL,
  rationale TEXT NOT NULL,
  params_json TEXT,
  reverted INTEGER NOT NULL DEFAULT 0,
  branch_id TEXT NOT NULL DEFAULT 'main',
  source_event_seq INTEGER NOT NULL
);
"#,
    ),
    (
        "diagnostics",
        r#"
CREATE TABLE diagnostics(
  diag_id INTEGER PRIMARY KEY AUTOINCREMENT,
  subject TEXT NOT NULL,
  severity TEXT NOT NULL,
  message TEXT NOT NULL,
  raw_value TEXT,
  source_event_seq INTEGER NOT NULL
);
CREATE INDEX idx_marks_target ON marks(target_kind, target_id);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
"#,
    ),
];

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY,
           name TEXT NOT NULL,
           applied_at TEXT NOT NULL
         );",
    )?;
    let applied: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version > applied {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version,name,applied_at) VALUES(?1,?2,datetime('now'))",
                rusqlite::params![version, name],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}
