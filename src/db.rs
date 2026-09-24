use crate::models::{Branch, Event};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

pub struct Store {
    pub conn: Mutex<Connection>,
}

const MIGRATIONS: [&str; 2] = [
    "CREATE TABLE branches (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        parent_branch_id TEXT,
        fork_seq INTEGER NOT NULL,
        head_seq INTEGER NOT NULL
    );
    CREATE TABLE events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id TEXT NOT NULL UNIQUE,
        event_type TEXT NOT NULL,
        branch_id TEXT NOT NULL,
        parent_event_id TEXT,
        seq_in_branch INTEGER NOT NULL,
        payload TEXT NOT NULL,
        sources TEXT NOT NULL DEFAULT '[]',
        FOREIGN KEY(branch_id) REFERENCES branches(id)
    );",
    "ALTER TABLE events ADD COLUMN rationale TEXT;
    ALTER TABLE branches ADD COLUMN created_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z';",
];

impl Store {
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<(), rusqlite::Error> {
        let mut conn = self.conn.lock().expect("database lock");
        let current: i64 =
            conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for (index, sql) in MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            if current < version {
                conn.execute_batch("BEGIN")?;
                conn.execute_batch(sql)?;
                conn.pragma_update(None, "user_version", version)?;
                conn.execute_batch("COMMIT")?;
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool, rusqlite::Error> {
        let conn = self.conn.lock().expect("database lock");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM branches", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    pub fn list_branches(&self) -> Result<Vec<Branch>, rusqlite::Error> {
        let conn = self.conn.lock().expect("database lock");
        let mut stmt = conn.prepare(
            "SELECT id, name, parent_branch_id, fork_seq, head_seq
             FROM branches ORDER BY fork_seq, id",
        )?;
        let rows = stmt.query_map([], row_to_branch)?;
        rows.collect()
    }

    pub fn list_events(&self) -> Result<Vec<Event>, rusqlite::Error> {
        let conn = self.conn.lock().expect("database lock");
        let mut stmt = conn.prepare(
            "SELECT seq, event_id, event_type, branch_id, parent_event_id, seq_in_branch,
                    payload, sources, rationale
             FROM events ORDER BY seq",
        )?;
        let rows = stmt.query_map([], row_to_event)?;
        rows.collect()
    }

    pub fn create_branch(
        &self,
        branch: Branch,
        event: Event,
    ) -> Result<(Branch, Event), rusqlite::Error> {
        let mut conn = self.conn.lock().expect("database lock");
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO branches(id, name, parent_branch_id, fork_seq, head_seq, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                branch.id,
                branch.name,
                branch.parent_branch_id,
                branch.fork_seq,
                branch.head_seq,
                "1970-01-01T00:00:00Z"
            ],
        )?;
        insert_event(&tx, &event)?;
        tx.execute(
            "UPDATE branches SET head_seq = ?1 WHERE id = ?2",
            params![event.seq, event.branch_id],
        )?;
        tx.commit()?;
        Ok((branch, event))
    }

    pub fn append_event(
        &self,
        mut event: Event,
        expected_head_seq: Option<i64>,
    ) -> Result<Event, (rusqlite::Error, i64)> {
        let mut conn = self.conn.lock().expect("database lock");
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(err) => return Err((err, -1)),
        };
        let head: i64 = match tx.query_row(
            "SELECT head_seq FROM branches WHERE id = ?1",
            params![event.branch_id],
            |row| row.get(0),
        ) {
            Ok(head) => head,
            Err(err) => return Err((err, -1)),
        };
        if let Some(expected) = expected_head_seq {
            if expected != head {
                return Err((rusqlite::Error::InvalidQuery, head));
            }
        }
        let parent_event_id: Option<String> = tx.query_row(
            "SELECT event_id FROM events WHERE branch_id = ?1 ORDER BY seq DESC LIMIT 1",
            params![event.branch_id],
            |row| row.get(0),
        )?;
        event.parent_event_id = parent_event_id;
        let seq_in_branch: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq_in_branch), 0) + 1 FROM events WHERE branch_id = ?1",
            params![event.branch_id],
            |row| row.get(0),
        )?;
        event.seq_in_branch = seq_in_branch;
        if let Err(err) = insert_live_event(&tx, &event) {
            return Err((err, head));
        }
        let assigned_seq = tx.last_insert_rowid();
        event.seq = assigned_seq;
        if let Err(err) = tx.execute(
            "UPDATE branches SET head_seq = ?1 WHERE id = ?2",
            params![assigned_seq, event.branch_id],
        ) {
            return Err((err, head));
        }
        if let Err(err) = tx.commit() {
            return Err((err, head));
        }
        Ok(event)
    }

    pub fn replace_all(&self, events: &[Event], branches: &[Branch]) -> Result<(), String> {
        let mut conn = self.conn.lock().expect("database lock");
        let tx = conn.transaction().map_err(|err| err.to_string())?;
        tx.execute_batch(
            "DELETE FROM events;
             DELETE FROM branches;
             DELETE FROM sqlite_sequence WHERE name IN ('events','branches');",
        )
        .map_err(|err| err.to_string())?;
        for branch in branches {
            tx.execute(
                "INSERT INTO branches(id, name, parent_branch_id, fork_seq, head_seq, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '1970-01-01T00:00:00Z')",
                params![
                    branch.id,
                    branch.name,
                    branch.parent_branch_id,
                    branch.fork_seq,
                    branch.head_seq
                ],
            )
            .map_err(|err| err.to_string())?;
        }
        for event in events {
            insert_event(&tx, event).map_err(|err| err.to_string())?;
        }
        tx.commit().map_err(|err| err.to_string())?;
        Ok(())
    }
}

