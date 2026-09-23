//! 事件账本回放引擎：把 (材料 + 事件) 投影为某分支某时刻的谱系视图。
//!
//! 规则：
//! - POLICY-COMP-001：近奇异补偿矩阵不能被默默求逆（多候选 + 显式裁定）。
//! - POLICY-GATE-DENOM：门的百分比分母必须钉住父群结果版本。
//! - POLICY-STALE-001：修订上游门后下游结果标记过期(stale)，不覆盖旧结论；
//!   新计算以 proposed 呈现，需显式 accept_refresh 才成为 current。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::gating::{gate_events, pinned_fraction, PinnedFraction};
use crate::linalg;
use crate::model::*;
use crate::Service;

pub const RULE_COMP: &str = "POLICY-COMP-001";
pub const RULE_DENOM: &str = "POLICY-GATE-DENOM";
pub const RULE_STALE: &str = "POLICY-STALE-001";

// ---------- 事件类型 ----------

pub mod ev {
    pub const MATERIAL_IMPORT: &str = "material_imported";
    pub const INTERPRETATION: &str = "interpretation_added";
    pub const COMP_ADJUDICATED: &str = "compensation_adjudicated";
    pub const ADJUDICATION_REVOKED: &str = "adjudication_revoked";
    pub const GATE_REVISED: &str = "gate_revised";
    pub const PARAM_REVISED: &str = "parameter_revised";
    pub const REFRESH_ACCEPTED: &str = "refresh_accepted";
    pub const FORK: &str = "branch_forked";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterpretationRef {
    pub interp_id: String,
    pub material_id: String,
    pub seq: i64,
    pub event_id: String,
    pub label: String,
    pub payload: InterpPayload,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationCandidate {
    pub candidate_id: String,
    pub method: String,
    pub matrix: CompensationMatrix,
    pub finding: serde_json::Value,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompensationNode {
    pub source_material_id: String,
    pub interpretation_id: String,
    pub submitted_matrix: CompensationMatrix,
    pub status: String, // ready | blocked_near_singular | blocked_singular | accepted | rejected
    pub finding: serde_json::Value,
    pub rule: String,
    pub candidates: Vec<CompensationCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_candidate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_matrix: Option<CompensationMatrix>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjudication_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopResult {
    pub result_id: String,
    pub node_id: String,
    pub node_label: String,
    pub parent_node_id: Option<String>,
    pub gate_id: Option<String>,
    pub result_version: i64,
    pub status: String, // current | stale | proposed | blocked
    pub count: i64,
    pub fraction: Option<PinnedFraction>,
    pub boundary_event_count: i64,
    pub unplottable_event_count: i64,
    pub event_ids: Vec<usize>,
    pub based_on: ResultBasis,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_superseded_by_event: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResultBasis {
    pub gate_version: String,
    pub transform_version: String,
    pub compensation_status: String,
    pub source_materials: Vec<String>,
    pub computed_at_event: String,
    pub parent_result_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleMark {
    pub result_id: String,
    pub node_id: String,
    pub reason: String,
    pub rule: String,
    pub caused_by_event: String,
    pub affected_subtree: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RevisionInfo {
    pub event_id: String,
    pub kind: String, // gate | transform | compensation
    pub target: String,
    pub invalidation_scope: Vec<String>,
    pub rule: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchInfo {
    pub branch_id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub fork_seq: Option<i64>,
    pub head_seq: i64,
}
