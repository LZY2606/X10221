//! 应用服务：把固定案例包装载为初始事件，并提供脚本化场景操作。
//! 所有写操作都经过 Store::append_event（事件溯源 + 乐观并发）。

use crate::fixedcase;
use crate::model::{GateDef, SpilloverSpec, TransformSpec};
use crate::store::Store;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub const ROOT_BRANCH: &str = "main";

pub struct AppState {
    pub store: Store,
}

impl AppState {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn is_seeded(&self) -> Result<bool> {
        Ok(self.store.head_seq(ROOT_BRANCH)? > 0)
    }

    /// 初始导入：原始通道值/spillover/变换/门/父子关系作为一个原子事件写入。
    /// 失败回滚（append 本身事务化），不会留下半份记录。
    pub fn seed_baseline(&self) -> Result<i64> {
        if self.is_seeded? {
            return Err(anyhow!("已经导入过基线案例；重新解释请新建分支"));
        }
        let case = fixedcase::baseline_case();
        self.ensure_root_branch()?;
        let out = self.store.append_event(
            ROOT_BRANCH,
            "imported.case",
            serde_json::to_value(&case)?,
            Some("RULE_IMPORT_ATOMIC_KEEP_RAW"),
            None,
        )?;
        Ok(out.seq)
    }

    fn ensure_root_branch(&self) -> Result<()> {
        let branches = self.store.list_branches()?;
        if !branches.iter().any(|b| b.branch_id == ROOT_BRANCH) {
            self.store.create_root_branch(ROOT_BRANCH, "主线（基线）", "#7aa2f7")?;
        }
        Ok(())
    }

    pub fn fork_branch(&self, id: &str, name: &str, parent: &str, color: &str) -> Result<i64> {
        let head = self.store.head_seq(parent)?;
        self.store.create_branch(id, name, parent, head, color)?;
        Ok(head)
    }
}

