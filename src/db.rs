use rusqlite::Connection;

pub const MIGRATION_V1: &str = r#"
CREATE TABLE entities(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  created_event INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE versions(
  id TEXT PRIMARY KEY,
  entity_id TEXT NOT NULL REFERENCES entities(id),
  seq INTEGER NOT NULL,
  unit TEXT NOT NULL DEFAULT '',
  basis TEXT NOT NULL DEFAULT '',
  payload TEXT NOT NULL,
  created_event INTEGER NOT NULL DEFAULT 0,
  UNIQUE(entity_id, seq)
);
CREATE TABLE events(
  seq INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  branch_id TEXT,
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE result_deps(
  result_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  role TEXT NOT NULL,
  PRIMARY KEY(result_id, version_id, role)
);
CREATE TABLE results(
  id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,
  status TEXT NOT NULL,
  denominator_version TEXT,
  payload TEXT NOT NULL,
  created_event INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE branches(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  parent_id TEXT,
  created_event INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE decisions(
  id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  target_id TEXT NOT NULL,
  action TEXT NOT NULL,
  payload TEXT NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0,
  created_event INTEGER NOT NULL DEFAULT 0
);
"#;

pub const MIGRATION_V2: &str = r#"
ALTER TABLE versions ADD COLUMN source_hash TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_versions_entity ON versions(entity_id, seq);
CREATE INDEX idx_results_branch ON results(branch_id, kind, subject);
"#;

pub const MIGRATIONS: &[&str] = &[MIGRATION_V1, MIGRATION_V2];

pub fn migrate_up_to(conn: &Connection, target: usize) -> Result<(), String> {
    let current: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let current = current as usize;
    if current > target {
        return Err(format!("database at version {current}, cannot downgrade to {target}"));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i + 1;
        if v > current && v <= target {
            conn.execute_batch(sql).map_err(|e| format!("migration v{v}: {e}"))?;
            conn.pragma_update(None, "user_version", v as i64).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> Result<(), String> {
    migrate_up_to(conn, MIGRATIONS.len())
}
