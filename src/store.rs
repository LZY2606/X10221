use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::model::{canonical, event_id, Event, Id, Object};

pub type Res<T> = Result<T, String>;

/// Migration chain. The app only ever reaches the current schema by applying
/// every step from an older version, so state migrations are exercised.
pub const MIGRATIONS: &[&str] = &[
    // v1: content-addressed objects, raw blobs, append-only event log, branches
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
     CREATE TABLE objects (
       obj_id TEXT PRIMARY KEY,
       kind TEXT NOT NULL,
       logical_key TEXT NOT NULL,
       version_no INTEGER NOT NULL,
       prev_obj_id TEXT,
       payload TEXT NOT NULL,
       seq INTEGER NOT NULL
     );
     CREATE INDEX idx_objects_key ON objects(logical_key);
     CREATE TABLE blobs (blob_id TEXT PRIMARY KEY, bytes BLOB NOT NULL);
     CREATE TABLE events (
       event_id TEXT PRIMARY KEY,
       kind TEXT NOT NULL,
       parents TEXT NOT NULL,
       payload TEXT NOT NULL,
       seq INTEGER NOT NULL
     );
     CREATE TABLE branches (name TEXT PRIMARY KEY, head_event_id TEXT NOT NULL);",
    // v2: provenance dependency edges (derived object -> input object versions)
    "CREATE TABLE deps (
       derived_obj_id TEXT NOT NULL,
       input_obj_id TEXT NOT NULL,
       PRIMARY KEY (derived_obj_id, input_obj_id)
     );
     CREATE INDEX idx_deps_input ON deps(input_obj_id);",
    // v3: candidate sets for ambiguous decisions (multi-candidate branches)
    "CREATE TABLE candidates (
       set_id TEXT NOT NULL,
       candidate_id TEXT NOT NULL,
       payload TEXT NOT NULL,
       status TEXT NOT NULL DEFAULT 'pending',
       PRIMARY KEY (set_id, candidate_id)
     );",
];

