use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::Mutex;

/// Schema migrations, applied in order. Each entry is one version.
pub const MIGRATIONS: &[&str] = &[
    // v1: versioned artifacts + machine-readable event log
    "CREATE TABLE artifacts(
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        version INTEGER NOT NULL,
        branch TEXT NOT NULL DEFAULT 'main',
        payload TEXT NOT NULL,
        raw TEXT,
        unit TEXT,
        source TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(kind, name, version, branch)
    );
    CREATE TABLE events(
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL,
        branch TEXT NOT NULL DEFAULT 'main',
        created_at TEXT NOT NULL
    );",
    // v2: explicit dependency edges between pinned artifact versions
    "CREATE TABLE dependencies(
        dependent TEXT NOT NULL,
        dep TEXT NOT NULL,
        PRIMARY KEY(dependent, dep)
    );",
    // v3: freshness status (fresh | stale | diagnostic), values never rewritten
    "ALTER TABLE artifacts ADD COLUMN status TEXT NOT NULL DEFAULT 'fresh';",
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Artifact {
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub branch: String,
    pub payload: Value,
    pub raw: Option<String>,
    pub unit: Option<String>,
    pub source: String,
    pub status: String,
    pub created_seq: i64,
}

impl Artifact {
    pub fn key(&self) -> String {
        key(&self.kind, &self.name, self.version, &self.branch)
    }
    pub fn identity(&self) -> String {
        format!("{}:{}@v{}[{}]", self.kind, self.name, self.version, self.branch)
    }
}

