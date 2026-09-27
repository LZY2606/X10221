use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawCase {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: String,
    pub channels: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixFile {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub unit: String,
    pub channels: Vec<String>,
    pub values: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformFile {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub method: String,
    pub output_unit: String,
    pub parameters: Vec<TransformParameter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParameter {
    pub channel: String,
    pub parameter: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolygonFile {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub polygons: Vec<Polygon>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Polygon {
    pub gate_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: String,
    pub unit: String,
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageFile {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub nodes: Vec<LineageNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageNode {
    pub node_id: String,
    pub gate_id: String,
    pub name: String,
    pub parent_node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverFile {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "material_id")]
    pub material_id: String,
    pub version: i64,
    pub coordinate_basis: String,
    pub unit: String,
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub single_stain_controls: Vec<Value>,
    pub spillover_matrix_reference: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Membership {
    Inside,
    Outside,
    Boundary,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CandidateStatus {
    Proposed,
    Accepted,
    Rejected,
    Stale,
    Undone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub affected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedResultRow {
    pub stable_id: String,
    pub event_id: i64,
    pub branch_id: String,
    pub object_id: String,
    pub result_kind: String,
    pub status: String,
    pub input_refs: Value,
    pub value: Value,
    pub denominator_ref: Option<Value>,
    pub diagnostics: Vec<Value>,
}

pub fn kind_for_material_id(material_id: &str) -> &'static str {
    match material_id {
        "fcs-events" => "raw_events",
        "spillover-metadata" => "spillover_metadata",
        "compensation-matrix" => "compensation_matrix",
        "transform-parameters" => "transform_parameters",
        "gate-polygons" => "gate_polygons",
        "gate-lineage" => "gate_lineage",
        _ => "unknown",
    }
}
