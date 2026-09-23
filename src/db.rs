//! SQLite storage with an append-only event ledger and schema migrations.

use crate::error::{Error, Result};
use rusqlite::Connection;

pub const CURRENT_SCHEMA: u32 = 3;

pub fn open(path: &str) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn schema_version(conn: &Connection) -> Result<u32> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
        [],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(0);
    }
    Ok(conn.query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| {
        r.get::<_, i64>(0)
    })? as u32)
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let v = schema_version(conn)?;
    if v == 0 {
        apply_v1(conn)?;
    }
    let v = schema_version(conn)?;
    if v < 2 {
        apply_v2(conn)?;
    }
    let v = schema_version(conn)?;
    if v < 3 {
        apply_v3(conn)?;
    }
    Ok(())
}

/// Build a connection already fixed at schema version `v` (migration tests).
pub fn open_at_version(v: u32) -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    match v {
        1 => apply_v1(&mut conn)?,
        2 => {
            apply_v1(&mut conn)?;
            apply_v2(&mut conn)?;
        }
        3 => migrate(&mut conn)?,
        other => return Err(Error::Domain(format!("unknown schema version {other}"))),
    }
    Ok(conn)
}

fn apply_v1(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        r#"
        CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);

        -- Original imported bytes, never mutated.
        CREATE TABLE blobs (
            blob_id     TEXT PRIMARY KEY,
            file_name   TEXT NOT NULL,
            bytes       BLOB NOT NULL,
            byte_length INTEGER NOT NULL,
            imported_at INTEGER NOT NULL
        );

        -- Versioned documents: each kind/version row is immutable content.
        CREATE TABLE documents (
            kind        TEXT NOT NULL,
            doc_id      TEXT NOT NULL,
            version     INTEGER NOT NULL,
            cid         TEXT NOT NULL,
            parent_cid  TEXT,
            content     TEXT NOT NULL,
            PRIMARY KEY (kind, doc_id, version)
        );

        -- Append-only event ledger shared by all branches.
        CREATE TABLE branch_events (
            event_id      TEXT PRIMARY KEY,
            seq           INTEGER NOT NULL,
            branch_id     TEXT NOT NULL,
            kind          TEXT NOT NULL,
            basis_json    TEXT NOT NULL,
            ref_blob_ids  TEXT NOT NULL,
            ref_doc_ids   TEXT NOT NULL,
            supersedes    TEXT,
            created_at    INTEGER NOT NULL,
            UNIQUE (branch_id, seq)
        );

        -- Branch tips with optimistic-concurrency revision = event seq.
        CREATE TABLE branches (
            branch_id   TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            parent_id   TEXT,
            forked_from TEXT,
            revision    INTEGER NOT NULL DEFAULT 0,
            open        INTEGER NOT NULL DEFAULT 1,
            created_at  INTEGER NOT NULL
        );

        -- Reviewer decisions (accept/reject/adjust) kept, adopted or not.
        CREATE TABLE decisions (
            decision_id TEXT PRIMARY KEY,
            branch_id   TEXT NOT NULL,
            event_id    TEXT NOT NULL,
            verdict     TEXT NOT NULL,
            target_kind TEXT NOT NULL,
            target_id   TEXT NOT NULL,
            adopted     INTEGER NOT NULL,
            rationale   TEXT NOT NULL
        );

        -- Latest pointer per branch/entity kind/id (tip read model).
        CREATE TABLE node_pointers (
            branch_id TEXT NOT NULL,
            kind      TEXT NOT NULL,
            doc_id    TEXT NOT NULL,
            version   INTEGER NOT NULL,
            cid       TEXT NOT NULL,
            PRIMARY KEY (branch_id, kind, doc_id)
        );

        CREATE TABLE case_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        "#,
    )?;
    tx.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', 1)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn apply_v2(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        r#"
        ALTER TABLE branches ADD COLUMN locked INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE event_attachments (
            event_id TEXT NOT NULL,
            name     TEXT NOT NULL,
            content  TEXT NOT NULL,
            PRIMARY KEY (event_id, name)
        );
        "#,
    )?;
    tx.execute("UPDATE schema_meta SET value=2 WHERE key='version'", [])?;
    tx.commit()?;
    Ok(())
}

fn apply_v3(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(
        r#"
        ALTER TABLE blobs ADD COLUMN sha256 TEXT NOT NULL DEFAULT '';
        CREATE TABLE stale_markers (
            branch_id       TEXT NOT NULL,
            kind            TEXT NOT NULL,
            entity_id       TEXT NOT NULL,
            stale_event_id  TEXT NOT NULL,
            because_event   TEXT NOT NULL,
            reason          TEXT NOT NULL,
            PRIMARY KEY (branch_id, kind, entity_id, stale_event_id)
        );
        "#,
    )?;
    tx.execute("UPDATE schema_meta SET value=3 WHERE key='version'", [])?;
    tx.commit()?;
    Ok(())
}
