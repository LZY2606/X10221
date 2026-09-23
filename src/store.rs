//! Append-only provenance store backed by SQLite.
//!
//! Design invariants:
//! - Raw sources (FCS events, spillover metadata, compensation matrices,
//!   transform parameters, gate polygons, parent/child hierarchy) are each
//!   versioned independently and never rewritten in place.
//! - Derived results record their exact input pins (`deps`) and are never
//!   mutated: staleness is a separate append-only `invalidations` stream, so
//!   replaying to any old event seq restores the conclusions visible then.
//! - Every state change is a machine-readable event in `events`.

use rusqlite::{params, Connection};
use serde_json::Value;

pub const MIGRATIONS: &[&str] = &[
    // v1: core append-only provenance schema
    "CREATE TABLE events(
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE sources(
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        kind TEXT NOT NULL,
        content TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(id, version)
    );
    CREATE TABLE derived(
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        inputs TEXT NOT NULL,
        content TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(id, version)
    );
    CREATE TABLE deps(
        derived_id TEXT NOT NULL,
        derived_version INTEGER NOT NULL,
        source_id TEXT NOT NULL,
        source_version INTEGER NOT NULL
    );
    CREATE TABLE invalidations(
        derived_id TEXT NOT NULL,
        derived_version INTEGER NOT NULL,
        seq INTEGER NOT NULL
    );
    CREATE TABLE kv(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // v2: decision branches on events and derived records
    "ALTER TABLE events ADD COLUMN branch TEXT NOT NULL DEFAULT 'main';
     ALTER TABLE derived ADD COLUMN branch TEXT NOT NULL DEFAULT 'main';",
    // v3: explicit multi-candidate sets for non-unique judgments
    "CREATE TABLE candidates(
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        status TEXT NOT NULL,
        options TEXT NOT NULL,
        resolved_by TEXT,
        created_seq INTEGER NOT NULL
    );
    CREATE INDEX idx_derived_status ON derived(status);
    CREATE INDEX idx_events_kind ON events(kind);",
];

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub branch: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SourceRecord {
    pub id: String,
    pub version: i64,
    pub kind: String,
    pub content: Value,
    pub created_seq: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DerivedRecord {
    pub id: String,
    pub version: i64,
    pub branch: String,
    pub kind: String,
    /// base status as recorded at creation: valid | blocked
    pub status: String,
    pub inputs: Vec<(String, i64)>,
    pub content: Value,
    pub created_seq: i64,
    /// effective status at the queried seq (stale if invalidated since)
    #[serde(default)]
    pub effective_status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CandidateSet {
    pub id: String,
    pub kind: String,
    pub status: String, // open | resolved
    pub options: Value,
    pub resolved_by: Option<String>,
    pub created_seq: i64,
}

pub struct Store {
    pub conn: Connection,
}

fn now_logical(seq: i64) -> String {
    format!("T+{:06}", seq)
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Ok(Store { conn })
    }

    pub fn open_memory() -> rusqlite::Result<Store> {
        Ok(Store {
            conn: Connection::open_in_memory()?,
        })
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        self.migrate_to(MIGRATIONS.len())
    }

    pub fn migrate_to(&self, upto: usize) -> rusqlite::Result<()> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (idx, sql) in MIGRATIONS.iter().enumerate() {
            let version = idx as i64 + 1;
            if version > current && idx < upto {
                self.conn.execute_batch(sql)?;
                self.conn
                    .execute_batch(&format!("PRAGMA user_version = {version}"))?;
            }
        }
        Ok(())
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))
    }

    // ---------- events ----------

    pub fn append_event(&self, branch: &str, kind: &str, payload: Value) -> rusqlite::Result<Event> {
        self.conn.execute(
            "INSERT INTO events(kind, payload, created_at, branch) VALUES (?1, ?2, '', ?3)",
            params![kind, payload.to_string(), branch],
        )?;
        let seq = self.conn.last_insert_rowid();
        let id = format!("evt-{seq:06}");
        let created_at = now_logical(seq);
        self.conn.execute(
            "UPDATE events SET id=?1, created_at=?2 WHERE seq=?3",
            params![id, created_at, seq],
        )?;
        Ok(Event {
            seq,
            id,
            branch: branch.to_string(),
            kind: kind.to_string(),
            payload,
            created_at,
        })
    }

    pub fn head_seq(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
    }

    pub fn events(&self, upto: Option<i64>) -> rusqlite::Result<Vec<Event>> {
        let limit = upto.unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT seq,id,kind,payload,created_at,branch FROM events WHERE seq<=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(Event {
                seq: r.get(0)?,
                id: r.get(1)?,
                kind: r.get(2)?,
                payload: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                created_at: r.get(4)?,
                branch: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    // ---------- sources ----------

    /// Create the next version of a source. Raw representation is stored
    /// verbatim; no irreversible cleaning happens here.
    pub fn put_source(&self, id: &str, kind: &str, content: &Value, seq: i64) -> rusqlite::Result<i64> {
        let version = self.source_head_version(id)? + 1;
        self.conn.execute(
            "INSERT INTO sources(id,version,kind,content,created_seq) VALUES (?1,?2,?3,?4,?5)",
            params![id, version, kind, content.to_string(), seq],
        )?;
        Ok(version)
    }

    pub fn source_head_version(&self, id: &str) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM sources WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
    }

    pub fn source_at(&self, id: &str, version: i64) -> rusqlite::Result<Option<SourceRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,version,kind,content,created_seq FROM sources WHERE id=?1 AND version=?2",
        )?;
        let mut rows = stmt.query_map(params![id, version], |r| {
            Ok(SourceRecord {
                id: r.get(0)?,
                version: r.get(1)?,
                kind: r.get(2)?,
                content: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                created_seq: r.get(4)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Latest version of every source visible at `seq` (time travel).
    pub fn head_sources(&self, seq: i64) -> rusqlite::Result<Vec<SourceRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id,s.version,s.kind,s.content,s.created_seq FROM sources s
             JOIN (SELECT id, MAX(version) v FROM sources WHERE created_seq<=?1 GROUP BY id) h
               ON s.id=h.id AND s.version=h.v ORDER BY s.id",
        )?;
        let rows = stmt.query_map(params![seq], |r| {
            Ok(SourceRecord {
                id: r.get(0)?,
                version: r.get(1)?,
                kind: r.get(2)?,
                content: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                created_seq: r.get(4)?,
            })
        })?;
        rows.collect()
    }

    // ---------- derived ----------

    pub fn put_derived(
        &self,
        branch: &str,
        id: &str,
        kind: &str,
        status: &str,
        inputs: &[(String, i64)],
        content: &Value,
        seq: i64,
    ) -> rusqlite::Result<i64> {
        let version: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM derived WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO derived(id,version,kind,status,inputs,content,created_seq,branch)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                version,
                kind,
                status,
                Value::from(
                    inputs
                        .iter()
                        .map(|(i, v)| serde_json::json!({"id": i, "version": v}))
                        .collect::<Vec<_>>()
                )
                .to_string(),
                content.to_string(),
                seq,
                branch
            ],
        )?;
        for (sid, sv) in inputs {
            self.conn.execute(
                "INSERT INTO deps(derived_id,derived_version,source_id,source_version) VALUES (?1,?2,?3,?4)",
                params![id, version, sid, sv],
            )?;
        }
        Ok(version)
    }

    /// Latest derived record per id visible at `seq`, with effective status.
    pub fn head_derived(&self, seq: i64) -> rusqlite::Result<Vec<DerivedRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id,d.version,d.kind,d.status,d.inputs,d.content,d.created_seq,d.branch
             FROM derived d
             JOIN (SELECT id, MAX(version) v FROM derived WHERE created_seq<=?1 GROUP BY id) h
               ON d.id=h.id AND d.version=h.v ORDER BY d.id",
        )?;
        let rows = stmt.query_map(params![seq], |r| {
            let inputs_raw: String = r.get(4)?;
            let inputs_json: Value = serde_json::from_str(&inputs_raw).unwrap_or(Value::Null);
            let inputs = inputs_json
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| {
                            (
                                p["id"].as_str().unwrap_or("").to_string(),
                                p["version"].as_i64().unwrap_or(0),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok((
                DerivedRecord {
                    id: r.get(0)?,
                    version: r.get(1)?,
                    kind: r.get(2)?,
                    status: r.get(3)?,
                    inputs,
                    content: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
                    created_seq: r.get(6)?,
                    branch: r.get(7)?,
                    effective_status: String::new(),
                },
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let mut d = r?;
            d.effective_status = self.effective_status(&d.id, d.version, &d.status, seq)?;
            out.push(d);
        }
        Ok(out)
    }

    pub fn effective_status(
        &self,
        id: &str,
        version: i64,
        base: &str,
        seq: i64,
    ) -> rusqlite::Result<String> {
        let invalidated: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM invalidations WHERE derived_id=?1 AND derived_version=?2 AND seq<=?3)",
            params![id, version, seq],
            |r| r.get(0),
        )?;
        Ok(if invalidated {
            "stale".to_string()
        } else {
            base.to_string()
        })
    }

    /// Mark derived records stale (append-only invalidation stream).
    pub fn invalidate(&self, derived_id: &str, derived_version: i64, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO invalidations(derived_id,derived_version,seq) VALUES (?1,?2,?3)",
            params![derived_id, derived_version, seq],
        )?;
        Ok(())
    }

    /// All currently-valid derived records that depend on `source_id`.
    pub fn dependents_of(&self, source_id: &str, seq: i64) -> rusqlite::Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT d.id, d.version FROM deps dp
             JOIN derived d ON d.id=dp.derived_id AND d.version=dp.derived_version
             WHERE dp.source_id=?1 AND d.created_seq<=?2
               AND NOT EXISTS(SELECT 1 FROM invalidations i
                              WHERE i.derived_id=d.id AND i.derived_version=d.version AND i.seq<=?2)",
        )?;
        let rows = stmt.query_map(params![source_id, seq], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    // ---------- candidates ----------

    pub fn put_candidates(&self, id: &str, kind: &str, options: &Value, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO candidates(id,kind,status,options,resolved_by,created_seq)
             VALUES (?1,?2,'open',?3,NULL,?4)",
            params![id, kind, options.to_string(), seq],
        )
    }

    pub fn resolve_candidates(&self, id: &str, event_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE candidates SET status='resolved', resolved_by=?1 WHERE id=?2",
            params![event_id, id],
        )
    }

    pub fn candidates(&self, seq: i64) -> rusqlite::Result<Vec<CandidateSet>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,status,options,resolved_by,created_seq FROM candidates WHERE created_seq<=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![seq], |r| {
            Ok(CandidateSet {
                id: r.get(0)?,
                kind: r.get(1)?,
                status: r.get(2)?,
                options: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
                resolved_by: r.get(4)?,
                created_seq: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    // ---------- kv ----------

    pub fn kv_get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM kv WHERE key=?1")?;
        let mut rows = stmt.query_map(params![key], |r| r.get(0))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn kv_put(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO kv(key,value) VALUES (?1,?2)",
            params![key, value],
        )
    }

    // ---------- maintenance ----------

    pub fn wipe(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM invalidations; DELETE FROM deps; DELETE FROM derived;
             DELETE FROM sources; DELETE FROM events; DELETE FROM candidates; DELETE FROM kv;
             DELETE FROM sqlite_sequence WHERE name='events';",
        )
    }
}
