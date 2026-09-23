//! 领域模型：原始材料、变换、门、谱系，以及事件日志中每个事件的载荷。
//!
//! 设计原则：
//! - 来源（原始字节/原始表示）与解释（派生参数、裁定）分成不同事件分别版本化。
//! - 所有领域量携带单位或坐标基准。
//! - 无法转换的值保留在原位置并附诊断，绝不为画图而丢弃。

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// 一次原始通道测量。`raw_token` 是 FCS 文件里的原始表示，永不丢弃。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measurement {
    /// 可解析的浮点值（包含 Inf/饱和）。
    Value {
        value: f64,
        raw_token: String,
    },
    /// 无法转换的值：留在原位置，附诊断。
    Unconvertible {
        raw_token: String,
        diagnostic: String,
    },
}

impl Measurement {
    pub fn value(&self) -> Option<f64> {
        match self {
            Measurement::Value { value, .. } => Some(*value),
            Measurement::Unconvertible { .. } => None,
        }
    }
    pub fn raw_token(&self) -> &str {
        match self {
            Measurement::Value { raw_token, .. } | Measurement::Unconvertible { raw_token, .. } => raw_token,
        }
    }
    pub fn is_unconvertible(&self) -> bool {
        matches!(self, Measurement::Unconvertible { .. })
    }
}

/// 通道定义。所有坐标量带单位（unit）与坐标基准（basis）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    /// 通道短名，如 FSC-A
    pub short_name: String,
    /// $PnN（光学名/参数名）
    pub optical_name: String,
    /// $PnS（荧光试剂名，可空）
    pub stain: Option<String>,
    pub detector: String,
    /// 原始数据单位（ADC counts / linear scale / ...）
    pub unit: String,
    /// 坐标基准：raw / compensated / transformed
    pub basis: String,
    pub gain: f64,
    pub amplifier: String,
    /// 该通道是否参与荧光补偿（散射通道为 false）
    pub fluorescent: bool,
}

/// 一个细胞事件：每个通道一份原始测量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEvent {
    /// 稳定事件身份（导入时由 FCS 字节与序号决定）
    pub id: String,
    pub index: usize,
    /// channel short_name -> 原始测量
    pub channels: BTreeMap<String, Measurement>,
    /// 导入期诊断（按通道）
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub channel: String,
    pub code: String,
    pub message: String,
    pub raw_token: String,
}

/// spillover 元数据（来源层，与“采纳的补偿矩阵”分开版本化）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub version_id: String,
    pub label: String,
    pub source: String,
    /// 单染对照名 -> 通道
    pub single_stains: BTreeMap<String, String>,
    pub channels: Vec<String>,
    /// 行=探测器通道，列=荧光染料；N_i = S x_true
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
    /// 从哪份原始 FCS 字节解释而来
    pub source_fcs: String,
    pub notes: Option<String>,
}

/// 补偿候选（矩阵奇异时可能有多个合法候选，绝不只默默保留一个逆）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationCandidate {
    pub id: String,
    /// unique / least_squares / nullspace_family
    pub family: String,
    /// 该候选给出的补偿矩阵 M^{-1}
    pub matrix: Vec<Vec<f64>>,
    /// nullspace 族的自由参数（若适用）
    pub free_parameter: Option<f64>,
    pub note: String,
}

/// 补偿评估：规则引擎对某版 spillover 矩阵给出的判定与全部合法候选。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationEvaluation {
    pub spillover_version: String,
    /// ok / near_singular / singular
    pub status: String,
    pub rank: usize,
    pub n: usize,
    /// 最小奇异值与倒条件数
    pub sigma_min: f64,
    pub rcond: f64,
    pub near_singular_threshold: f64,
    /// 判定依据（机器可读规则）
    pub rule: RuleVerdict,
    /// 全部合法候选（ok 时恰好一个；singular 时一个族 + 采样候选）
    pub candidates: Vec<CompensationCandidate>,
    /// 逐事件的极端信号/诊断汇总（不丢点）
    pub signal_flags: Vec<SignalFlag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SignalFlag {
    pub event_id: String,
    pub channel: String,
    /// saturated / negative_after_comp / overflow / unconvertible
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleVerdict {
    /// 稳定规则身份，如 rule.near_singular_no_silent_inverse
    pub rule_id: String,
    pub severity: String,
    pub rationale: String,
    /// 判定所引用的数值阈值等
    pub parameters: BTreeMap<String, f64>,
}

/// 一版被采纳/声明的补偿矩阵（解释层，单独版本化）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationDecision {
    pub version_id: String,
    pub spillover_version: String,
    /// unique（自动）或选定候选 id（人工）
    pub candidate_id: String,
    pub decided_by: String,
    /// accept / reject / adjust
    pub decision: String,
    pub reason: String,
}

/// 变换参数（按通道）。输出坐标基准：transformed（asinh 双指数式“bi-exponential-like”）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub version_id: String,
    pub label: String,
    /// channel -> (cofactor, 输入单位, 输出基准)
    pub channels: BTreeMap<String, TransformChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformChannel {
    pub cofactor: f64,
    pub input_unit: String,
    pub output_basis: String,
}

