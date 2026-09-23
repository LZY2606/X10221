//! Typed payloads for the separately-versioned domain objects.
//!
//! Every object is either *authored* (imported or revised by a human action) or
//! *derived* (computed). Both are stored as immutable versions inside events;
//! derived versions additionally carry diagnostics and candidates.

use crate::engine::math::RidgeCandidate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    #[serde(rename = "raw_events")]
    RawEvents,
    #[serde(rename = "spillover")]
    Spillover,
    #[serde(rename = "compensation_matrix")]
    Compensation,
    #[serde(rename = "transform_params")]
    Transform,
    #[serde(rename = "gate_polygon")]
    Gate,
    #[serde(rename = "gate_edges")]
    GateEdges,
    #[serde(rename = "compensated_events")]
    Compensated,
    #[serde(rename = "population")]
    Population,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::RawEvents => "raw_events",
            NodeKind::Spillover => "spillover",
            NodeKind::Compensation => "compensation_matrix",
            NodeKind::Transform => "transform_params",
            NodeKind::Gate => "gate_polygon",
            NodeKind::GateEdges => "gate_edges",
            NodeKind::Compensated => "compensated_events",
            NodeKind::Population => "population",
        }
    }
    pub fn derived(self) -> bool {
        matches!(self, NodeKind::Compensated | NodeKind::Population)
    }
    pub fn parse(s: &str) -> Option<NodeKind> {
        NodeKind::all().into_iter().find(|k| k.as_str() == s)
    }
    pub fn all() -> [NodeKind; 8] {
        [
            NodeKind::RawEvents,
            NodeKind::Spillover,
            NodeKind::Compensation,
            NodeKind::Transform,
            NodeKind::Gate,
            NodeKind::GateEdges,
            NodeKind::Compensated,
            NodeKind::Population,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawBatch {
    pub file_name: String,
    pub sha1: String,
    pub fcs_version: String,
    pub role: String, // "sample" | "single_stain:<channel>"
    pub channel_names: Vec<String>,
    pub channel_units: Vec<Option<String>>,
    pub event_count: usize,
    /// Parallel rows; unconvertible values stay as tokens with numeric=null.
    pub values: Vec<Vec<RawValue>>,
    pub diagnostics: Vec<RawDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    /// Original token text from FCS; never rewritten.
    pub token: String,
    pub numeric: Option<f64>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawDiagnostic {
    pub event_index: usize,
    pub channel_index: usize,
    pub channel: String,
    pub token: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub source: String, // file + keyword of origin
    pub channels: Vec<String>,
    /// Spillover matrix S: row i is signal observed in detector i.
    pub matrix: Vec<Vec<f64>>,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub channels: Vec<String>,
    /// Compensated = C * measured. Equals S^-1 when well-conditioned.
    pub matrix: Vec<Vec<f64>>,
    pub rcond: f64,
    pub determinant: f64,
    pub basis: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub channels: Vec<String>,
    pub t: f64,
    pub w: f64,
    pub m: f64,
    pub a: f64,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub gate_id: String,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    /// "raw:ADC counts" | "logicle:dimensionless"
    pub coordinate_basis: String,
    pub transform_version: Option<String>,
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateEdges {
    pub edges: Vec<GateEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateEdge {
    pub child_gate: String,
    pub parent_gate: Option<String>, // None => root child of "all events"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventPoint {
    pub event_index: usize,
    pub coords: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensatedEventSet {
    pub raw_version: String,
    pub spillover_version: String,
    pub compensation_version: String,
    pub channels: Vec<String>,
    /// Non-finite or unconvertible outputs are omitted and diagnosed, never
    /// silently replaced by zero.
    pub values: Vec<Vec<Option<f64>>>,
    pub diagnostics: Vec<String>,
    pub blocked: bool,
    pub candidates: Vec<RidgeCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventClass {
    pub event_index: usize,
    pub x: f64,
    pub y: f64,
    pub classification: String, // inside | boundary | outside
    pub included: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Population {
    pub gate_id: String,
    pub gate_version: String,
    pub edges_version: String,
    pub transform_version: String,
    pub compensated_version: String,
    pub raw_version: String,
    /// Pinned denominator population-version, not "latest".
    pub denominator_version: String,
    pub denominator_label: String,
    pub parent_gate: Option<String>,
    pub event_classes: Vec<EventClass>,
    pub numerator: usize,
    pub denominator: usize,
    pub percent: f64,
    pub boundary_count: usize,
}
