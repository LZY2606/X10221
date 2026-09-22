//! SQLite schema migration.
//!
//! Migrations are applied strictly in order and recorded with
//! `PRAGMA user_version`, so an existing database is upgraded in place while
//! preserving every previously written row (state-migration tests assert
//! exactly that).

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1: interpretation branches, separately versioned artifacts, event log.
    r#"
CREATE TABLE branches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id TEXT,
    base_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE artifacts (
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    branch_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    raw_repr TEXT,
    supersedes INTEGER,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY (id, version, branch_id)
);
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_id TEXT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
CREATE INDEX idx_artifacts_lookup ON artifacts(branch_id, kind, id, version);
"#,
    // v2: derived results (never overwritten) and adjudication candidates.
    r#"
CREATE TABLE results (
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    branch_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    deps TEXT NOT NULL,
    status TEXT NOT NULL,
    value TEXT,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY (id, version, branch_id)
);
CREATE TABLE candidates (
    id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    options TEXT NOT NULL,
    status TEXT NOT NULL,
    resolution TEXT,
    created_seq INTEGER NOT NULL,
    resolved_seq INTEGER
);
CREATE INDEX idx_results_lookup ON results(branch_id, kind, id, version);
CREATE INDEX idx_candidates_branch ON candidates(branch_id, status);
"#,
    // v3: human rulings (undoable), diagnostics, artifact->ruling provenance.
    r#"
CREATE TABLE rulings (
    id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    undone INTEGER NOT NULL DEFAULT 0,
    created_seq INTEGER NOT NULL
);
CREATE TABLE diagnostics (
    id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_seq INTEGER NOT NULL
);
ALTER TABLE artifacts ADD COLUMN created_by_ruling TEXT;
CREATE INDEX idx_rulings_branch ON rulings(branch_id);
CREATE INDEX idx_diagnostics_branch ON diagnostics(branch_id);
"#,
];

pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 1;
        if current < v {
            conn.execute_batch(sql)?;
            conn.execute_batch(&format!("PRAGMA user_version = {v}"))?;
        }
    }
    Ok(())
}
