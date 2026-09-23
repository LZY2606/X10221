//! Domain model for flow cytometry: raw channels, spillover, compensation,
//! transforms, gates, lineage and derived populations.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub channel_id: String,
    pub short_name: String,
    pub range: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: String,
    #[serde(default)]
    pub tag: String,
    /// channel_id -> raw detector value, preserved in original representation
    pub values: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvents {
    pub artifact_id: String,
    pub version: String,
    pub source_file: String,
    pub representation: String,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleStainControl {
    pub control_id: String,
    pub file: String,
    pub channel_id: String,
    pub fluorochrome: String,
    #[serde(rename = "voltage_v")]
    pub voltage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub artifact_id: String,
    pub version: String,
    pub acquisition: BTreeMap<String, String>,
    pub controls: Vec<SingleStainControl>,
    pub spillover_matrix_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matrix {
    pub artifact_id: String,
    pub version: String,
    pub channels: Vec<String>,
    #[serde(default)]
    pub unit_basis: String,
    pub rows: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsinhTransform {
    pub channel_id: String,
    #[serde(rename = "family")]
    pub family: String,
    pub cofactor: f64,
    #[serde(default)]
    pub cofactor_unit: String,
    #[serde(default)]
    pub output_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearChannel {
    pub channel_id: String,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformParams {
    pub artifact_id: String,
    pub version: String,
    pub transforms: Vec<AsinhTransform>,
    pub linear_channels: Vec<LinearChannel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    pub gate_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// polygon vertices in the declared coordinate basis
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gates {
    pub artifact_id: String,
    pub version: String,
    #[serde(default)]
    pub coord_basis: String,
    pub gates: Vec<Gate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEdge {
    pub child_gate: String,
    pub parent_gate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lineage {
    pub artifact_id: String,
    pub version: String,
    pub edges: Vec<LineageEdge>,
}

/// Artifact kinds whose source and interpretation are stored/versioned apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RawEvents,
    SpilloverMeta,
    Matrix,
    Transform,
    Gates,
    Lineage,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::RawEvents => "raw_events",
            ArtifactKind::SpilloverMeta => "spillover_meta",
            ArtifactKind::Matrix => "matrix",
            ArtifactKind::Transform => "transform",
            ArtifactKind::Gates => "gates",
            ArtifactKind::Lineage => "lineage",
        }
    }
    pub fn all() -> [ArtifactKind; 6] {
        [
            ArtifactKind::RawEvents,
            ArtifactKind::SpilloverMeta,
            ArtifactKind::Matrix,
            ArtifactKind::Transform,
            ArtifactKind::Gates,
            ArtifactKind::Lineage,
        ]
    }
}
