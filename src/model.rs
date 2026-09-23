//! 领域模型：来源材料（分开版本化）与派生结论共用的类型。

use serde::{Deserialize, Serialize};

/// 推导规则集版本：参数或参考版本变化后按依赖关系计算失效范围。
pub const RULE_SET_VERSION: &str = "ruleset-1.0";
/// 规则：近奇异补偿矩阵不能被默默求逆。
pub const RULE_NEAR_SINGULAR: &str = "rule:near-singular-no-silent-inverse";
/// 规则：门的百分比分母必须钉住父群版本。
pub const RULE_DENOM_PINNED: &str = "rule:denominator-pinned-to-parent-version";
/// 规则：修订上游门后下游结果标记过期而不覆盖。
pub const RULE_STALE_NOT_OVERWRITE: &str = "rule:upstream-revision-marks-stale";

/// 材料类别：六类来源分开版本化。
pub mod kind {
    pub const RAW_CHANNELS: &str = "raw_channels";
    pub const SPILLOVER_META: &str = "spillover_meta";
    pub const COMP_MATRIX: &str = "comp_matrix";
    pub const TRANSFORM_PARAMS: &str = "transform_params";
    pub const GATE_POLYGON: &str = "gate_polygon";
    pub const GATE_HIERARCHY: &str = "gate_hierarchy";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub name: String,
    pub unit: String,
    /// 坐标基准，如 "linear:0..262143" 或 "asinh:cofactor=150"。
    pub basis: String,
    pub range: [f64; 2],
}

/// 原始通道值：导入时保留原始表示（字符串），派生层才解析规范化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawChannels {
    pub sample: String,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    /// 与 channels 对齐的原始字符串表示；无法转换的值原样保留。
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub fluor_channels: Vec<String>,
    pub controls: Vec<Control>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub fluor: String,
    /// 探测器 -> 观测响应系数（单染对照测量值）。
    pub detector_response: std::collections::BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    /// 行 = 探测器，列 = 荧光素，与 channels 顺序对齐。
    pub rows: Vec<Vec<f64>>,
    pub derived_from: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParams {
    pub transforms: Vec<TransformSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformSpec {
    pub channel: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub cofactor: Option<f64>,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePolygon {
    pub gate_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub x_basis: String,
    pub y_basis: String,
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateHierarchy {
    pub nodes: Vec<HierarchyNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyNode {
    pub gate_id: String,
    pub parent: Option<String>,
    pub name: String,
}

/// 群体结论的稳定身份与来源链（pins）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopPins {
    pub raw: String,
    pub spillover: String,
    pub comp: String,
    pub transform: String,
    pub gates: std::collections::BTreeMap<String, i64>,
    pub hierarchy: String,
    pub rule_set: String,
}

/// 候选：无法唯一决定或人工冲突时保留的全部合法选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub kind: String,
    pub subject: String,
    pub options: Vec<String>,
    /// pending | adopted | rejected
    pub status: String,
    #[serde(default)]
    pub chosen: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}
