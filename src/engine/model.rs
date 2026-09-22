//! Domain model. Every physical quantity carries a unit or coordinate basis,
//! and every original representation is preserved (unconvertible values stay
//! in place with diagnostics instead of being dropped for plotting).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChannelSpec {
    pub id: String,
    pub label: String,
    pub detector: String,
    pub unit: String,
    pub basis: String,
    pub raw_min: f64,
    pub raw_max: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RawChannelValue {
    pub channel: String,
    /// Exactly as imported; never rewritten by the derived layers.
    pub raw_text: String,
    /// Parsed numeric value; None means unconvertible (diagnostic explains why).
    pub value: Option<f64>,
    pub unit: String,
    pub basis: String,
    pub flags: Vec<String>,
    pub diagnostic: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FcsEvent {
    pub event_id: String,
    pub index: i64,
    pub values: Vec<RawChannelValue>,
}

/// FCS-like import envelope: raw event-level detector signals.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FcsFile {
    pub media: String,
    /// Raw FCS keyword pairs ($PnN, $PnE, $SPILL, ...) preserved verbatim.
    pub fcs_keywords: Vec<[String; 2]>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<FcsEvent>,
    /// Partial re-read scope: only these detector channels were re-interpreted.
    /// A full import uses an empty vector.
    #[serde(default)]
    pub re_read_scope: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpillRow {
    pub stain: String,
    pub detector: String,
    /// (into-detector, coefficient) pairs; coefficients are dimensionless fractions.
    pub into: Vec<[f64; 2]>,
    pub into_detectors: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpilloverMeta {
    pub media: String,
    pub single_stain_controls: Vec<String>,
    pub basis: String,
    pub unit: String,
    pub rows: Vec<SpillRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CompMatrix {
    pub order: Vec<String>,
    /// Square detector-to-fluor matrix; compensation = matrix * raw_signal.
    pub matrix: Vec<Vec<f64>>,
    pub basis: String,
    pub unit: String,
    #[serde(default)]
    pub derived_from: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TransformParam {
    pub channel: String,
    pub kind: String,
    pub m: f64,
    pub t: f64,
    pub in_unit: String,
    pub out_unit: String,
    pub basis: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TransformSet {
    pub params: Vec<TransformParam>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GatePolygon {
    pub gate_id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub space: String,
    pub coordinate_basis: String,
    pub vertices: Vec<[f64; 2]>,
    /// "include" | "exclude" | "unspecified" (last emits a boundary candidate)
    pub boundary: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GateNode {
    pub gate_id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GateEdge {
    pub gate_id: String,
    /// None => hangs directly off the root population.
    pub parent: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GateTree {
    pub nodes: Vec<GateNode>,
    pub edges: Vec<GateEdge>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactContent {
    RawEvents(FcsFile),
    Spillover(SpilloverMeta),
    CompMatrix(CompMatrix),
    Transform(TransformSet),
    GatePolygon(GatePolygon),
    GateTree(GateTree),
}

impl ArtifactContent {
    pub fn kind(&self) -> &'static str {
        match self {
            ArtifactContent::RawEvents(_) => "raw_events",
            ArtifactContent::Spillover(_) => "spillover",
            ArtifactContent::CompMatrix(_) => "comp_matrix",
            ArtifactContent::Transform(_) => "transform",
            ArtifactContent::GatePolygon(_) => "gate_polygon",
            ArtifactContent::GateTree(_) => "gate_tree",
        }
    }
}

/// Commands carried by import/revision events.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportCommand {
    pub material_id: String,
    pub content: ArtifactContent,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Import(ImportCommand),
    Branch { name: String, fork_seq: i64 },
    Decision {
        candidate_id: String,
        choice: String,
        #[serde(default)]
        rationale: String,
    },
    DecisionUndo { candidate_id: String },
    /// Recompute stale populations after review (new event, never overwrites).
    Refresh { population_ids: Vec<String> },
}
