//! 领域类型与纯函数：量纲、矩阵、补偿、变换、门几何。
//!
//! 本模块不接触数据库与网络，全部为可单测的纯计算。

pub mod gate;
pub mod matrix;
pub mod transform;
pub mod quantity;

use serde::{Deserialize, Serialize};

pub use gate::{GateGeometry, Point};
pub use matrix::{Matrix, MatrixDiag};
pub use quantity::Quantity;
pub use transform::TransformSpec;

/// 通道定义：单位与坐标基准在导入时固定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    pub id: String,
    pub name: String,
    pub fluorophore: String,
    pub detector: String,
    /// 原始单位，例如 "ADC-equivalent counts"。
    pub unit: String,
}

/// 单个事件在单个通道上的读数。raw 永远保留导入时的原始表示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelValue {
    pub channel_id: String,
    pub raw: String,
    /// 仅当 raw 可确定解析为有限数时为 Some。
    pub value: Option<f64>,
    pub unit: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

/// 一次导入的事件集（原始层）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelDataset {
    pub dataset_id: String,
    pub version: u32,
    pub blob_id: String,
    pub channels: Vec<ChannelDef>,
    pub events: Vec<EventRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRow {
    pub event_id: String,
    pub values: Vec<ChannelValue>,
}

/// Spillover（溢出）元数据与矩阵：来自单染对照，是补偿的“直接来源”。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverSpec {
    pub spillover_id: String,
    pub version: u32,
    pub blob_id: String,
    pub labels: Vec<String>,
    pub matrix: Matrix,
    /// 元数据，例如单染对照编号、采集日期；与数值分开保存。
    pub metadata: std::collections::BTreeMap<String, String>,
}

/// 补偿矩阵候选。近奇异时系统不默默求逆，而是显式保存多个合法候选。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationProposal {
    pub compensation_id: String,
    pub version: u32,
    pub spillover_id: String,
    pub spillover_version: u32,
    /// 被求逆/求解的矩阵；None 表示没有任何候选（严格奇异）。
    pub basis: Matrix,
    pub candidates: Vec<CompensationCandidate>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationCandidate {
    pub candidate_id: String,
    pub method: String,
    /// 解出的补偿矩阵；无法唯一决定时对每个合法 ridge 参数各保存一个。
    pub matrix: Matrix,
    pub ridge_lambda: Option<f64>,
    pub residual: f64,
    pub condition_estimate: f64,
}

/// 变换参数版本（派生层）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformVersion {
    pub transform_id: String,
    pub version: u32,
    pub channel_id: String,
    pub spec: TransformSpec,
}

/// 门多边形版本（派生层）。坐标基准显式保存，绝不隐式假定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateVersion {
    pub gate_id: String,
    pub version: u32,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    /// "raw" | "compensated:<comp_id>" | "transformed:<transform_id>"
    pub basis: String,
    pub geometry: GateGeometry,
}

/// 门父子关系版本：与门几何分开版本化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreeEdgeVersion {
    pub edge_id: String,
    pub version: u32,
    pub parent_gate: Option<String>,
    pub child_gate: String,
}

/// 群体结果版本：成员、分母都钉住具体上游版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopulationResult {
    pub population_id: String,
    pub version: u32,
    pub gate_id: String,
    pub gate_version: u32,
    pub edge_version: u32,
    /// 分母钉住的父群体结果版本；根群为 None（分母为数据集版本）。
    pub denominator: DenominatorRef,
    pub member_event_ids: Vec<String>,
    pub numerator: usize,
    pub denominator_count: usize,
    pub percent: f64,
    /// 计算本结果时实际使用的补偿版本（若坐标基准需要）。
    pub compensation_id: Option<String>,
    pub compensation_version: Option<u32>,
    pub transform_versions: Vec<(String, u32)>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DenominatorRef {
    Dataset { dataset_id: String, version: u32 },
    Population { population_id: String, version: u32 },
}
