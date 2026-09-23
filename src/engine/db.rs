//! SQLite persistence. The event log is append-only; mutable state is confined
//! to per-branch heads and the candidate workspace. Migrations run once at open.

use rusqlite::{params, Connection};
use std::sync::Mutex;

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

pub struct Db(pub Mutex<Connection>);

pub fn open(path: &str) -> Result<Db, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    let db = Db(Mutex::new(conn));
    db.migrate()?;
    Ok(db)
}

pub fn open_in_memory() -> Result<Db, String> {
    let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    let db = Db(Mutex::new(conn));
    db.migrate()?;
    Ok(db)
}

impl Db {
    fn migrate(&self) -> Result<(), String> {
        let conn = &mut *self.0.lock().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta(
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        let v: Option<String> = conn
            .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| {
                r.get(0)
            })
            .ok();
        let version: u32 = v.and_then(|s| s.parse().ok()).unwrap_or(0);
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)
                .map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO schema_meta(key,value) VALUES('version','1')
                 ON CONFLICT(key) DO UPDATE SET value='1'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if version < 2 {
            // v2 adds the export-sequence table used to preserve ordering.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS export_seq(
                    id INTEGER PRIMARY KEY CHECK (id=1),
                    counter INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO export_seq(id,counter) VALUES(1,0);",
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE schema_meta SET value='2' WHERE key='version'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> u32 {
        self.0
            .lock()
            .unwrap()
            .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// Append an event inside an optional transaction (used by import rollback).
    pub fn append_event_tx(
        &self,
        conn: &Connection,
        branch_id: &str,
        seq: i64,
        kind: &str,
        node_kind: Option<&str>,
        node_id: Option<&str>,
        version_uid: Option<&str>,
        payload: &str,
        parent_event: Option<i64>,
        base_hash: Option<&str>,
    ) -> Result<i64, String> {
        conn.execute(
            "INSERT INTO events(branch_id, seq, kind, node_kind, node_id,
                 version_uid, payload, parent_event, base_hash)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                branch_id,
                seq,
                kind,
                node_kind,
                node_id,
                version_uid,
                payload,
                parent_event,
                base_hash
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    pub fn next_seq(&self, conn: &Connection, branch_id: &str) -> Result<i64, String> {
        conn.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch_id=?1",
            params![branch_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    }

    pub fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.0.lock().unwrap()
    }
}

const SCHEMA_V1: &str = r#"
CREATE TABLE branches(
    branch_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    parent_branch TEXT,
    parent_seq INTEGER,
    created_event_id INTEGER NOT NULL,
    head_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE events(
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    branch_id TEXT NOT NULL REFERENCES branches(branch_id),
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    node_kind TEXT,
    node_id TEXT,
    version_uid TEXT,
    payload TEXT NOT NULL,
    parent_event INTEGER,
    base_hash TEXT,
    UNIQUE(branch_id, seq)
);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
CREATE INDEX idx_events_node ON events(branch_id, node_kind, node_id, seq);
CREATE TABLE candidates(
    candidate_id TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    produced_event_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    label TEXT NOT NULL,
    payload TEXT NOT NULL
);
CREATE TABLE deps(
    producer_event_id INTEGER NOT NULL,
    upstream_kind TEXT NOT NULL,
    upstream_node TEXT NOT NULL,
    upstream_version_uid TEXT NOT NULL,
    PRIMARY KEY(producer_event_id, upstream_kind, upstream_node)
);
CREATE TABLE raw_material(
    sha1 TEXT PRIMARY KEY,
    file_name TEXT NOT NULL,
    bytes BLOB NOT NULL,
    imported_event_id INTEGER NOT NULL
);
CREATE TABLE decisions(
    decision_key TEXT PRIMARY KEY,
    branch_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    active INTEGER NOT NULL,
    chosen_candidate_id TEXT,
    decided_event_id INTEGER NOT NULL
);
"#;
