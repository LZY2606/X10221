//! SQLite 持久化：版本化来源、事件快照、分支、裁定与冲突留档。

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub mod migrations;

/// 持久化包装。单连接由服务层 Mutex 串行化。
pub struct Store {
    pub conn: Connection,
}

#[derive(Debug)]
pub struct EventRow {
    pub id: String,
    pub seq: i64,
    pub branch: String,
    pub kind: String,
    pub summary: String,
    pub payload: String,
    pub view: Option<String>,
    pub created_at: i64,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    /// 顺序应用尚未应用的迁移，并在 meta 记录版本。
    pub fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let applied: i64 = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for (idx, (_name, sql)) in migrations::MIGRATIONS
            .iter()
            .enumerate()
            .skip(applied as usize)
        {
            self.conn.execute_batch(sql)?;
            self.conn.execute(
                "INSERT OR REPLACE INTO meta(key,value) VALUES('schema_version', ?1)",
                params![(idx as i64 + 1).to_string()],
            )?;
        }
        Ok(())
    }

    pub fn schema_version(&self) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    /// 分配稳定序列 id：`prefix-0042`。计数器存于 meta。
    pub fn alloc_id(&self, prefix: &str) -> rusqlite::Result<String> {
        let key = format!("seq_{prefix}");
        let next: i64 = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
            + 1;
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES(?1, ?2)",
            params![key, next.to_string()],
        )?;
        Ok(format!("{prefix}-{next:04}"))
    }

    pub fn next_source_seq(&self, kind: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM source_versions WHERE kind=?1",
                params![kind],
                |r| r.get(0),
            )?)
    }

    pub fn insert_source(
        &self,
        kind: &str,
        payload: &Value,
        created_by: &str,
    ) -> rusqlite::Result<String> {
        let seq = self.next_source_seq(kind)?;
        let id = format!("{kind}-v{seq}");
        let body = serde_json::to_string(payload).unwrap();
        self.conn.execute(
            "INSERT INTO source_versions(id,kind,seq,payload,created_by) \
             VALUES(?1,?2,?3,?4,?5)",
            params![id, kind, seq, body, created_by],
        )?;
        Ok(id)
    }

    pub fn source_payload(&self, id: &str) -> rusqlite::Result<Value> {
        let body: String = self.conn.query_row(
            "SELECT payload FROM source_versions WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&body).unwrap())
    }

    pub fn insert_branch(
        &self,
        id: &str,
        name: &str,
        parent: Option<&str>,
        fork_event: Option<&str>,
        head: &str,
        created_seq: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO branches(id,name,parent,fork_event,head,status,created_seq) \
             VALUES(?1,?2,?3,?4,?5,'open',?6)",
            params![id, name, parent, fork_event, head, created_seq],
        )?;
        Ok(())
    }

    pub fn set_branch_head(&self, id: &str, head: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE branches SET head=?1 WHERE id=?2",
            params![head, id],
        )?;
        Ok(())
    }

    pub fn branch_head(&self, id: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT head FROM branches WHERE id=?1", params![id], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn branches(&self) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,parent,fork_event,head,status,created_seq FROM branches ORDER BY created_seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "parent": r.get::<_, Option<String>>(2)?,
                "fork_event": r.get::<_, Option<String>>(3)?,
                "head": r.get::<_, String>(4)?,
                "status": r.get::<_, String>(5)?,
                "created_seq": r.get::<_, i64>(6)?,
            }))
        })?;
        rows.collect()
    }

    pub fn insert_event(
        &self,
        id: &str,
        seq: i64,
        branch: &str,
        kind: &str,
        summary: &str,
        payload: &Value,
        view: Option<&Value>,
        created_at: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO events(id,seq,branch,kind,summary,payload,view,created_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                seq,
                branch,
                kind,
                summary,
                serde_json::to_string(payload).unwrap(),
                view.map(|v| serde_json::to_string(v).unwrap()),
                created_at
            ],
        )?;
        Ok(())
    }

    pub fn event_seq(&self) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))?)
    }

    pub fn get_event(&self, id: &str) -> rusqlite::Result<Option<EventRow>> {
        self.conn
            .query_row(
                "SELECT id,seq,branch,kind,summary,payload,view,created_at \
                 FROM events WHERE id=?1",
                params![id],
                |r| {
                    Ok(EventRow {
                        id: r.get(0)?,
                        seq: r.get(1)?,
                        branch: r.get(2)?,
                        kind: r.get(3)?,
                        summary: r.get(4)?,
                        payload: r.get(5)?,
                        view: r.get(6)?,
                        created_at: r.get(7)?,
                    })
                },
            )
            .optional()
    }

    pub fn events(&self) -> rusqlite::Result<Vec<EventRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,seq,branch,kind,summary,payload,view,created_at FROM events ORDER BY seq")?;
        let rows = stmt.query_map([], |r| {
            Ok(EventRow {
                id: r.get(0)?,
                seq: r.get(1)?,
                branch: r.get(2)?,
                kind: r.get(3)?,
                summary: r.get(4)?,
                payload: r.get(5)?,
                view: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn insert_decision(
        &self,
        id: &str,
        event_id: &str,
        branch: &str,
        action: &str,
        subject: &str,
        rationale: &str,
        reviewer: &str,
        status: &str,
        revert_of: Option<&str>,
        conflict_group: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO decisions(id,event_id,branch,action,subject,rationale,reviewer,\
             status,revert_of,conflict_group) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id,
                event_id,
                branch,
                action,
                subject,
                rationale,
                reviewer,
                status,
                revert_of,
                conflict_group
            ],
        )?;
    }

    pub fn decisions_for_subject(&self, branch: &str, subject: &str) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,event_id,action,subject,rationale,reviewer,status,revert_of,conflict_group \
             FROM decisions WHERE branch=?1 AND subject=?2 ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![branch, subject], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "event_id": r.get::<_, String>(1)?,
                "action": r.get::<_, String>(2)?,
                "subject": r.get::<_, String>(3)?,
                "rationale": r.get::<_, String>(4)?,
                "reviewer": r.get::<_, String>(5)?,
                "status": r.get::<_, String>(6)?,
                "revert_of": r.get::<_, Option<String>>(7)?,
                "conflict_group": r.get::<_, Option<String>>(8)?,
            }))
        })?;
        rows.collect()
    }

    pub fn decisions(&self) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,event_id,branch,action,subject,rationale,reviewer,status,revert_of,conflict_group \
             FROM decisions ORDER BY rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "event_id": r.get::<_, String>(1)?,
                "branch": r.get::<_, String>(2)?,
                "action": r.get::<_, String>(3)?,
                "initial decision row": r.get::<_, String>(4)?,
                "rationale": r.get::<_, String>(5)?,
                "reviewer": r.get::<_, String>(6)?,
                "status": r.get::<_, String>(7)?,
                "revert_of": r.get::<_, Option<String>>(8)?,
                "conflict_group": r.get::<_, Option<String>>(9)?,
            }))
        })?;
        rows.collect()
    }

    pub fn mark_decision_status(&self, id: &str, status: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE decisions SET status=?1 WHERE id=?2",
            params![status, id],
        )?;
    }

    pub fn set_decision_conflict(&self, id: &str, group: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE decisions SET conflict_group=?1, status='conflicting' WHERE id=?2",
            params![group, id],
        )?;
    }

    pub fn insert_rejected(
        &self,
        id: &str,
        branch: &str,
        base_event: &str,
        current_head: &str,
        action: &str,
        payload: &Value,
        diff: &Value,
        created_at: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO rejected_commits(id,branch,base_event,current_head,action,payload,diff,created_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                branch,
                base_event,
                current_head,
                action,
                serde_json::to_string(payload).unwrap(),
                serde_json::to_string(diff).unwrap(),
                created_at
            ],
        )?;
    }

    pub fn rejected_commits(&self) -> rusqlite::Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,branch,base_event,current_head,action,payload,diff,created_at \
             FROM rejected_commits ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "branch": r.get::<_, String>(1)?,
                "base_event": r.get::<_, String>(2)?,
                "current_head": r.get::<_, String>(3)?,
                "action": r.get::<_, String>(4)?,
                "payload": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap(),
                "diff": serde_json::from_str::<Value>(&r.get::<_, String>(6)?).unwrap(),
                "created_at": r.get::<_, i64>(7)?,
            }))
        })?;
        rows.collect()
    }

    pub fn transaction<F, T>(&self, f: F) -> rusqlite::Result<T>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T>,
    {
        let mut tx = self.conn.unchecked_transaction()?;
        let result = f(&mut tx)?;
        tx.commit()?;
        Ok(result)
    }
}
