//! Repository：事务、分支、内容寻址材料、追加式事件写入。

use super::ids::*;
use crate::domain::{Branch, EventPayload, EventRecord, Material};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

pub struct Repo {
    pub conn: Connection,
}

#[derive(Debug)]
pub struct StoreError {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError {
            code: "SQL".into(),
            message: e.to_string(),
        }
    }
}

pub fn err(code: &str, message: impl Into<String>) -> StoreError {
    StoreError {
        code: code.into(),
        message: message.into(),
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

impl Repo {
    pub fn open(path: &str) -> StoreResult<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        super::db::migrate(&mut conn)?;
        Ok(Repo { conn })
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        super::db::migrate(&mut conn)?;
        Ok(Repo { conn })
    }

    pub fn create_branch(&mut self, name: &str) -> StoreResult<Branch> {
        let ts = now_ms();
        let id = branch_id(name, ts);
        self.conn.execute(
            "INSERT INTO branches(id,name,head_event_id,created_at) VALUES(?1,?2,NULL,?3)",
            params![id, name, ts],
        )?;
        Ok(Branch {
            id,
            name: name.into(),
            head_event_id: None,
            created_at: ts,
        })
    }

    pub fn get_branch(&self, id: &str) -> StoreResult<Branch> {
        self.conn
            .query_row(
                "SELECT id,name,head_event_id,created_at FROM branches WHERE id=?1",
                params![id],
                |r| {
                    Ok(Branch {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        head_event_id: r.get(2)?,
                        created_at: r.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| err("BRANCH_NOT_FOUND", format!("分支 {id} 不存在")))
    }

    pub fn list_branches(&self) -> StoreResult<Vec<Branch>> {
        let mut s = self.conn.prepare(
            "SELECT id,name,head_event_id,created_at FROM branches ORDER BY created_at, id",
        )?;
        let rows = s.query_map([], |r| {
            Ok(Branch {
                id: r.get(0)?,
                name: r.get(1)?,
                head_event_id: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn next_seq(&self, branch_id: &str) -> StoreResult<i64> {
        let max: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(seq) FROM events WHERE branch_id=?1",
                params![branch_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten()
        Ok(max.unwrap_or(0) + 1)
    }

    /// 追加事件（append-only）。由调用方在事务内调用。
    pub fn append_event_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        actor: &str,
        payload: &EventPayload,
    ) -> StoreResult<EventRecord> {
        let max: Option<i64> = tx
            .query_row(
                "SELECT MAX(seq) FROM events WHERE branch_id=?1",
                params![branch_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let seq = max.unwrap_or(0) + 1;
        let prev: Option<String> = tx
            .query_row(
                "SELECT id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                params![branch_id],
                |r| r.get(0),
            )
            .optional()?;
        let ts = now_ms();
        let payload_json = serde_json::to_string(payload).unwrap();
        let id = event_id(branch_id, seq, &payload_json, prev.as_deref());
        tx.execute(
            "INSERT INTO events(id,seq,branch_id,timestamp,actor,payload,prev_event_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![id, seq, branch_id, ts, actor, payload_json, prev],
        )?;
        tx.execute(
            "UPDATE branches SET head_event_id=?1 WHERE id=?2",
            params![id, branch_id],
        )?;
        Ok(EventRecord {
            id,
            seq,
            branch_id: branch_id.into(),
            timestamp: ts,
            actor: actor.into(),
            payload: payload.clone(),
            prev_event_id: prev,
        })
    }

 pub fn append_event(
        &mut self,
        branch_id: &str,
        actor: &str,
        payload: EventPayload,
    ) -> StoreResult<EventRecord> {
        let tx = self.conn.transaction()?;
        let rec = Self::append_event_tx(&tx, branch_id, actor, &payload)?;
        tx.commit()?;
        Ok(rec)
    }

    pub fn list_events(&self, branch_id: &str) -> StoreResult<Vec<EventRecord>> {
        let mut s = self.conn.prepare(
            "SELECT id,seq,branch_id,timestamp,actor,payload,prev_event_id
             FROM events WHERE branch_id=?1 ORDER BY seq",
        )?;
        let rows = s.query_map(params![branch_id], |r| {
            let payload_str: String = r.get(5)?;
            Ok(EventRecord {
                id: r.get(0)?,
                seq: r.get(1)?,
                branch_id: r.get(2)?,
                timestamp: r.get(3)?,
                actor: r.get(4)?,
                payload: serde_json::from_str(&payload_str).unwrap(),
                prev_event_id: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 插入材料；相同 sha 复用既有行（幂等）。
    pub fn insert_material_tx(
        tx: &rusqlite::Transaction,
        filename: &str,
        kind: &str,
        content: &[u8],
        label: &str,
    ) -> StoreResult<(Material, bool)> {
        let sha = sha256_hex(content);
        let id = material_id(&sha);
        if let Some(existing) = tx
            .query_row(
                "SELECT id,filename,kind,sha256,size,imported_at,label FROM materials WHERE id=?1",
                params![id],
                material_row,
            )
            .optional()?
        {
            return Ok((existing, false));
        }
        let ts = now_ms();
        tx.execute(
            "INSERT INTO materials(id,filename,kind,sha256,size,content,imported_at,label)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, filename, kind, sha, content.len() as i64, content, ts, label],
        )?;
        Ok((
            Material {
                id,
                filename: filename.into(),
                kind: kind.into(),
                sha256,
                size: content.len() as i64,
                imported_at: ts,
                label: label.into(),
            },
            true,
        ))
    }

    pub fn material_content(&self, id: &str) -> StoreResult<Vec<u8>> {
        self.conn
            .query_row("SELECT content FROM materials WHERE id=?1", params![id], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()?
            .ok_or_else(|| err("MATERIAL_NOT_FOUND", id))
    }

    pub fn list_materials(&self) -> StoreResult<Vec<Material>> {
        let mut s = self.conn.prepare(
            "SELECT id,filename,kind,sha256,size,imported_at,label FROM materials ORDER BY imported_at, id",
        )?;
        let rows = s.query_map([], material_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_diagnostic_tx(
        tx: &rusqlite::Transaction,
        material_id: &str,
        seq: i64,
        code: &str,
        detail: &str,
    ) -> StoreResult<()> {
        tx.execute(
            "INSERT INTO material_diagnostics(material_id,seq,code,detail) VALUES(?1,?2,?3,?4)",
            rusqlite::params![material_id, seq, code, detail],
        )?;
        Ok(())
    }

    pub fn list_diagnostics(
        &self,
        material_id: &str,
    ) -> StoreResult<Vec<crate::service::derive::Diagnostic>> {
        let mut s = self
            .conn
            .prepare("SELECT code,detail FROM material_diagnostics WHERE material_id=?1 ORDER BY seq")?;
        let rows = s.query_map(rusqlite::params![material_id], |r| {
            Ok(crate::service::derive::Diagnostic {
                code: r.get(0)?,
                detail: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 链完整性：prev 指针与 seq 连续性。
    pub fn verify_chain(&self, branch_id: &str) -> StoreResult<serde_json::Value> {
        let events = self.list_events(branch_id)?;
        let mut expected_prev: Option<String> = None;
        for (i, e) in events.iter().enumerate() {
            if e.seq != i as i64 + 1 {
                return Ok(json!({"ok": false, "reason": format!("seq 不连续于 {}", e.id)}));
            }
            if e.prev_event_id != expected_prev {
                return Ok(json!({"ok": false, "reason": format!("prev 指针断裂于 {}", e.id)}));
            }
            expected_prev = Some(e.id.clone());
        }
        Ok(json!({"ok": true, "events": events.len()}))
    }
}

fn material_row(r: &rusqlite::Row) -> rusqlite::Result<Material> {
    Ok(Material {
        id: r.get(0)?,
        filename: r.get(1)?,
        kind: r.get(2)?,
        sha256: r.get(3)?,
        size: r.get(4)?,
        imported_at: r.get(5)?,
        label: r.get(6)?,
    })
}
