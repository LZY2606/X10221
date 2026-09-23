//! 领域模型：所有进入系统的材料都“来源与解释分离”，六类来源分别版本化。
//!
//! 六类来源（raw_events / spillover / matrix / transform / gate / tree）都
//! 不可原地修改：重新解释时创建新版本（新 id），旧版本与引用它的派生结果继续可读。
//!
//! 所有领域量都携带单位或坐标基准；派生层才做规范化，无法转换的值留在原位并附诊断。

use serde::{Deserialize, Serialize};

/// 稳定来源种类。
pub const KINDS: [&str; 6] = [
    "raw_events",
    "spillover",
    "matrix",
    "transform",
    "gate",
    "tree",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    RawEvents,
    Spillover,
    Matrix,
    Transform,
    Gate,
    Tree,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::RawEvents => "raw_events",
            SourceKind::Spillover => "spillover",
            SourceKind::Matrix => "matrix",
            SourceKind::Transform => "transform",
            SourceKind::Gate => "gate",
            SourceKind::Tree => "tree",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_events" => SourceKind::RawEvents,
            "spillover" => SourceKind::Spillover,
            "matrix" => SourceKind::Matrix,
            "transform" => SourceKind::Transform,
            "gate" => SourceKind::Gate,
            "tree" => SourceKind::Tree,
            _ => return None,
        })
    }
}

/// 带单位/坐标基准的原始通道值。value 可能为数字，也可能保留原始的不可转换表示（如 "OVF"、null）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    /// 原始表示，永不丢失（数字、"OVF"、null 等）。
    pub raw: serde_json::Value,
    /// 物理量单位 / 坐标基准，例如 "adc_counts"。
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    /// 事件在采集管内的稳定序号（从 0 起）。
    pub index: i64,
    /// 通道名 -> 带单位原始值。
    pub channels: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub short: String,
    pub long: String,
    /// 坐标基准：线性 ADC，增益，单位计数。
    pub unit: String,
    pub gain: f64,
}

/// raw_events 来源载荷。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEventsDoc {
    pub detector: String,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
    /// 若来自 FCS，保留原始字节指纹；否则为合成案例。
    pub origin: OriginRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OriginRef {
    pub transport: String, // "synthetic" | "fcs"
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub byte_length: Option<i64>,
}

/// 单染对照与溢出（spillover）元数据：解释来源，不等于补偿矩阵本身。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub fluor: String,
    pub channel: String,
    /// 该单染在各荧光通道上的溢出系数（原始测量）。
    pub spill_into: serde_json::Map<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverDoc {
    pub channels: Vec<String>,
    pub controls: Vec<SingleStainControl>,
    pub note: String,
}

/// matrix 来源载荷：行=源荧光，列=探测器；detector 轴对角为 1。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixDoc {
    pub channels: Vec<String>,
    /// 行主序方阵。
    pub rows: Vec<Vec<f64>>,
    pub derived_from: Option<String>,
    pub note: String,
}

/// transform 来源载荷：asinh 双指数显示变换，携带坐标基准。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformDoc {
    pub channels: Vec<String>,
    pub kind: String, // "asinh"
    pub cofactor: f64,
    pub output_unit: String, // "asinh_cofactor_units"
    pub input_unit: String,  // "adc_counts_compensated"
}

/// gate 来源载荷：多边形门，坐标位于变换后的二维坐标基准。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Polygon {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDoc {
    pub gate_id: String,
    /// 展示名（可改名，不改变稳定身份）。
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: String, // "transformed_asinh"
    pub polygon: Polygon,
}

/// tree 来源载荷：门的父子关系单独版本化。parent 为根门时为 "ROOT"。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateNode {
    pub gate_id: String,
    pub parent: String,
    pub order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeDoc {
    pub root_label: String,
    pub nodes: Vec<GateNode>,
}

/// 任意来源的枚举载荷。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum SourcePayload {
    #[serde(rename = "raw_events")]
    RawEvents(RawEventsDoc),
    #[serde(rename = "spillover")]
    Spillover(SpilloverDoc),
    #[serde(rename = "matrix")]
    Matrix(MatrixDoc),
    #[serde(rename = "transform")]
    Transform(TransformDoc),
    #[serde(rename = "gate")]
    Gate(GateDoc),
    #[serde(rename = "tree")]
    Tree(TreeDoc),
}

impl SourcePayload {
    pub fn kind(&self) -> SourceKind {
        match self {
            SourcePayload::RawEvents(_) => SourceKind::RawEvents,
            SourcePayload::Spillover(_) => SourceKind::Spillover,
            SourcePayload::Matrix(_) => SourceKind::Matrix,
            SourcePayload::Transform(_) => SourceKind::Transform,
            SourcePayload::Gate(_) => SourceKind::Gate,
            SourcePayload::Tree(_) => SourceKind::Tree,
        }
    }
}
