//! SQLite persistence: append-only branch/event log with separately versioned
//! artifacts, derived result versions, candidate options and staleness state.

use crate::domain::*;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

pub const CURRENT_SCHEMA: u32 = 2;

const SCHEMA_V1: &str = r#"
CREATE TABLE branches (
  branch_id TEXT PRIMARY KEY,
  parent_branch TEXT,
  base_event_id TEXT,
  label TEXT NOT NULL,
  created_event_seq INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE events (
  event_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  parent_event_id TEXT,
  event_type TEXT NOT NULL,
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(branch_id, seq)
);
CREATE TABLE artifacts (
  row_id INTEGER PRIMARY KEY AUTOINCREMENT,
  artifact_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  version TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  content TEXT NOT NULL
);
CREATE TABLE result_versions (
  result_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  layer TEXT NOT NULL,
  gate_id TEXT,
  scope_seq INTEGER NOT NULL,
  content TEXT,
  status TEXT NOT NULL,
  reason TEXT,
  created_event_id TEXT NOT NULL,
  stale_since_event_id TEXT
);
CREATE TABLE candidate_options (
  option_id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  matrix_artifact_id TEXT NOT NULL,
  matrix_version TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  content TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  decided_event_id TEXT
);
CREATE TABLE diagnostics (
  diag_id INTEGER PRIMARY KEY AUTOINCREMENT,
  branch_id TEXT NOT NULL,
  code TEXT NOT NULL,
  event_id TEXT,
  channel_id TEXT,
  message TEXT NOT NULL,
  created_event_id TEXT NOT NULL
);
CREATE TABLE judgments (
  judgment_id INTEGER PRIMARY KEY AUTOINCREMENT,
  code TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  subject TEXT NOT NULL,
  evidence TEXT NOT NULL,
  seq INTEGER NOT NULL
);
"#;

pub fn fingerprint(canonical: &str) -> String {
    let mut h = DefaultHasher::new();
    canonical.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub struct Store {
    pub conn: Mutex<Connection>,
}

pub enum OpenMode {
    Create,
    Open,
    Memory,
}

#[derive(Debug)]
pub struct Conflict {
    pub message: String,
    pub your_payload: Value,
    pub server_head: Value,
}

impl Store {
    pub fn open(path: &str, mode: OpenMode) -> rusqlite::Result<Self> {
        let conn = match mode {
            OpenMode::Memory => Connection::open_in_memory()?,
            _ => Connection::open(path)?,
        };
        let store = Store { conn: Mutex::new(conn) };
        match mode {
            OpenMode::Open => {
                let v = store.schema_version()?;
                if v == 0 {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            }
            _ => store.init()?,
        }
        Ok(store)
    }

    pub fn init(&self) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.execute_batch(
            "CREATE TABLE schema_meta (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;
        tx.execute("INSERT INTO schema_meta VALUES (1, datetime('now'))", [])?;
        tx.execute(
            "ALTER TABLE artifacts ADD COLUMN fingerprint TEXT",
            [],
        )?;
        tx.execute(
            "UPDATE artifacts SET fingerprint = 'legacy:' || length(content)",
            [],
        )?;
        tx.execute("INSERT INTO schema_meta VALUES (2, datetime('now'))", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Initialize *only* v1 schema; used by migration tests.
    pub fn init_v1_only(&self) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        tx.execute_batch(
            "CREATE TABLE schema_meta (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;
        tx.execute("INSERT INTO schema_meta VALUES (1, datetime('now'))", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn migrate_to_current(&self) -> rusqlite::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let v: u32 = tx
            .query_row("SELECT COALESCE(MAX(version),0) FROM schema_meta", [], |r| {
                r.get(0)
            })?;
        if v < 2 {
            tx.execute("ALTER TABLE artifacts ADD COLUMN fingerprint TEXT", [])?;
            tx.execute(
                "UPDATE artifacts SET fingerprint = 'legacy:' || length(content)",
                [],
            )?;
            tx.execute("INSERT INTO schema_meta VALUES (2, datetime('now'))", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn schema_version(&self) -> rusqlite::Result<u32> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_meta",
            [],
            |r| r.get(0),
        )
    }

    pub fn has_fingerprint_column(&self) -> bool {
        let c = self.conn.lock().unwrap();
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('artifacts') WHERE name='fingerprint'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        n > 0
    }

    // ---- branch / event ancestry ---------------------------------------

    pub fn branch_exists(&self, branch_id: &str) -> rusqlite::Result<bool> {
        let c = self.conn.lock().unwrap();
        let n: i64 = c.query_row(
            "SELECT COUNT(*) FROM branches WHERE branch_id=?1",
            params![branch_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn head(&self, branch_id: &str) -> rusqlite::Result<Option<(String, i64)>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(
                "SELECT event_id, seq FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![branch_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .flatten()
            .collect::<Vec<_>>();
        Ok(rows.into_iter().next())
    }

    /// Branches in ancestry order: the branch itself then parents.
    pub fn branch_ancestors(&self, branch_id: &str) -> rusqlite::Result<Vec<(String, Option<String>)>> {
        let c = self.conn.lock().unwrap();
        let mut out = Vec::new();
        let mut cur = branch_id.to_string();
        loop {
            let row = c
                .query_row(
                    "SELECT branch_id, parent_branch, base_event_id FROM branches WHERE branch_id=?1",
                    params![cur],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)),
                )
                .ok();
            match row {
                Some((bid, parent, _base)) => {
                    out.push((bid.clone(), parent.clone()));
                    match parent {
                        Some(p) => cur = p,
                        None => break,
                    }
                }
                None => break,
            }
        }
        Ok(out)
    }

    pub fn events_on_branch(&self, branch_id: &str) -> rusqlite::Result<Vec<String>> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT event_id FROM events WHERE branch_id=?1 ORDER BY seq")?;
        let ids: Vec<String> = stmt
            .query_map(params![branch_id], |r| r.get(0))?
            .flatten()
            .collect();
        Ok(ids)
    }

    /// Global event timeline restricted to branch ancestry, ordered across
    /// branches via (created branch depth, seq). Events carry a monotonic
    /// global insertion rowid used for ordering.
    pub fn ancestry_event_ids(&self, branch_id: &str) -> rusqlite::Result<Vec<String>> {
        let ancestors = self.branch_ancestors(branch_id)?;
        let c = self.conn.lock().unwrap();
        let mut ids = Vec::new();
        // parents first, then the child's own events
        for (bid, _) in ancestors.iter().rev() {
            let mut stmt = c.prepare(
                "SELECT event_id FROM events WHERE branch_id=?1 ORDER BY seq",
            )?;
            let part: Vec<String> = stmt
                .query_map(params![bid], |r| r.get(0))?
                .flatten()
                .collect();
            ids.extend(part);
        }
        Ok(ids)
    }

    pub fn get_event(&self, event_id: &str) -> rusqlite::Result<(String, String, String, Value)> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT branch_id, event_type, parent_event_id, payload FROM events WHERE event_id=?1",
            params![event_id],
            |r| {
                let payload: String = r.get(3)?;
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    serde_json::from_str(&payload).unwrap_or(Value::Null),
                ))
            },
        )
    }
}
