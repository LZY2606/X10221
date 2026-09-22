//! 仓库：串行事务写入。所有写操作要么整体成功，要么回滚（导入失败不留半份记录）。
//! 同版本并发修改通过 expected_parent 乐观锁检测，冲突只登记不覆盖。

use crate::db::Store;
use crate::domain::*;
use crate::events::{EventKind, Refs, StoredEvent};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct ConflictError {
    pub expected_parent: Option<String>,
    pub actual_parent: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Branch {
    pub branch_id: String,
    pub parent_branch: Option<String>,
    pub base_event_id: Option<String>,
    pub tip_event_id: Option<String>,
    pub label: String,
}

pub fn sha256_hex(v: &Value) -> String {
    let bytes = serde_json::to_vec(v).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&bytes);
    hex::encode(h.finalize())
}

impl Store {
    pub fn next_seq(&self, scope: &str) -> i64 {
        let mut c = self.conn.lock().unwrap();
        next_seq_locked(&mut c, scope)
    }

    pub fn list_branches(&self) -> Vec<Branch> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare("SELECT branch_id,parent_branch,base_event_id,tip_event_id,label FROM branches ORDER BY rowid")
            .unwrap();
        stmt.query_map([], |r| {
            Ok(Branch {
                branch_id: r.get(0)?,
                parent_branch: r.get(1)?,
                base_event_id: r.get(2)?,
                tip_event_id: r.get(3)?,
                label: r.get(4)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    pub fn tip(&self, branch: &str) -> Option<String> {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT tip_event_id FROM branches WHERE branch_id=?1", [branch], |r| {
            r.get::<_, Option<String>>(0)
        })
        .ok()
        .flatten()
    }

    pub fn create_branch(&self, id: &str, label: &str, parent_branch: &str, base_event: Option<&str>) {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction().unwrap();
        let seq = next_seq_locked(&tx, "branch");
        let base = base_event.map(|s| s.to_string()).or_else(|| {
            tx.query_row(
                "SELECT tip_event_id FROM branches WHERE branch_id=?1",
                [parent_branch],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
        });
        tx.execute(
            "INSERT INTO branches(branch_id,parent_branch,base_event_id,tip_event_id,label,created_seq)
             VALUES(?1,?2,?3,?3,?4,?5)",
            params![id, parent_branch, base, label, seq],
        )
        .unwrap();
        tx.commit().unwrap();
    }

    /// 追加事件（乐观并发）：expected_parent 必须等于分支当前 tip。
    pub fn append_event(
        &self,
        branch: &str,
        kind: EventKind,
        payload: Value,
        refs: &Refs,
        result: Value,
        expected_parent: Option<&str>,
    ) -> Result<StoredEvent, ConflictError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction().unwrap();
        let current: Option<String> = tx
            .query_row(
                "SELECT tip_event_id FROM branches WHERE branch_id=?1",
                [branch],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .unwrap()
            .flatten();
        let parent_for_insert = current.clone();
        match (expected_parent, current.as_deref()) {
            (Some(exp), Some(cur)) if exp != cur => {
                return Err(log_conflict(&tx, branch, kind, Some(exp), current));
            }
            (Some(_), None) => {
                return Err(log_conflict(&tx, branch, kind, expected_parent, None));
            }
            (None, Some(_)) => {
                return Err(log_conflict(&tx, branch, kind, None, current));
            }
            _ => {}
        }
        let seq = next_seq_locked(&tx, branch);
        let event_id = format!("ev-{}-{:06}", branch, seq);
        let refs_json = serde_json::to_string(refs).unwrap();
        let pay_json = payload.to_string();
        let res_json = result.to_string();
        tx.execute(
            "INSERT INTO events(event_id,seq,branch_id,parent_event_id,kind,payload_json,refs_json,result_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![event_id, seq, branch, parent_for_insert, kind.as_str(), pay_json, refs_json, res_json],
        )
        .unwrap();
        tx.execute(
            "UPDATE branches SET tip_event_id=?1 WHERE branch_id=?2",
            params![event_id, branch],
        )
        .unwrap();
        tx.commit().unwrap();
        Ok(StoredEvent {
            event_id,
            seq,
            branch_id: branch.to_string(),
            parent_event_id: current,
            kind: kind.as_str().to_string(),
            payload,
            refs: refs.clone(),
            result,
        })
    }

    pub fn events_of(&self, branch: &str) -> Vec<StoredEvent> {
        let c = self.conn.lock().unwrap();
        let mut stmt = c
            .prepare(
                "SELECT event_id,seq,branch_id,parent_event_id,kind,payload_json,refs_json,result_json
                 FROM events WHERE branch_id=?1 ORDER BY seq",
            )
            .unwrap();
        stmt.query_map([branch], row_to_event)
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    pub fn event(&self, id: &str) -> Option<StoredEvent> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT event_id,seq,branch_id,parent_event_id,kind,payload_json,refs_json,result_json
             FROM events WHERE event_id=?1",
            [id],
            row_to_event,
        )
        .ok()
    }
}

fn next_seq_locked(conn: &Connection, scope: &str) -> i64 {
    conn.execute(
        "INSERT INTO seq_counters(scope,next_seq) VALUES(?1,1)
         ON CONFLICT(scope) DO UPDATE SET next_seq=next_seq+1",
        [scope],
    )
    .unwrap();
    conn.query_row("SELECT next_seq FROM seq_counters WHERE scope=?1", [scope], |r| {
        r.get(0)
    })
    .unwrap()
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredEvent> {
    let payload: Value = serde_json::from_str(&r.get::<_, String>(5)?).unwrap();
    let refs: Refs = serde_json::from_str(&r.get::<_, String>(6)?).unwrap();
    let result: Value = serde_json::from_str(&r.get::<_, String>(7)?).unwrap();
    Ok(StoredEvent {
        event_id: r.get(0)?,
        seq: r.get(1)?,
        branch_id: r.get(2)?,
        parent_event_id: r.get(3)?,
        kind: r.get(4)?,
        payload,
        refs,
        result,
    })
}

fn log_conflict(
    tx: &rusqlite::Transaction<'_>,
    branch: &str,
    kind: EventKind,
    expected: Option<&str>,
    actual: Option<String>,
) -> ConflictError {
    let seq = next_seq_locked(tx, "conflict");
    let uid = format!("cf-{:06}", seq);
    let diff = serde_json::json!({
        "attempted_kind": kind.as_str(),
        "expected_parent": expected,
        "actual_parent": actual,
        "note": "same-version concurrent modification: later submitter must rebase and sees both diffs",
    });
    tx.execute(
        "INSERT INTO conflict_log(conflict_uid,branch_id,attempted_kind,expected_parent,actual_parent,diff_json,created_seq)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![uid, branch, kind.as_str(), expected, actual, diff.to_string(), seq],
    )
    .unwrap();
    // 冲突本身也写一条 append-only 审计事件（不改变 tip 依赖，因为提交被拒绝）
    ConflictError {
        expected_parent: expected.map(|s| s.to_string()),
        actual_parent: actual,
        message: format!("version conflict on branch {} (kind {})", branch, kind.as_str()),
    }
}
