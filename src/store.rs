use crate::model::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// 状态迁移：v1 基础表；v2 增加审阅批注表；v3 给 events 增加 supersedes 列。
pub const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE branches(
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            parent TEXT,
            base_seq INTEGER NOT NULL,
            head_seq INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'open',
            created_at TEXT NOT NULL
        );
        CREATE TABLE artifacts(
            id TEXT PRIMARY KEY,
            branch_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            version INTEGER NOT NULL,
            payload TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(branch_id, kind, version)
        );
        CREATE TABLE derived(
            id TEXT PRIMARY KEY,
            branch_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            deps TEXT NOT NULL,
            payload TEXT NOT NULL,
            status TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE events(
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL,
            branch_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE decisions(
            id TEXT PRIMARY KEY,
            branch_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            ruling TEXT NOT NULL,
            payload TEXT NOT NULL,
            event_seq INTEGER NOT NULL,
            undone INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );",
    ),
    (
        2,
        "CREATE TABLE annotations(
            id TEXT PRIMARY KEY,
            target_id TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    ),
    (3, "ALTER TABLE events ADD COLUMN supersedes TEXT;"),
];

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );",
        )?;
        let applied: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        for (v, sql) in MIGRATIONS {
            if *v > applied {
                self.conn.execute_batch(sql)?;
                self.conn.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1,?2)",
                    params![v, now_iso()],
                )?;
            }
        }
        Ok(())
    }

    pub fn migration_versions(&self) -> Vec<i64> {
        let mut stmt = self
            .conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    // ---------- 事件 ----------
    pub fn record_event(&self, branch_id: &str, kind: &str, payload: &Value) -> rusqlite::Result<i64> {
        let id = format!("evt:{}:{}", kind, self.next_event_seq_hint());
        self.conn.execute(
            "INSERT INTO events(id, branch_id, kind, payload, created_at) VALUES(?1,?2,?3,?4,?5)",
            params![id, branch_id, kind, payload.to_string(), now_iso()],
        )?;
        let seq = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE branches SET head_seq=?1 WHERE id=?2",
            params![seq, branch_id],
        )?;
        Ok(seq)
    }

    fn next_event_seq_hint(&self) -> i64 {
        self.conn
            .query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events", [], |r| r.get(0))
            .unwrap_or(1)
    }

    pub fn list_events(&self, branch: Option<&str>) -> Vec<Event> {
        let (sql, p): (String, Vec<String>) = match branch {
            Some(b) => (
                "SELECT seq,id,branch_id,kind,payload,supersedes,created_at FROM events WHERE branch_id=?1 ORDER BY seq".into(),
                vec![b.to_string()],
            ),
            None => (
                "SELECT seq,id,branch_id,kind,payload,supersedes,created_at FROM events ORDER BY seq".into(),
                vec![],
            ),
        };
        let mut stmt = self.conn.prepare(&sql).unwrap();
        let rows = if p.is_empty() {
            stmt.query_map([], row_to_event).unwrap()
        } else {
            stmt.query_map(params![p[0]], row_to_event).unwrap()
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    // ---------- 分支 ----------
    pub fn create_branch(&self, id: &str, name: &str, parent: Option<&str>, base_seq: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO branches(id,name,parent,base_seq,head_seq,status,created_at) VALUES(?1,?2,?3,?4,?4,'open',?5)",
            params![id, name, parent, base_seq, now_iso()],
        )
        .map(|_| ())
    }

    pub fn get_branch(&self, id: &str) -> Option<Branch> {
        self.conn
            .query_row(
                "SELECT id,name,parent,base_seq,head_seq,status,created_at FROM branches WHERE id=?1",
                params![id],
                |r| {
                    Ok(Branch {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        parent: r.get(2)?,
                        base_seq: r.get(3)?,
                        head_seq: r.get(4)?,
                        status: r.get(5)?,
                        locked: false,
                        created_at: r.get(6)?,
                    })
                },
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn list_branches(&self) -> Vec<Branch> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,name,parent,base_seq,head_seq,status,created_at FROM branches ORDER BY created_at, id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok(Branch {
                id: r.get(0)?,
                name: r.get(1)?,
                parent: r.get(2)?,
                base_seq: r.get(3)?,
                head_seq: r.get(4)?,
                status: r.get(5)?,
                locked: false,
                created_at: r.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // ---------- 工件 ----------
    pub fn next_artifact_version(&self, branch_id: &str, kind: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM artifacts WHERE branch_id=?1 AND kind=?2",
                params![branch_id, kind],
                |r| r.get(0),
            )
            .unwrap_or(1)
    }

    pub fn insert_artifact(&self, branch_id: &str, kind: &str, payload: &Value, event_seq: i64) -> rusqlite::Result<(String, i64)> {
        let version = self.next_artifact_version(branch_id, kind);
        let id = format!("art:{}:{}:{}", kind, branch_id, version);
        self.conn.execute(
            "INSERT INTO artifacts(id,branch_id,kind,version,payload,event_seq,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![id, branch_id, kind, version, payload.to_string(), event_seq, now_iso()],
        )?;
        Ok((id, version))
    }

    pub fn artifact_head(&self, branch_id: &str, kind: &str) -> Option<Artifact> {
        self.conn
            .query_row(
                "SELECT id,branch_id,kind,version,payload,event_seq,created_at FROM artifacts
                 WHERE branch_id=?1 AND kind=?2 ORDER BY version DESC LIMIT 1",
                params![branch_id, kind],
                row_to_artifact,
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn artifact_at(&self, branch_id: &str, kind: &str, version: i64) -> Option<Artifact> {
        self.conn
            .query_row(
                "SELECT id,branch_id,kind,version,payload,event_seq,created_at FROM artifacts
                 WHERE branch_id=?1 AND kind=?2 AND version=?3",
                params![branch_id, kind, version],
                row_to_artifact,
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn list_artifacts(&self, branch_id: &str) -> Vec<Artifact> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,branch_id,kind,version,payload,event_seq,created_at FROM artifacts WHERE branch_id=?1 ORDER BY kind, version")
            .unwrap();
        stmt.query_map(params![branch_id], row_to_artifact)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    // ---------- 派生结果 ----------
    pub fn insert_derived(&self, branch_id: &str, kind: &str, deps: &Value, payload: &Value, status: &str, event_seq: i64) -> rusqlite::Result<String> {
        let id = format!("der:{}:{}", kind, event_seq);
        self.conn.execute(
            "INSERT INTO derived(id,branch_id,kind,deps,payload,status,event_seq,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, branch_id, kind, deps.to_string(), payload.to_string(), status, event_seq, now_iso()],
        )?;
        Ok(id)
    }

    pub fn get_derived(&self, id: &str) -> Option<Derived> {
        self.conn
            .query_row(
                "SELECT id,branch_id,kind,deps,payload,status,event_seq,created_at FROM derived WHERE id=?1",
                params![id],
                row_to_derived,
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn list_derived(&self, branch_id: &str) -> Vec<Derived> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,branch_id,kind,deps,payload,status,event_seq,created_at FROM derived WHERE branch_id=?1 ORDER BY event_seq, id")
            .unwrap();
        stmt.query_map(params![branch_id], row_to_derived)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// 状态变化必须经由事件落库，历史视图才能按 seq 重放。
    pub fn set_derived_status(&self, id: &str, to: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE derived SET status=?1 WHERE id=?2",
            params![to, id],
        )?;
        Ok(())
    }

    // ---------- 裁定 ----------
    pub fn insert_decision(&self, branch_id: &str, target_id: &str, ruling: &str, payload: &Value, event_seq: i64) -> rusqlite::Result<String> {
        let id = format!("dec:{}", event_seq);
        self.conn.execute(
            "INSERT INTO decisions(id,branch_id,target_id,ruling,payload,event_seq,undone,created_at) VALUES(?1,?2,?3,?4,?5,?6,0,?7)",
            params![id, branch_id, target_id, ruling, payload.to_string(), event_seq, now_iso()],
        )?;
        Ok(id)
    }

    pub fn get_decision(&self, id: &str) -> Option<Decision> {
        self.conn
            .query_row(
                "SELECT id,branch_id,target_id,ruling,payload,event_seq,undone,created_at FROM decisions WHERE id=?1",
                params![id],
                |r| {
                    Ok(Decision {
                        id: r.get(0)?,
                        branch_id: r.get(1)?,
                        target_id: r.get(2)?,
                        ruling: r.get(3)?,
                        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                        event_seq: r.get(5)?,
                        undone: r.get::<_, i64>(6)? != 0,
                        created_at: r.get(7)?,
                    })
                },
            )
            .optional()
            .unwrap_or(None)
    }

    pub fn mark_decision_undone(&self, id: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("UPDATE decisions SET undone=1 WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn list_decisions(&self, branch_id: &str) -> Vec<Decision> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,branch_id,target_id,ruling,payload,event_seq,undone,created_at FROM decisions WHERE branch_id=?1 ORDER BY event_seq")
            .unwrap();
        stmt.query_map(params![branch_id], |r| {
            Ok(Decision {
                id: r.get(0)?,
                branch_id: r.get(1)?,
                target_id: r.get(2)?,
                ruling: r.get(3)?,
                payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                event_seq: r.get(5)?,
                undone: r.get::<_, i64>(6)? != 0,
                created_at: r.get(7)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // ---------- 导出 / 导入 / 重置 ----------
    /// 导出完整档案：身份、顺序与来源关系全部保留。
    pub fn export_archive(&self) -> Value {
        let artifacts: Vec<Artifact> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id,branch_id,kind,version,payload,event_seq,created_at FROM artifacts ORDER BY kind, version, id")
                .unwrap();
            stmt.query_map([], row_to_artifact).unwrap().filter_map(|r| r.ok()).collect()
        };
        let derived: Vec<Derived> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id,branch_id,kind,deps,payload,status,event_seq,created_at FROM derived ORDER BY event_seq, id")
                .unwrap();
            stmt.query_map([], row_to_derived).unwrap().filter_map(|r| r.ok()).collect()
        };
        let decisions: Vec<Decision> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id,branch_id,target_id,ruling,payload,event_seq,undone,created_at FROM decisions ORDER BY event_seq")
                .unwrap();
            stmt.query_map([], |r| {
                Ok(Decision {
                    id: r.get(0)?,
                    branch_id: r.get(1)?,
                    target_id: r.get(2)?,
                    ruling: r.get(3)?,
                    payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
                    event_seq: r.get(5)?,
                    undone: r.get::<_, i64>(6)? != 0,
                    created_at: r.get(7)?,
                })
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
        };
        serde_json::json!({
            "format": "flow-lineage-archive/v1",
            "branches": self.list_branches(),
            "events": self.list_events(None),
            "artifacts": artifacts,
            "derived": derived,
            "decisions": decisions,
        })
    }

    /// 导入档案：整体事务，失败回滚，不留下半份记录。
    pub fn import_archive(&self, archive: &Value) -> Result<(), String> {
        if archive.get("format").and_then(|f| f.as_str()) != Some("flow-lineage-archive/v1") {
            return Err("未知档案格式".into());
        }
        let tx = self.conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let run = || -> Result<(), String> {
            for b in archive["branches"].as_array().cloned().unwrap_or_default() {
                tx.execute(
                    "INSERT INTO branches(id,name,parent,base_seq,head_seq,status,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        b["id"].as_str().unwrap_or_default(),
                        b["name"].as_str().unwrap_or_default(),
                        b["parent"].as_str(),
                        b["base_seq"].as_i64().unwrap_or(0),
                        b["head_seq"].as_i64().unwrap_or(0),
                        b["status"].as_str().unwrap_or("open"),
                        b["created_at"].as_str().unwrap_or_default(),
                    ],
                ).map_err(|e| e.to_string())?;
            }
            for e in archive["events"].as_array().cloned().unwrap_or_default() {
                tx.execute(
                    "INSERT INTO events(seq,id,branch_id,kind,payload,supersedes,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        e["seq"].as_i64().unwrap_or(0),
                        e["id"].as_str().unwrap_or_default(),
                        e["branch_id"].as_str().unwrap_or_default(),
                        e["kind"].as_str().unwrap_or_default(),
                        serde_json::to_string(&e["payload"]).unwrap_or_default(),
                        e["supersedes"].as_str(),
                        e["created_at"].as_str().unwrap_or_default(),
                    ],
                ).map_err(|e| e.to_string())?;
            }
            for a in archive["artifacts"].as_array().cloned().unwrap_or_default() {
                tx.execute(
                    "INSERT INTO artifacts(id,branch_id,kind,version,payload,event_seq,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        a["id"].as_str().unwrap_or_default(),
                        a["branch_id"].as_str().unwrap_or_default(),
                        a["kind"].as_str().unwrap_or_default(),
                        a["version"].as_i64().unwrap_or(0),
                        serde_json::to_string(&a["payload"]).unwrap_or_default(),
                        a["event_seq"].as_i64().unwrap_or(0),
                        a["created_at"].as_str().unwrap_or_default(),
                    ],
                ).map_err(|e| e.to_string())?;
            }
            for d in archive["derived"].as_array().cloned().unwrap_or_default() {
                tx.execute(
                    "INSERT INTO derived(id,branch_id,kind,deps,payload,status,event_seq,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        d["id"].as_str().unwrap_or_default(),
                        d["branch_id"].as_str().unwrap_or_default(),
                        d["kind"].as_str().unwrap_or_default(),
                        serde_json::to_string(&d["deps"]).unwrap_or_default(),
                        serde_json::to_string(&d["payload"]).unwrap_or_default(),
                        d["status"].as_str().unwrap_or_default(),
                        d["event_seq"].as_i64().unwrap_or(0),
                        d["created_at"].as_str().unwrap_or_default(),
                    ],
                ).map_err(|e| e.to_string())?;
            }
            for d in archive["decisions"].as_array().cloned().unwrap_or_default() {
                tx.execute(
                    "INSERT INTO decisions(id,branch_id,target_id,ruling,payload,event_seq,undone,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        d["id"].as_str().unwrap_or_default(),
                        d["branch_id"].as_str().unwrap_or_default(),
                        d["target_id"].as_str().unwrap_or_default(),
                        d["ruling"].as_str().unwrap_or_default(),
                        serde_json::to_string(&d["payload"]).unwrap_or_default(),
                        d["event_seq"].as_i64().unwrap_or(0),
                        if d["undone"].as_bool().unwrap_or(false) { 1 } else { 0 },
                        d["created_at"].as_str().unwrap_or_default(),
                    ],
                ).map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        match run() {
            Ok(()) => tx.commit().map_err(|e| e.to_string()),
            Err(e) => {
                let _ = tx.rollback();
                Err(format!("导入失败，已回滚本次变更: {e}"))
            }
        }
    }

    pub fn reset(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM decisions; DELETE FROM derived; DELETE FROM artifacts; DELETE FROM events; DELETE FROM branches; DELETE FROM annotations;",
        )
    }
}

fn row_to_event(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        seq: r.get(0)?,
        id: r.get(1)?,
        branch_id: r.get(2)?,
        kind: r.get(3)?,
        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
        supersedes: r.get(5)?,
        created_at: r.get(6)?,
    })
}

fn row_to_artifact(r: &rusqlite::Row) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: r.get(0)?,
        branch_id: r.get(1)?,
        kind: r.get(2)?,
        version: r.get(3)?,
        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
        event_seq: r.get(5)?,
        created_at: r.get(6)?,
    })
}

fn row_to_derived(r: &rusqlite::Row) -> rusqlite::Result<Derived> {
    Ok(Derived {
        id: r.get(0)?,
        branch_id: r.get(1)?,
        kind: r.get(2)?,
        deps: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
        payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
        status: r.get(5)?,
        event_seq: r.get(6)?,
        created_at: r.get(7)?,
    })
}
