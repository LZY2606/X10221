use crate::domain::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::Mutex;

pub struct AppDb {
    pub connection: Mutex<Connection>,
}

#[derive(Debug)]
pub struct DbError(pub String);

impl std::fmt::Display for DbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(error: rusqlite::Error) -> Self {
        DbError(error.to_string())
    }
}

impl From<serde_json::Error> for DbError {
    fn from(error: serde_json::Error) -> Self {
        DbError(error.to_string())
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Clone)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub description: String,
    pub latest_event_id: String,
}

impl AppDb {
    pub fn open(path: &str) -> DbResult<Self> {
        let connection = Connection::open(path)?;
        let db = AppDb { connection: Mutex::new(connection) };
        db.configure_and_migrate()?;
        Ok(db)
    }

    pub fn in_memory() -> DbResult<Self> {
        let connection = Connection::open_in_memory()?;
        let db = AppDb { connection: Mutex::new(connection) };
        db.configure_and_migrate()?;
        Ok(db)
    }

    pub fn initialize_v1_for_migration_test(&self) -> DbResult<()> {
        let connection = self.connection.lock().unwrap();
        connection.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        Ok(())
    }

    pub fn schema_version(&self) -> DbResult<i64> {
        let connection = self.connection.lock().unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        Ok(version)
    }

    fn configure_and_migrate(&self) -> DbResult<()> {
        let mut connection = self.connection.lock().unwrap();
        let tx = connection.transaction()?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap_or(0);
        if version == 0 {
            tx.execute_batch(include_str!("../migrations/001_initial.sql"))?;
            tx.execute_batch(include_str!("../migrations/002_lineage.sql"))?;
        } else if version == 1 {
            tx.execute_batch(include_str!("../migrations/002_lineage.sql"))?;
        }
        tx.pragma_update(None, "foreign_keys", "ON")?;
        tx.commit()?;
        Ok(())
    }

    pub fn reset_and_seed_fixture(&self) -> DbResult<()> {
        let mut connection = self.connection.lock().unwrap();
        let tx = connection.transaction()?;
        tx.execute_batch(
            "DELETE FROM audit_events;
             DELETE FROM records;
             DELETE FROM file_evidence;
             DELETE FROM branches;",
        )?;
        seed::seed_case(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn is_seeded(&self) -> DbResult<bool> {
        let connection = self.connection.lock().unwrap();
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM branches", [], |row| row.get(0))?;
        Ok(count > 0)
    }
}

pub(crate) fn now_stamp() -> String {
    "2026-09-23T09:00:00+09:00".to_string()
}

pub(crate) fn insert_branch(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    name: &str,
    description: &str,
    root_event_id: &str,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO branches
         (id, name, description, root_event_id, latest_event_id, base_case_version, created_from_branch)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, NULL)",
        params![id, name, description, root_event_id, CASE_ID],
    )?;
    Ok(())
}

pub(crate) fn insert_record(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    seq: i64,
    id: &str,
    stable_identity: &str,
    kind: &str,
    version: &str,
    status: &str,
    dependency_key: &str,
    parent_record_id: Option<&str>,
    event_id: &str,
    payload: &Value,
    file_path: Option<&str>,
    source_sha: Option<&str>,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO records
         (id, stable_identity, branch_id, seq, kind, version, status, dependency_key,
          parent_record_id, created_event_id, file_path, source_sha256, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            id,
            stable_identity,
            branch,
            seq,
            kind,
            version,
            status,
            dependency_key,
            parent_record_id,
            event_id,
            file_path,
            source_sha,
            serde_json::to_string(payload)?
        ],
    )?;
    Ok(())
}

pub(crate) fn insert_event(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    seq: i64,
    id: &str,
    parent: Option<&str>,
    event_type: &str,
    status: &str,
    actor: &str,
    payload: &Value,
) -> DbResult<()> {
    tx.execute(
        "INSERT INTO audit_events
         (id, branch_id, seq, parent_event_id, event_type, status, actor, occurred_at, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            branch,
            seq,
            parent,
            event_type,
            status,
            actor,
            now_stamp(),
            serde_json::to_string(payload)?
        ],
    )?;
    tx.execute(
        "UPDATE branches SET latest_event_id = ?1 WHERE id = ?2",
        params![id, branch],
    )?;
    Ok(())
}

pub(crate) fn latest_event(tx: &rusqlite::Transaction<'_>, branch: &str) -> DbResult<(String, i64)> {
    tx.query_row(
        "SELECT id, seq FROM audit_events WHERE branch_id = ?1 ORDER BY seq DESC LIMIT 1",
        params![branch],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(Into::into)
}

pub(crate) fn next_seq(tx: &rusqlite::Transaction<'_>, branch: &str) -> DbResult<i64> {
    let value: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM audit_events WHERE branch_id = ?1",
        params![branch],
        |row| row.get(0),
    )?;
    Ok(value)
}

pub(crate) fn next_record_seq(tx: &rusqlite::Transaction<'_>, branch: &str) -> DbResult<i64> {
    let value: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM records WHERE branch_id = ?1",
        params![branch],
        |row| row.get(0),
    )?;
    Ok(value)
}

pub mod seed;
pub mod queries;
pub mod actions;
pub mod archive;
