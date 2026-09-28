use anyhow::{Context, Result};
use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // v1: initial schema for the flow cytometry lineage store.
    r#"
    CREATE TABLE raw_material (
        id          TEXT PRIMARY KEY,
        kind        TEXT NOT NULL,
        filename    TEXT NOT NULL,
        sha256      TEXT NOT NULL,
        bytes       BLOB NOT NULL,
        imported_at TEXT NOT NULL
    );
    CREATE TABLE material_version (
        id          TEXT PRIMARY KEY,
        raw_id      TEXT NOT NULL REFERENCES raw_material(id),
        kind        TEXT NOT NULL,
        version_no  INTEGER NOT NULL,
        parse_ok    INTEGER NOT NULL,
        diagnostics TEXT NOT NULL DEFAULT '[]',
        repr        TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        UNIQUE(raw_id, version_no)
    );
    CREATE TABLE branch (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        tip_seq    INTEGER NOT NULL DEFAULT 0,
        created_at TEXT NOT NULL
    );
    CREATE TABLE event (
        id          TEXT PRIMARY KEY,
        branch_id   TEXT NOT NULL REFERENCES branch(id),
        seq         INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        actor       TEXT NOT NULL,
        note        TEXT NOT NULL DEFAULT '',
        refs_before TEXT NOT NULL DEFAULT '{}',
        refs_after  TEXT NOT NULL DEFAULT '{}',
        payload     TEXT NOT NULL DEFAULT '{}',
        created_at  TEXT NOT NULL,
        UNIQUE(branch_id, seq)
    );
    CREATE TABLE comp_assessment (
        id               TEXT PRIMARY KEY,
        branch_id        TEXT NOT NULL REFERENCES branch(id),
        spillover_ver    TEXT NOT NULL,
        verdict          TEXT NOT NULL,
        rcond            REAL,
        min_pivot        REAL,
        diagnostics      TEXT NOT NULL DEFAULT '[]'
    );
    CREATE TABLE candidate (
        id             TEXT PRIMARY KEY,
        branch_id      TEXT NOT NULL REFERENCES branch(id),
        kind           TEXT NOT NULL,
        source_ref     TEXT NOT NULL,
        label          TEXT NOT NULL,
        matrix         TEXT,
        quality        TEXT,
        adopted        INTEGER NOT NULL DEFAULT 0,
        rejected       INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE gate_result (
        id                 TEXT PRIMARY KEY,
        branch_id          TEXT NOT NULL REFERENCES branch(id),
        population_id      TEXT NOT NULL,
        gate_ver           TEXT NOT NULL,
        hierarchy_ver      TEXT NOT NULL,
        events_ver         TEXT NOT NULL,
        spillover_ver      TEXT NOT NULL,
        transform_ver      TEXT NOT NULL,
        parent_result_id   TEXT,
        parent_gate_ver    TEXT,
        denom              TEXT NOT NULL,
        member_count       INTEGER NOT NULL,
        parent_count       INTEGER NOT NULL,
        percent            REAL NOT NULL,
        members            TEXT NOT NULL,
        diagnostics        TEXT NOT NULL DEFAULT '[]',
        status             TEXT NOT NULL DEFAULT 'current',
        superseded_by      TEXT
    );
    CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
    "#,
    // v2: review bookkeeping columns (state-migration test fixture).
    r#"
    ALTER TABLE branch ADD COLUMN baseline_seq INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE event ADD COLUMN scenario_step TEXT NOT NULL DEFAULT '';
    "#,
];

pub fn open(path: &str) -> Result<Connection> {
    let conn = Connection::open(path).context("open database")?;
    init(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    init(&mut conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "on")?;
    let current: i64 = conn
        .query_row("SELECT user_version FROM pragma_user_version", [], |r| r.get(0))
        .unwrap_or(0);
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i64;
        if version <= current {
            continue;
        }
        conn.execute_batch(&format!("BEGIN;{sql};\nPRAGMA user_version = {version};\nCOMMIT;"))?;
    }
    Ok(())
}
