//! 领域类型：流式事件、spillover、补偿矩阵、变换、门多边形、父子关系。
//!
//! 设计约束（见 cases 与测试中的判断依据）：
//! * 来源（原始字节/参数/多边形/父子关系）与解释（派生的补偿、群体结果）分开保存；
//! * 每一类来源独立版本化，所有版本只追加、不改写；
//! * 每个领域量携带单位或坐标基准；
//! * 无法转换的值（仪器溢出/负值）停留在原始层并附诊断，不做不可逆清洗。

use serde::{Deserialize, Serialize};

/// 内容对象类别——每一类都是独立版本化的“直接来源”。
pub mod kind {
    pub const RAW_DATA: &str = "raw_data";
    pub const SPILLOVER: &str = "spillover";
    pub const TRANSFORM: &str = "transform_params";
    pub const GATE: &str = "gate_polygon";
    pub const LINEAGE: &str = "gate_lineage";
}

/// 机器可读的判断依据规则身份。规则修订时升版本号，历史事件保留旧版本引用。
pub mod rule {
    pub const NEAR_SINGULAR_GUARD: &str = "comp.near_singular_no_silent_inverse/v1";
    pub const PERCENT_PINS_PARENT: &str = "gate.percent_denominator_pins_parent_version/v1";
    pub const STALE_NO_OVERWRITE: &str = "lineage.upstream_revision_marks_stale_no_overwrite/v1";
    pub const OPTIMISTIC_LOCK: &str = "branch.same_version_concurrent_modification/v1";
    pub const ATOMIC_IMPORT: &str = "import.all_or_nothing_rollback/v1";
    pub const BRANCH_KEEP_BOTH: &str = "decision.reinterpretation_forks_keeps_both/v1";
    pub const RAW_VALUE_PRESERVATION: &str = "raw.unconvertible_value_stays_in_place/v1";
    pub const BOUNDARY_MULTI_CANDIDATE: &str = "gate.boundary_points_keep_all_legal_candidates/v1";
    pub const LOCALIZED_INVALIDATION: &str = "depgraph.invalidation_scoped_to_dependents/v1";

    /// 近奇异判定阈值（1-范数倒数条件数估计 rcond 低于该值则拒绝默默求逆）。
    pub const RCOND_BLOCK_THRESHOLD: f64 = 1e-4;
}

/// 事件类型常量。
pub mod etype {
    pub const CASE_IMPORTED: &str = "case_imported";
    pub const IMPORT_ROLLED_BACK: &str = "import_rolled_back";
    pub const CONTENT_COMMITTED: &str = "content_committed";
    pub const COMPENSATION_EVALUATED: &str = "compensation_evaluated";
    pub const GATE_RESULT_COMPUTED: &str = "gate_result_computed";
    pub const RESULTS_STALE: &str = "results_marked_stale";
    pub const DECISION_MADE: &str = "decision_made";
    pub const DECISION_UNDONE: &str = "decision_undone";
    pub const CONFLICT_REJECTED: &str = "concurrent_modification_rejected";
    pub const BRANCH_CREATED: &str = "branch_created";
    pub const BOUNDARY_AMBIGUITY: &str = "boundary_ambiguity";
    pub const EXTREME_SIGNAL_DIAGNOSTIC: &str = "extreme_signal_diagnostic";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    /// FCS `$PnN` 短名（稳定身份，导入时原样保留）。
    pub name: String,
    /// 平台长名/荧光素描述。
    pub description: String,
    /// 原始量单位，如 "a.u. (linearized detector area)"。
    pub unit: String,
    /// 坐标基准：该通道值取自哪一份原始表示的哪一列。
    pub coordinate_basis: String,
    /// 仪器溢出值（饱和哨兵），来自 FCS `$PnR`，不参与清洗。
    pub range_ceiling: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    /// 原始数值表示（导入时的字节解析值，不做任何规范化）。
    pub value: f64,
    /// 平台报告标记，如 "OVF"(溢出)、"NEG"(负信号)；正常为 null。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// 无法进入派生层时的诊断（保留在原位置，绝不丢弃该事件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    /// 事件在 FCS DATA 段中的 0-based 序号（稳定身份）。
    pub index: u32,
    /// 与 ChannelDef.name 对齐的原始通道值。
    pub channels: Vec<RawValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawDataPayload {
    pub channels: Vec<ChannelDef>,
    pub events: Vec<RawEvent>,
    /// 原始字节指纹（sha256），用于来源链反查。
    pub source_sha256: String,
    pub source_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverPayload {
    /// 行列标签：荧光素/检测通道短名（方阵，行=检测通道，列=荧光素）。
    pub labels: Vec<String>,
    /// 行优先的无量纲溢漏分数（光谱串色比例）。
    pub matrix: Vec<Vec<f64>>,
    /// 单位说明。
    pub unit: String,
    /// 来源元数据原文（FCS `$SPILLOVER` / 平台单染对照导出）。
    pub source_meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformChannel {
    pub channel: String,
    pub kind: String, // "linear" | "log10_offset"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    pub input_unit: String,
    /// 输出坐标基准说明（如 "decades: log10(x + offset)"）。
    pub output_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformPayload {
    pub channels: Vec<TransformChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateSpace {
    pub x_channel: String,
    /// 采用的变换参数对象引用（None = 原始线性坐标）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_transform: Option<String>,
    pub y_channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_transform: Option<String>,
    /// 坐标基准（"instrument.raw" 或 "compensated" 等）。
    pub coordinate_basis: String,
    /// 顶点坐标单位（如 "decades"、"a.u."）。
    pub vertex_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePayload {
    /// 跨改名稳定的门身份。
    pub gate_id: String,
    /// 显示名（可改，改名不改身份）。
    pub name: String,
    pub space: GateSpace,
    /// 多边形顶点（闭合、简单多边形）。
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineagePayload {
    /// gate_id -> 父门 gate_id（根门映射到 null）。
    pub parent_of: Vec<(String, Option<String>)>,
}

/// 内容对象（一种直接来源的一个不可变版本）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContentObject {
    pub id: String,
    pub kind: String,
    pub version: u32,
    /// 同一 id 的上一版本对象引用；首版为 null。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub payload: serde_json::Value,
}

/// 机器可读的判断依据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Basis {
    pub rule: String,
    pub threshold: serde_json::Value,
    /// 参与判定的输入对象引用（来源链）。
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    /// 全局稳定身份：<case_id>#<seq 零填充>。
    pub id: String,
    /// 全局追加序号（导出/删除/导入后保持相同身份与顺序）。
    pub seq: u64,
    /// 分支内序号；冲突拒绝事件不进入任何分支，为 null。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_seq: Option<u64>,
    pub case_id: String,
    pub branch: String,
    pub event_type: String,
    pub summary: String,
    pub payload: serde_json::Value,
    /// 该事件直接依赖的对象/产物引用。
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// 机器可读判断依据；纯叙述事件可为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<Basis>,
    /// 操作来源：reviewer 窗口标识，用于并发冲突双方差异。
    pub actor: String,
    pub created_at: String,
}