fn insert_live_event(
    tx: &rusqlite::Transaction<'_>,
    event: &Event,
) -> Result<(), rusqlite::Error> {
    let payload = serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string());
    let sources = serde_json::to_string(&event.sources).unwrap_or_else(|_| "[]".to_string());
    tx.execute(
        "INSERT INTO events(event_id, event_type, branch_id, parent_event_id,
                            seq_in_branch, payload, sources, rationale)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            event.event_id,
            event.event_type,
            event.branch_id,
            event.parent_event_id,
            event.seq_in_branch,
            payload,
            sources,
            event.rationale
        ],
    )?;
    Ok(())
}

fn insert_event(
    tx: &rusqlite::Transaction<'_>,
    event: &Event,
) -> Result<(), rusqlite::Error> {
    let payload = serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string());
    let sources = serde_json::to_string(&event.sources).unwrap_or_else(|_| "[]".to_string());
    tx.execute(
        "INSERT INTO events(seq, event_id, event_type, branch_id, parent_event_id,
                            seq_in_branch, payload, sources, rationale)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.seq,
            event.event_id,
            event.event_type,
            event.branch_id,
            event.parent_event_id,
            event.seq_in_branch,
            payload,
            sources,
            event.rationale
        ],
    )?;
    Ok(())
}

fn row_to_branch(row: &rusqlite::Row<'_>) -> rusqlite::Result<Branch> {
    Ok(Branch {
        id: row.get(0)?,
        name: row.get(1)?,
        parent_branch_id: row.get(2)?,
        fork_seq: row.get(3)?,
        head_seq: row.get(4)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let payload: String = row.get(6)?;
    let sources: String = row.get(7)?;
    Ok(Event {
        seq: row.get(0)?,
        event_id: row.get(1)?,
        event_type: row.get(2)?,
        branch_id: row.get(3)?,
        parent_event_id: row.get(4)?,
        seq_in_branch: row.get(5)?,
        payload: serde_json::from_str(&payload).unwrap_or(serde_json::json!({})),
        sources: serde_json::from_str(&sources).unwrap_or(serde_json::json!([])),
        rationale: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_database() {
        let store = Store::open(":memory:").unwrap();
        store.migrate().unwrap();
        assert!(store.is_empty().unwrap());
    }

    #[test]
    fn migration_keeps_existing_events() {
        let store = Store::open(":memory:").unwrap();
        let branch = Branch {
            id: "main".into(),
            name: "Main".into(),
            parent_branch_id: None,
            fork_seq: 0,
            head_seq: 0,
        };
        let event = Event {
            seq: 1,
            event_id: "e1".into(),
            event_type: "raw_channels.imported".into(),
            branch_id: "main".into(),
            parent_event_id: None,
            seq_in_branch: 1,
            payload: serde_json::json!({"id":"raw","version":1}),
            sources: serde_json::json!([]),
            rationale: None,
        };
        store.create_branch(branch, event).unwrap();
        store.migrate().unwrap();
        assert_eq!(store.list_events().unwrap().len(), 1);
        assert_eq!(store.list_branches().unwrap()[0].head_seq, 1);
    }
}


impl Store {
    pub fn visible_events(&self, branch_id: &str, as_of_seq: Option<i64>) -> Result<Vec<Event>, rusqlite::Error> {
        let conn = self.conn.lock().expect("database lock");
        let mut stmt = conn.prepare(
            "WITH RECURSIVE lineage(branch_id, parent_branch_id, visible_seq) AS (
                SELECT id, parent_branch_id, 9223372036854775807 FROM branches WHERE id = ?1
                UNION ALL
                SELECT parent.id, parent.parent_branch_id, child.fork_seq
                FROM branches parent
                JOIN lineage child ON parent.id = child.parent_branch_id
             )
             SELECT e.seq, e.event_id, e.event_type, e.branch_id, e.parent_event_id,
                    e.seq_in_branch, e.payload, e.sources, e.rationale
             FROM events e JOIN lineage l ON e.branch_id = l.branch_id
             WHERE e.seq <= l.visible_seq AND (?2 IS NULL OR e.seq <= ?2)
             ORDER BY e.seq",
        )?;
        let rows = stmt.query_map(params![branch_id, as_of_seq], row_to_event)?;
        rows.collect()
    }

    pub fn branch(&self, id: &str) -> Result<Option<Branch>, rusqlite::Error> {
        let conn = self.conn.lock().expect("database lock");
        conn.query_row(
            "SELECT id, name, parent_branch_id, fork_seq, head_seq FROM branches WHERE id = ?1",
            params![id],
            row_to_branch,
        )
        .optional()
    }
}
