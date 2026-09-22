//! 应用层：材料注册、版本化写入、事件持久化、失效范围落库、候选集落库、分支/归档。
//! 所有写动作：先在内存折叠状态上计算影响，再在同一事务追加事件 + 写版本/结果/状态/候选。

use crate::db::{now_iso, Db};
use crate::eventlog::{self, EventRow};
use crate::fold::{self, FoldState};
use crate::ids::{content_id, sha256_hex};
use crate::linalg;
use crate::models::*;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;

pub struct Engine<'a> {
    pub db: &'a Db,
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub event_id: String,
    pub branch_seq: i64,
    pub invalidated: Vec<String>,
    pub blocked: Vec<String>,
    pub warnings: Vec<String>,
    pub candidates: Option<Value>,
}

pub fn load_state(db: &Db, branch_id: &str, at_seq: Option<i64>) -> FoldState {
    let events = eventlog::list_branch_events(db, branch_id, at_seq);
    fold::fold(&events)
}

impl<'a> Engine<'a> {
    pub fn new(db: &'a Db) -> Self {
        Engine { db }
    }

    pub fn register_blob(&self, kind: &str, bytes: &[u8]) -> String {
        let id = format!("blob-{}-{}", kind, sha256_hex(bytes)[..16].to_string());
        let conn = self.db.0.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO blobs(blob_id,kind,bytes,sha256,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![id, kind, bytes, sha256_hex(bytes), now_iso()],
        )
        .unwrap();
        id
    }

    fn insert_version(&self, kind: &str, entity_id: &str, payload: &Value, event_id: &str, draft_id: &str, supersedes: Option<&str>) -> String {
        let version_id = content_id(&format!("ver-{}", kind), &json!({"entity": entity_id, "payload": payload}));
        let conn = self.db.0.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO versions(version_id,kind,entity_id,supersedes_version_id,payload,event_id,draft_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![version_id, kind, entity_id, supersedes, payload.to_string(), event_id, draft_id],
        )
        .unwrap();
        version_id
    }

    pub fn create_draft_with_branch(
        &self,
        draft_id: &str,
        name: &str,
        case_id: Option<&str>,
        branch_id: &str,
        branch_name: &str,
    ) {
        eventlog::create_draft(self.db, draft_id, name, case_id);
        eventlog::create_branch(self.db, branch_id, draft_id, branch_name, None, None, None);
    }

    pub fn fork_branch(
        &self,
        draft_id: &str,
        source_branch_id: &str,
        at_seq: Option<i64>,
        new_branch_id: &str,
        new_name: &str,
        actor: &str,
    ) -> Result<ActionResult, String> {
        let events = eventlog::list_branch_events(self.db, source_branch_id, at_seq);
        let tip = events.last();
        let fork_event_id = tip.map(|e| e.event_id.clone());
        let fork_seq = tip.map(|e| e.branch_seq);
        eventlog::create_branch(
            self.db,
            new_branch_id,
            draft_id,
            new_name,
            Some(source_branch_id),
            fork_event_id.as_deref(),
            fork_seq,
        );
        // 先逐字复制到分叉点（保持 event_id、parent 链与历史结论），再追加 fork 标记
        for ev in &events {
            self.copy_event_to_fork(draft_id, new_branch_id, ev)?;
        }
        let payload = json!({
            "source_branch_id": source_branch_id,
            "fork_event_id": fork_event_id,
            "fork_branch_seq": fork_seq,
        });
        let out = eventlog::append(self.db, draft_id, new_branch_id, "branch_forked", &payload, actor, None)?;
        Ok(ActionResult {
            event_id: out.event_id,
            branch_seq: out.branch_seq,
            invalidated: vec![],
            blocked: vec![],
            warnings: vec![],
            candidates: None,
        })
    }

