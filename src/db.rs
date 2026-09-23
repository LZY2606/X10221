//! SQLite persistence with explicit, testable schema migrations.

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 3;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_to(conn, SCHEMA_VERSION)
}

/// Apply migrations up to `target`. Exposed so migration tests can stop at
/// an intermediate version, seed old-shaped data, then continue.
pub fn migrate_to(conn: &Connection, target: u32) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );",
    )?;
    let current: u32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    for v in (current + 1)..=target {
        let sql = migration_sql(v);
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
            rusqlite::params![v, chrono::Utc::now().to_rfc3339()],
        )?;
    }
    Ok(())
}

fn migration_sql(version: u32) -> &'static str {
    match version {
        1 => {
            "
            CREATE TABLE sources(
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                raw BLOB NOT NULL,
                sha256 TEXT NOT NULL,
                imported_at TEXT NOT NULL
            );
            CREATE TABLE channel_values(
                source_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                event_index INTEGER NOT NULL,
                channel TEXT NOT NULL,
                raw_value TEXT NOT NULL,
                value REAL,
                unit TEXT NOT NULL,
                basis TEXT NOT NULL,
                diagnostic TEXT,
                PRIMARY KEY(source_id, version, event_index, channel)
            );
            CREATE TABLE spillover_meta(
                id TEXT NOT NULL,
                version INTEGER NOT NULL,
                fluorochrome TEXT NOT NULL,
                detector TEXT NOT NULL,
                coefficient REAL NOT NULL,
                unit TEXT NOT NULL,
                source_ref TEXT,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(id, version, fluorochrome, detector)
            );
            CREATE TABLE comp_matrices(
                id TEXT NOT NULL,
                version INTEGER NOT NULL,
                fluorochromes TEXT NOT NULL,
                detectors TEXT NOT NULL,
                matrix TEXT NOT NULL,
                unit TEXT NOT NULL,
                condition_number REAL,
                status TEXT NOT NULL,
                source_ref TEXT,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(id, version)
            );
            CREATE TABLE transform_params(
                id TEXT NOT NULL,
                version INTEGER NOT NULL,
                channel TEXT NOT NULL,
                kind TEXT NOT NULL,
                params TEXT NOT NULL,
                basis TEXT NOT NULL,
                unit TEXT NOT NULL,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(id, version, channel)
            );
            CREATE TABLE gates(
                id TEXT NOT NULL,
                version INTEGER NOT NULL,
                draft TEXT NOT NULL,
                name TEXT NOT NULL,
                parent_id TEXT,
                parent_version INTEGER,
                x_channel TEXT NOT NULL,
                y_channel TEXT NOT NULL,
                basis TEXT NOT NULL,
                unit TEXT NOT NULL,
                polygon TEXT NOT NULL,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(id, version, draft)
            );
            CREATE TABLE events(
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT NOT NULL,
                branch_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                actor TEXT,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            "
        }
        2 => {
            "
            CREATE TABLE branches(
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_branch TEXT,
                base_event_seq INTEGER NOT NULL DEFAULT 0,
                version INTEGER NOT NULL DEFAULT 0,
                head_adjudication TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE candidates(
                id TEXT PRIMARY KEY,
                branch_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                status TEXT NOT NULL,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE adjudications(
                id TEXT PRIMARY KEY,
                branch_id TEXT NOT NULL,
                candidate_id TEXT,
                decision TEXT NOT NULL,
                params TEXT,
                base_version INTEGER NOT NULL,
                prev_id TEXT,
                undone INTEGER NOT NULL DEFAULT 0,
                actor TEXT,
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            "
        }
        3 => {
            "
            CREATE TABLE derived_results(
                id TEXT PRIMARY KEY,
                branch_id TEXT NOT NULL,
                draft TEXT NOT NULL,
                gate_id TEXT NOT NULL,
                gate_version INTEGER NOT NULL,
                count INTEGER NOT NULL,
                denominator_gate_id TEXT,
                denominator_gate_version INTEGER,
                denominator_count INTEGER NOT NULL,
                percent REAL,
                deps TEXT NOT NULL,
                status TEXT NOT NULL,
                stale_reason TEXT,
                boundary_events TEXT NOT NULL DEFAULT '[]',
                diagnostics TEXT NOT NULL DEFAULT '[]',
                seq INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX idx_derived_results_gate
                ON derived_results(branch_id, draft, gate_id, status);
            "
        }
        _ => panic!("unknown migration version {version}"),
    }
}

pub fn next_event_seq(conn: &Connection) -> Result<i64> {
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM events",
        [],
        |r| r.get(0),
    )?;
    Ok(seq)
}

pub fn emit_event(
    conn: &Connection,
    branch_id: &str,
    kind: &str,
    actor: Option<&str>,
    payload: &serde_json::Value,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO events(id, branch_id, kind, actor, payload, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            format!("ev-{}", uuid::Uuid::new_v4().simple()),
            branch_id,
            kind,
            actor,
            serde_json::to_string(payload)?,
            chrono::Utc::now().to_rfc3339()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}
