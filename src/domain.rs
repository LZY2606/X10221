//! Domain model for the flow-cytometry boundary chamber.
//!
//! Sources and interpretations are stored separately:
//! raw channel values, spillover metadata, compensation matrices, transform
//! parameters, gate polygons and lineage edges each get their own versioned
//! artifact. Values carry units / coordinate basis; raw representations are
//! preserved on import and only derived layers normalize.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ImportDiagnostic {
    pub code: String,
    pub event_index: Option<u32>,
    pub channel: Option<String>,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellQuality {
    Ok,
    NonFinite,
    OutOfRange,
}

/// One raw channel cell. `raw` is the original representation; `value` is the
/// parsed finite value, if any. A value that cannot be converted stays in place
/// (`value = None`) with a diagnostic instead of being dropped for plotting.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RawCell {
    pub raw: String,
    pub value: Option<f64>,
    pub quality: CellQuality,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Channel {
    pub index: u32,
    pub name: String,
    pub detector: Option<String>,
    pub fluorophore: Option<String>,
    /// Unit of the raw detector signal, e.g. "detector-counts".
    pub unit: String,
    pub voltage: Option<f64>,
    pub gain: Option<f64>,
    pub range_min: f64,
    pub range_max: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RawFile {
    pub filename: String,
    pub fcs_version: String,
    pub sha256: String,
    pub channels: Vec<Channel>,
    pub events: Vec<Vec<RawCell>>,
    pub diagnostics: Vec<ImportDiagnostic>,
}

/// Spillover metadata exactly as carried by the FCS file / single-stain
/// controls. `matrix_text` preserves the original keyword representation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SpilloverMeta {
    pub name: String,
    pub keyword: String,
    pub detectors: Vec<String>,
    pub markers: Vec<String>,
    pub matrix_text: String,
    pub matrix: Vec<Vec<f64>>,
    pub source_raw_ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CompMatrix {
    pub detectors: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub source_spillover_id: String,
    pub note: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MatrixVerdict {
    pub determinant: f64,
    pub sigma_min: f64,
    pub sigma_max: f64,
    pub kappa: Option<f64>,
    pub status: String, // "ok" | "near_singular" | "singular"
}

/// Coordinate reference for an axis.
/// `raw:detector-counts`, `compensated:detector-counts` or `transformed:asinh-decade`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AxisRef {
    pub channel: String,
    pub basis: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PolygonGate {
    /// Stable across renames / revisions.
    pub gate_uid: String,
    pub label: String,
    pub x: AxisRef,
    pub y: AxisRef,
    pub polygon: Vec<[f64; 2]>,
    /// Boundary convention: points on the edge count as members ("include").
    pub boundary_rule: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LineageEdge {
    pub gate_uid: String,
    /// "root" or the parent gate uid.
    pub parent: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Lineage {
    pub edges: Vec<LineageEdge>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TransformSpec {
    pub channel: String,
    /// "asinh" | "linear".
    pub kind: String,
    pub cofactor: Option<f64>,
    pub gain: Option<f64>,
    pub offset: Option<f64>,
    /// Output unit label, e.g. "asinh-decade".
    pub output_unit: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CandidateResult {
    pub candidate_id: String,
    pub method: String, // "inverse" | "pseudoinverse" | "ridge" | "unresolved-channels"
    pub method_label: String,
    pub params: serde_json::Value,
    /// Rows aligned to raw events; None marks non-finite / unresolved cells.
    pub compensated: Vec<Vec<Option<f64>>>,
    /// Per-cell flags: "extreme" | "non-finite" | "unresolved".
    pub flags: Vec<Vec<Vec<String>>>,
    pub unresolved_channels: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CompRun {
    pub run_id: String,
    pub raw_id: String,
    pub matrix_id: String,
    /// Detector name -> column index in each candidate's output rows.
    pub matrix_position: std::collections::BTreeMap<String, usize>,
    pub status: String, // "unique" | "underdetermined"
    pub candidates: Vec<CandidateResult>,
    pub picked_candidate_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DenominatorRef {
    /// None means the root population (raw file).
    pub parent_pop_id: Option<String>,
    pub parent_gate_uid: Option<String>,
    pub parent_label: String,
    /// Event that materialized the pinned parent version.
    pub pinned_event_id: String,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PopResult {
    pub pop_id: String,
    pub gate_uid: String,
    pub label: String,
    pub gate_artifact_id: String,
    pub lineage_artifact_id: String,
    pub comp_run_id: Option<String>,
    pub candidate_id: Option<String>,
    pub transform_ids: Vec<String>,
    /// None = gates directly on the root population.
    pub input_pop_id: Option<String>,
    pub count: u32,
    pub percent: Option<f64>,
    pub denominator: DenominatorRef,
    pub members: Vec<u32>,
    pub boundary_members: Vec<u32>,
    pub basis: String,
    pub stale: bool,
    pub stale_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LedgerEvent {
    pub id: String,
    pub seq: u64,
    pub branch: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub parents: Vec<String>,
    pub author: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ArtifactRec {
    pub id: String,
    pub kind: String,
    /// Stable domain key, e.g. channel name for transforms, gate uid for gates.
    pub key: String,
    pub event_id: String,
    pub seq: u64,
    pub payload: serde_json::Value,
}