    fn copy_event_to_fork(&self, draft_id: &str, new_branch_id: &str, ev: &EventRow) -> Result<(), String> {
        let conn = self.db.0.lock().unwrap();
        let max_seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(branch_seq),0) FROM events WHERE branch_id=?1",
                params![new_branch_id],
                |r| r.get(0),
            )
            .unwrap();
        let new_seq = max_seq + 1;
        let parent: Option<String> = if new_seq == 1 {
            None
        } else {
            conn.query_row(
                "SELECT event_id FROM events WHERE branch_id=?1 ORDER BY branch_seq DESC LIMIT 1",
                params![new_branch_id],
                |r| r.get(0),
            ).ok()
        };
        conn.execute(
            "INSERT INTO events(event_id,draft_id,branch_id,branch_seq,parent_event_id,kind,payload,created_at,actor)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                ev.event_id,
                draft_id,
                new_branch_id,
                new_seq,
                parent,
                ev.kind,
                ev.payload.to_string(),
                ev.created_at,
                ev.actor
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn current_version_id_for(st: &FoldState, kind: &str, entity_id: &str) -> Option<String> {
    match kind {
        "gate" => st.gates.get(entity_id).map(|(_, v)| v.clone()),
        "transform" => st.transforms.get(entity_id).map(|(_, v)| v.clone()),
        "matrix" => st
            .active_matrix_version
            .clone()
            .filter(|v| st.matrices.get(v).is_some()),
        _ => None,
    }
}

impl<'a> Engine<'a> {
    /// 通用“追加事件 + 版本行 + 落库派生”流程。
    fn commit(
        &self,
        draft_id: &str,
        branch_id: &str,
        kind: &str,
        mut payload: Value,
        actor: &str,
        expected_tip: Option<&str>,
        before: &FoldState,
    ) -> Result<ActionResult, String> {
        // 版本行需要在事件内引用 version_id：先放占位，由 content_id 决定 version_id，
        // 但 event_id 依赖 payload，所以这里先构造好 version_id 再追加事件。
        let out = eventlog::append(self.db, draft_id, branch_id, kind, &payload, actor, expected_tip)?;
        payload["event_id"] = json!(out.event_id);
        payload["branch_seq"] = json!(out.branch_seq);

        // 持久化版本 / 结果 / 状态 / 候选（事件是唯一权威，这些是物化读模型）
        self.materialize(kind, &payload, &out.event_id, draft_id, before)?;

        // 折叠后状态，用于返回失效范围与阻塞对象
        let after = load_state(self.db, branch_id, None);
        let mut invalidated = Vec::new();
        let mut blocked = Vec::new();
        for (id, pop) in &after.pops {
            let (status, _) = fold::effective_status(&after, pop);
            if status == "stale" && fold::effective_status(before, pop).0 != "stale" {
                invalidated.push(id.clone());
            }
            if status == "blocked" {
                blocked.push(id.clone());
            }
        }
        let mut warnings = Vec::new();
        let mut candidates = None;
        if kind == "comp_matrix_revised" {
            let vid = payload["version_id"].as_str().unwrap_or("");
            let set_id = format!("candset-comp-{}", vid.trim_start_matches("ver-comp-"));
            if let Some(cs) = after.candidates.get(&set_id) {
                blocked.push(format!("fluor-populations-pending:{set_id}"));
                warnings.push(cs.reason.clone());
                candidates = Some(serialize_candidate_set(cs));
            }
        }
        Ok(ActionResult {
            event_id: out.event_id,
            branch_seq: out.branch_seq,
            invalidated,
            blocked,
            warnings,
            candidates,
        })
    }

    fn materialize(
        &self,
        kind: &str,
        payload: &Value,
        event_id: &str,
        draft_id: &str,
        _before: &FoldState,
    ) -> Result<(), String> {
        match kind {
            "fcs_imported" | "events_revised" => {
                let fcs = &payload["fcs"];
                self.insert_version("fcs", &payload["source_blob_id"].as_str().unwrap_or("fcs"), fcs, event_id, draft_id, None);
            }
            "spillover_revised" => {
                let entity = payload["spillover"]["source_blob_id"].as_str().unwrap_or("spill");
                self.insert_version("spillover", entity, &payload["spillover"], event_id, draft_id, None);
            }
            "comp_matrix_revised" => {
                let entity = payload.get("entity_id").and_then(|v| v.as_str()).unwrap_or("primary");
                self.insert_version("comp_matrix", entity, &payload["matrix"], event_id, draft_id, None);
            }
            "transform_revised" => {
                let entity = match serde_json::from_value::<TransformSpec>(payload["transform"].clone()) {
                    Ok(t) => fold_channel_id(&t),
                    Err(_) => "transform".to_string(),
                };
                self.insert_version("transform", &entity, &payload["transform"], event_id, draft_id, None);
            }
            "gate_revised" => {
                let entity = payload["gate"]["gate_id"].as_str().unwrap();
                self.insert_version("gate", entity, &payload["gate"], event_id, draft_id, None);
            }
            "gate_tree_revised" => {
                self.insert_version("gate_tree", "primary", &payload["tree"], event_id, draft_id, None);
            }
            "pop_recomputed" => {
                let pop = &payload["pop"];
                let conn = self.db.0.lock().unwrap();
                conn.execute(
                    "INSERT OR IGNORE INTO pop_results(
                        pop_result_id,gate_id,gate_geom_version_id,parent_pop_result_id,
                        denominator_kind,denominator_ref,denominator_count,numerator_count,
                        excluded_unconvertible,percent,status,detail,deps,created_event_id)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                    params![
                        pop["pop_result_id"], pop["gate_id"], pop["gate_geom_version_id"],
                        pop["parent_pop_result_id"], pop["denominator_kind"], pop["denominator_ref"],
                        pop["denominator_count"], pop["numerator_count"], pop["excluded_unconvertible"],
                        pop["percent"], pop["status"],
                        pop["detail"].to_string(), pop["deps"].to_string(), event_id,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            "pop_marked_stale" => {
                let ids: Vec<String> = serde_json::from_value(payload["pop_result_ids"].clone()).unwrap();
                let note = payload["reason"].as_str().unwrap_or("");
                let conn = self.db.0.lock().unwrap();
                for id in ids {
                    conn.execute(
                        "INSERT INTO pop_status_log(pop_result_id,branch_id,branch_seq,status,reason_event_id,note)
                         VALUES(?1,?2,?3,'stale',?4,?5)",
                        params![id, payload["branch_id_for_log"], id, event_id, note],
                    )
                    .ok();
                }
            }
            "comp_decision" => {
                let conn = self.db.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO decisions(decision_id,candidate_set_id,candidate_id,action,rationale,event_id)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        payload["decision_id"], payload["candidate_set_id"], payload["candidate_id"],
                        payload["action"], payload["rationale"], event_id,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            _ => {}
        }
        // 候选集物化
        if kind == "comp_matrix_revised" {
            let after = load_state(self.db, branch_id_from(payload), None);
            for cs in after.candidates.values() {
                for c in &cs.candidates {
                    let conn = self.db.0.lock().unwrap();
                    conn.execute(
                        "INSERT OR IGNORE INTO candidates(candidate_id,candidate_set_id,label,legal,payload,created_event_id)
                         VALUES(?1,?2,?3,1,?4,?5)",
                        params![c.candidate_id, c.candidate_set_id, c.label, c.detail.to_string(), event_id],
                    )
                    .ok();
                }
            }
        }
        Ok(())
    }
}

fn fold_channel_id(t: &TransformSpec) -> String {
    match t {
        TransformSpec::Linear { channel_id, .. }
        | TransformSpec::Log10Offset { channel_id, .. } => channel_id.clone(),
    }
}

fn branch_id_from(payload: &Value) -> &str {
    payload.get("branch_id").and_then(|v| v.as_str()).unwrap_or("")
}

pub fn serialize_candidate_set(cs: &fold::CandidateSet) -> Value {
    json!({
        "candidate_set_id": cs.set_id,
        "reason": cs.reason,
        "matrix_version_id": cs.matrix_version_id,
        "candidates": cs.candidates.iter().map(|c| json!({
            "candidate_id": c.candidate_id,
            "label": c.label,
            "legal": c.legal,
            "detail": c.detail,
        })).collect::<Vec<_>>(),
    })
}
