use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnitBasis {
    Count,
    Fluorescence { channel: String },
    Scatter,
    Transformed,
    Unitless,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    pub value: Option<f64>,
    pub representation: String,
    pub unit: String,
    pub basis: UnitBasis,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDef {
    pub channel_id: String,
    pub short_name: String,
    pub detector: String,
    pub fluorophore: Option<String>,
    pub unit: String,
    pub basis: UnitBasis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsData {
    pub source_blob_id: String,
    pub filename: String,
    pub channels: Vec<ChannelDef>,
    /// rows[event][channel] 保留原始表示；无法转换留原位置附诊断
    pub rows: Vec<Vec<RawValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub source_blob_id: String,
    pub labels: Vec<String>,
    pub convention: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompMatrix {
    pub labels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransformSpec {
    Linear {
        channel_id: String,
        scale: f64,
        input_unit: String,
    },
    Log10Offset {
        channel_id: String,
        offset: f64,
        input_unit: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Polygon {
    pub vertices: Vec<Point>,
    pub coordinate_system: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateGeom {
    Polygon {
        x_channel: String,
        y_channel: String,
        polygon: Polygon,
    },
    Rectangle {
        x_channel: String,
        y_channel: String,
        x0: f64,
        x1: f64,
        y0: f64,
        y1: f64,
        coordinate_system: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDef {
    pub gate_id: String,
    pub name: String,
    pub geom: GateGeom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateNode {
    pub gate_id: String,
    pub parent_gate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateTree {
    pub nodes: Vec<GateNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub candidate_id: String,
    pub candidate_set_id: String,
    pub label: String,
    pub legal: bool,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopMembership {
    pub event_index: u32,
    pub value_id_x: String,
    pub value_id_y: String,
    pub inside: bool,
    pub boundary: bool,
    pub excluded: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopResult {
    pub pop_result_id: String,
    pub gate_id: String,
    pub gate_name: String,
    pub gate_geom_version_id: String,
    pub parent_pop_result_id: Option<String>,
    pub denominator_kind: String,
    pub denominator_ref: String,
    pub denominator_count: i64,
    pub numerator_count: i64,
    pub excluded_unconvertible: i64,
    pub percent: Option<f64>,
    pub status: String,
    pub detail: Vec<PopMembership>,
    pub deps: serde_json::Value,
    pub created_event_id: String,
}