#[derive(Debug, Clone)]
pub struct Conflict {
    pub branch: String,
    pub expected_head: Id,
    pub current_head: Id,
    pub events_between: Vec<Event>,
}

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> Res<Store> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn memory() -> Res<Store> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn user_version(&self) -> Res<u32> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .map_err(|e| e.to_string())
    }

    pub fn migrate(&self) -> Res<()> {
        let v = self.user_version()?;
        for (i, ddl) in MIGRATIONS.iter().enumerate() {
            if (i as u32) >= v {
                self.conn.execute_batch(ddl).map_err(|e| format!("migration {i}: {e}"))?;
                self.conn
                    .execute_batch(&format!("PRAGMA user_version = {};", i + 1))
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Res<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Res<()> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl Store {
    pub fn put_blob(&self, bytes: &[u8]) -> Res<Id> {
        let id = crate::model::blob_id(bytes);
        self.conn
            .execute("INSERT OR IGNORE INTO blobs(blob_id, bytes) VALUES(?1, ?2)", params![id, bytes])
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn get_blob(&self, id: &str) -> Res<Option<Vec<u8>>> {
        self.conn
            .query_row("SELECT bytes FROM blobs WHERE blob_id = ?1", params![id], |r| {
                let v: Vec<u8> = r.get(0)?;
                Ok(v)
            })
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn put_object(&self, obj: &Object) -> Res<()> {
        let seq: u64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(seq),0) + 1 FROM objects", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT OR IGNORE INTO objects(obj_id, kind, logical_key, version_no, prev_obj_id, payload, seq)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![obj.obj_id, obj.kind, obj.logical_key, obj.version_no, obj.prev, canonical(&obj.payload), seq],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_object(&self, id: &str) -> Res<Option<Object>> {
        self.conn
            .query_row(
                "SELECT obj_id, kind, logical_key, version_no, prev_obj_id, payload FROM objects WHERE obj_id = ?1",
                params![id],
                row_to_object,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn objects_by_key(&self, logical_key: &str) -> Res<Vec<Object>> {
        let mut stmt = self
            .conn
            .prepare("SELECT obj_id, kind, logical_key, version_no, prev_obj_id, payload FROM objects WHERE logical_key = ?1 ORDER BY version_no")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![logical_key], row_to_object)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn all_objects_ordered(&self) -> Res<Vec<Object>> {
        let mut stmt = self
            .conn
            .prepare("SELECT obj_id, kind, logical_key, version_no, prev_obj_id, payload FROM objects ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], row_to_object).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn add_deps(&self, derived: &str, inputs: &[String]) -> Res<()> {
        for input in inputs {
            self.conn
                .execute("INSERT OR IGNORE INTO deps(derived_obj_id, input_obj_id) VALUES(?1, ?2)", params![derived, input])
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Transitive dependent objects (provenance reachability).
    pub fn dependents_of(&self, obj_ids: &[String]) -> Res<Vec<String>> {
        let mut out = Vec::new();
        for root in obj_ids {
            let mut stmt = self
                .conn
                .prepare(
                    "WITH RECURSIVE d(obj) AS (
                       SELECT ?1
                       UNION
                       SELECT de.derived_obj_id FROM deps de JOIN d ON de.input_obj_id = d.obj
                     )
                     SELECT obj FROM d",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![root], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            for r in rows {
                out.push(r.map_err(|e| e.to_string())?);
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

fn row_to_object(r: &rusqlite::Row<'_>) -> rusqlite::Result<Object> {
    let payload_str: String = r.get(5)?;
    Ok(Object {
        obj_id: r.get(0)?,
        kind: r.get(1)?,
        logical_key: r.get(2)?,
        version_no: r.get(3)?,
        prev: r.get(4)?,
        payload: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
    })
}

impl Store {
    pub fn next_event_seq(&self) -> Res<u64> {
        self.conn
            .query_row("SELECT COALESCE(MAX(seq),0) + 1 FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    }

    pub fn append_event(&self, kind: &str, parents: &[Id], payload: &Value) -> Res<Event> {
        let id = event_id(kind, parents, payload);
        if let Some(existing) = self.get_event(&id)? {
            return Ok(existing);
        }
        let seq = self.next_event_seq()?;
        self.conn
            .execute(
                "INSERT INTO events(event_id, kind, parents, payload, seq) VALUES(?1,?2,?3,?4,?5)",
                params![id, kind, serde_json::to_string(parents).unwrap(), canonical(payload), seq],
            )
            .map_err(|e| e.to_string())?;
        Ok(Event { event_id: id, kind: kind.into(), parents: parents.to_vec(), payload: payload.clone(), seq })
    }

    pub fn get_event(&self, id: &str) -> Res<Option<Event>> {
        self.conn
            .query_row(
                "SELECT event_id, kind, parents, payload, seq FROM events WHERE event_id = ?1",
                params![id],
                row_to_event,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn events_ordered(&self) -> Res<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT event_id, kind, parents, payload, seq FROM events ORDER BY seq")
            .map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], row_to_event).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn branch_head(&self, name: &str) -> Res<Option<Id>> {
        self.conn
            .query_row("SELECT head_event_id FROM branches WHERE name = ?1", params![name], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn branches(&self) -> Res<Vec<(String, Id)>> {
        let mut stmt = self.conn.prepare("SELECT name, head_event_id FROM branches ORDER BY name").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// Compare-and-set a branch head. Returns Ok(false) on optimistic-lock mismatch.
    pub fn cas_branch_head(&self, name: &str, expected: Option<&Id>, new_head: &Id) -> Res<bool> {
        let cur = self.branch_head(name)?;
        match (expected, &cur) {
            (None, None) => {
                self.conn
                    .execute("INSERT INTO branches(name, head_event_id) VALUES(?1, ?2)", params![name, new_head])
                    .map_err(|e| e.to_string())?;
                Ok(true)
            }
            (Some(exp), Some(cur)) if exp == cur => {
                self.conn
                    .execute("UPDATE branches SET head_event_id = ?2 WHERE name = ?1", params![name, new_head])
                    .map_err(|e| e.to_string())?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn ensure_branch(&self, name: &str, head: &Id) -> Res<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO branches(name, head_event_id) VALUES(?1, ?2)", params![name, head])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_branch_head_direct(&self, name: &str, head: &Id) -> Res<()> {
        self.conn
            .execute(
                "INSERT INTO branches(name, head_event_id) VALUES(?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET head_event_id = excluded.head_event_id",
                params![name, head],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn upsert_candidate(&self, set_id: &str, candidate_id: &str, payload: &Value, status: &str) -> Res<()> {
        self.conn
            .execute(
                "INSERT INTO candidates(set_id, candidate_id, payload, status) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(set_id, candidate_id) DO UPDATE SET status = excluded.status",
                params![set_id, candidate_id, canonical(payload), status],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn candidates(&self) -> Res<Vec<(String, String, Value, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT set_id, candidate_id, payload, status FROM candidates ORDER BY set_id, rowid")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let p: String = r.get(2)?;
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, serde_json::from_str(&p).unwrap_or(Value::Null), r.get::<_, String>(3)?))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn all_blobs(&self) -> Res<Vec<(Id, Vec<u8>)>> {
        let mut stmt = self.conn.prepare("SELECT blob_id, bytes FROM blobs ORDER BY blob_id").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn wipe(&self) -> Res<()> {
        self.conn
            .execute_batch("DELETE FROM meta; DELETE FROM deps; DELETE FROM candidates; DELETE FROM events; DELETE FROM objects; DELETE FROM blobs; DELETE FROM branches;")
            .map_err(|e| e.to_string())
    }

    pub fn begin(&self) -> Res<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())
    }
    pub fn commit_tx(&self) -> Res<()> {
        self.conn.execute_batch("COMMIT").map_err(|e| e.to_string())
    }
    pub fn rollback(&self) {
        let _ = self.conn.execute_batch("ROLLBACK");
    }

    /// Events on the first-parent path from `current` back to (exclusive) `expected`.
    pub fn events_between(&self, current: &Id, expected: &Id) -> Res<Vec<Event>> {
        let mut out = Vec::new();
        let mut cur = Some(current.clone());
        while let Some(id) = cur {
            if &id == expected {
                break;
            }
            let ev = self
                .get_event(&id)?
                .ok_or_else(|| format!("event {id} not found"))?;
            cur = ev.parents.first().cloned();
            out.push(ev);
        }
        out.reverse();
        Ok(out)
    }
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let parents_str: String = r.get(2)?;
    let payload_str: String = r.get(3)?;
    Ok(Event {
        event_id: r.get(0)?,
        kind: r.get(1)?,
        parents: serde_json::from_str(&parents_str).unwrap_or_default(),
        payload: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
        seq: r.get(4)?,
    })
}
