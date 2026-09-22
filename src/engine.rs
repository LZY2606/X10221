//! 事件溯源引擎：所有写入都是追加事件；当前/历史状态由回放得到。

use crate::case::CaseFile;
use crate::db;
use crate::domain;
use crate::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct Store {
    pub conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub version: i64,
    pub kind: String,
    pub logical_id: String,
    pub content: Value,
    pub sha256: String,
    pub introduced_event_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub artifacts: HashMap<String, ArtifactRef>,
    pub channels: Vec<Value>,
    pub raw_events: BTreeMap<String, Value>,
    pub order: Vec<String>,
    pub decisions: HashMap<String, String>, // eval_id -> strategy
    pub evals: HashMap<String, Value>,
    pub pops: HashMap<String, Value>,
}

pub fn now_iso() -> String {
    "2026-09-23T09:00:00+09:00".to_string()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn canonical_sha(v: &Value) -> String {
    let mut buf = Vec::new();
    serde_json::to_writer(&mut buf, v).ok();
    sha256_hex(&buf)
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self { conn: db::open(path)? })
    }

    /// 分支上从根到 head 的事件序列（跨 fork 链接祖先）。
    pub fn branch_events(&self, branch: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cur = self
            .conn
            .query_row("SELECT head_event_id FROM branches WHERE branch_id=?1", params![branch], |r| {
                r.get::<_, Option<String>>(0)
            })
            .map_err(|e| e.to_string())?;
        let mut guard = 0;
        while let Some(eid) = cur {
            guard += 1;
            if guard > 100000 {
                return Err("事件链异常过长".into());
            }
            let row = self
                .conn
                .query_row("SELECT parent_event_id, kind, payload_json FROM events WHERE event_id=?1", params![eid], |r| {
                    Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .map_err(|e| e.to_string())?;
            let (parent, event_kind, payload) = row;
            let mut obj: Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            obj["event_id"] = json!(eid);
            obj["parent_event_id"] = json!(parent);
            obj["kind"] = json!(event_kind);
            obj["source_fragment_id"] = json!(obj.get("source_fragment_id").cloned());
            out.push(obj);
            cur = parent;
        }
        out.reverse();
        Ok(out)
    }

    /// 回放到某个事件（含）。at=None 表示分支 head。
    pub fn state_at(&self, branch: &str, at: Option<&str>) -> Result<State> {
        let events = self.branch_events(branch)?;
        let mut state = State::default();
        for ev in events {
            apply_event(&mut state, &ev)?;
            if Some(ev["event_id"].as_str().unwrap_or("")) == at {
                break;
            }
        }
        Ok(state)
    }

    /// 追加一个事件（事务）。expected_rev 为乐观并发版本；不匹配返回 Err("CONFLICT:..")。
    /// 任何失败整体回滚，不留半份记录。
    pub fn append(
        &mut self,
        branch: &str,
        kind: &str,
        mut content: Value,
        expected_rev: Option<i64>,
        source_fragment: Option<&str>,
    ) -> Result<(String, i64)> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        let (head, rev): (Option<String>, i64) = {
            let mut stmt = tx
                .prepare("SELECT head_event_id, revision FROM branches WHERE branch_id=?1")
                .map_err(|e| e.to_string())?;
            let mut rows = stmt.query(params![branch]).map_err(|e| e.to_string())?;
            let row = rows.next().map_err(|e| e.to_string())?.ok_or("分支不存在")?;
            let head_ref = row.get_ref(0).map_err(|e| e.to_string())?;
            let head = if matches!(head_ref, rusqlite::types::ValueRef::Null) {
                None
            } else {
                Some(head_ref.as_str().map_err(|e| e.to_string())?.to_string())
            };
            (head, row.get(1).map_err(|e| e.to_string())?)
        };
        if let Some(exp) = expected_rev {
            if exp != rev {
                let their: Option<String> = tx
                    .query_row(
                        "SELECT payload_json FROM events WHERE event_id=COALESCE(?1,'')",
                        params![head],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .ok()
                    .flatten();
                return Err(format!(
                    "CONFLICT: branch={branch} expected_rev={exp} current_rev={rev} their_event={}",
                    their.unwrap_or_default()
                ));
            }
        }
        let seq: i64 = tx
            .query_row(
                "SELECT COALESCE((SELECT MAX(seq) FROM events WHERE branch_id=?1),0)+1",
                params![branch],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let eid = format!("evt-{branch}-{seq:03}");
        content["event_id"] = json!(eid);
        content["seq"] = json!(seq);
        let payload = serde_json::to_string(&content).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO events(event_id,seq,branch_id,parent_event_id,kind,payload_json,basis_revision,created_at,source_fragment_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![eid, seq, branch, head, kind, payload, rev, now_iso(), source_fragment],
        )
        .map_err(|e| e.to_string())?;
        let new_rev = rev + 1;
        tx.execute(
            "UPDATE branches SET head_event_id=?1, revision=?2 WHERE branch_id=?3",
            params![eid, new_rev, branch],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok((eid, new_rev))
    }

    pub fn branch_revision(&self, branch: &str) -> Result<i64> {
        self.conn
            .query_row("SELECT revision FROM branches WHERE branch_id=?1", params![branch], |r| r.get(0))
            .map_err(|e| e.to_string())
    }

    pub fn create_branch(&mut self, study: &str, name: &str, fork_event: Option<&str>) -> Result<String> {
        let id = name.to_string();
        self.conn
            .execute(
                "INSERT INTO branches(branch_id,study_id,name,fork_event_id,head_event_id,revision,created_at)
                 VALUES(?1,?2,?3,?4,?4,0,?5)",
                params![id, study, name, fork_event, now_iso()],
            )
            .map_err(|e| e.to_string())?;
        Ok(id)
    }
}

/// 追加一个构件版本事件；返回 (event_id, revision)。
pub fn publish_artifact(
    store: &mut Store,
    branch: &str,
    kind: &str,
    logical_id: &str,
    version: i64,
    content: Value,
    expected_rev: Option<i64>,
) -> Result<(String, i64)> {
    let sha = canonical_sha(&content);
    let artifact_id = format!("art-{kind}-{logical_id}");
    let payload = json!({
        "artifact_kind": kind,
        "logical_id": logical_id,
        "artifact_id": artifact_id,
        "version": version,
        "content": content,
        "sha256": sha,
    });
    store.append(branch, "artifact", payload, expected_rev, None)
}

fn artifact_key(kind: &str, logical_id: &str) -> String {
    format!("{kind}:{logical_id}")
}

fn put_artifact(state: &mut State, ev_id: &str, payload: &Value) {
    let kind = payload["artifact_kind"].as_str().unwrap_or("unknown").to_string();
    let logical_id = payload["logical_id"].as_str().unwrap_or("unknown").to_string();
    let version = payload["version"].as_i64().unwrap_or(1);
    let content = payload["content"].clone();
    let sha = payload["sha256"].as_str().unwrap_or("").to_string();
    let artifact_id = payload["artifact_id"].as_str().unwrap_or("").to_string();
    let aref = ArtifactRef {
        artifact_id,
        version,
        kind: kind.clone(),
        logical_id: logical_id.clone(),
        content,
        sha256: sha,
        introduced_event_id: ev_id.to_string(),
    };
    state.artifacts.insert(artifact_key(&kind, &logical_id), aref);
}

fn apply_event(state: &mut State, ev: &Value) -> Result<()> {
    let ev_id = ev["event_id"].as_str().unwrap_or("");
    let kind = ev["kind"].as_str().unwrap_or("");
    match kind {
        "artifact" => put_artifact(state, ev_id, ev),
        // 兼容 payload 形如 {"kind":..,"content":{真实构件}} 的历史写法
        "channels" => {
            state.channels = ev["content"]["channels"].as_array().cloned().unwrap_or_default();
        }
        "raw_snapshot" => {
            for (i, item) in ev["content"]["events"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                let id = item["id"].as_str().unwrap_or("").to_string();
                state.raw_events.insert(id.clone(), item.clone());
                state.order.push(id);
                let _ = i;
            }
        }
        "raw_patch" => {
            if let Some(frag) = ev["content"]["fragment_id"].as_str() {
                let _ = frag;
            }
            for item in ev["content"]["add"].as_array().cloned().unwrap_or_default() {
                let id = item["id"].as_str().unwrap_or("").to_string();
                state.raw_events.insert(id.clone(), item);
                state.order.push(id);
            }
            for item in ev["content"]["modify"].as_array().cloned().unwrap_or_default() {
                let id = item["id"].as_str().unwrap_or("").to_string();
                if let Some(entry) = state.raw_events.get_mut(&id) {
                    if let Some(obj) = item.get("raw").and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            entry["raw"][k] = v.clone();
                        }
                    }
                    entry["diagnostics"] = json!([{
                        "source_revision": ev["content"]["fragment_id"],
                        "note": item["note"]
                    }]);
                }
            }
        }
        "comp_evaluated" => {
            let eval_id = ev["content"]["eval_id"].as_str().unwrap_or("").to_string();
            state.evals.insert(eval_id, ev["content"].clone());
        }
        "comp_decision" => {
            let eval_id = ev["content"]["eval_id"].as_str().unwrap_or("").to_string();
            if let Some(strategy) = ev["content"]["strategy"].as_str() {
                state.decisions.insert(eval_id, strategy.to_string());
            }
        }
        "decision_undone" => {
            if let Some(eval_id) = ev["content"]["eval_id"].as_str() {
                state.decisions.remove(eval_id);
            }
        }
        "pop_recomputed" => {
            for pop in ev["content"]["populations"].as_array().cloned().unwrap_or_default() {
                let id = pop["result_id"].as_str().unwrap_or("").to_string();
                state.pops.insert(id, pop);
            }
        }
        "pop_stale" => {
            for id in ev["content"]["expired_result_ids"].as_array().cloned().unwrap_or_default() {
                if let Some(p) = state.pops.get_mut(id.as_str().unwrap_or("")) {
                    p["status"] = json!("stale");
                    p["stale"] = json!(true);
                    p["stale_reason_event"] = json!(ev_id);
                }
            }
        }
        _ => {}
    }
    Ok(())
}