/// 门多边形。坐标钉住某个坐标基准与通道版本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDef {
    pub gate_id: String,
    /// 门版本 id（改名也产生新版本，gate_id 不变）
    pub version_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub x_basis: String,
    pub y_basis: String,
    pub polygon: Vec<[f64; 2]>,
    /// 父门 gate_id；根门为 None
    pub parent_gate: Option<String>,
    /// 父门版本 id：百分比分母钉住的版本
    pub parent_version: Option<String>,
    /// 补偿/变换版本（谱系依赖）
    pub compensation_version: Option<String>,
    pub transform_version: String,
    pub dataset_version: String,
    pub geometry_rule: String,
}

/// 门评估结果（不可变：每次门版本/依赖变化追加，不覆盖旧结论）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateEvaluation {
    pub eval_id: String,
    pub gate_id: String,
    pub gate_version: String,
    pub status: String,
    /// 落在多边形内（含边界，除非 boundary_disputed）
    pub included_event_ids: Vec<String>,
    /// 恰好在边界上的点
    pub boundary_event_ids: Vec<String>,
    pub excluded_event_ids: Vec<String>,
    pub unconvertible_event_ids: Vec<String>,
    /// 分母（钉住父群版本）
    pub denominator: Denominator,
    pub numerator_count: usize,
    pub denominator_count: usize,
    /// 0..=1
    pub percent: Option<f64>,
    /// 边界点是否产生无法唯一决定的情况
    pub boundary_disputed: bool,
    /// 两个合法候选：边界点 exclude / include
    pub candidates: Vec<GateResultCandidate>,
    pub rule: RuleVerdict,
    /// 该评估依赖的所有版本（失效传播图）
    pub dependencies: GateDependencies,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Denominator {
    /// parent_population / all_events
    pub kind: String,
    pub parent_gate_id: Option<String>,
    pub parent_gate_version: Option<String>,
    pub parent_eval_id: Option<String>,
    pub count: usize,
    pub dataset_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateResultCandidate {
    pub id: String,
    /// exclude_boundary / include_boundary
    pub treatment: String,
    pub included_event_ids: Vec<String>,
    pub numerator_count: usize,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDependencies {
    pub dataset_version: String,
    pub compensation_version: Option<String>,
    pub transform_version: String,
    pub parent_gate_version: Option<String>,
}

/// 门结果的人工裁定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDecision {
    pub gate_id: String,
    pub gate_version: String,
    pub eval_id: String,
    /// 选定候选 id（边界处理）
    pub candidate_id: String,
    pub decided_by: String,
    /// accept / reject / adjust
    pub decision: String,
    pub reason: String,
}

/// 失效传播事件内容：修订上游后，只标记列出的下游为过期，不覆盖其结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleItem {
    pub gate_id: String,
    pub gate_version: String,
    pub changed_dependency: String,
    pub reason: String,
}

/// 并发冲突记录（后提交者所见）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub conflict_id: String,
    pub branch: String,
    pub expected_head: i64,
    pub actual_head: i64,
    pub attempted_event_type: String,
    pub attempted_by: String,
    /// 双方差异
    pub diff: ConflictDiff,
    pub at_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConflictDiff {
    pub local_event_type: String,
    pub remote_event_type: String,
    pub local_summary: String,
    pub remote_summary: String,
}

// ---------------- 事件日志 ----------------

/// 事件类型标签（与载荷分开存储，便于迁移与审阅）。
pub const ET_RESET: &str = "reset";
pub const ET_FCS_IMPORTED: &str = "fcs.imported";
pub const ET_SPILLOVER_IMPORTED: &str = "spillover.imported";
pub const ET_COMPENSATION_EVALUATED: &str = "compensation.evaluated";
pub const ET_COMPENSATION_DECIDED: &str = "compensation.decided";
pub const ET_TRANSFORM_PUBLISHED: &str = "transform.published";
pub const ET_GATE_PUBLISHED: &str = "gate.published";
pub const ET_GATE_EVALUATED: &str = "gate.evaluated";
pub const ET_GATE_DECIDED: &str = "gate.decided";
pub const ET_STALENESS_PROPAGATED: &str = "staleness.propagated";
pub const ET_HEAD_MOVED: &str = "head.moved";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsImportedPayload {
    pub material_id: String,
    pub version_id: String,
    pub filename: String,
    pub bytes_hash: String,
    pub byte_len: usize,
    pub text_segment: String,
    pub channels: Vec<ChannelDef>,
    pub events: Vec<FlowEvent>,
    pub reimport_of: Option<String>,
    pub changed_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePublishedPayload {
    pub gate: GateDef,
    /// 若是修订（含改名），指向旧版本
    pub revises_version: Option<String>,
    pub change_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StalenessPayload {
    pub trigger_event_type: String,
    pub trigger_version_id: String,
    pub items: Vec<StaleItem>,
    /// 显式记录未受影响的门，证明失效范围被限制
    pub unaffected_gate_ids: Vec<String>,
    pub rule: RuleVerdict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadMovedPayload {
    /// undo / redo
    pub movement: String,
    pub new_head_seq: i64,
    pub target_event_id: Option<String>,
    pub undone_event_id: Option<String>,
    pub decided_by: String,
    pub reason: String,
}

/// 不可变事件记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub seq: i64,
    pub event_id: String,
    pub event_type: String,
    pub branch: String,
    /// 分支内前一事件（分支起点为 None）
    pub parent_event_id: Option<String>,
    /// 分支从哪个事件分出（fork 点）
    pub from_event_id: Option<String>,
    pub actor: String,
    pub created_at: String,
    pub command_id: String,
    pub payload: serde_json::Value,
}
