//! 领域模型：案例包（离线 FCS 风格材料）与复核视图使用的类型。
//!
//! 关键约定：
//! - 原始表示（`FcsFile.text`）逐字节保留，解析结果属于独立版本化的解释层。
//! - 所有领域量携带单位 / 坐标基准。
//! - 无法转换的原始 token 保留原位（value=None）并附诊断，不丢弃。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const CASE_SCHEMA: &str = "flow-lineage-case/1";
pub const ARCHIVE_SCHEMA: &str = "flow-lineage-archive/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CasePackage {
    pub schema: String,
    pub case: CaseMeta,
    pub channels: Vec<ChannelSpec>,
    pub fcs: FcsFile,
    pub spillover: SpilloverDoc,
    pub compensation: CompMatrixDoc,
    pub transform: TransformDoc,
    pub gates: Vec<GateDoc>,
    pub hierarchy: HierarchyDoc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseMeta {
    pub code: String,
    pub title: String,
    pub narrative: String,
    pub coordinate_system: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    /// FCS $PnN 简称，例如 FITC-A
    pub short_name: String,
    /// FCS $PnS 长名（标志物）
    pub long_name: String,
    /// 采集单位
    pub unit: String,
    /// 探测器线性范围（原始通道值）
    pub detector_min: f64,
    pub detector_max: f64,
    /// $PnG 增益，仅元数据保留
    pub gain: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsFile {
    pub filename: String,
    pub media_type: String,
    /// 原始文本表示，导入时逐字节保留（sha256 钉住）。
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventTable {
    pub source_document: String,
    pub columns: Vec<String>,
    pub rows: Vec<RawRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawRow {
    /// 稳定事件身份（原始文件中的 EVENT_ID）
    pub event_id: String,
    pub label: String,
    /// 原始 token 字符串（未规范化的原始表示）
    pub tokens: BTreeMap<String, String>,
    /// 解析后的数值；无法转换时为 None，token 仍在 `tokens` 中
    pub values: BTreeMap<String, Option<f64>>,
    pub diagnostics: Vec<Diag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diag {
    pub code: String,
    pub severity: String,
    pub channel: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverDoc {
    pub id: String,
    pub method: String,
    pub controls: Vec<SingleStainControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub id: String,
    pub label: String,
    pub fluorochrome: String,
    pub channel: String,
    pub event_count: usize,
    pub basis: String,
    pub observed_spillover: Vec<SpillCoefficient>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpillCoefficient {
    pub channel: String,
    pub coefficient: f64,
    /// 无量纲：溢入通道信号 / 主通道信号
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompMatrixDoc {
    pub id: String,
    pub channel_order: Vec<String>,
    /// 方阵，按 channel_order 排列；单位为无量纲溢漏比例
    pub rows: Vec<Vec<f64>>,
    pub unit: String,
    pub derived_from_controls: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformDoc {
    pub id: String,
    pub specs: BTreeMap<String, TransformChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformChannel {
    pub kind: String, // "linear" | "log_offset"
    pub offset: f64,
    pub base: f64,
    pub input_unit: String,
    pub output_unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDoc {
    pub id: String,
    pub name: String,
    pub geometry: Geometry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Geometry {
    pub kind: String, // "rectangle" | "polygon"
    pub x_channel: String,
    pub y_channel: String,
    /// rectangle 的闭区间 [min, max]（边界点计入，closed-inclusive）
    pub x_min: Option<f64>,
    pub x_max: Option<f64>,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    pub polygon: Vec<[f64; 2]>,
    /// 该多边形使用的坐标基准，例如 "log_offset(base=10,offset=1)@transform:v1"
    pub coordinate_basis: String,
    pub boundary_rule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchyDoc {
    pub id: String,
    pub root_node: String,
    /// child -> parent（parent 可为 root_node）
    pub edges: Vec<HierarchyEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchyEdge {
    pub child_gate: String,
    pub parent: String,
}

/// 事件数据解释层的局部修订（不改动原始字节）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventOverlay {
    pub event_id: String,
    pub channel: String,
    pub original_token: String,
    pub revised_value: f64,
    pub revised_unit: String,
    pub reason: String,
}
