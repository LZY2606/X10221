//! SQLite 存储：只追加事件 + 事务原子性 + 乐观并发 + 归档导入导出。

use crate::projection::{replay, Projection, ReplayEvent};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub branch_id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub forked_from_event: Option<String>,
    pub head_seq: i64,
    pub color: String,
    pub fork_cutoff_seq: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictDetail {
    pub branch_id: String,
    pub expected_base_seq: i64,
    pub actual_head_seq: i64,
    pub local_kind: String,
    pub remote_kind: String,
    pub local_payload: Value,
    pub remote_payload: Value,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AppendOutcome {
    pub event_id: String,
    pub seq: i64,
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    ("0002_drafts", include_str!("../migrations/0002_drafts.sql")),
    ("0003_labels", include_str!("../migrations/0003_labels.sql")),
];

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let mut conn = Connection::open(path)
            .with_context(|| format!("打开数据库失败: {path}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn user_version(&self) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn list_branches(&self) -> Result<Vec<BranchInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT b.branch_id, b.name, b.parent_branch, b.forked_from_event,
                    h.head_seq, b.color, b.fork_cutoff_seq
             FROM branches b LEFT JOIN branch_heads h ON h.branch_id = b.branch_id
             ORDER BY b.rowid")?;
        let rows = stmt.query_map([], |r| {
            Ok(BranchInfo {
                branch_id: r.get(0)?,
                name: r.get(1)?,
                parent_branch: r.get(2)?,
                forked_from_event: r.get(3)?,
                head_seq: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                color: r.get::<_, String>(5).unwrap_or_else(|_| "#7aa2f7".into()),
                fork_cutoff_seq: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn head_seq(&self, branch: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row("SELECT head_seq FROM branch_heads WHERE branch_id=?1", params![branch], |r| r.get(0))
            .unwrap_or(0))
    }

    /// 新建分支（fork）。forked_seq 钉住分叉点，重放只取该点之前（含）的事件。
    pub fn create_branch(&self, id: &str, name: &str, parent: &str, forked_seq: i64, color: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM branches WHERE branch_id=?1", params![id], |r| r.get(0))?;
        if exists != 0 {
            return Err(anyhow!("分支 {id} 已存在"));
        }
        let fork_event: Option<String> = tx
            .query_row(
                "SELECT event_id FROM events WHERE branch_id=?1 AND seq=?2",
                params![parent, parent_head],
                |r| r.get(0),
            )
            .ok()
            .or(if parent_head == 0 { None } else { None });
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO branches(branch_id,name,parent_branch,forked_from_event,fork_cutoff_seq,created_at,color)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![id, name, parent, fork_event, forked_seq, now, color],
        )?;
        tx.execute(
            "INSERT INTO branch_heads(branch_id,head_seq,updated_at,conflict_count)
            VALUES(?1,0,?2,0)",
            params![id, now],
        )?;
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (idx, (_, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .with_context(|| format!("迁移 {version} 失败"))?;
        conn.pragma_update(None, "user_version", version)?;
    }
    Ok(())
}

impl Store {
    /// 读取分支可见事件：祖先链 + 本分支，按 seq<=分叉/head 过滤。
    /// 返回分支链路（从根到当前）与每段的截止 seq。
    fn branch_chain(&self, conn: &Connection, branch: &str) -> Result<Vec<(String, i64)>> {
        // 自当前分支向上收集原始元组：(id, own_head, parent, fork_cutoff)
        #[derive(Clone)]
        struct Seg { id: String, head: i64, parent: Option<String>, cutoff: i64 }
        let mut raw: Vec<Seg> = Vec::new();
        let mut cur = Some(branch.to_string());
        let mut guard = 0usize;
        while let Some(id) = cur {
            guard += 1;
            if guard > 64 {
                return Err(anyhow!("分支链超过 64 层，疑似环"));
            }
            let row = conn.query_row(
                "SELECT b.parent_branch,
                        COALESCE((SELECT head_seq FROM branch_heads h WHERE h.branch_id=b.branch_id),0),
                        b.fork_cutoff_seq
                 FROM branches b WHERE b.branch_id=?1",
                params![id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
            );
            match row {
                Ok((parent, head, cutoff)) => {
                    cur = parent.clone();
                    raw.push(Seg { id, head, parent, cutoff });
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => break,
                Err(e) => return Err(e.into()),
            }
        }
        // raw[0] 是当前分支（叶子），raw[last] 是根。
        // 可见范围：叶子段=自己的 head；其余祖先段=其直接子段分叉时钉住的 cutoff。
        let mut chain: Vec<(String, i64)> = Vec::new();
        for (i, seg) in raw.iter().enumerate() {
            let cutoff = if i == 0 {
                seg.head
            } else {
                raw[i - 1].cutoff
            };
            chain.push((seg.id.clone(), cutoff));
        }
        chain.reverse();
        Ok(chain)
    }

    pub fn projection(&self, branch: &str) -> Result<Projection> {
        let conn = self.conn.lock().unwrap();
        let chain = self.branch_chain(&conn, branch)?;
        let head_seq = chain.last().map(|(_, s)| *s).unwrap_or(0);
        let mut events = Vec::new();
        for (idx, (bid, cutoff)) in chain.iter().enumerate() {
            // 祖先段只能看到分叉点（即子链 cutoff 在祖先上的投影）。
            // 简化模型：祖先段 cutoff 由该分支记录的 head（fork 时钉住）决定；
            // 当前段取实时 head。
            let mut stmt = conn.prepare(
                "SELECT event_id,seq,parent_event,kind,payload,rule,created_at
                 FROM events WHERE branch_id=?1 AND seq<=?2 ORDER BY seq")?;
            let rows = stmt.query_map(params![bid, cutoff], |r| {
                Ok(ReplayEvent {
                    event_id: r.get(0)?,
                    seq: r.get(1)?,
                    parent_event: r.get(2)?,
                    kind: r.get(3)?,
                    payload: serde_json::from_str(r.get::<_, String>(4)?.as_str())
                        .unwrap_or(Value::Null),
                    rule: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?;
            for r in rows {
                events.push(r?);
            }
        }
        events.sort_by_key(|e| e.seq);
        Ok(replay(branch, head_seq, &events))
    }

    /// 在分支上追加事件。expected_base_seq 用于乐观并发：
    /// 若分支头已经前进，则不写入业务事件，而是记录 draft.conflict 审计事件
    /// （审计事件也走同一序列，保留双方差异），返回冲突错误。
    pub fn append_event(
        &self,
        branch: &str,
        kind: &str,
        payload: Value,
        rule: Option<&str>,
        expected_base_seq: Option<i64>,
    ) -> Result<AppendOutcome> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let head: i64 = tx
            .query_row("SELECT head_seq FROM branch_heads WHERE branch_id=?1", params![branch], |r| {
                r.get(0)
            })
            .unwrap_or(0);

        if let Some(base) = expected_base_seq {
            if base != head {
                // 拉取对方最新事件用于双方差异
                let remote = tx
                    .query_row(
                        "SELECT kind,payload FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                        params![branch],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .ok();
                let (remote_kind, remote_payload) = match remote {
                    Some((k, p)) => (k, serde_json::from_str(&p).unwrap_or(Value::Null)),
                    None => (String::new(), Value::Null),
                };
                let detail = ConflictDetail {
                    branch_id: branch.into(),
                    expected_base_seq: base,
                    actual_head_seq: head,
                    local_kind: kind.into(),
                    remote_kind,
                    local_payload: payload.clone(),
                    remote_payload,
                };
                let seq = next_seq(&tx)?;
                let now = Utc::now().to_rfc3339();
                let event_id = format!("evt-{seq}");
                let conflict_payload = serde_json::to_value(&detail)?;
                tx.execute(
                    "INSERT INTO events(event_id,seq,branch_id,parent_event,kind,payload,rule,created_at)
                     VALUES(?1,?2,?3,NULL,'draft.conflict',?4,'RULE_SAME_VERSION_CONCURRENT_MODIFY',?5)",
                    params![event_id, seq, branch, conflict_payload.to_string(), now],
                )?;
                tx.execute(
                    "UPDATE branch_heads SET head_seq=?1, updated_at=?2,
                        conflict_count=conflict_count+1 WHERE branch_id=?3",
                    params![seq, now, branch],
                )?;
                tx.commit()?;
                return Err(anyhow!("VERSION_CONFLICT base={base} head={head}"));
            }
        }

        let parent_event: Option<String> = tx
            .query_row(
                "SELECT event_id FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
                params![branch],
                |r| r.get(0),
            )
            .ok();
        let seq = next_seq(&tx)?;
        let now = Utc::now().to_rfc3339();
        let event_id = format!("evt-{seq}");
        tx.execute(
            "INSERT INTO events(event_id,seq,branch_id,parent_event,kind,payload,rule,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                event_id, seq, branch, parent_event, kind, payload.to_string(), rule, now
            ],
        )?;
        tx.execute(
            "INSERT INTO branch_heads(branch_id,head_seq,updated_at,conflict_count)
             VALUES(?1,?2,?3,0)
             ON CONFLICT(branch_id) DO UPDATE SET head_seq=excluded.head_seq, updated_at=excluded.updated_at",
            params![branch, seq, now],
        )?;
        tx.commit()?;
        Ok(AppendOutcome { event_id, seq })
    }

    /// 撤销：把分支头回退到指定事件（事件本身保留，可再次前进/查看）。
    pub fn rollback_head(&self, branch: &str, to_seq: i64) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let head: i64 = tx
            .query_row("SELECT head_seq FROM branch_heads WHERE branch_id=?1", params![branch], |r| {
                r.get(0)
            })?;
        if to_seq > head || to_seq < 0 {
            return Err(anyhow!("回退目标 {to_seq} 超出当前范围 0..={head}"));
        }
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE branch_heads SET head_seq=?1, updated_at=?2 WHERE branch_id=?3",
            params![to_seq, now, branch],
        )?;
        tx.commit()?;
        Ok(to_seq)
    }
}

fn next_seq(tx: &rusqlite::Transaction) -> Result<i64> {
    Ok(tx.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events", [], |r| r.get(0))?)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveEvent {
    pub event_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_event: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub rule: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveBranch {
    pub branch_id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub forked_from_event: Option<String>,
    pub fork_cutoff_seq: i64,
    pub head_seq: i64,
    pub color: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Archive {
    pub format: String,
    pub version: u32,
    pub exported_at: String,
    pub branches: Vec<ArchiveBranch>,
    pub events: Vec<ArchiveEvent>,
}

impl Store {
    pub fn export_archive(&self) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let mut branches = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT b.branch_id,b.name,b.parent_branch,b.forked_from_event,b.fork_cutoff_seq,
                        COALESCE(h.head_seq,0),b.color
                 FROM branches b LEFT JOIN branch_heads h ON h.branch_id=b.branch_id
                 ORDER BY b.rowid")?;
            let rows = stmt.query_map([], |r| {
                Ok(ArchiveBranch {
                    branch_id: r.get(0)?,
                    name: r.get(1)?,
                    parent_branch: r.get(2)?,
                    forked_from_event: r.get(3)?,
                    fork_cutoff_seq: r.get(4)?,
                    head_seq: r.get(5)?,
                    color: r.get(6)?,
                })
            })?;
            for b in rows {
                branches.push(b?);
            }
        }
        let mut events = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT event_id,seq,branch_id,parent_event,kind,payload,rule,created_at
                 FROM events ORDER BY seq")?;
            let rows = stmt.query_map([], |r| {
                Ok(ArchiveEvent {
                    event_id: r.get(0)?,
                    seq: r.get(1)?,
                    branch_id: r.get(2)?,
                    parent_event: r.get(3)?,
                    kind: r.get(4)?,
                    payload: serde_json::from_str(r.get::<_, String>(5)?.as_str())
                        .unwrap_or(Value::Null),
                    rule: r.get(6)?,
                    created_at: r.get(7)?,
                })
            })?;
            for e in rows {
                events.push(e?);
            }
        }
        let archive = Archive {
            format: "flowline-archive".into(),
            version: 1,
            exported_at: Utc::now().to_rfc3339(),
            branches,
            events,
        };
        Ok(serde_json::to_string_pretty(&archive)?)
    }

    /// 导入归档到空数据库；任何失败整体回滚，不留半份记录。
    /// 导入后身份（event_id/branch_id）、顺序（seq）、来源关系（parent/chain）保持不变。
    pub fn import_archive(&self, json_text: &str) -> Result<()> {
        let archive: Archive = serde_json::from_str(json_text)
            .context("归档格式解析失败")?;
        if archive.format != "flowline-archive" {
            return Err(anyhow!("不是 flowline-archive 归档"));
        }
        let mut conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        if count != 0 {
            return Err(anyhow!("目标数据库非空，拒绝覆盖导入"));
        }
        let tx = conn.transaction()?;
        for b in &archive.branches {
            tx.execute(
                "INSERT INTO branches(branch_id,name,parent_branch,forked_from_event,fork_cutoff_seq,created_at,color)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    b.branch_id, b.name, b.parent_branch, b.forked_from_event,
                    b.fork_cutoff_seq, Utc::now().to_rfc3339(), b.color
                ],
            )?;
        }
        let mut last_seq_by_branch: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let mut events_sorted = archive.events.clone();
        events_sorted.sort_by_key(|e| e.seq);
        for e in &events_sorted {
            tx.execute(
                "INSERT INTO events(event_id,seq,branch_id,parent_event,kind,payload,rule,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    e.event_id, e.seq, e.branch_id, e.parent_event, e.kind,
                    e.payload.to_string(), e.rule, e.created_at
                ],
            )?;
            last_seq_by_branch.insert(e.branch_id.clone(), e.seq);
        }
        // 来源关系一致性校验：parent_event 必须存在；seq 唯一连续
        for e in &events_sorted {
            if let Some(pe) = &e.parent_event {
                let exists: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM events WHERE event_id=?1", params![pe], |r| r.get(0))?;
                if exists == 0 {
                    return Err(anyhow!("来源链断裂：事件 {} 引用不存在的父事件 {}", e.event_id, pe));
                }
            }
        }
        for b in &archive.branches {
            let head = b.head_seq;
            tx.execute(
                "INSERT INTO branch_heads(branch_id,head_seq,updated_at,conflict_count)
                 VALUES(?1,?2,?3,0)
                 ON CONFLICT(branch_id) DO UPDATE SET head_seq=excluded.head_seq",
                params![b.branch_id, head, Utc::now().to_rfc3339()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn create_root_branch(&self, id: &str, name: &str, color: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM branches WHERE branch_id=?1", params![id], |r| r.get(0))?;
        if exists != 0 {
            return Err(anyhow!("分支 {id} 已存在"));
        }
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO branches(branch_id,name,parent_branch,forked_from_event,fork_cutoff_seq,created_at,color)
             VALUES(?1,?2,NULL,NULL,0,?3,?4)",
            params![id, name, now, color],
        )?;
        tx.execute(
            "INSERT INTO branch_heads(branch_id,head_seq,updated_at,conflict_count)
             VALUES(?1,0,?2,0)",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn reset(&self) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM events; DELETE FROM branches; DELETE FROM branch_heads;
             DELETE FROM blobs; DELETE FROM drafts;")?;
        tx.commit()?;
        Ok(())
    }
}
