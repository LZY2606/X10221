use rusqlite::Connection;
use std::path::Path;

pub const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE sources (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        revision INTEGER NOT NULL,
        name TEXT NOT NULL,
        content_kind TEXT NOT NULL,
        raw_json TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE artifacts (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        version INTEGER NOT NULL,
        branch_id TEXT,
        parent_artifact_id TEXT,
        source_id TEXT,
        payload_json TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE events (
        id TEXT PRIMARY KEY,
        seq INTEGER NOT NULL UNIQUE,
        branch_id TEXT,
        event_type TEXT NOT NULL,
        basis TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE branches (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_branch_id TEXT,
        base_event_seq INTEGER NOT NULL,
        head_seq INTEGER NOT NULL,
        position INTEGER NOT NULL
    );
    CREATE TABLE judgments (
        event_id TEXT PRIMARY KEY,
        branch_id TEXT NOT NULL,
        decision TEXT NOT NULL,
        artifact_id TEXT NOT NULL,
        basis_code TEXT NOT NULL,
        rationale TEXT NOT NULL,
        accepted INTEGER NOT NULL
    );
    CREATE TABLE derivations (
        id TEXT PRIMARY KEY,
        branch_id TEXT NOT NULL,
        population_id TEXT NOT NULL,
        gate_id TEXT NOT NULL,
        gate_version INTEGER NOT NULL,
        parent_population_id TEXT,
        parent_gate_version INTEGER,
        transform_refs_json TEXT NOT NULL,
        compensation_id TEXT NOT NULL,
        compensation_version INTEGER NOT NULL,
        source_id TEXT NOT NULL,
        source_revision INTEGER NOT NULL,
        numerator INTEGER NOT NULL,
        denominator INTEGER NOT NULL,
        percent REAL NOT NULL,
        denominator_strategy TEXT NOT NULL,
        denominator_version TEXT NOT NULL,
        event_ids_json TEXT NOT NULL,
        stale_reason TEXT
    );
    CREATE TABLE diagnostics (
        id TEXT PRIMARY KEY,
        branch_id TEXT,
        event_id TEXT NOT NULL,
        code TEXT NOT NULL,
        severity TEXT NOT NULL,
        subject TEXT NOT NULL,
        message TEXT NOT NULL,
        raw_value_json TEXT,
        retained INTEGER NOT NULL
    );
    "#,
    r#"
    ALTER TABLE events ADD COLUMN base_version TEXT;
    ALTER TABLE events ADD COLUMN revision INTEGER;
    ALTER TABLE derivations ADD COLUMN latest_event_seq INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE derivations ADD COLUMN adopted INTEGER NOT NULL DEFAULT 1;
    "#,
];

pub fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let target = MIGRATIONS.len() as i64;
    if current > target {
        return Err(rusqlite::Error::ToSqlConversionFailure(
            format!("database migration {current} is newer than binary {target}").into(),
        ));
    }
    for index in current..target {
        let sql = MIGRATIONS[index as usize];
        conn.execute_batch("BEGIN;")?;
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", index + 1)?;
        conn.execute_batch("COMMIT;")?;
    }
    Ok(())
}

#[cfg(test)]
pub fn force_user_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
    conn.pragma_update(None, "user_version", version)
}
