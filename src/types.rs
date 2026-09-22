//! 跨层共享的数据类型。所有领域量带单位/坐标基准；原始表示与派生表示分离。

use serde::{Deserialize, Serialize};

use crate::geom::Point;

/// 通道（参数）描述。unit 为原始信号单位（ADC count 等）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub label: String,
    pub unit: String,
    /// FCS $PnR 满量程，ADC count。
    pub range: f64,
    /// 原始值分辨率/增益来源（如 "$PnG"），仅作来源记录。
    pub gain_source: String,
}

/// 原始通道值：保留原始词法表示，派生层才规范化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawValue {
    /// 原始词法 token（"12345"、"65535 SAT" 解析为 "SAT" 之外的数值由 token_text 保存）。
    pub token: String,
    /// 可转换时的数值（ADC count）；无法转换为 null。
    pub numeric: Option<f64>,
    /// 诊断标签：none | overflow | saturation | bad_token
    pub diagnostic: String,
}

/// 事件（细胞）。values 按通道顺序；坐标基准始终为 "ADC count"。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventData {
    pub event_id: String,
    pub ordinal: usize,
    pub values: Vec<RawValue>,
}

/// 原始通道值集合（不可变 blob，kind=dataset）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dataset {
    pub dataset_id: String,
    pub basis: String,
    pub channels: Vec<Channel>,
    pub events: Vec<EventData>,
    /// 来源文件原始字节 sha256，证明来源链。
    pub source_sha256: String,
    pub source_name: String,
}

/// spillover 元数据（kind=spill_meta）：单染对照来源，与矩阵分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpillMeta {
    pub spill_meta_id: String,
    pub channels: Vec<String>,
    pub controls: Vec<SingleStainControl>,
    pub source_keyword: String,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SingleStainControl {
    pub control_id: String,
    pub fluorophore: String,
    pub primary_channel: String,
    /// 单位：dimensionless_fraction_of_primary
    pub spill: Vec<(String, f64)>,
}

/// 补偿/spillover 矩阵（kind=comp_matrix）。rows/cols 均按 channels 顺序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompMatrix {
    pub matrix_id: String,
    pub channels: Vec<String>,
    /// spillover S：S[i][j] = 通道 i 信号泄漏进通道 j 的比例；对角=1。
    pub s: Vec<Vec<f64>>,
    pub unit: String,
    pub derived_from_spill_meta: String,
    pub note: String,
}

/// 变换参数（kind=transform），荧光通道使用 asinh：t = asinh(x/cofactor)，单位 dimensionless。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParams {
    pub transform_id: String,
    /// 每个荧光通道一个 cofactor（ADC count）；散射通道保持 "linear:ADC count"。
    pub per_channel: Vec<ChannelTransform>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTransform {
    pub channel: String,
    pub method: String,
    pub cofactor: Option<f64>,
    pub cofactor_unit: Option<String>,
}

/// 门多边形（kind=gate），显式坐标基准。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub gate_id: String,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    pub basis: String,
    pub vertices: Vec<Point>,
    /// 多边形顶点顺序约定：math_ccw。
    pub orientation: String,
    /// 边界规则标识（半开射线法，左/下含、右/上不含）。
    pub boundary_rule: String,
}

/// 父子关系（kind=lineage）：边集合，与门定义分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lineage {
    pub lineage_id: String,
    /// 边 (parent_gate_id_or_ROOT, child_gate_id, population_name)。
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEdge {
    pub parent: String,
    pub child: String,
    pub population: String,
}

/// 门结果（kind=gate_result）：不可变快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub gate_id: String,
    pub label: String,
    pub parent: String,
    pub population: String,
    pub member_event_ids: Vec<String>,
    pub included_boundary: Vec<String>,
    pub excluded_boundary: Vec<String>,
    pub dropped_unconvertible: Vec<String>,
    pub numerator: usize,
    /// 分母钉住的父群结果（ROOT 时为数据集）。
    pub denominator: usize,
    pub denominator_ref: DenominatorRef,
    pub percent: f64,
    pub percent_unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DenominatorRef {
    pub kind: String,
    pub dataset_id: String,
    pub parent_gate_id: Option<String>,
    /// 父群门结果所在快照版本（blob_id 对 gate_result 集合），钉住而非取最新。
    pub parent_result_snapshot: Option<String>,
    pub parent_event_count: usize,
}
