//! SQLite persistence: schema migrations, versioned objects, event journal.

use crate::model;
use rusqlite::{params, Connection};
use serde_json::Value;

pub const SCHEMA_VERSION: i64 = 2;

pub struct Store {
    pub conn: Connection,
}

/// A recorded journal event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub seq: i64,
    pub event_id: String,
    pub kind: String,
    pub payload: Value,
    pub branch: String,
    pub created_at: i64,
}

impl Store {
    pub fn open(path: &str) -> anyhow::Result<Store> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let mut store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn schema_version(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Apply pending migrations in order. Each migration is one transaction:
    /// a failed migration rolls back only its own change, never half a record.
    pub fn migrate(&mut self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );",
        )?;
        let current = self.schema_version();
        for v in (current + 1)..=SCHEMA_VERSION {
            let tx = self.conn.transaction()?;
            match v {
                1 => migrate_v1(&tx)?,
                2 => migrate_v2(&tx)?,
                _ => unreachable!(),
            }
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
                params![v, now()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Append an event to the journal. Returns the assigned seq + stable id.
    pub fn log(&mut self, kind: &str, payload: Value, branch: &str) -> anyhow::Result<Event> {
        let tx = self.conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events",
            [],
            |r| r.get(0),
        )?;
        let eid = model::event_id(seq, kind, &payload);
        tx.execute(
            "INSERT INTO events(seq, event_id, kind, payload, branch, created_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![seq, eid, kind, serde_json::to_string(&payload)?, branch, now()],
        )?;
        tx.commit()?;
        Ok(Event {
            seq,
            event_id: eid,
            kind: kind.to_string(),
            payload,
            branch: branch.to_string(),
            created_at: now(),
        })
    }

    /// Insert a new version of an object. Old versions stay readable.
    pub fn put_version(
        &mut self,
        kind: &str,
        id: &str,
        payload: Value,
        seq: i64,
    ) -> anyhow::Result<model::VersionedObject> {
        let prev = self.latest(kind, id)?;
        let version = prev.as_ref().map(|p| p.version + 1).unwrap_or(1);
        let hash = model::content_hash(kind, id, version, &payload);
        let supersedes = prev.map(|p| p.content_hash);
        self.conn.execute(
            "INSERT INTO objects(kind,id,version,content_hash,payload,supersedes,seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                kind,
                id,
                version,
                hash,
                serde_json::to_string(&payload)?,
                supersedes,
                seq
            ],
        )?;
        Ok(model::VersionedObject {
            kind: kind.to_string(),
            id: id.to_string(),
            version,
            content_hash: hash,
            payload,
            supersedes,
            seq,
        })
    }

    pub fn latest(&self, kind: &str, id: &str) -> anyhow::Result<Option<model::VersionedObject>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind,id,version,content_hash,payload,supersedes,seq FROM objects
             WHERE kind=?1 AND id=?2 ORDER BY version DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![kind, id], row_to_obj)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_version(
        &self,
        kind: &str,
        id: &str,
        version: i64,
    ) -> anyhow::Result<Option<model::VersionedObject>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind,id,version,content_hash,payload,supersedes,seq FROM objects
             WHERE kind=?1 AND id=?2 AND version=?3",
        )?;
        let mut rows = stmt.query_map(params![kind, id, version], row_to_obj)?;
        Ok(rows.next().transpose()?)
    }

    /// Latest version of every object of a kind, as of `as_of_seq` (time travel).
    pub fn latest_all_as_of(
        &self,
        kind: &str,
        as_of_seq: i64,
    ) -> anyhow::Result<Vec<model::VersionedObject>> {
        let mut stmt = self.conn.prepare(
            "SELECT o.kind,o.id,o.version,o.content_hash,o.payload,o.supersedes,o.seq
             FROM objects o
             JOIN (SELECT id, MAX(version) mv FROM objects
                   WHERE kind=?1 AND seq<=?2 GROUP BY id) t
             ON o.id=t.id AND o.version=t.mv
             WHERE o.kind=?1 AND o.seq<=?2
             ORDER BY o.id",
        )?;
        let rows = stmt.query_map(params![kind, as_of_seq], row_to_obj)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn events_as_of(&self, as_of_seq: i64) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,event_id,kind,payload,branch,created_at FROM events
             WHERE seq<=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![as_of_seq], |r| {
            Ok(Event {
                seq: r.get(0)?,
                event_id: r.get(1)?,
                kind: r.get(2)?,
                payload: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                branch: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn head_seq(&self) -> i64 {
        self.conn
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .unwrap_or(0)
    }
}

fn row_to_obj(r: &rusqlite::Row) -> rusqlite::Result<model::VersionedObject> {
    Ok(model::VersionedObject {
        kind: r.get(0)?,
        id: r.get(1)?,
        version: r.get(2)?,
        content_hash: r.get(3)?,
        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
        supersedes: r.get(5)?,
        seq: r.get(6)?,
    })
}

fn migrate_v1(tx: &Connection) -> anyhow::Result<()> {
    tx.execute_batch(
        "CREATE TABLE events(
            seq INTEGER PRIMARY KEY,
            event_id TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE objects(
            kind TEXT NOT NULL,
            id TEXT NOT NULL,
            version INTEGER NOT NULL,
            content_hash TEXT NOT NULL UNIQUE,
            payload TEXT NOT NULL,
            supersedes TEXT,
            seq INTEGER NOT NULL,
            PRIMARY KEY(kind,id,version)
        );",
    )?;
    Ok(())
}

/// v2 adds branch column to events (branching adjudications).
fn migrate_v2(tx: &Connection) -> anyhow::Result<()> {
    tx.execute_batch("ALTER TABLE events ADD COLUMN branch TEXT NOT NULL DEFAULT 'main';")?;
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