impl AppState {
    /// 统一动作入口。base_seq=Some 时启用乐观并发。
    pub fn act(&self, req: ActionRequest) -> Result<serde_json::Value> {
        match req.action.as_str() {
            "fork" => {
                let id = req.param("branch_id")?;
                let name = req.param_or("name", &id);
                let parent = req.param_or("parent", ROOT_BRANCH);
                let color = req.param_or("color", "#bb9af7");
                let cutoff = self.fork_branch(&id, &name, &parent, &color)?;
                Ok(json!({"forked_at_seq": cutoff, "branch_id": id}))
            }
            "set_spillover" => {
                let spec: SpilloverSpec = serde_json::from_value(
                    req.payload.get("spillover").cloned().unwrap_or(Value::Null))?;
                let out = self.store.append_event(
                    &req.branch, "spillover.revised", serde_json::to_value(&spec)?,
                    Some("RULE_UPSTREAM_STALE_NO_OVERWRITE"), req.base_seq)?;
                Ok(json!({"seq": out.seq, "event_id": out.event_id}))
            }
            "set_singular_spillover" => {
                let spec: SpilloverSpec =
                    serde_json::from_value(fixedcase::singular_spillover())?;
                let out = self.store.append_event(
                    &req.branch, "spillover.revised", serde_json::to_value(&spec)?,
                    Some("RULE_COMP_SINGULAR_NO_INVERSE"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "set_near_singular_spillover" => {
                let spec: SpilloverSpec =
                    serde_json::from_value(fixedcase::near_singular_spillover())?;
                let out = self.store.append_event(
                    &req.branch, "spillover.revised", serde_json::to_value(&spec)?,
                    Some("RULE_COMP_NEAR_SINGULAR_MULTI_CANDIDATE"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "comp_ruling" => {
                let candidate = req.param("candidate_id")?;
                let action = req.param_or("action", "accept");
                let note = req.param_or("note", "");
                let out = self.store.append_event(
                    &req.branch, "comp.ruling",
                    json!({"candidate_id": candidate, "action": action, "note": note}),
                    Some("RULE_NEAR_SINGULAR_REQUIRES_HUMAN_RULING"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "gate_rename" => {
                let gate_id = req.param("gate_id")?;
                let name = req.param("name")?;
                let out = self.store.append_event(
                    &req.branch, "gate.renamed",
                    json!({"gate_id": gate_id, "name": name}),
                    Some("RULE_GATE_IDENTITY_STABLE_ON_RENAME"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "gate_geometry" => {
                let gate: GateDef =
                    serde_json::from_value(req.payload.get("gate").cloned().unwrap_or(Value::Null))?;
                let out = self.store.append_event(
                    &req.branch, "gate.geometry_revised",
                    json!({"gate": gate}),
                    Some("RULE_UPSTREAM_STALE_NO_OVERWRITE"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "boundary_ruling" => {
                let gate_id = req.param("gate_id")?;
                let convention = req.param_or("convention", "edge-in");
                let action = req.param_or("action", "accept");
                let out = self.store.append_event(
                    &req.branch, "gate.boundary_ruling",
                    json!({"gate_id": gate_id, "convention": convention, "action": action}),
                    Some("RULE_GATE_BOUNDARY_AMBIGUITY_KEEP_CANDIDATES"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "revise_source" => {
                let event_id = req.param("event_id")?;
                let channel = req.param("channel")?;
                let raw = req.param("raw")?;
                let out = self.store.append_event(
                    &req.branch, "source.revised",
                    json!({"event_id": event_id, "channel": channel, "raw": raw}),
                    Some("RULE_SOURCE_LOCALIZED_INVALIDATION"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "revise_transform" => {
                let t: TransformSpec =
                    serde_json::from_value(req.payload.get("transform").cloned().unwrap_or(Value::Null))?;
                let out = self.store.append_event(
                    &req.branch, "transform.revised", serde_json::to_value(&t)?,
                    Some("RULE_TRANSFORM_VERSIONED"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "recompute" => {
                let out = self.store.append_event(
                    &req.branch, "review.recomputed", json!({}),
                    Some("RULE_RECOMPUTE_MARKS_NEW_EVENT"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "conflict_decision" => {
                // 互相冲突的人工决定：在当前分支记录一次显式裁定冲突事件（被采纳/未采纳并存）。
                let winner = req.param_or("winner", "A");
                let out = self.store.append_event(
                    &req.branch, "ruling.conflict",
                    json!({
                        "branch_id": req.branch,
                        "winner": winner,
                        "decision_a": req.payload.get("decision_a").cloned().unwrap_or(Value::Null),
                        "decision_b": req.payload.get("decision_b").cloned().unwrap_or(Value::Null),
                        "resolution": format!("采纳 {winner}，另一决定保留为未采纳候选")
                    }),
                    Some("RULE_KEEP_ACCEPTED_AND_REJECTED_RESULTS"), req.base_seq)?;
                Ok(json!({"seq": out.seq}))
            }
            "rollback" => {
                let to: i64 = req.param("to_seq")?.parse()?;
                let s = self.store.rollback_head(&req.branch, to)?;
                Ok(json!({"head_seq": s}))
            }
            "reset" => {
                self.store.reset()?;
                Ok(json!({"reset": true}))
            }
            other => Err(anyhow!("未知动作: {other}")),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct ActionRequest {
    #[serde(default = "default_branch")]
    pub branch: String,
    pub action: String,
    #[serde(default)]
    pub base_seq: Option<i64>,
    #[serde(default)]
    pub payload: Value,
}

fn default_branch() -> String {
    ROOT_BRANCH.into()
}

impl ActionRequest {
    fn param(&self, key: &str) -> Result<String> {
        self.payload
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("缺少参数 {key}"))
    }
    fn param_or(&self, key: &str, default: &str) -> String {
        self.payload.get(key).and_then(|v| v.as_str()).unwrap_or(default).to_string()
    }
}
