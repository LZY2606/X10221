//! SQLite storage. All mutations inside review actions run in transactions;
//! a failed import rolls the whole batch back so no half-records remain.
use crate::error::{AppError, AppResult};
use rusqlite::Connection;
use std::sync::Mutex;

pub const MIGRATIONS: &[&str] = &[
    // v1 - core provenance / versioning / audit schema
    r#"
CREATE TABLE artifacts (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    ordinal_sort  INTEGER NOT NULL
);
CREATE TABLE artifact_versions (
    id            TEXT PRIMARY KEY,
    artifact_id   TEXT NOT NULL REFERENCES artifacts(id),
    ordinal       INTEGER NOT NULL,
    content_json  TEXT NOT NULL,
    content_sha   TEXT NOT NULL,
    source_ref    TEXT NOT NULL,
    import_batch  TEXT NOT NULL,
    created_seq   INTEGER NOT NULL,
    UNIQUE(artifact_id, ordinal)
);
CREATE TABLE blobs (
    sha256     TEXT PRIMARY KEY,
    filename   TEXT NOT NULL,
    media_type TEXT NOT NULL,
    bytes      BLOB NOT NULL
);
CREATE TABLE branches (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    parent_id  TEXT,
    fork_seq   INTEGER NOT NULL DEFAULT 0,
    head_seq   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE audit_events (
    id           TEXT PRIMARY KEY,
    branch_id    TEXT NOT NULL REFERENCES branches(id),
    seq          INTEGER NOT NULL,
    kind         TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    reason       TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    UNIQUE(branch_id, seq)
);
CREATE TABLE branch_pins (
    branch_id   TEXT NOT NULL REFERENCES branches(id),
    artifact_id TEXT NOT NULL REFERENCES artifacts(id),
    ordinal     INTEGER NOT NULL,
    PRIMARY KEY (branch_id, artifact_id)
);
CREATE TABLE derived_results (
    id          TEXT PRIMARY KEY,
    branch_id   TEXT NOT NULL REFERENCES branches(id),
    kind        TEXT NOT NULL,
    scope       TEXT NOT NULL,
    inputs_json TEXT NOT NULL,
    data_json   TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_seq INTEGER NOT NULL,
    decided_event_id TEXT,
    UNIQUE(branch_id, id)
);
CREATE TABLE diagnostics (
    id          TEXT PRIMARY KEY,
    batch_id    TEXT NOT NULL,
    event_id    TEXT,
    code        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    message     TEXT NOT NULL,
    context_json TEXT NOT NULL
);
CREATE TABLE import_batches (
    id        TEXT PRIMARY KEY,
    source    TEXT NOT NULL,
    status    TEXT NOT NULL,
    started_at INTEGER NOT NULL
);
    "#,
    // v2 - export bookkeeping (proves state migration works)
    r#"
CREATE TABLE export_bundles (
    id          TEXT PRIMARY KEY,
    created_seq INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    byte_count  INTEGER NOT NULL
);
ALTER TABLE branches ADD COLUMN forked_from_event TEXT;
    "#,
];

#[derive(Debug)]
pub struct Db {
    pub conn: Mutex<Connection>,
}

pub fn open(path: &str) -> AppResult<Db> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(Db { conn: Mutex::new(conn) })
}

pub fn open_in_memory() -> AppResult<Db> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(Db { conn: Mutex::new(conn) })
}

pub fn migrate(conn: &mut Connection) -> AppResult<()> {
    let current: i64 =
        conn.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap_or(0);
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if current < version {
            conn.execute_batch("BEGIN;")?;
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", version)?;
            conn.execute_batch("COMMIT;")?;
        }
    }
    Ok(())
}

/// Apply the *first* migration only, used by the state-migration test to
/// prove a v1 database upgrades cleanly to v2.
pub fn migrate_to_v1_only(conn: &mut Connection) -> AppResult<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(MIGRATIONS[0])?;
    conn.pragma_update(None, "user_version", 1i64)?;
    Ok(())
}

pub fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap_or(0)
}

pub fn conflict(message: impl Into<String>) -> AppError {
    AppError::Conflict(message.into())
}
