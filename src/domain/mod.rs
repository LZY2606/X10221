//! 领域类型：原始通道值、spillover、补偿矩阵、变换、门、谱系与事件。
//!
//! 约定：
//! * 所有材料都带稳定身份 `(id, version)`。
//! * 领域量带单位 / 坐标基准；导入层保留原始字符串表示，无法转换的值原地保留并附诊断。
//! * 事件只追加（append-only）；分支是事件序列的复刻前缀。

use serde::{Deserialize, Serialize};

/// 单个通道读数：同时保留原始表示与（可能缺失的）数值解释。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelValue {
    /// 导入时的原始表示（字符串原样保留，不做不可逆清洗）。
    pub raw: String,
    /// 规范化后的数值；无法转换时为 null，诊断写入 `diagnostic`。
    #[serde(default)]
    pub value: Option<f64>,
    /// 例如 "adu"（线性化探测器数字单位）。
    pub unit: String,
    /// 坐标 / 计量基准标识，例如 "raw-detector-linear/v1"。
    pub basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub name: String,
    pub unit: String,
    pub basis: String,
    /// 荧光通道是否参与补偿（散射通道不参与）。
    pub compensable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsEvent {
    pub id: String,
    pub channels: std::collections::BTreeMap<String, ChannelValue>,
}

/// 原始 FCS 事件材料（版本化）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvents {
    pub raw_id: String,
    pub version: u32,
    pub origin: String,
    pub sha256: String,
    pub acquisition: Acquisition,
    pub channel_specs: Vec<ChannelSpec>,
    pub events: Vec<FcsEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Acquisition {
    pub instrument: String,
    pub date: String,
    pub total_events: usize,
}

/// Spillover（溢出）元数据：单染对照导出的比例矩阵 S，使 观测 = S·真实。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Spillover {
    pub spillover_id: String,
    pub version: u32,
    /// 通道顺序同时决定行列含义。
    pub channel_order: Vec<String>,
    /// 行优先，单位 fraction（无量纲比例）。
    pub matrix: Vec<Vec<f64>>,
    pub controls: Vec<SingleStainControl>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub channel: String,
    pub control_events_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvertibilityReport {
    pub invertible: bool,
    /// 最小主元相对比例（部分主元消去）。
    pub min_pivot_ratio: f64,
    /// 无穷范数条件数；奇异时为 null。
    #[serde(default)]
    pub condition_number: Option<f64>,
    pub pivot_threshold: f64,
    pub condition_threshold: f64,
    pub near_singular: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCandidate {
    pub candidate_id: String,
    /// "inverse" | "ridge"
    pub method: String,
    /// ridge 正则强度；inverse 为 0。
    #[serde(default)]
    pub lambda: f64,
    /// 候选补偿矩阵 M（行优先），近似或精确。
    pub matrix: Vec<Vec<f64>>,
    pub report: InvertibilityReport,
    /// ridge 候选的残差范数 ||S M - I||，供审阅者权衡。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual: Option<f64>,
}

/// 门多边形（坐标显式标注所在坐标系）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub gate_id: String,
    pub version: u32,
    pub name: String,
    pub frame: GateFrame,
    pub x_channel: String,
    pub y_channel: String,
    pub vertices: Vec<[f64; 2]>,
    /// 边界判定规则，例如 "edge-exclusive/v1"（落在边上的点判外）。
    pub boundary_rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<GateDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDecision {
    pub state: String, // accepted | rejected
    pub at_seq: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateFrame {
    /// "raw" | "transformed"
    pub kind: String,
    /// transformed 时钉住的变换版本。
    #[serde(default)]
    pub transform_ref: Option<Ref>,
    /// 补偿矩阵引用（被补偿通道坐标系钉住的补偿版本）。
    #[serde(default)]
    pub comp_ref: Option<Ref>,
}

/// 版本引用：材料 id + version。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ref {
    pub id: String,
    pub version: u32,
}

impl Ref {
    pub fn new(id: impl Into<String>, version: u32) -> Self {
        Ref { id: id.into(), version }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub transform_id: String,
    pub version: u32,
    pub family: String, // asinh
    /// x' = asinh((x - offset) / cofactor)
    pub cofactor: f64,
    pub offset: f64,
    pub input_unit: String,
    pub output_unit: String,
    pub basis: String,
}

/// 父子关系材料（与门本身分开版本化）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lineage {
    pub lineage_id: String,
    pub version: u32,
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineageEdge {
    pub child_gate: String,
    pub parent_gate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Denominator {
    /// "raw"（根群：全部有效导入事件）| "gate"
    pub kind: String,
    /// 钉住的全部上游版本，保证分母可回溯。
    pub pinned: PinnedRefs,
    pub gate_ref: Option<Ref>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PinnedRefs {
    pub raw: Ref,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spillover: Option<Ref>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comp: Option<Ref>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Ref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", default)]
    pub gates: Vec<Ref>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<Ref>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationSnapshot {
    pub population_id: String,
    pub gate_ref: Ref,
    pub numerator_count: usize,
    pub numerator_event_ids: Vec<String>,
    pub denominator: Denominator,
    /// 父群中的比例 numerator / denominator.count。
    pub fraction: f64,
    pub computed_at_seq: u64,
    /// 读取时依据当前分支末端判定的过期状态（不写回、不覆盖快照）。
    #[serde(default)]
    pub stale: Option<StaleInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaleInfo {
    pub is_stale: bool,
    /// 数值结论实际是否改变（仅引用版本漂移时可能为 false）。
    pub value_changed: bool,
    pub changed_refs: Vec<String>,
    pub reason: String,
}

/// 只追加的谱系事件。kind 与 payload 一一对应，payload 保留原始 JSON。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub kind: String,
    pub seq: u64,
    pub at: String,
    pub payload: serde_json::Value,
}
