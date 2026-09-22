//! SQLite schema and migrations.
//! Every migration is a numbered, append-only SQL unit; migration tests drive
//! a v1 database through `migrate` and assert data survival.

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1 — versioned objects, append-only events, raw blobs, draft branches.
    r#"
    CREATE TABLE objects (
        id          TEXT NOT NULL,
        version     INTEGER NOT NULL,
        branch      TEXT NOT NULL,
        kind        TEXT NOT NULL,
        status      TEXT NOT NULL DEFAULT 'current',
        payload     TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY (id, version, branch)
    );
    CREATE INDEX idx_objects_branch_kind ON objects(branch, kind);
    CREATE TABLE events (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        id      TEXT NOT NULL,
        ts      TEXT NOT NULL,
        branch  TEXT NOT NULL,
        kind    TEXT NOT NULL,
        payload TEXT NOT NULL
    );
    CREATE INDEX idx_events_branch ON events(branch, seq);
    CREATE TABLE blobs (
        id       TEXT PRIMARY KEY,
        filename TEXT NOT NULL,
        checksum TEXT NOT NULL,
        bytes    BLOB NOT NULL
    );
    CREATE TABLE branches (
        name      TEXT PRIMARY KEY,
        head_seq  INTEGER NOT NULL DEFAULT 0,
        fork_from TEXT,
        fork_seq  INTEGER
    );
    "#,
    // v2 — structured diagnostics attached to (object version).
    r#"
    CREATE TABLE diagnostics (
        id             TEXT PRIMARY KEY,
        branch         TEXT NOT NULL,
        object_id      TEXT NOT NULL,
        object_version INTEGER NOT NULL,
        event_seq      INTEGER,
        severity       TEXT NOT NULL,
        message        TEXT NOT NULL,
        payload        TEXT NOT NULL
    );
    CREATE INDEX idx_diag_obj ON diagnostics(branch, object_id, object_version);
    ALTER TABLE objects ADD COLUMN note TEXT NOT NULL DEFAULT '';
    "#,
];

pub fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())?;
    let applied: i64 = conn
        .query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let v = (idx + 1) as i64;
        if v <= applied {
            continue;
        }
        conn.execute_batch(sql).map_err(|e| format!("migration {v} failed: {e}"))?;
        conn.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![v, "applied"],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Used only by the migration test to build a legacy v1 database.
#[doc(hidden)]
pub fn apply_v1_only(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())?;
    conn.execute_batch(MIGRATIONS[0]).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (1, 'legacy')",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
