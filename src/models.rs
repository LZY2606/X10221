//! Versioned entity payload kinds.
//!
//! The six *separately versioned* layers required by the case narrative:
//!   1. raw channel values       -> `raw_fcs_events`
//!   2. spillover metadata       -> `spillover_meta`
//!   3. compensation matrix      -> `comp_matrix`
//!   4. transform parameters     -> `transform_params`
//!   5. gate polygons            -> `gate_polygon`
//!   6. parent/child relations   -> `gate_hierarchy`
//!
//! Re-interpreting material never mutates an entity: a new version is written
//! to the same stream and the previous bytes stay readable forever.

use serde::{Deserialize, Serialize};

use crate::domain::{Signal, UnitBasis};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub index: usize,
    pub short_name: String,
    pub long_name: String,
    pub basis: UnitBasis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_index: usize,
    /// One Signal per channel, in `ChannelRef` order.
    pub values: Vec<Signal>,
    /// Original textual representation straight out of the FCS DATA word
    /// (f32 bits rendered canonically). Kept so no raw information is lost.
    pub raw_tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFile {
    pub filename: String,
    pub media_type: String,
    pub sha256: String,
    pub byte_len: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFcsEvents {
    pub file_id: String,
    pub role: String,
    pub channels: Vec<ChannelRef>,
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SingleStainControl {
    pub fluorochrome: String,
    pub detector_channel: String,
    pub events_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub source_file_id: String,
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub single_stains: Vec<SingleStainControl>,
    pub note: String,
}

/// Numeric spillover matrix S (rows = detectors, columns = fluorochromes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMatrix {
    pub channel_order: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub meta_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvertRecord {
    pub status: String,
    pub min_pivot: f64,
    pub condition_estimate: f64,
    pub threshold: f64,
    pub message: String,
}

/// Compensation matrix = S^{-1}. It may legitimately have no numbers: an
/// exactly singular or near-singular spillover matrix is never silently
/// inverted, and that judgement is versioned here instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompMatrix {
    pub channel_order: Vec<String>,
    pub matrix: Option<Vec<Vec<f64>>>,
    pub source_spill_matrix_id: String,
    pub invert: InvertRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParams {
    pub method: String,
    pub channel_order: Vec<String>,
    /// asinh: y = asinh(x / cofactor); unit_in -> unit_out recorded below.
    pub cofactor: f64,
    pub unit_in: String,
    pub unit_out: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatePolygon {
    pub gate_key: String,
    pub label: String,
    /// [x channel, y channel]
    pub channels: [String; 2],
    pub transform_method: String,
    /// Polygon coordinates in the transformed coordinate basis.
    pub points: Vec<crate::domain::Point>,
    /// Proposed edge convention. A point exactly on an edge is a source of
    /// legitimate ambiguity; resolution is a ruling, not a silent default.
    pub edge_policy_proposal: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HierarchyNode {
    pub population_key: String,
    pub gate_key: String,
    pub parent_population_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateHierarchy {
    pub nodes: Vec<HierarchyNode>,
}
