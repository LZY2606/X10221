//! SQLite storage and schema migrations.
//!
//! Materials are global and content-addressed; per-branch state lives in the
//! append-only `events` ledger. Derived `results` rows are immutable: an
//! upstream revision appends a `stale` row, it never overwrites the prior
//! payload, so historical conclusions remain readable at any old event.

use rusqlite::{Connection, OptionalExtension};

pub fn open(path: Option<&str>) -> rusqlite::Result<Connection> {
    let mut conn = match path {
        Some(path) => Connection::open(path)?,
        None => Connection::open_in_memory()?,
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
}

fn set_version(conn: &Connection, v: i64) -> rusqlite::Result<()> {
    conn.pragma_update(None, "user_version", v)
}

/// Explicit numbered migrations. The migration tests assert that a database
/// initialized at v1 can be stepped through v2/v3 and that old data survives.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let mut version = user_version(conn)?;

    if version == 0 {
        conn.execute_batch(MIGRATION_001)?;
        version = 1;
        set_version(conn, version)?;
    }
    if version == 1 {
        conn.execute_batch(MIGRATION_002)?;
        version = 2;
        set_version(conn, version)?;
    }
    if version == 2 {
        conn.execute_batch(MIGRATION_003)?;
        version = 3;
        set_version(conn, version)?;
    }
    Ok(())
}

const MIGRATION_001: &str = r#"
CREATE TABLE materials (
    kind       TEXT NOT NULL,
    id         TEXT NOT NULL,
    version    TEXT NOT NULL,
    seq_in_id  INTEGER NOT NULL,
    raw_json   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (kind, id, version)
);

CREATE TABLE branches (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    parent_branch TEXT,
    fork_seq  INTEGER NOT NULL,
    head_seq  INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE events (
    branch     TEXT NOT NULL REFERENCES branches(id),
    seq        INTEGER NOT NULL,
    type       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    timestamp  INTEGER NOT NULL,
    PRIMARY KEY (branch, seq)
);
"#;

const MIGRATION_002: &str = r#"
CREATE TABLE results (
    branch     TEXT NOT NULL,
    scope      TEXT NOT NULL,
    logical_id TEXT NOT NULL,
    event_seq  INTEGER NOT NULL,
    status     TEXT NOT NULL,
    payload    TEXT NOT NULL,
    PRIMARY KEY (branch, scope, logical_id, event_seq)
);
CREATE INDEX idx_results_branch ON results(branch, event_seq);

ALTER TABLE events ADD COLUMN note TEXT;
"#;

const MIGRATION_003: &str = r#"
CREATE TABLE archive_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO archive_meta(key, value) VALUES ('schema_version', '3');
"#;

pub fn latest_schema_version() -> i64 {
    3
}

pub fn materials_lookup(
    conn: &Connection,
    kind: &str,
    id: &str,
    version: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT raw_json FROM materials WHERE kind = ?1 AND id = ?2 AND version = ?3",
        [kind, id, version],
        |r| r.get::<_, String>(0),
    )
    .optional()
}

pub fn all_materials(conn: &Connection) -> rusqlite::Result<Vec<(String, String, String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT kind, id, version, seq_in_id, raw_json FROM materials ORDER BY kind, id, seq_in_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