pub fn key(kind: &str, name: &str, version: i64, branch: &str) -> String {
    format!("{}|{}|{}|{}", kind, name, version, branch)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Event {
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub branch: String,
    pub created_at: String,
}

pub struct Store {
    pub conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        migrate(&conn)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    pub fn open_memory() -> rusqlite::Result<Store> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Store { conn: Mutex::new(conn) })
    }

    /// Append a machine-readable event; returns its global sequence number.
    pub fn record_event(&self, kind: &str, payload: Value, branch: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events(kind, payload, branch, created_at) VALUES (?1,?2,?3,datetime('now'))",
            params![kind, payload.to_string(), branch],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    pub fn put_artifact(&self, a: &Artifact) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO artifacts(kind,name,version,branch,payload,raw,unit,source,created_seq,status)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                a.kind, a.name, a.version, a.branch,
                a.payload.to_string(), a.raw, a.unit, a.source, a.created_seq, a.status
            ],
        )
        .unwrap();
    }

    fn row_to_artifact(r: &rusqlite::Row) -> rusqlite::Result<Artifact> {
        let payload: String = r.get(4)?;
        Ok(Artifact {
            kind: r.get(0)?,
            name: r.get(1)?,
            version: r.get(2)?,
            branch: r.get(3)?,
            payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
            raw: r.get(5)?,
            unit: r.get(6)?,
            source: r.get(7)?,
            created_seq: r.get(8)?,
            status: r.get(9)?,
        })
    }

    const COLS: &'static str = "kind,name,version,branch,payload,raw,unit,source,created_seq,status";

    pub fn artifact_at(&self, kind: &str, name: &str, version: i64, branch: &str) -> Option<Artifact> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {} FROM artifacts WHERE kind=?1 AND name=?2 AND version=?3 AND branch=?4", Self::COLS),
            params![kind, name, version, branch],
            Self::row_to_artifact,
        )
        .ok()
    }

    pub fn latest(&self, kind: &str, name: &str, branch: &str) -> Option<Artifact> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {} FROM artifacts WHERE kind=?1 AND name=?2 AND branch=?3 ORDER BY version DESC LIMIT 1", Self::COLS),
            params![kind, name, branch],
            Self::row_to_artifact,
        )
        .ok()
    }

    /// Latest version per (kind,name,branch) visible at `as_of` (None = now).
    pub fn list_all(&self, as_of: Option<i64>) -> Vec<Artifact> {
        let conn = self.conn.lock().unwrap();
        let sql = match as_of {
            Some(_) => format!("SELECT {} FROM artifacts WHERE created_seq <= ?1 ORDER BY kind,name,branch,version", Self::COLS),
            None => format!("SELECT {} FROM artifacts ORDER BY kind,name,branch,version", Self::COLS),
        };
        let mut stmt = conn.prepare(&sql).unwrap();
        let rows: Vec<Artifact> = match as_of {
            Some(s) => stmt.query_map([s], Self::row_to_artifact).unwrap().map(|r| r.unwrap()).collect(),
            None => stmt.query_map([], Self::row_to_artifact).unwrap().map(|r| r.unwrap()).collect(),
        };
        let mut latest: std::collections::BTreeMap<(String, String, String), Artifact> = Default::default();
        for a in rows {
            latest.insert((a.kind.clone(), a.name.clone(), a.branch.clone()), a);
        }
        latest.into_values().collect()
    }

    pub fn events(&self, as_of: Option<i64>) -> Vec<Event> {
        let conn = self.conn.lock().unwrap();
        let (sql, p): (String, Vec<i64>) = match as_of {
            Some(s) => ("SELECT seq,kind,payload,branch,created_at FROM events WHERE seq <= ?1 ORDER BY seq".into(), vec![s]),
            None => ("SELECT seq,kind,payload,branch,created_at FROM events ORDER BY seq".into(), vec![]),
        };
        let mut stmt = conn.prepare(&sql).unwrap();
        let map = |r: &rusqlite::Row| -> rusqlite::Result<Event> {
            let payload: String = r.get(2)?;
            Ok(Event { seq: r.get(0)?, kind: r.get(1)?, payload: serde_json::from_str(&payload).unwrap_or(Value::Null), branch: r.get(3)?, created_at: r.get(4)? })
        };
        match as_of {
            Some(_) => stmt.query_map(params![p[0]], map).unwrap().map(|r| r.unwrap()).collect(),
            None => stmt.query_map([], map).unwrap().map(|r| r.unwrap()).collect(),
        }
    }

    pub fn max_seq(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0)).unwrap()
    }

    pub fn add_dependency(&self, dependent: &str, dep: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR IGNORE INTO dependencies(dependent, dep) VALUES (?1,?2)", params![dependent, dep]).unwrap();
    }

    /// All keys that transitively depend on `target` (BFS over dependency edges).
    pub fn dependents_of(&self, target: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        let mut frontier = vec![target.to_string()];
        while let Some(t) = frontier.pop() {
            let mut stmt = conn.prepare("SELECT dependent FROM dependencies WHERE dep=?1").unwrap();
            let direct: Vec<String> = stmt.query_map([&t], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
            for d in direct {
                if seen.insert(d.clone()) {
                    frontier.push(d);
                }
            }
        }
        seen.into_iter().collect()
    }

    /// Status transitions only ever move fresh -> stale/diagnostic; payloads are immutable.
    pub fn set_status(&self, kind: &str, name: &str, version: i64, branch: &str, status: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE artifacts SET status=?5 WHERE kind=?1 AND name=?2 AND version=?3 AND branch=?4",
            params![kind, name, version, branch, status],
        )
        .unwrap();
    }

    /// Fold the event log into the currently effective rulings (undo-aware).
    pub fn effective_rulings(&self, as_of: Option<i64>) -> Vec<(String, Value, i64)> {
        let events = self.events(as_of);
        let mut by_target: std::collections::BTreeMap<String, (Value, i64)> = Default::default();
        for e in &events {
            match e.kind.as_str() {
                "ruling" => {
                    if let Some(t) = e.payload.get("target").and_then(|v| v.as_str()) {
                        by_target.insert(t.to_string(), (e.payload.clone(), e.seq));
                    }
                }
                "undo" => {
                    if let Some(u) = e.payload.get("undoes").and_then(|v| v.as_i64()) {
                        if let Some(ev) = events.iter().find(|x| x.seq == u) {
                            if let Some(t) = ev.payload.get("target").and_then(|v| v.as_str()) {
                                by_target.remove(t);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        by_target.into_iter().map(|(t, (p, s))| (t, p, s)).collect()
    }

    pub fn export_dump(&self) -> Value {
        let artifacts = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(&format!("SELECT {} FROM artifacts ORDER BY created_seq, kind, name, version, branch", Self::COLS)).unwrap();
            stmt.query_map([], Self::row_to_artifact).unwrap().map(|r| r.unwrap()).collect::<Vec<_>>()
        };
        let events = self.events(None);
        let deps = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT dependent, dep FROM dependencies ORDER BY dependent, dep").unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .map(|r| {
                    let (dependent, dep) = r.unwrap();
                    json!({"dependent": dependent, "dep": dep})
                })
                .collect::<Vec<_>>()
        };
        json!({"format": "flow-lineage-dump/1", "artifacts": artifacts, "events": events, "dependencies": deps})
    }

    /// Atomic import: any failure rolls back the whole change, never a half record.
    pub fn import_dump(&self, dump: &Value) -> Result<(), String> {
        if dump.get("format").and_then(|v| v.as_str()) != Some("flow-lineage-dump/1") {
            return Err("unrecognized dump format".into());
        }
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = || -> Result<(), String> {
            for e in dump.get("events").and_then(|v| v.as_array()).ok_or("missing events")? {
                tx.execute(
                    "INSERT INTO events(seq,kind,payload,branch,created_at) VALUES (?1,?2,?3,?4,?5)",
                    params![
                        e["seq"].as_i64().ok_or("event.seq")?,
                        e["kind"].as_str().ok_or("event.kind")?,
                        e["payload"].to_string(),
                        e["branch"].as_str().unwrap_or("main"),
                        e["created_at"].as_str().unwrap_or(""),
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            for a in dump.get("artifacts").and_then(|v| v.as_array()).ok_or("missing artifacts")? {
                tx.execute(
                    "INSERT INTO artifacts(kind,name,version,branch,payload,raw,unit,source,created_seq,status)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        a["kind"].as_str().ok_or("artifact.kind")?,
                        a["name"].as_str().ok_or("artifact.name")?,
                        a["version"].as_i64().ok_or("artifact.version")?,
                        a["branch"].as_str().unwrap_or("main"),
                        a["payload"].to_string(),
                        a["raw"].as_str(),
                        a["unit"].as_str(),
                        a["source"].as_str().unwrap_or("import"),
                        a["created_seq"].as_i64().ok_or("artifact.created_seq")?,
                        a["status"].as_str().unwrap_or("fresh"),
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            for d in dump.get("dependencies").and_then(|v| v.as_array()).ok_or("missing dependencies")? {
                tx.execute(
                    "INSERT OR IGNORE INTO dependencies(dependent, dep) VALUES (?1,?2)",
                    params![d["dependent"].as_str().ok_or("dep.dependent")?, d["dep"].as_str().ok_or("dep.dep")?],
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        match run() {
            Ok(()) => tx.commit().map_err(|e| e.to_string()),
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    }
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
    )?;
    let current: i64 = conn.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = (i + 1) as i64;
        if v > current {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO schema_migrations(version, applied_at) VALUES (?1, datetime('now'))", params![v])?;
            tx.commit()?;
        }
    }
    Ok(())
}
