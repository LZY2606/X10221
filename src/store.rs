//! 版本化存储：来源与解释分开保存；追加式版本；乐观并发；机器可读事件。

use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::db;
use crate::ids;
use crate::model::{Event, StateView, Version};

#[derive(Debug)]
pub enum StoreError {
    Db(rusqlite::Error),
    /// 同版本并发修改：携带当前头版本，供调用方返回双方差异。
    Conflict { head: Option<Version> },
    NotFound(String),
    BadInput(String),
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Db(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Db(e) => write!(f, "数据库错误: {e}"),
            StoreError::Conflict { head } => write!(
                f,
                "版本冲突：当前头版本为 {}",
                head.as_ref().map(|h| h.id.as_str()).unwrap_or("<无>")
            ),
            StoreError::NotFound(id) => write!(f, "未找到: {id}"),
            StoreError::BadInput(m) => write!(f, "输入非法: {m}"),
        }
    }
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn row_to_version(row: &rusqlite::Row) -> rusqlite::Result<Version> {
    let payload: String = row.get(5)?;
    Ok(Version {
        id: row.get(0)?,
        kind: row.get(1)?,
        line_id: row.get(2)?,
        branch: row.get(3)?,
        seq: row.get(4)?,
        base_id: row.get(6)?,
        payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
        has_raw: row.get::<_, Option<Vec<u8>>>(7)?.is_some(),
        created_ms: row.get(8)?,
        status: row.get(9)?,
    })
}

const VERSION_COLS: &str = "id, kind, line_id, branch, seq, payload, base_id, raw, created_ms, status";

impl Store {
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        db::migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        db::migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 提交新版本。`base_seq` 为调用方基于的头版本序号：
    /// 与当前头不一致即冲突（同版本并发修改），返回当前头供比对。
    pub fn commit(
        &self,
        kind: &str,
        line_id: &str,
        branch: &str,
        base_seq: Option<i64>,
        payload: serde_json::Value,
        raw: Option<Vec<u8>>,
    ) -> Result<Version, StoreError> {
        let conn = self.conn.lock().unwrap();
        let head = Self::head_locked(&conn, line_id, branch)?;
        if let Some(bs) = base_seq {
            let head_seq = head.as_ref().map(|h| h.seq).unwrap_or(0);
            if bs != head_seq {
                return Err(StoreError::Conflict { head });
            }
        }
        let seq = head.as_ref().map(|h| h.seq + 1).unwrap_or(1);
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();
        let id = ids::version_id(kind, line_id, branch, seq, &payload_str);
        let now = ids::now_ms();
        conn.execute(
            "INSERT INTO versions(id, kind, line_id, branch, seq, base_id, payload, raw, created_ms, status)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'fresh')",
            params![
                id,
                kind,
                line_id,
                branch,
                seq,
                head.as_ref().map(|h| h.id.clone()),
                payload_str,
                raw,
                now
            ],
        )?;
        Ok(Version {
            id,
            kind: kind.into(),
            line_id: line_id.into(),
            branch: branch.into(),
            seq,
            base_id: head.map(|h| h.id),
            payload,
            has_raw: raw.is_some(),
            created_ms: now,
            status: "fresh".into(),
        })
    }

