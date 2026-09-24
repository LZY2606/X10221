//! SQLite schema + migrations. Migrations are ordered; `user_version` tracks
//! how far a database file has been migrated. State-migration tests create a
//! v1 database, fill it, then migrate to latest and assert data survives.

use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1: core lineage schema
    r#"
    CREATE TABLE events(
        seq INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        branch_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE branches(
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_id TEXT,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE branch_heads(
        branch_id TEXT PRIMARY KEY,
        version INTEGER NOT NULL
    );
    CREATE TABLE branch_head_history(
        branch_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        PRIMARY KEY(branch_id, version)
    );
    CREATE TABLE source_versions(
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        kind TEXT NOT NULL,
        raw_payload TEXT NOT NULL,
        normalized TEXT,
        diagnostics TEXT NOT NULL,
        PRIMARY KEY(id, version)
    );
    CREATE TABLE branch_pins(
        branch_id TEXT NOT NULL,
        source_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        PRIMARY KEY(branch_id, source_id, seq)
    );
    CREATE TABLE derived_states(
        result_id TEXT NOT NULL,
        branch_id TEXT NOT NULL,
        seq INTEGER NOT NULL,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        inputs TEXT NOT NULL,
        value TEXT,
        PRIMARY KEY(result_id, branch_id, seq)
    );
    CREATE TABLE candidates(
        set_id TEXT NOT NULL,
        branch_id TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        kind TEXT NOT NULL,
        context TEXT NOT NULL,
        options TEXT NOT NULL,
        status TEXT NOT NULL,
        decision TEXT,
        decision_seq INTEGER,
        PRIMARY KEY(set_id, branch_id)
    );
    CREATE TABLE candidate_history(
        set_id TEXT NOT NULL,
        branch_id TEXT NOT NULL,
        seq INTEGER NOT NULL,
        status TEXT NOT NULL,
        options TEXT NOT NULL,
        decision TEXT,
        PRIMARY KEY(set_id, branch_id, seq)
    );
    "#,
    // v2: reviewer annotations attached to any stable identity
    r#"
    CREATE TABLE annotations(
        id TEXT PRIMARY KEY,
        branch_id TEXT NOT NULL,
        target_id TEXT NOT NULL,
        body TEXT NOT NULL,
        seq INTEGER NOT NULL
    );
    "#,
];

pub fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn set_user_version(conn: &Connection, v: i64) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {v}"))
}

/// Migrate up to `target` (defaults to the latest migration).
pub fn migrate_to(conn: &Connection, target: Option<usize>) -> rusqlite::Result<()> {
    let target = target.unwrap_or(MIGRATIONS.len());
    let mut v = user_version(conn)? as usize;
    while v < target {
        conn.execute_batch(MIGRATIONS[v])?;
        v += 1;
        set_user_version(conn, v as i64)?;
    }
    Ok(())
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    migrate_to(conn, None)
}

pub const ALL_TABLES: &[&str] = &[
    "events",
    "branches",
    "branch_heads",
    "branch_head_history",
    "source_versions",
    "branch_pins",
    "derived_states",
    "candidates",
    "candidate_history",
    "annotations",
];
