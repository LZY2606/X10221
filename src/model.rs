//! 领域模型：来源与解释拆开保存的版本化对象、派生群体、候选与分支快照。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 领域量：保留原始表示 raw；value 为 None 表示无法转换，留在原位置并附诊断。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub raw: String,
    pub value: Option<f64>,
    pub unit: String,
    pub basis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawEventRow {
    pub id: String,
    pub channels: BTreeMap<String, Quantity>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawEvents {
    pub channel_order: Vec<String>,
    pub events: Vec<RawEventRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpillEntry {
    pub detector: String,
    pub fluorochrome: String,
    pub coefficient: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spillover {
    pub source: String,
    pub entries: Vec<SpillEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    pub values: Vec<Vec<f64>>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transform {
    pub kind: String,
    pub cofactor: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformParams {
    pub per_channel: BTreeMap<String, Transform>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gate {
    pub gate_id: String,
    pub name: String,
    pub channels: [String; 2],
    pub basis: String,
    pub polygon: Vec<(f64, f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeEdge {
    pub child: String,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateTree {
    pub edges: Vec<TreeEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub kind: String,
    pub subject: String,
    pub message: String,
}

/// 百分比分母钉住的父群版本引用。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DenominatorRef {
    pub gate_id: String,
    pub gate_version: u64,
    pub count: usize,
    pub event_seq: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PopulationRow {
    pub gate_id: String,
    pub gate_name: String,
    pub gate_version: u64,
    pub parent_gate: Option<String>,
    pub denominator: Option<DenominatorRef>,
    pub count: usize,
    pub percent: Option<f64>,
    /// ok | stale | undecided
    pub status: String,
    pub members: Vec<String>,
    #[serde(default)]
    pub boundary_members: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CandidateOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    #[serde(default)]
    pub adopted: bool,
    #[serde(default)]
    pub rejected: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CandidateSet {
    pub id: String,
    pub subject: String,
    pub rule: String,
    pub options: Vec<CandidateOption>,
    /// pending | decided
    pub status: String,
    pub decided_event: Option<i64>,
}

/// 某个分支在某一事件之后的完整可见状态；历史回放心按事件快照读取。
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct BranchState {
    /// 对象稳定 id -> 被钉住的版本（raw_events / comp_matrix / transform_params / gate:* / gate_tree）。
    #[serde(default)]
    pub pins: BTreeMap<String, u64>,
    #[serde(default)]
    pub populations: Vec<PopulationRow>,
    #[serde(default)]
    pub candidates: Vec<CandidateSet>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default)]
    pub scenario_cursor: usize,
    #[serde(default)]
    pub log: Vec<String>,
}
