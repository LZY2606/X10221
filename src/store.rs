//! SQLite store with explicit numbered schema migrations.

use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

pub const LATEST_MIGRATION: u32 = 3;

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    Conflict {
        draft_id: String,
        base_rev: i64,
        current_rev: i64,
        theirs: Value,
        yours: Value,
    },
    Other(String),
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sql(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Other(e.to_string())
    }
}
impl StoreError {
    pub fn other(s: impl Into<String>) -> Self {
        StoreError::Other(s.into())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

const M1: &str = "
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);
CREATE TABLE branches (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    base_event_seq INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'active'
);
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    branch_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    subject_id TEXT,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE layers (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    version INTEGER NOT NULL,
    supersedes TEXT,
    raw_repr TEXT NOT NULL,
    content TEXT NOT NULL,
    units TEXT NOT NULL,
    hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    created_seq INTEGER NOT NULL DEFAULT 0,
    UNIQUE(kind, name, version)
);
CREATE TABLE candidates (
    id TEXT PRIMARY KEY,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    label TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    decided_event_id TEXT
);
CREATE TABLE derived (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    inputs TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'valid',
    created_event_seq INTEGER NOT NULL
);
CREATE TABLE deps (
    from_id TEXT NOT NULL,
    to_id TEXT NOT NULL,
    rel TEXT NOT NULL,
    PRIMARY KEY(from_id, to_id, rel)
);
CREATE TABLE drafts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    rev INTEGER NOT NULL,
    content TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
";

const M2: &str = "
ALTER TABLE derived ADD COLUMN stale_reason TEXT;
ALTER TABLE derived ADD COLUMN stale_event_seq INTEGER;
CREATE INDEX idx_deps_to ON deps(to_id);
CREATE INDEX idx_events_branch ON events(branch_id, seq);
";

const M3: &str = "
CREATE INDEX idx_layers_kind_ver ON layers(kind, version);
CREATE INDEX idx_derived_branch ON derived(branch_id, kind, status);
";

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let mut s = Store { conn };
        s.migrate_to(LATEST_MIGRATION)?;
        Ok(s)
    }

    /// Apply migrations up to and including `target`. Normal startup uses
    /// LATEST_MIGRATION; the migration test deliberately stops at v1 first.
    pub fn migrate_to(&mut self, target: u32) -> Result<()> {
        let current: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        for v in (current as u32 + 1)..=target {
            let sql = match v {
                1 => M1,
                2 => M2,
                3 => M3,
                _ => return Err(StoreError::other(format!("unknown migration {v}"))),
            };
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?, ?)",
                params![v as i64, now_millis()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn applied_migrations(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- branches -------------------------------------------------------

    pub fn ensure_branch(&mut self, id: &str, name: &str, base_seq: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO branches(id,name,base_event_seq,status) VALUES (?,?,?,?)",
            params![id, name, base_seq, "active"],
        )?;
        Ok(())
    }

    pub fn branches(&self) -> Result<Vec<Branch>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,base_event_seq,status FROM branches ORDER BY rowid")?;
        let rows = stmt.query_map([], |r| {
            Ok(Branch {
                id: r.get(0)?,
                name: r.get(1)?,
                base_event_seq: r.get(2)?,
                status: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- events ---------------------------------------------------------

    pub fn add_event(
        &mut self,
        branch_id: &str,
        kind: &str,
        subject_id: Option<&str>,
        payload: &Value,
    ) -> Result<Event> {
        let id = new_id("evt");
        let ts = now_millis();
        self.conn.execute(
            "INSERT INTO events(id,branch_id,kind,subject_id,payload,created_at)
             VALUES (?,?,?,?,?,?)",
            params![id, branch_id, kind, subject_id, payload.to_string(), ts],
        )?;
        let seq = self.conn.last_insert_rowid();
        Ok(Event {
            seq,
            id,
            branch_id: branch_id.to_string(),
            kind: kind.to_string(),
            subject_id: subject_id.map(|s| s.to_string()),
            payload: payload.clone(),
            created_at: ts,
        })
    }

    pub fn events(&self) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,id,branch_id,kind,subject_id,payload,created_at FROM events ORDER BY seq",
        )?;
        let rows = stmt.query_map([], map_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn events_up_to(&self, seq: i64) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,id,branch_id,kind,subject_id,payload,created_at
             FROM events WHERE seq <= ? ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![seq], map_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn event(&self, seq: i64) -> Result<Option<Event>> {
        self.conn
            .query_row(
                "SELECT seq,id,branch_id,kind,subject_id,payload,created_at
                 FROM events WHERE seq = ?",
                params![seq],
                map_event,
            )
            .optional()
    }

    // ---- layers ---------------------------------------------------------

    pub fn insert_layer(&self, tx: &Transaction<'_>, layer: &LayerVersion) -> Result<()> {
        tx.execute(
            "INSERT INTO layers(id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq)
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                layer.id, layer.kind, layer.name, layer.version, layer.supersedes,
                layer.raw_repr, layer.content.to_string(), layer.units.to_string(),
                layer.hash, layer.created_at, layer.created_seq
            ],
        )?;
        Ok(())
    }

    pub fn latest_version(&self, kind: &str) -> Result<Option<LayerVersion>> {
        self.conn
            .query_row(
                "SELECT id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq
                 FROM layers WHERE kind=? ORDER BY version DESC LIMIT 1",
                params![kind],
                map_layer,
            )
            .optional()
    }

    pub fn layer_by_id(&self, id: &str) -> Result<Option<LayerVersion>> {
        self.conn
            .query_row(
                "SELECT id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq
                 FROM layers WHERE id=?",
                params![id],
                map_layer,
            )
            .optional()
    }

    /// Latest version of a layer created at or before event `seq`.
    pub fn layer_as_of(&self, kind: &str, seq: i64) -> Result<Option<LayerVersion>> {
        self.conn
            .query_row(
                "SELECT id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq
                 FROM layers WHERE kind=? AND created_seq<=? ORDER BY version DESC LIMIT 1",
                params![kind, seq],
                map_layer,
            )
            .optional()
    }

    pub fn all_layers(&self) -> Result<Vec<LayerVersion>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq
             FROM layers ORDER BY kind, version",
        )?;
        let rows = stmt.query_map([], map_layer)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- candidates -----------------------------------------------------

    pub fn insert_candidate(&self, c: &Candidate) -> Result<()> {
        self.conn.execute(
            "INSERT INTO candidates(id,subject_kind,subject_id,branch_id,label,payload,status,decided_event_id)
             VALUES (?,?,?,?,?,?,?,?)",
            params![
                c.id, c.subject_kind, c.subject_id, c.branch_id, c.label,
                c.payload.to_string(), c.status, c.decided_event_id
            ],
        )?;
        Ok(())
    }

    pub fn candidates_for(&self, subject_id: &str) -> Result<Vec<Candidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,subject_kind,subject_id,branch_id,label,payload,status,decided_event_id
             FROM candidates WHERE subject_id=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![subject_id], map_candidate)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn candidate(&self, id: &str) -> Result<Option<Candidate>> {
        self.conn
            .query_row(
                "SELECT id,subject_kind,subject_id,branch_id,label,payload,status,decided_event_id
                 FROM candidates WHERE id=?",
                params![id],
                map_candidate,
            )
            .optional()
    }

    pub fn all_candidates(&self) -> Result<Vec<Candidate>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,subject_kind,subject_id,branch_id,label,payload,status,decided_event_id
             FROM candidates ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], map_candidate)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_candidate_status(
        &mut self,
        id: &str,
        status: &str,
        decided_event_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE candidates SET status=?, decided_event_id=? WHERE id=?",
            params![status, decided_event_id, id],
        )?;
        Ok(())
    }

    // ---- derived & deps -------------------------------------------------

    pub fn insert_derived(&self, d: &Derived) -> Result<()> {
        self.conn.execute(
            "INSERT INTO derived(id,kind,branch_id,inputs,payload,status,created_event_seq,stale_reason,stale_event_seq)
             VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                d.id, d.kind, d.branch_id, d.inputs.to_string(), d.payload.to_string(),
                d.status, d.created_event_seq,
                d.stale_reason.as_ref().map(|v| v.to_string()), d.stale_event_seq
            ],
        )?;
        Ok(())
    }

    pub fn all_derived(&self) -> Result<Vec<Derived>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,branch_id,inputs,payload,status,created_event_seq,stale_reason,stale_event_seq
             FROM derived ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], map_derived)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn derived_up_to(&self, seq: i64) -> Result<Vec<Derived>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,kind,branch_id,inputs,payload,status,created_event_seq,stale_reason,stale_event_seq
             FROM derived WHERE created_event_seq<=? ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![seq], map_derived)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn derived_by_id(&self, id: &str) -> Result<Option<Derived>> {
        self.conn
            .query_row(
                "SELECT id,kind,branch_id,inputs,payload,status,created_event_seq,stale_reason,stale_event_seq
                 FROM derived WHERE id=?",
                params![id],
                map_derived,
            )
            .optional()
    }

    pub fn set_derived_status(
        &mut self,
        id: &str,
        status: &str,
        stale_reason: Option<&Value>,
        stale_event_seq: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE derived SET status=?, stale_reason=?, stale_event_seq=? WHERE id=?",
            params![
                status,
                stale_reason.map(|v| v.to_string()),
                stale_event_seq,
                id
            ],
        )?;
        Ok(())
    }

    pub fn add_dep(&self, from_id: &str, to_id: &str, rel: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO deps(from_id,to_id,rel) VALUES (?,?,?)",
            params![from_id, to_id, rel],
        )?;
        Ok(())
    }

    pub fn deps(&self) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT from_id,to_id,rel FROM deps ORDER BY from_id,to_id")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// All derived ids that transitively depend on `id` (layer or derived).
    pub fn dependents_of(&self, id: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![id.to_string()];
        let mut seen = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur.clone()) {
                continue;
            }
            let mut stmt = self
                .conn
                .prepare("SELECT from_id FROM deps WHERE to_id=?")?;
            let rows = stmt.query_map(params![cur], |r| r.get::<_, String>(0))?;
            for r in rows {
                let f = r?;
                if !seen.contains(&f) {
                    out.push(f.clone());
                    stack.push(f);
                }
            }
        }
        Ok(out)
    }

    // ---- drafts (optimistic concurrency) --------------------------------

    pub fn upsert_draft_new(&mut self, name: &str, content: &Value) -> Result<Draft> {
        let d = Draft {
            id: new_id("drf"),
            name: name.to_string(),
            rev: 1,
            content: content.clone(),
            updated_at: now_millis(),
        };
        self.conn.execute(
            "INSERT INTO drafts(id,name,rev,content,updated_at) VALUES (?,?,?,?,?)",
            params![d.id, d.name, d.rev, d.content.to_string(), d.updated_at],
        )?;
        Ok(d)
    }

    pub fn draft(&self, id: &str) -> Result<Option<Draft>> {
        self.conn
            .query_row(
                "SELECT id,name,rev,content,updated_at FROM drafts WHERE id=?",
                params![id],
                map_draft,
            )
            .optional()
    }

    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,rev,content,updated_at FROM drafts ORDER BY name")?;
        let rows = stmt.query_map([], map_draft)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Save with optimistic concurrency: caller must present the rev it read.
    /// A mismatch yields a Conflict carrying both sides for the UI to show.
    pub fn save_draft(&mut self, id: &str, base_rev: i64, content: &Value) -> Result<Draft> {
        let cur = self
            .draft(id)?
            .ok_or_else(|| StoreError::other(format!("draft {id} not found")))?;
        if cur.rev != base_rev {
            return Err(StoreError::Conflict {
                draft_id: id.to_string(),
                base_rev,
                current_rev: cur.rev,
                theirs: cur.content.clone(),
                yours: content.clone(),
            });
        }
        let new_rev = cur.rev + 1;
        let ts = now_millis();
        self.conn.execute(
            "UPDATE drafts SET rev=?, content=?, updated_at=? WHERE id=? AND rev=?",
            params![new_rev, content.to_string(), ts, id, base_rev],
        )?;
        Ok(Draft {
            id: id.to_string(),
            name: cur.name,
            rev: new_rev,
            content: content.clone(),
            updated_at: ts,
        })
    }

    // ---- export / import ------------------------------------------------

    pub fn export_all(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "format": "flowbound-export/1",
            "branches": self.branches()?,
            "events": self.events()?,
            "layers": self.all_layers()?,
            "candidates": self.all_candidates()?,
            "derived": self.all_derived()?,
            "deps": self.deps()?.iter().map(|(a,b,r)| serde_json::json!({"from":a,"to":b,"rel":r})).collect::<Vec<_>>(),
            "drafts": self.drafts()?,
        }))
    }

    /// Import into an (expected empty) database inside one transaction:
    /// a failure rolls back the whole import, never half a record set.
    pub fn import_all(&mut self, dump: &Value) -> Result<()> {
        let tx = self.conn.transaction()?;
        for b in dump["branches"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO branches(id,name,base_event_seq,status) VALUES (?,?,?,?)",
                params![b["id"], b["name"], b["base_event_seq"], b["status"]],
            )?;
        }
        for e in dump["events"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO events(seq,id,branch_id,kind,subject_id,payload,created_at)
                 VALUES (?,?,?,?,?,?,?)",
                params![
                    e["seq"], e["id"], e["branch_id"], e["kind"],
                    e["subject_id"], e["payload"].to_string(), e["created_at"]
                ],
            )?;
        }
        for l in dump["layers"].as_array().cloned().unwrap_or_default() {
            let raw = l["raw_repr"].as_str().unwrap_or("").to_string();
            let hash = sha256_hex(&raw);
            if hash != l["hash"].as_str().unwrap_or("") {
                return Err(StoreError::other(format!(
                    "hash mismatch on layer {}",
                    l["id"]
                )));
            }
            tx.execute(
                "INSERT INTO layers(id,kind,name,version,supersedes,raw_repr,content,units,hash,created_at,created_seq)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    l["id"], l["kind"], l["name"], l["version"], l["supersedes"],
                    raw, l["content"].to_string(), l["units"].to_string(),
                    l["hash"], l["created_at"], l["created_seq"]
                ],
            )?;
        }
        for c in dump["candidates"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO candidates(id,subject_kind,subject_id,branch_id,label,payload,status,decided_event_id)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![
                    c["id"], c["subject_kind"], c["subject_id"], c["branch_id"],
                    c["label"], c["payload"].to_string(), c["status"], c["decided_event_id"]
                ],
            )?;
        }
        for d in dump["derived"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO derived(id,kind,branch_id,inputs,payload,status,created_event_seq,stale_reason,stale_event_seq)
                 VALUES (?,?,?,?,?,?,?,?,?)",
                params![
                    d["id"], d["kind"], d["branch_id"], d["inputs"].to_string(),
                    d["payload"].to_string(), d["status"], d["created_event_seq"],
                    d["stale_reason"].as_ref().map(|v| v.to_string()), d["stale_event_seq"]
                ],
            )?;
        }
        for dep in dump["deps"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO deps(from_id,to_id,rel) VALUES (?,?,?)",
                params![dep["from"], dep["to"], dep["rel"]],
            )?;
        }
        for d in dump["drafts"].as_array().cloned().unwrap_or_default() {
            tx.execute(
                "INSERT INTO drafts(id,name,rev,content,updated_at) VALUES (?,?,?,?,?)",
                params![d["id"], d["name"], d["rev"], d["content"].to_string(), d["updated_at"]],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

// ---- row mappers ---------------------------------------------------------

fn map_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    let payload: String = r.get(5)?;
    Ok(Event {
        seq: r.get(0)?,
        id: r.get(1)?,
        branch_id: r.get(2)?,
        kind: r.get(3)?,
        subject_id: r.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        created_at: r.get(6)?,
    })
}

fn map_layer(r: &rusqlite::Row<'_>) -> rusqlite::Result<LayerVersion> {
    let content: String = r.get(6)?;
    let units: String = r.get(7)?;
    Ok(LayerVersion {
        id: r.get(0)?,
        kind: r.get(1)?,
        name: r.get(2)?,
        version: r.get(3)?,
        supersedes: r.get(4)?,
        raw_repr: r.get(5)?,
        content: serde_json::from_str(&content).unwrap_or(Value::Null),
        units: serde_json::from_str(&units).unwrap_or(Value::Null),
        hash: r.get(8)?,
        created_at: r.get(9)?,
        created_seq: r.get(10)?,
    })
}

fn map_candidate(r: &rusqlite::Row<'_>) -> rusqlite::Result<Candidate> {
    let payload: String = r.get(5)?;
    Ok(Candidate {
        id: r.get(0)?,
        subject_kind: r.get(1)?,
        subject_id: r.get(2)?,
        branch_id: r.get(3)?,
        label: r.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        status: r.get(6)?,
        decided_event_id: r.get(7)?,
    })
}

fn map_derived(r: &rusqlite::Row<'_>) -> rusqlite::Result<Derived> {
    let inputs: String = r.get(3)?;
    let payload: String = r.get(4)?;
    let stale_reason: Option<String> = r.get(7)?;
    Ok(Derived {
        id: r.get(0)?,
        kind: r.get(1)?,
        branch_id: r.get(2)?,
        inputs: serde_json::from_str(&inputs).unwrap_or(Value::Null),
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        status: r.get(5)?,
        created_event_seq: r.get(6)?,
        stale_reason: stale_reason.and_then(|s| serde_json::from_str(&s).ok()),
        stale_event_seq: r.get(8)?,
    })
}

fn map_draft(r: &rusqlite::Row<'_>) -> rusqlite::Result<Draft> {
    let content: String = r.get(3)?;
    Ok(Draft {
        id: r.get(0)?,
        name: r.get(1)?,
        rev: r.get(2)?,
        content: serde_json::from_str(&content).unwrap_or(Value::Null),
        updated_at: r.get(4)?,
    })
}
