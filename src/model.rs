use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 六类分开版本化的材料。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    RawChannels,
    SpilloverMeta,
    CompensationMatrix,
    TransformParams,
    GatePolygon,
    GateHierarchy,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RawChannels => "raw-channels",
            Self::SpilloverMeta => "spillover-meta",
            Self::CompensationMatrix => "compensation-matrix",
            Self::TransformParams => "transform-params",
            Self::GatePolygon => "gate-polygon",
            Self::GateHierarchy => "gate-hierarchy",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw-channels" => Self::RawChannels,
            "spillover-meta" => Self::SpilloverMeta,
            "compensation-matrix" => Self::CompensationMatrix,
            "transform-params" => Self::TransformParams,
            "gate-polygon" => Self::GatePolygon,
            "gate-hierarchy" => Self::GateHierarchy,
            _ => return None,
        })
    }
    pub fn all() -> [Self; 6] {
        [
            Self::RawChannels,
            Self::SpilloverMeta,
            Self::CompensationMatrix,
            Self::TransformParams,
            Self::GatePolygon,
            Self::GateHierarchy,
        ]
    }
}

/// 领域量：保留原始表示，单位与坐标基准随行；派生层才做规范化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelValue {
    pub raw: String,
    #[serde(default)]
    pub value: Option<f64>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    pub values: BTreeMap<String, ChannelValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawChannelsPayload {
    pub channels: Vec<String>,
    pub detector: DetectorMeta,
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectorMeta {
    pub linear_max: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMetaPayload {
    pub controls: Vec<ControlMeta>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMeta {
    pub fluorochrome: String,
    pub detector: String,
    pub coefficient: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompMatrixPayload {
    pub order: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParamsPayload {
    pub transforms: BTreeMap<String, TransformSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformSpec {
    pub kind: String,
    pub cofactor: f64,
    pub output_unit: String,
    pub output_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePolygonPayload {
    pub gates: Vec<GateDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDef {
    pub gate_id: String,
    pub x_channel: String,
    pub y_channel: String,
    pub basis: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateHierarchyPayload {
    pub nodes: Vec<HierNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierNode {
    pub gate_id: String,
    pub name: String,
    pub parent: Option<String>,
}

/// 候选结论状态。
pub mod status {
    pub const PENDING: &str = "pending";
    pub const ACCEPTED: &str = "accepted";
    pub const REJECTED: &str = "rejected";
    pub const STALE: &str = "stale";
    pub const BLOCKED: &str = "blocked";
}

/// 机器可读取事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}
