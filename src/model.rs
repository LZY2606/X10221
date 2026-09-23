//! Content types. The six provenance layers are versioned independently:
//! raw_channels, spillover, compensation, transform, gate_polygons, gate_tree.
//! Original representation is preserved on import; normalization only happens
//! in derived structures.

use serde::{Deserialize, Serialize};

pub const LAYER_KINDS: [&str; 6] = [
    "raw_channels",
    "spillover",
    "compensation",
    "transform",
    "gate_polygons",
    "gate_tree",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum LayerKind {
    RawChannels,
    Spillover,
    Compensation,
    Transform,
    GatePolygons,
    GateTree,
}

impl LayerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LayerKind::RawChannels => "raw_channels",
            LayerKind::Spillover => "spillover",
            LayerKind::Compensation => "compensation",
            LayerKind::Transform => "transform",
            LayerKind::GatePolygons => "gate_polygons",
            LayerKind::GateTree => "gate_tree",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_channels" => LayerKind::RawChannels,
            "spillover" => LayerKind::Spillover,
            "compensation" => LayerKind::Compensation,
            "transform" => LayerKind::Transform,
            "gate_polygons" => LayerKind::GatePolygons,
            "gate_tree" => LayerKind::GateTree,
            _ => return None,
        })
    }
}

/// raw_channels layer: keeps original channel basis, original f32 bit
/// representation is held in the artifact blob; values + diagnostics here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawChannelsV {
    pub schema: String,
    pub file_id: String,
    pub channels: Vec<RawChannel>,
    pub events: Vec<Vec<f64>>,
    /// original representation for flagged positions, indexed by event/channel
    pub retained_unconvertible: Vec<UnconvertibleCell>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawChannel {
    pub name: String,
    pub short_name: Option<String>,
    pub unit_or_basis: String,
    pub raw_range: i64,
    pub amp: String,
    pub gain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnconvertibleCell {
    pub event_index: usize,
    pub channel: String,
    pub raw_b32_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub event_index: Option<usize>,
    pub channel: Option<String>,
    pub message: String,
}

/// spillover layer: metadata separate from any computed inverse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverV {
    pub schema: String,
    pub source_file_id: String,
    pub keyword: String,
    pub fluorochromes: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub single_stain_controls: Vec<SingleStainControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleStainControl {
    pub file_id: String,
    pub fluorochrome: String,
    pub note: String,
}

/// compensation layer: derived candidates from a spillover layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationV {
    pub schema: String,
    pub based_on_spillover_id: String,
    pub reciprocal_condition: f64,
    pub near_singular: bool,
    pub near_singular_threshold: f64,
    pub candidates: Vec<CompCandidate>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompCandidate {
    pub candidate_id: String,
    pub method: String,
    pub matrix: Vec<Vec<f64>>,
    pub ridge_lambda: Option<f64>,
    pub invertible: bool,
    pub explanation: String,
}

/// transform layer: all parameters carry units / coordinate basis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformV {
    pub schema: String,
    pub transforms: Vec<TransformParam>,
    pub near_singular_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParam {
    pub channel: String,
    pub family: String,
    pub cofactor: f64,
    pub input_unit: String,
    pub output_basis: String,
}

/// gate polygon library (geometry) — separated from tree (parent/child).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolygonLibV {
    pub schema: String,
    pub polygons: Vec<Polygon>,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Polygon {
    pub polygon_id: String,
    pub display_name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// points in transform output coordinates
    pub points: Vec<[f64; 2]>,
    pub boundary_epsilon: f64,
    pub boundary_policy: String,
}

/// gate tree: named gates + parent relationships, pinned to polygon ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTreeV {
    pub schema: String,
    pub root_name: String,
    pub nodes: Vec<GateNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateNode {
    pub node_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub polygon_id: Option<String>,
}