    fn head_locked(conn: &Connection, line_id: &str, branch: &str) -> Result<Option<Version>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {VERSION_COLS} FROM versions WHERE line_id=?1 AND branch=?2 ORDER BY seq DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map(params![line_id, branch], row_to_version)?;
        Ok(rows.next().transpose()?)
    }

    pub fn head(&self, line_id: &str, branch: &str) -> Result<Option<Version>, StoreError> {
        let conn = self.conn.lock().unwrap();
        Self::head_locked(&conn, line_id, branch)
    }

    pub fn get(&self, id: &str) -> Result<Option<Version>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {VERSION_COLS} FROM versions WHERE id=?1"))?;
        let mut rows = stmt.query_map(params![id], row_to_version)?;
        Ok(rows.next().transpose()?)
    }

    pub fn raw_of(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT raw FROM versions WHERE id=?1")?;
        let mut rows = stmt.query(params![id])?;
        Ok(if let Some(row) = rows.next()? { row.get(0)? } else { None })
    }

    pub fn versions_of(&self, line_id: &str) -> Result<Vec<Version>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {VERSION_COLS} FROM versions WHERE line_id=?1 ORDER BY branch, seq"
        ))?;
        let rows = stmt.query_map(params![line_id], row_to_version)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn all_versions(&self) -> Result<Vec<Version>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {VERSION_COLS} FROM versions ORDER BY created_ms, kind, line_id, branch, seq"
        ))?;
        let rows = stmt.query_map([], row_to_version)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn record_event(&self, kind: &str, payload: serde_json::Value) -> Result<Event, StoreError> {
        let conn = self.conn.lock().unwrap();
        let seq: i64 = conn.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events", [], |r| r.get(0))?;
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();
        let id = ids::event_id(seq, kind, &payload_str);
        let now = ids::now_ms();
        conn.execute(
            "INSERT INTO events(id, seq, kind, payload, created_ms) VALUES(?1,?2,?3,?4,?5)",
            params![id, seq, kind, payload_str, now],
        )?;
        Ok(Event { id, seq, kind: kind.into(), payload, created_ms: now })
    }

    pub fn events(&self) -> Result<Vec<Event>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, seq, kind, payload, created_ms FROM events ORDER BY seq")?;
        let rows = stmt.query_map([], |row| {
            let payload: String = row.get(3)?;
            Ok(Event {
                id: row.get(0)?,
                seq: row.get(1)?,
                kind: row.get(2)?,
                payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
                created_ms: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 标记过期：只改状态列，版本内容保持原样（不覆盖历史结论）。
    pub fn mark_stale(&self, version_ids: &[String]) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        for id in version_ids {
            conn.execute("UPDATE versions SET status='stale' WHERE id=?1", params![id])?;
        }
        Ok(())
    }

    /// 时间旅行：回到某事件时可见的版本与结论。
    /// 版本按创建时间截断；过期标记由截至该时刻的事件重放得出。
    pub fn view_at(&self, event_seq: Option<i64>) -> Result<StateView, StoreError> {
        let events = self.events()?;
        let (cut_ms, cut_seq) = match event_seq {
            Some(seq) => {
                let ev = events
                    .iter()
                    .find(|e| e.seq == seq)
                    .ok_or_else(|| StoreError::NotFound(format!("event seq {seq}")))?;
                (ev.created_ms, ev.seq)
            }
            None => (i64::MAX, i64::MAX),
        };
        let visible_events: Vec<Event> = events.into_iter().filter(|e| e.seq <= cut_seq).collect();
        let mut stale_ids: Vec<String> = Vec::new();
        for e in &visible_events {
            if e.kind == "population.stale" {
                if let Some(ids) = e.payload.get("stale_ids").and_then(|v| v.as_array()) {
                    for id in ids {
                        if let Some(s) = id.as_str() {
                            stale_ids.push(s.to_string());
                        }
                    }
                }
            }
        }
        let versions: Vec<Version> = self
            .all_versions()?
            .into_iter()
            .filter(|v| v.created_ms <= cut_ms)
            .map(|mut v| {
                if stale_ids.contains(&v.id) {
                    v.status = "stale".into();
                } else if event_seq.is_some() {
                    v.status = "fresh".into();
                }
                v
            })
            .collect();
        Ok(StateView { versions, events: visible_events, stale_ids })
    }

    /// 重置（删除本地数据库内容）：仅用于导出→删除→导入流程。
    pub fn reset(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("DELETE FROM events; DELETE FROM versions;")?;
        Ok(())
    }

    /// 导入档案行（幂等：id 已存在则跳过）。
    pub fn insert_version_raw(&self, v: &Version, raw: Option<Vec<u8>>) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let payload_str = serde_json::to_string(&v.payload).unwrap_or_default();
        let n = conn.execute(
            "INSERT OR IGNORE INTO versions(id, kind, line_id, branch, seq, base_id, payload, raw, created_ms, status)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![v.id, v.kind, v.line_id, v.branch, v.seq, v.base_id, payload_str, raw, v.created_ms, v.status],
        )?;
        Ok(n > 0)
    }

    pub fn insert_event_raw(&self, e: &Event) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let payload_str = serde_json::to_string(&e.payload).unwrap_or_default();
        let n = conn.execute(
            "INSERT OR IGNORE INTO events(id, seq, kind, payload, created_ms) VALUES(?1,?2,?3,?4,?5)",
            params![e.id, e.seq, e.kind, payload_str, e.created_ms],
        )?;
        Ok(n > 0)
    }
}
