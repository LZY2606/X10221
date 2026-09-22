//! SQLite storage. Source material and interpretations are stored separately:
//! raw channel values, spillover metadata, compensation matrices, transform
//! parameters, gate polygons and parent/child relations are each versioned
//! independently. Nothing is ever overwritten; status changes are logged so
//! any historical event can be re-entered with its dependency versions.

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MIGRATION_1: &str = r#"
CREATE TABLE materials(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  raw_bytes BLOB NOT NULL,
  sha256 TEXT NOT NULL,
  imported_at TEXT NOT NULL
);
CREATE TABLE source_versions(
  id TEXT PRIMARY KEY,
  material_id TEXT NOT NULL REFERENCES materials(id),
  kind TEXT NOT NULL,
  version_no INTEGER NOT NULL,
  payload TEXT NOT NULL,
  basis TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(material_id, kind, version_no)
);
CREATE TABLE diagnostics(
  id TEXT PRIMARY KEY,
  source_version_id TEXT,
  severity TEXT NOT NULL,
  code TEXT NOT NULL,
  message TEXT NOT NULL,
  subject TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE branches(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  parent_branch_id TEXT,
  created_by_event INTEGER,
  created_at TEXT NOT NULL
);
CREATE TABLE candidates(
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  options TEXT NOT NULL,
  status TEXT NOT NULL,
  branch_id TEXT,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE populations(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  gate_id TEXT NOT NULL,
  gate_version_id TEXT NOT NULL,
  parent_population_id TEXT,
  parent_version_id TEXT,
  comp_matrix_version_id TEXT,
  transform_version_id TEXT,
  raw_channels_version_id TEXT NOT NULL,
  count INTEGER NOT NULL,
  total INTEGER NOT NULL,
  percent REAL NOT NULL,
  status TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE population_status_history(
  population_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  status TEXT NOT NULL
);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE drafts(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  base_version INTEGER NOT NULL,
  payload TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
"#;

/// v2 adds a stable display order so exported archives preserve ordering
/// independent of insertion happenstance.
pub const MIGRATION_2: &str = r#"
ALTER TABLE source_versions ADD COLUMN display_order INTEGER NOT NULL DEFAULT 0;
"#;

pub const MIGRATIONS: &[&str] = &[MIGRATION_1, MIGRATION_2];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn migrate_to(conn: &Connection, target: usize) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
           version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let v = (idx + 1) as i64;
        if v > current && idx < target {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(?1,?2)",
                params![v, now()],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

pub fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

pub fn new_id(prefix: &str, seed: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update(nanos.to_string().as_bytes());
    let hex = format!("{:x}", h.finalize());
    format!("{}-{}", prefix, &hex[..16])
}

pub fn content_id(prefix: &str, bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let hex = format!("{:x}", h.finalize());
    format!("{}-{}", prefix, &hex[..16])
}

/// Append a machine-readable event; returns its sequence number, which also
/// serves as the global version counter for optimistic concurrency.
pub fn record_event(
    conn: &Connection,
    kind: &str,
    payload: &serde_json::Value,
) -> rusqlite::Result<i64> {
    let id = new_id("evt", &format!("{kind}:{payload}"));
    conn.execute(
        "INSERT INTO events(id,kind,payload,created_at) VALUES(?1,?2,?3,?4)",
        params![id, kind, payload.to_string(), now()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
}
