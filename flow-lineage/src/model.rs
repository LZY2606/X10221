//! Domain objects. Every measured quantity carries a unit or coordinate basis;
//! raw representations are preserved and normalization happens only in the
//! derived layer.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    /// Stable detector/channel name as written by the platform.
    pub name: String,
    /// "linear" | "logical" raw representation
    pub signal: String,
    /// Physical unit, e.g. "fluorescence-area" or "forward-scatter".
    pub unit: String,
    /// Short label used by spillover columns.
    pub fluorophore: Option<String>,
}

/// One FCS event: raw channel values kept in their original representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawEvent {
    /// Stable event ordinal inside the acquisition.
    pub index: i64,
    /// Raw channel values in acquisition order. Values may be strings such as
    /// "OVRFL" or null; unconvertible values stay in place with diagnostics.
    pub values: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawDataset {
    pub channels: Vec<Channel>,
    pub events: Vec<RawEvent>,
    /// Coordinate basis of the raw acquisition, e.g. "instrument-linear".
    pub coordinate_basis: String,
    pub source_label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    /// Channel names in matrix row/column order.
    pub order: Vec<String>,
    /// Plateform-provided spillover table (fractions, unit = "fraction").
    pub spillover: Vec<Vec<f64>>,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub order: Vec<String>,
    /// Compensation matrix S; compensated = S^{-1} * raw.
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    /// Optional reference to the spillover metadata this was derived from.
    pub derived_from: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransformSpec {
    /// Map fluorophore label -> {cofactor, kind}; scatter stays linear.
    pub params: std::collections::BTreeMap<String, TransformParam>,
    pub target_basis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransformParam {
    pub kind: String,
    pub cofactor: f64,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    /// Entity identity kept across geometry revisions.
    pub gate_id: String,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    /// "compensated-linear" | "transformed-asinh" | "raw"
    pub coordinate_basis: String,
    pub polygon: Vec<[f64; 2]>,
}

/// Separately versioned parent/child relationship ("gate nesting").
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GateLink {
    pub link_id: String,
    pub gate_id: String,
    /// gate_id of parent, or null for the root ungated population.
    pub parent_gate_id: Option<String>,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GatingTree {
    pub gates: Vec<GatePolygon>,
    pub links: Vec<GateLink>,
}
