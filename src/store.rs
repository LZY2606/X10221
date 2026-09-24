//! SQLite storage: an append-only ledger plus versioned artifact tables.
//!
//! The ledger (`events`) is the single source of truth and is never rewritten.
//! Replaying a branch's parent chain reconstructs every derived conclusion at
//! any historical point. `blobs` holds original FCS bytes; the artifact tables
//! are caches rebuildable by replay (used by import and tests).

use crate::domain::{ArtifactRec, LedgerEvent};
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub const SCHEMA_VERSION: i64 = 2;

pub struct Store {
    pub conn: Mutex<Connection>,
}

pub struct Conflict {
    pub expected_tip: Option<String>,
    pub actual_tip: Option<String>,
}

#[derive(Debug)]
pub enum AppendError {
    Conflict(Conflict),
    Sql(String),
    Other(String),
}

impl From<rusqlite::Error> for AppendError {
    fn from(e: rusqlite::Error) -> Self {
        AppendError::Sql(e.to_string())
    }
}

pub fn current_schema(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT version FROM flow_schema WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let v = current_schema(conn);
    if v == 0 {
        // ---- v1: historical schema exercised by the migration test ----
        conn.execute_batch(
            r#"
            CREATE TABLE flow_schema (id INTEGER PRIMARY KEY, version INTEGER NOT NULL);
            INSERT INTO flow_schema (id, version) VALUES (1, 1);

            CREATE TABLE branches (
              id TEXT PRIMARY KEY,
              label TEXT NOT NULL,
              base_branch TEXT,
              base_event_id TEXT,
              tip_event_id TEXT,
              tip_seq INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE events (
              id TEXT PRIMARY KEY,
              seq INTEGER NOT NULL,
              branch_id TEXT NOT NULL,
              kind TEXT NOT NULL,
              payload TEXT NOT NULL,
              parent_event_id TEXT,
              author TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE TABLE artifacts (
              id TEXT PRIMARY KEY,
              kind TEXT NOT NULL,
              key TEXT NOT NULL,
              event_id TEXT NOT NULL,
              seq INTEGER NOT NULL,
              payload TEXT NOT NULL
            );
            CREATE TABLE blobs (sha256 TEXT PRIMARY KEY, bytes BLOB NOT NULL);
            "#,
        )?;
        // ---- v2 ----
        conn.execute_batch(
            r#"
            ALTER TABLE branches ADD COLUMN note TEXT NOT NULL DEFAULT '';
            UPDATE flow_schema SET version = 2 WHERE id = 1;
            "#,
        )?;
    } else if v == 1 {
        conn.execute_batch(
            r#"
            ALTER TABLE branches ADD COLUMN note TEXT NOT NULL DEFAULT '';
            UPDATE flow_schema SET version = 2 WHERE id = 1;
            "#,
        )?;
    }
    Ok(())
}

impl Store {
    pub fn open(path: &str) -> Result<Store, String> {
        let mut conn = Connection::open(path).map_err(|e| e.to_string())?;
        migrate(&mut conn).map_err(|e| e.to_string())?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Store, String> {
        let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        migrate(&mut conn).map_err(|e| e.to_string())?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    pub fn put_blob(&self, sha: &str, bytes: &[u8]) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO blobs (sha256, bytes) VALUES (?1, ?2)",
            params![sha, bytes],
        )?;
        Ok(())
    }

    pub fn get_blob(&self, sha: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT bytes FROM blobs WHERE sha256 = ?1", params![sha], |r| {
            r.get::<_, Vec<u8>>(0)
        })
        .ok()
    }

    pub fn create_branch(
        &self,
        id: &str,
        label: &str,
        base_branch: Option<&str>,
        base_event_id: Option<&str>,
        note: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branches (id, label, base_branch, base_event_id, tip_event_id, tip_seq, note)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
            params![id, label, base_branch, base_event_id, base_event_id, note],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn branch_exists(&self, id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT 1 FROM branches WHERE id = ?1", params![id], |_| Ok(()))
            .is_ok()
    }

    pub fn list_branches(&self) -> Vec<BranchRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, label, base_branch, base_event_id, tip_event_id, tip_seq, note FROM branches ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| {
            Ok(BranchRow {
                id: row.get(0)?,
                label: row.get(1)?,
                base_branch: row.get(2)?,
                base_event_id: row.get(3)?,
                tip_event_id: row.get(4)?,
                tip_seq: row.get(5)?,
                note: row.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn branch_tip(&self, branch: &str) -> Result<(Option<String>, u64), String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT tip_event_id, tip_seq FROM branches WHERE id = ?1",
            params![branch],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? as u64)),
        )
        .map_err(|e| e.to_string())
    }

    /// Append one event with optimistic concurrency on the branch tip.
    /// `expected_tip` must equal the current stored tip or the append is
    /// rejected so a second concurrent saver sees the conflict.
    pub fn append(
        &self,
        branch: &str,
        author: &str,
        kind: &str,
        payload: &serde_json::Value,
        expected_tip: Option<&str>,
        created_at: Option<&str>,
    ) -> Result<LedgerEvent, AppendError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| AppendError::Sql(e.to_string()))?;
        let (tip, seq): (Option<String>, i64) = tx
            .query_row(
                "SELECT tip_event_id, tip_seq FROM branches WHERE id = ?1",
                params![branch],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(|e| AppendError::Sql(e.to_string()))?;
        if expected_tip.map(|s| s.to_string()) != tip {
            return Err(AppendError::Conflict(Conflict {
                expected_tip: expected_tip.map(|s| s.to_string()),
                actual_tip: tip,
            }));
        }
        let payload_text = serde_json::to_string(payload).unwrap();
        let parents: Vec<String> = tip.iter().cloned().collect();
        let new_seq = seq as u64 + 1;
        let id = crate::ids::event_id(branch, new_seq, kind, &payload_text, &parents);
        let ts = created_at
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::now_iso());
        tx.execute(
            "INSERT INTO events (id, seq, branch_id, kind, payload, parent_event_id, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, new_seq, branch, kind, payload_text, parents.first(), author, ts],
        )?;
        tx.execute(
            "UPDATE branches SET tip_event_id = ?1, tip_seq = ?2 WHERE id = ?3",
            params![id, new_seq, branch],
        )?;
        tx.commit()?;
        Ok(LedgerEvent {
            id,
            seq: new_seq,
            branch: branch.to_string(),
            kind: kind.to_string(),
            payload: payload.clone(),
            parents,
            author: author.to_string(),
            created_at: ts,
        })
    }

    pub fn get_event(&self, id: &str) -> Option<LedgerEvent> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, seq, branch_id, kind, payload, parent_event_id, author, created_at
             FROM events WHERE id = ?1",
            params![id],
            map_event_pub,
        )
        .ok()
    }

    pub fn all_events(&self) -> Vec<LedgerEvent> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, seq, branch_id, kind, payload, parent_event_id, author, created_at
                 FROM events ORDER BY created_at, id",
            )
            .unwrap();
        stmt.query_map([], map_event_pub)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn put_artifact(&self, rec: &ArtifactRec) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        let payload = serde_json::to_string(&rec.payload).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO artifacts (id, kind, key, event_id, seq, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rec.id, rec.kind, rec.key, rec.event_id, rec.seq, payload],
        )?;
        Ok(())
    }

    pub fn count_events(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }
}

pub fn map_event_pub(row: &rusqlite::Row<'_>) -> rusqlite::Result<LedgerEvent> {
    let parent: Option<String> = row.get(5)?;
    let payload_text: String = row.get(4)?;
    Ok(LedgerEvent {
        id: row.get(0)?,
        seq: row.get::<_, i64>(1)? as u64,
        branch: row.get(2)?,
        kind: row.get(3)?,
        payload: serde_json::from_str(&payload_text).unwrap(),
        parents: parent.into_iter().collect(),
        author: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct BranchRow {
    pub id: String,
    pub label: String,
    pub base_branch: Option<String>,
    pub base_event_id: Option<String>,
    pub tip_event_id: Option<String>,
    pub tip_seq: i64,
    pub note: String,
}
