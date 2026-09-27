//! 领域模型：流式细胞术补偿/门控谱系。
//! 所有版本化材料（原始通道值、spillover 元数据、补偿矩阵、变换参数、
//! 门多边形、父子关系）分别版本化存储；派生结果（细胞群）记录其依赖版本。

use serde::{Deserialize, Serialize};

/// 版本化材料的种类。
pub const KIND_RAW_EVENTS: &str = "raw_events";
pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_COMP_MATRIX: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_GATE_EDGE: &str = "gate_edge";

/// 机器可读取判定规则 ID（写入事件，供复核与回放断言）。
pub mod rule {
    /// 近奇异补偿矩阵不能被默默求逆。
    pub const NEAR_SINGULAR_NO_INVERSE: &str = "near_singular_no_inverse";
    /// 门的百分比分母必须钉住父群版本。
    pub const DENOMINATOR_PINNED_TO_PARENT: &str = "denominator_pinned_to_parent";
    /// 修订上游门后下游结果标记过期而不覆盖。
    pub const UPSTREAM_STALE_NOT_OVERWRITE: &str = "upstream_stale_not_overwrite";
    /// 极端信号保留原值并附诊断。
    pub const EXTREME_SIGNAL_KEPT: &str = "extreme_signal_kept";
    /// 无法转换的值留在原位置并附诊断。
    pub const UNCONVERTIBLE_KEPT_IN_PLACE: &str = "unconvertible_kept_in_place";
    /// 边界点产生多候选。
    pub const BOUNDARY_POINT_CANDIDATES: &str = "boundary_point_candidates";
    /// 同版本并发修改冲突。
    pub const SAME_VERSION_CONFLICT: &str = "same_version_conflict";
    /// 门嵌套改名保持身份。
    pub const NESTED_RENAME_KEEPS_IDENTITY: &str = "nested_rename_keeps_identity";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub unit: String,
    pub range: [f64; 2],
    /// 坐标基准：linear / log / asinh
    pub basis: String,
}

/// 原始事件：导入时保留原始表示（字符串 token），派生层才做规范化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvents {
    pub name: String,
    pub channels: Vec<Channel>,
    /// 每个事件是每个通道的原始 token（可能是 "NaN"、""、超量程值）。
    pub events: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spillover {
    pub fluorochromes: Vec<String>,
    pub detectors: Vec<String>,
    pub values: Vec<Vec<f64>>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompMatrix {
    pub labels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transform {
    pub kind: String, // "asinh" | "linear"
    pub cofactor: f64,
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gate {
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub x_unit: String,
    pub y_unit: String,
    pub polygon: Vec<[f64; 2]>,
}

/// 父子关系独立版本化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateEdge {
    pub parent: String, // gate id
    pub child: String,  // gate id
}

/// 草案：钉住它使用的材料版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftPayload {
    pub raw_events: (String, i64),
    pub comp_matrix: (String, i64),
    pub transform: (String, i64),
}

/// 细胞群候选（无法唯一决定时全部保留）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub label: String,
    pub count: i64,
}

/// 数据库行：版本化对象。
#[derive(Debug, Clone)]
pub struct ObjectRow {
    pub id: String,
    pub version: i64,
    pub kind: String,
    pub name: String,
    pub payload: serde_json::Value,
    pub created_seq: i64,
}

/// 数据库行：事件（机器可读取的判定依据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub rule: Option<String>,
    pub status: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

/// 数据库行：细胞群结果（带有效区间，支持按事件序号回看）。
#[derive(Debug, Clone)]
pub struct PopulationRow {
    pub id: String,
    pub draft_id: String,
    pub gate_id: String,
    pub gate_version: i64,
    pub parent_pop_id: Option<String>,
    /// 百分比分母钉住的父群行版本。
    pub parent_pop_version: Option<i64>,
    pub version: i64,
    pub count: Option<i64>,
    pub percent: Option<f64>,
    pub denominator_count: Option<i64>,
    pub status: String, // current | stale | blocked
    pub candidates: Vec<Candidate>,
    pub chosen: Option<String>,
    pub diag: serde_json::Value,
    pub created_seq: i64,
    pub retired_seq: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct AdjudicationRow {
    pub id: String,
    pub draft_id: String,
    pub target_id: String,
    pub decision: String, // accept | reject | adjust
    pub payload: serde_json::Value,
    pub base_seq: i64,
    pub created_seq: i64,
    pub retired_seq: Option<i64>,
}

pub fn new_id(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4().simple())
}
