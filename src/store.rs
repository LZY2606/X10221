//! SQLite 存储：只做追加日志的读写与原子提交；不含领域推导。

use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use crate::canonical::hex_encode;
use crate::events::{event_identity, Event, EventPayload};

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    ("0002_rejected_commits", include_str!("../migrations/0002_rejected_commits.sql")),
];
pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RejectedCommit {
    pub rejected_id: String,
    pub branch_id: String,
    pub base_event_id: Option<String>,
    pub payload: EventPayload,
    pub actor: String,
    pub reason: String,
    pub diff_summary: String,
    pub created_at_unix: i64,
}

#[derive(Debug)]
pub struct CommitConflict {
    pub base_event_id: Option<String>,
    pub current_head: Option<String>,
    pub reason: String,
    pub diff_summary: String,
}

impl std::fmt::Display for CommitConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "版本冲突: {}", self.reason)
    }
}
impl std::error::Error for CommitConflict {}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let db = Db { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    /// 测试辅助：只应用到指定版本，用于状态迁移测试。
    pub fn open_with_migrations_until(path: &str, until: usize) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        let db = Db { conn: Mutex::new(conn) };
        for (name, sql) in &MIGRATIONS[..until] {
            db.conn.lock().unwrap().execute_batch(sql).with_context(|| format!("migration {name}"))?;
            db.set_user_version(MIGRATIONS.iter().position(|(n, _)| n == name).unwrap() as i64 + 1)?;
        }
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
            let version = idx as i64 + 1;
            if version <= current {
                continue;
            }
            conn.execute_batch(sql).with_context(|| format!("migration {name}"))?;
            conn.pragma_update(None, "user_version", version)?;
        }
        Ok(())
    }

    fn set_user_version(&self, v: i64) -> Result<()> {
        self.conn.lock().unwrap().pragma_update(None, "user_version", v)?;
        Ok(())
    }

    pub fn user_version(&self) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn put_blob(&self, media_type: &str, bytes: &[u8], at: i64) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let blob_id = hex_encode(&hasher.finalize());
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO blobs(blob_id, media_type, bytes, imported_at) VALUES (?1,?2,?3,?4)",
            params![blob_id, media_type, bytes, at],
        )?;
        Ok(blob_id)
    }

    pub fn get_blob(&self, blob_id: &str) -> Result<Option<(String, Vec<u8>)>> {
        let mut stmt = self.conn.lock().unwrap().prepare(
            "SELECT media_type, bytes FROM blobs WHERE blob_id = ?1",
        )?;
        let mut rows = stmt.query(params![blob_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn create_branch(&self, branch_id: &str, title: &str,
                        forked_from_branch: Option<&str>, fork_event_id: Option<&str>, at: i64) -> Result<()> {
        let payload = EventPayload::BranchCreated {
            branch_id: branch_id.to_string(),
            forked_from_branch: forked_from_branch.map(|s| s.to_string()),
            fork_event_id: fork_event_id.map(|s| s.to_string()),
            title: title.to_string(),
        };
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO branches(branch_id,title,forked_from_branch,fork_event_id,created_at_unix) \
             VALUES (?1,?2,?3,?4,?5)",
            params![branch_id, title, forked_from_branch, fork_event_id, at],
        )?;
        let id = event_identity(branch_id, 1, &None, &payload, "system");
        tx.execute(
            "INSERT INTO events(id,branch_id,seq,parent_id,payload,actor,created_at_unix) \
             VALUES (?1,?2,1,NULL,?3,'system',?4)",
            params![id, branch_id, serde_json::to_string(&payload)?, at],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn branch_head(&self, branch_id: &str) -> Result<Option<(i64, String)>> {
        let mut stmt = self.conn.lock().unwrap().prepare(
            "SELECT seq, id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![branch_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn list_branches(&self) -> Result<Vec<crate::projection::BranchInfo>> {
        let mut stmt = self.conn.lock().unwrap().prepare(
            "SELECT branch_id,title,forked_from_branch,fork_event_id FROM branches ORDER BY created_at_unix, branch_id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(crate::projection::BranchInfo {
                branch_id: row.get(0)?,
                title: row.get(1)?,
                forked_from_branch: row.get(2)?,
                fork_event_id: row.get(3)?,
            });
        }
        Ok(out)
    }

    /// 追加一个事件。expected_head 实现乐观并发：两个窗口同版本保存时，
    /// 后提交者拿到 CommitConflict，且其载荷被存入 rejected_commits 供并排复核。
    pub fn commit(&self, branch_id: &str, expected_head: Option<String>, payload: EventPayload,
                  actor: &str, at: i64) -> Result<Event> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let (seq, parent_id): (i64, Option<String>) = match tx
            .query_row("SELECT seq, id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                params![branch_id], |r| Ok((r.get(0)?, r.get(1)?)))
        {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                bail!("分支 {} 不存在", branch_id)
            }
            Err(e) => return Err(e.into()),
        };
        if expected_head != parent_id {
            let diff = self.diff_summary(&tx, branch_id, &payload, parent_id.as_deref())?;
            let conflict = CommitConflict {
                base_event_id: expected_head,
                current_head: parent_id.clone(),
                reason: format!("基础版本 {:?} 已过期，当前头事件 {:?}", expected_head, parent_id),
                diff_summary: diff.clone(),
            };
            let mut hasher = Sha256::new();
            hasher.update(format!("rejected:{}:{}:{}", branch_id, at, diff).as_bytes());
            let rejected_id = hex_encode(&hasher.finalize());
            tx.execute(
                "INSERT INTO rejected_commits(rejected_id,branch_id,base_event_id,payload,actor,reason,diff_summary,created_at_unix) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![rejected_id, branch_id, expected_head,
                    serde_json::to_string(&payload)?, actor, conflict.reason, diff, at],
            )?;
            tx.commit()?;
            return Err(anyhow!(conflict));
        }
        let new_seq = seq + 1;
        let id = event_identity(branch_id, new_seq, &parent_id, &payload, actor);
        tx.execute(
            "INSERT INTO events(id,branch_id,seq,parent_id,payload,actor,created_at_unix) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, branch_id, new_seq, parent_id,
                serde_json::to_string(&payload)?, actor, at],
        )?;
        tx.commit()?;
        Ok(Event { id, branch_id: branch_id.to_string(), seq: new_seq, parent_id, payload, actor: actor.to_string(), created_at_unix: at })
    }

    fn diff_summary(&self, tx: &rusqlite::Transaction<'_>, branch_id: &str,
                    incoming: &EventPayload, current_head: Option<&str>) -> Result<String> {
        let current_payload: Option<String> = if let Some(head) = current_head {
            tx.query_row("SELECT payload FROM events WHERE id=?1", params![head], |r| r.get(0)).ok()
        } else {
            None
        };
        let head_kind = current_payload
            .and_then(|p| serde_json::from_str::<EventPayload>(&p).ok())
            .map(|p| p.kind().to_string())
            .unwrap_or_else(|| "<none>".to_string());
        Ok(format!(
            "本端基于 {:?} 提交 `{}`；服务端头事件为 `{}`",
            current_head, incoming.kind(), head_kind
        ))
    }

    pub fn list_rejected(&self, branch_id: &str) -> Result<Vec<RejectedCommit>> {
        let mut stmt = self.conn.lock().unwrap().prepare(
            "SELECT rejected_id,branch_id,base_event_id,payload,actor,reason,diff_summary,created_at_unix \
             FROM rejected_commits WHERE branch_id=?1 ORDER BY created_at_unix",
        )?;
        let mut rows = stmt.query(params![branch_id])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let payload_text: String = row.get(3)?;
            out.push(RejectedCommit {
                rejected_id: row.get(0)?,
                branch_id: row.get(1)?,
                base_event_id: row.get(2)?,
                payload: serde_json::from_str(&payload_text)?,
                actor: row.get(4)?,
                reason: row.get(5)?,
                diff_summary: row.get(6)?,
                created_at_unix: row.get(7)?,
            });
        }
        Ok(out)
    }

    pub fn events_on_branch(&self, branch_id: &str, up_to_seq: Option<i64>) -> Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT id,branch_id,seq,parent_id,payload,actor,created_at_unix \
             FROM events WHERE branch_id=?1");
        if up_to_seq.is_some() {
            sql.push_str(" AND seq<=?2");
        }
        sql.push_str(" ORDER BY seq");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = match up_to_seq {
            Some(up) => stmt.query(params![branch_id, up])?,
            None => stmt.query(params![branch_id])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let payload_text: String = row.get(4)?;
            out.push(Event {
                id: row.get(0)?,
                branch_id: row.get(1)?,
                seq: row.get(2)?,
                parent_id: row.get(3)?,
                payload: serde_json::from_str(&payload_text)?,
                actor: row.get(5)?,
                created_at_unix: row.get(6)?,
            });
        }
        Ok(out)
    }

    pub fn event_by_id(&self, id: &str) -> Result<Option<Event>> {
        let mut stmt = self.conn.lock().unwrap().prepare(
            "SELECT id,branch_id,seq,parent_id,payload,actor,created_at_unix FROM events WHERE id=?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let payload_text: String = row.get(4)?;
            Ok(Some(Event {
                id: row.get(0)?,
                branch_id: row.get(1)?,
                seq: row.get(2)?,
                parent_id: row.get(3)?,
                payload: serde_json::from_str(&payload_text)?,
                actor: row.get(5)?,
                created_at_unix: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn all_blobs(&self) -> Result<Vec<(String, String, Vec<u8>)>> {
        let mut stmt = self.conn.lock().unwrap()
            .prepare("SELECT blob_id, media_type, bytes FROM blobs ORDER BY blob_id")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    /// 原子导入整份档案：先校验全部事件身份与引用，再在一个事务内落库；
    /// 失败整体回滚，不留下半份记录。
    pub fn import_archive(&self, archive: &crate::projection::Archive, at: i64) -> Result<()> {
        for br in &archive.branches {
            let mut expected: Option<String> = None;
            for (idx, ev) in br.events.iter().enumerate() {
                let computed = event_identity(&ev.branch_id, ev.seq, &ev.parent_id, &ev.payload, &ev.actor);
                if computed != ev.id {
                    bail!("事件 {} 的身份校验失败（内容被改动或字节不一致）", ev.id);
                }
                if ev.seq != idx as i64 + 1 {
                    bail!("分支 {} 事件序号不连续", br.branch_id);
                }
                if ev.parent_id != expected {
                    bail!("分支 {} 事件链断裂于 seq={}", br.branch_id, ev.seq);
                }
                expected = Some(ev.id.clone());
            }
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for blob in &archive.blobs {
            let mut hasher = Sha256::new();
            hasher.update(&blob.bytes);
            let got = hex_encode(&hasher.finalize());
            if got != blob.blob_id {
                bail!("blob {} 字节哈希不一致", blob.blob_id);
            }
            tx.execute(
                "INSERT OR IGNORE INTO blobs(blob_id,media_type,bytes,imported_at) VALUES (?1,?2,?3,?4)",
                params![blob.blob_id, blob.media_type, blob.bytes, at],
            )?;
        }
        for br in &archive.branches {
            tx.execute(
                "INSERT OR IGNORE INTO branches(branch_id,title,forked_from_branch,fork_event_id,created_at_unix) \
                 VALUES (?1,?2,?3,?4,?5)",
                params![br.branch_id, br.title, br.forked_from_branch, br.fork_event_id, at],
            )?;
            for ev in &br.events {
                tx.execute(
                    "INSERT OR IGNORE INTO events(id,branch_id,seq,parent_id,payload,actor,created_at_unix) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![ev.id, ev.branch_id, ev.seq, ev.parent_id,
                        serde_json::to_string(&ev.payload)?, ev.actor, ev.created_at_unix],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
