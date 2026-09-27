use rusqlite::{Connection, OptionalExtension, Transaction};
use std::path::Path;

pub const CURRENT_SCHEMA_VERSION: i64 = 3;

pub fn open(path: impl AsRef<Path>) -> anyhow_lite::Result<Connection> {
    let mut db = Connection::open(path)?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.pragma_update(None, "busy_timeout", 5000)?;
    migrate(&mut db)?;
    Ok(db)
}

pub fn open_in_memory() -> anyhow_lite::Result<Connection> {
    let mut db = Connection::open_in_memory()?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&mut db)?;
    Ok(db)
}

pub fn migrate(db: &mut Connection) -> anyhow_lite::Result<()> {
    migrate_to(db, CURRENT_SCHEMA_VERSION)
}

pub fn migrate_to(db: &mut Connection, target: i64) -> anyhow_lite::Result<()> {
    db.pragma_update(None, "foreign_keys", "ON")?;
    let tx = db.transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );"
    )?;
    let version: Option<i64> = tx
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
        .optional()?
        .flatten();
    let current = version.unwrap_or(0);

    if target >= 1 && current < 1 {
        apply_v1(&tx)?;
    }
    if target >= 2 && current < 2 {
        apply_v2(&tx)?;
    }
    if target >= 3 && current < 3 {
        apply_v3(&tx)?;
    }
    if current > target {
        anyhow_lite::bail!("refusing to downgrade database schema from {current} to {target}");
    }
    tx.commit()?;
    Ok(())
}

fn apply_v1(tx: &Transaction) -> anyhow_lite::Result<()> {
    tx.execute_batch(
        r#"
        CREATE TABLE branches (
            branch_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            parent_branch_id TEXT REFERENCES branches(branch_id) DEFERRABLE INITIALLY DEFERRED,
            parent_event_id INTEGER,
            created_event_id INTEGER
        );
        CREATE TABLE events (
            event_id INTEGER PRIMARY KEY,
            branch_id TEXT NOT NULL REFERENCES branches(branch_id),
            seq INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            actor TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            summary TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            supersedes_event_id INTEGER REFERENCES events(event_id),
            UNIQUE(branch_id, seq)
        );
        CREATE TABLE materials (
            material_kind TEXT NOT NULL,
            material_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            event_id INTEGER NOT NULL REFERENCES events(event_id),
            branch_id TEXT NOT NULL REFERENCES branches(branch_id),
            raw_bytes BLOB NOT NULL,
            canonical_json TEXT NOT NULL,
            sha256_hex TEXT NOT NULL,
            content_type TEXT NOT NULL,
            filename TEXT NOT NULL,
            PRIMARY KEY(material_kind, material_id, version)
        );
        CREATE TABLE material_refs (
            ref_id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id INTEGER NOT NULL REFERENCES events(event_id),
            material_kind TEXT NOT NULL,
            material_id TEXT NOT NULL,
            version INTEGER NOT NULL,
            role TEXT NOT NULL,
            FOREIGN KEY(material_kind, material_id, version) REFERENCES materials(material_kind, material_id, version)
        );
        CREATE TABLE decisions (
            decision_id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id INTEGER NOT NULL REFERENCES events(event_id),
            branch_id TEXT NOT NULL REFERENCES branches(branch_id),
            candidate_stable_id TEXT NOT NULL,
            decision TEXT NOT NULL CHECK(decision IN ('accept', 'reject', 'undo_accept', 'undo_reject')),
            reason TEXT NOT NULL
        );
        INSERT INTO schema_migrations(version) VALUES (1);
        "#
    )?;
    Ok(())
}

fn apply_v2(tx: &Transaction) -> anyhow_lite::Result<()> {
    tx.execute_batch(
        r#"
        ALTER TABLE branches ADD COLUMN fork_from_event_id INTEGER;
        CREATE TABLE IF NOT EXISTS derived_results (
            result_id INTEGER PRIMARY KEY AUTOINCREMENT,
            stable_id TEXT NOT NULL UNIQUE,
            event_id INTEGER NOT NULL REFERENCES events(event_id),
            branch_id TEXT NOT NULL REFERENCES branches(branch_id),
            object_id TEXT NOT NULL,
            result_kind TEXT NOT NULL,
            status TEXT NOT NULL,
            input_refs_json TEXT NOT NULL,
            value_json TEXT NOT NULL,
            denominator_ref_json TEXT,
            diagnostics_json TEXT NOT NULL DEFAULT '[]'
        );
        CREATE TABLE IF NOT EXISTS invalidations (
            invalidation_id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id INTEGER NOT NULL REFERENCES events(event_id),
            branch_id TEXT NOT NULL REFERENCES branches(branch_id),
            changed_ref TEXT NOT NULL,
            stale_stable_id TEXT NOT NULL,
            invalidation_scope TEXT NOT NULL,
            reason_code TEXT NOT NULL,
            message TEXT NOT NULL
        );
        INSERT INTO schema_migrations(version) VALUES (2);
        "#
    )?;
    Ok(())
}

fn apply_v3(tx: &Transaction) -> anyhow_lite::Result<()> {
    tx.execute_batch(
        r#"
        ALTER TABLE events ADD COLUMN global_seq INTEGER;
        UPDATE events SET global_seq = event_id;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_events_global_seq ON events(global_seq);
        CREATE INDEX IF NOT EXISTS idx_derived_object ON derived_results(branch_id, object_id);
        CREATE INDEX IF NOT EXISTS idx_material_lookup ON materials(material_id, version);
        INSERT INTO schema_migrations(version) VALUES (3);
        "#
    )?;
    Ok(())
}

pub fn schema_version(db: &Connection) -> anyhow_lite::Result<i64> {
    db.query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |row| row.get(0))
        .map_err(Into::into)
}

pub mod anyhow_lite {
    pub use anyhow::{bail, Result};
    pub type Error = anyhow::Error;
}

pub use anyhow;
