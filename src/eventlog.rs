use crate::db::{now_iso, Db};
use crate::ids::content_id;
use rusqlite::params;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct EventRow {
    pub event_id: String,
    pub branch_id: String,
    pub branch_seq: i64,
    pub parent_event_id: Option<String>,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
    pub actor: String,
}

pub struct AppendOutcome {
    pub event_id: String,
    pub branch_seq: i64,
}

pub fn create_draft(db: &Db, draft_id: &str, name: &str, case_id: Option<&str>) {
    let conn = db.0.lock().unwrap();
    conn.execute(
        "INSERT INTO drafts(draft_id,name,case_id,created_at) VALUES(?1,?2,?3,?4)",
        params![draft_id, name, case_id, now_iso()],
    )
    .unwrap();
}

pub fn create_branch(
    db: &Db,
    branch_id: &str,
    draft_id: &str,
    name: &str,
    parent_branch_id: Option<&str>,
    fork_event_id: Option<&str>,
    fork_branch_seq: Option<i64>,
) {
    let conn = db.0.lock().unwrap();
    conn.execute(
        "INSERT INTO branches(branch_id,draft_id,name,parent_branch_id,fork_event_id,fork_branch_seq,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![branch_id, draft_id, name, parent_branch_id, fork_event_id, fork_branch_seq, now_iso()],
    )
    .unwrap();
}

pub fn branch_tip(db: &Db, branch_id: &str) -> Option<(String, i64)> {
    let conn = db.0.lock().unwrap();
    conn.query_row(
        "SELECT event_id, branch_seq FROM events WHERE branch_id=?1 ORDER BY branch_seq DESC LIMIT 1",
        params![branch_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

pub struct Precondition {
    pub expected_tip_event_id: Option<String>,
}

/// 追加事件；expected_seq 用于同版本并发修改的乐观锁。
/// 期望不匹配时返回 Err(409 语义的 String)，调用层翻译成 HTTP 409。
pub fn append(
    db: &Db,
    draft_id: &str,
    branch_id: &str,
    kind: &str,
    payload: &Value,
    actor: &str,
    expected_tip_event_id: Option<&str>,
) -> Result<AppendOutcome, String> {
    let mut conn = db.0.lock().unwrap();
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let tip: Option<(String, i64)> = tx
        .query_row(
            "SELECT event_id, branch_seq FROM events WHERE branch_id=?1 ORDER BY branch_seq DESC LIMIT 1",
            params![branch_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let next_seq = tip.as_ref().map(|(_, s)| s + 1).unwrap_or(0);
    if let Some(expected) = expected_tip_event_id {
        match &tip {
            Some((actual, _)) if actual == expected => {}
            Some((actual, _)) => {
                return Err(format!(
                    "VERSION_CONFLICT:期望 tip {expected}，实际 tip {actual}（同版本并发修改）"
                ))
            }
            None => return Err("VERSION_CONFLICT:分支已不再为空".into()),
        }
    }
    let parent_event_id = tip.map(|(id, _)| id);
    let canonical = serde_json::json!({
        "draft_id": draft_id, "branch_id": branch_id, "branch_seq": next_seq,
        "parent_event_id": parent_event_id, "kind": kind, "payload": payload,
    });
    let event_id = content_id("evt", &canonical);
    tx.execute(
        "INSERT INTO events(event_id,draft_id,branch_id,branch_seq,parent_event_id,kind,payload,created_at,actor)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            event_id,
            draft_id,
            branch_id,
            next_seq,
            parent_event_id,
            kind,
            payload.to_string(),
            now_iso(),
            actor
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(AppendOutcome { event_id, branch_seq: next_seq })
}

pub fn list_branch_events(db: &Db, branch_id: &str, up_to_seq: Option<i64>) -> Vec<EventRow> {
    let conn = db.0.lock().unwrap();
    let mut sql = String::from(
        "SELECT event_id,branch_id,branch_seq,parent_event_id,kind,payload,created_at,actor
         FROM events WHERE branch_id=?1",
    );
    if up_to_seq.is_some() {
        sql.push_str(" AND branch_seq<=?2");
    }
    sql.push_str(" ORDER BY branch_seq ASC");
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = if let Some(seq) = up_to_seq {
        stmt.query_map(params![branch_id, seq], map_event).unwrap()
    } else {
        stmt.query_map(params![branch_id], map_event).unwrap()
    };
    rows.map(|r| r.unwrap()).collect()
}

fn map_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    let payload_str: String = r.get(5)?;
    Ok(EventRow {
        event_id: r.get(0)?,
        branch_id: r.get(1)?,
        branch_seq: r.get(2)?,
        parent_event_id: r.get(3)?,
        kind: r.get(4)?,
        payload: serde_json::from_str(&payload_str).unwrap(),
        created_at: r.get(6)?,
        actor: r.get(7)?,
    })
}

pub fn list_drafts(db: &Db) -> Vec<(String, String, Option<String>)> {
    let conn = db.0.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT draft_id,name,case_id FROM drafts ORDER BY created_at,draft_id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

pub fn list_branches(db: &Db, draft_id: &str) -> Vec<serde_json::Value> {
    let conn = db.0.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT branch_id,name,parent_branch_id,fork_event_id,fork_branch_seq
             FROM branches WHERE draft_id=?1 ORDER BY created_at,branch_id",
        )
        .unwrap();
    stmt.query_map(params![draft_id], |r| {
        Ok(serde_json::json!({
            "branch_id": r.get::<_,String>(0)?,
            "name": r.get::<_,String>(1)?,
            "parent_branch_id": r.get::<_,Option<String>>(2)?,
            "fork_event_id": r.get::<_,Option<String>>(3)?,
            "fork_branch_seq": r.get::<_,Option<i64>>(4)?,
        }))
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}
