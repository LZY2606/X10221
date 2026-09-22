//! SQLite persistence: append-only event log with derived read models.
//!
//! Source of truth is `event_log`. Resource versions, result versions,
//! candidates and stale markers are all rebuilt by replaying the log, so an
//! export/import round-trip preserves identity, order and provenance.

pub mod build;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    pub case_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_seq: Option<i64>,
    pub kind: String,
    pub actor: String,
    pub created_at: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitOutput {
    pub event: StoredEvent,
    pub head_seq: i64,
}

pub struct Store {
    pub conn: Connection,
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../../migrations/0001_init.sql")),
    ("0002_draft_note", include_str!("../../migrations/0002_draft_note.sql")),
];

pub fn connect(path: &str) -> rusqlite::Result<Store> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    let mut store = Store { conn };
    store.migrate()?;
    Ok(store)
}

pub fn connect_memory() -> rusqlite::Result<Store> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    let mut store = Store { conn };
    store.migrate()?;
    Ok(store)
}

impl Store {
    fn migrate(&mut self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
               key TEXT PRIMARY KEY, value TEXT NOT NULL
             );",
        )?;
        for (name, sql) in MIGRATIONS {
            let applied: Option<String> = self
                .conn
                .query_row(
                    "SELECT value FROM schema_meta WHERE key='migration_'||?1",
                    params![name],
                    |r| r.get(0),
                )
                .ok();
            if applied.is_none() {
                self.conn.execute_batch(sql)?;
                self.conn.execute(
                    "INSERT INTO schema_meta(key, value) VALUES ('migration_'||?1, 'applied')",
                    params![name],
                )?;
            }
        }
        Ok(())
    }

    pub fn applied_migrations(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key FROM schema_meta WHERE key LIKE 'migration_%' ORDER BY key")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn put_blob(&mut self, sha: &str, bytes: &[u8], media_type: &str) -> rusqlite::Result<()> {
        let exists: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM raw_blob WHERE sha256=?1", params![sha], |r| r.get(0))?;
        if exists == 0 {
            self.conn.execute(
                "INSERT INTO raw_blob(sha256, bytes, media_type, byte_count)
                 VALUES (?1, ?2, ?3, ?4)",
                params![sha, bytes, media_type, bytes.len() as i64],
            )?;
        }
        Ok(())
    }

    pub fn get_blob(&self, sha: &str) -> rusqlite::Result<Option<Vec<u8>>> {
        let mut stmt = self.conn.prepare("SELECT bytes FROM raw_blob WHERE sha256=?1")?;
        let mut rows = stmt.query(params![sha])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get(0)?)),
            None => Ok(None),
        }
    }

    fn ensure_branch(&self, case_id: &str, branch_id: &str) -> rusqlite::Result<()> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM branch WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            self.conn.execute(
                "INSERT INTO branch(case_id, branch_id, parent_branch_id, fork_seq, label, created_seq)
                 VALUES (?1, ?2, NULL, 0, 'main review timeline', 0)",
                params![case_id, branch_id],
            )?;
        }
        Ok(())
    }

    fn visible_seqs(&self, case_id: &str, branch_id: &str) -> rusqlite::Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq FROM branch_event
             WHERE case_id=?1 AND branch_id=?2 AND visible=1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![case_id, branch_id], |r| r.get(0))?;
        rows.collect()
    }

    fn insert_branch_event(&self, case_id: &str, branch_id: &str, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO branch_event(case_id, branch_id, seq, visible)
             VALUES (?1, ?2, ?3, 1)",
            params![case_id, branch_id, seq],
        )?;
        Ok(())
    }

    /// Append one event. Caller passes the expected head for optimistic
    /// concurrency; a mismatch returns `Err("conflict")`.
    pub fn commit_event(
        &mut self,
        case_id: &str,
        branch_id: &str,
        expected_head: i64,
        parent_seq: Option<i64>,
        kind: &str,
        actor: &str,
        payload: &Value,
    ) -> rusqlite::Result<CommitOutput> {
        let tx = self.conn.transaction()?;
        let head: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM event_log WHERE case_id=?1",
                params![case_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if expected_head != head {
            return Err(rusqlite::Error::Other(
                format!(
                    "conflict: branch head is seq {head}, client expected {expected_head}"
                )
                .into(),
            ));
        }
        let next = head + 1;
        let now = "2026-09-22T00:00:00Z";
        tx.execute(
            "INSERT INTO event_log(case_id, seq, branch_id, parent_seq, kind, actor, created_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                case_id,
                next,
                branch_id,
                parent_seq,
                kind,
                actor,
                now,
                serde_json::to_string(payload).unwrap()
            ],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO branch(case_id, branch_id, parent_branch_id, fork_seq, label, created_seq)
             VALUES (?1, ?2, NULL, 0, 'main review timeline', ?3)",
            params![case_id, branch_id, next],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO branch_event(case_id, branch_id, seq, visible)
             VALUES (?1, ?2, ?3, 1)",
            params![case_id, branch_id, next],
        )?;
        tx.commit()?;
        self.rebuild(case_id)?;
        Ok(CommitOutput {
            event: StoredEvent {
                case_id: case_id.to_string(),
                seq: next,
                branch_id: branch_id.to_string(),
                parent_seq,
                kind: kind.to_string(),
                actor: actor.to_string(),
                created_at: now.to_string(),
                payload: payload.clone(),
            },
            head_seq: next,
        })
    }

    pub fn head_seq(&self, case_id: &str) -> rusqlite::Result<i64> {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM event_log WHERE case_id=?1",
                params![case_id],
                |r| r.get(0),
            )
    }

    pub fn events_on_branch(&self, case_id: &str, branch_id: &str) -> rusqlite::Result<Vec<StoredEvent>> {
        let seqs = self.visible_seqs(case_id, branch_id)?;
        let mut out = Vec::new();
        for seq in seqs {
            out.push(self.event_by_seq(case_id, seq)?);
        }
        Ok(out)
    }

    pub fn all_events(&self, case_id: &str) -> rusqlite::Result<Vec<StoredEvent>> {
        let mut stmt = self
            .conn
            .prepare("SELECT branch_id, parent_seq, kind, actor, created_at, payload, seq
                      FROM event_log WHERE case_id=?1 ORDER BY seq")?;
        let rows = stmt.query_map(params![case_id], |r| {
            let payload: String = r.get(5)?;
            Ok(StoredEvent {
                case_id: case_id.to_string(),
                seq: r.get(6)?,
                branch_id: r.get(0)?,
                parent_seq: r.get(1)?,
                kind: r.get(2)?,
                actor: r.get(3)?,
                created_at: r.get(4)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            })
        })?;
        rows.collect()
    }

    pub fn event_by_seq(&self, case_id: &str, seq: i64) -> rusqlite::Result<StoredEvent> {
        self.conn.query_row(
            "SELECT branch_id, parent_seq, kind, actor, created_at, payload
             FROM event_log WHERE case_id=?1 AND seq=?2",
            params![case_id, seq],
            |r| {
                let payload: String = r.get(5)?;
                Ok(StoredEvent {
                    case_id: case_id.to_string(),
                    seq,
                    branch_id: r.get(0)?,
                    parent_seq: r.get(1)?,
                    kind: r.get(2)?,
                    actor: r.get(3)?,
                    created_at: r.get(4)?,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                })
            },
        )
    }

    pub fn fork_branch(
        &mut self,
        case_id: &str,
        new_branch: &str,
        parent_branch: &str,
        fork_seq: i64,
        label: &str,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO branch(case_id, branch_id, parent_branch_id, fork_seq, label, created_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![case_id, new_branch, parent_branch, fork_seq, label, fork_seq],
        )?;
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO branch_event(case_id, branch_id, seq, visible)
             SELECT ?1, ?2, seq, 1 FROM branch_event
             WHERE case_id=?1 AND branch_id=?3 AND seq<=?4 AND visible=1",
        )?;
        stmt.execute(params![case_id, new_branch, parent_branch, fork_seq])?;
        tx.commit()?;
        Ok(())
    }

    pub fn branches(&self, case_id: &str) -> rusqlite::Result<Vec<(String, Option<String>, i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT branch_id, parent_branch_id, fork_seq, label FROM branch WHERE case_id=?1 ORDER BY branch_id",
        )?;
        let rows = stmt.query_map(params![case_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect()
    }

    /// Draft CRUD with optimistic locking: saving requires the current base_seq.
    pub fn save_draft(
        &mut self,
        case_id: &str,
        draft_id: &str,
        branch_id: &str,
        base_seq: i64,
        kind: &str,
        title: &str,
        note: &str,
        payload: &Value,
    ) -> rusqlite::Result<()> {
        let head = self.head_seq(case_id)?;
        if base_seq != head {
            return Err(rusqlite::Error::Other(
                format!("conflict: draft base {base_seq} != branch head {head}").into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO draft(case_id, draft_id, branch_id, base_seq, kind, title, payload, updated_at, note)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'2026-09-22T00:00:00Z',?8)
             ON CONFLICT(draft_id) DO UPDATE SET
               base_seq=excluded.base_seq, kind=excluded.kind, title=excluded.title,
               payload=excluded.payload, updated_at=excluded.updated_at, note=excluded.note",
            params![case_id, draft_id, branch_id, base_seq, kind, title,
                    serde_json::to_string(payload).unwrap(), note],
        )?;
        Ok(())
    }

    pub fn drafts(&self, case_id: &str) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT draft_id, branch_id, base_seq, kind, title, payload, note
             FROM draft WHERE case_id=?1 ORDER BY draft_id",
        )?;
        let rows = stmt.query_map(params![case_id], |r| {
            let payload: String = r.get(5)?;
            Ok(serde_json::json!({
                "draft_id": r.get::<_, String>(0)?,
                "branch_id": r.get::<_, String>(1)?,
                "base_seq": r.get::<_, i64>(2)?,
                "kind": r.get::<_, String>(3)?,
                "title": r.get::<_, String>(4)?,
                "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                "note": r.get::<_, String>(6)?,
            }))
        })?;
        rows.collect()
    }

    pub fn clear_case(&mut self, case_id: &str) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for table in [
            "stale_marker",
            "result_dependency",
            "result_version",
            "candidate",
            "resource_version",
            "branch_event",
            "branch",
            "draft",
            "event_log",
        ] {
            tx.execute(format!("DELETE FROM {table} WHERE case_id=?1").as_str(), params![case_id])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[allow(dead_code)]
fn _unused_ensure(_s: &Store) {
    // ensure_branch is used through commit_event; kept private.
}
