//! Domain objects for the flow-cytometry compensation/gating lineage.
//!
//! Provenance is split from interpretation: raw channels, spillover metadata,
//! compensation matrices, transform parameters, gates and parent/child links
//! are each independently versioned content-addressed objects.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoordinateBasis {
    /// e.g. "raw-pulse-area", "compensated-linear", "asinh-cofactor=150"
    pub space: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    /// Stable channel identity, e.g. "FITC-A".
    pub name: String,
    pub detector: String,
    pub fluorophore: Option<String>,
    pub unit: String,
    pub coordinate_basis: CoordinateBasis,
    pub declared_min: f64,
    pub declared_max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    pub index: u32,
    /// Values stored in the original representation, one per channel name.
    pub values: Vec<f64>,
    /// Per-channel diagnostics; nothing is dropped to make plotting easier.
    #[serde(default)]
    pub diagnostics: Vec<ValueDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValueDiagnostic {
    pub channel_index: usize,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvents {
    pub fcs_file: String,
    pub fcs_sha256: String,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
}

/// What the single-stain controls say about detector/fluorophore spillover.
/// Kept separately from the numeric matrix: a metadata reinterpretation must
/// not silently change the compensation numbers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub channels: Vec<String>,
    pub fluorophores: Vec<String>,
    pub single_stain_controls: Vec<SingleStainControl>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub fluorophore: String,
    pub control_file: String,
    pub control_sha256: String,
    pub primary_channel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    /// Square matrix rows, in `channels` order. Compensated = inverse(M) * raw.
    pub channels: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub channel: String,
    /// "raw" (original pulse signal) or "compensated" (post spillover matrix).
    pub input: String,
    pub family: TransformFamily,
    /// Coordinate basis produced by this transform.
    pub output_basis: CoordinateBasis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransformFamily {
    Linear { scale: f64 },
    Asinh { cofactor: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gate {
    /// Stable lineage identity (does not change across renames).
    pub stable_id: String,
    /// Mutable display name; renames create a new version, same stable_id.
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    /// Coordinate basis the polygon coordinates are expressed in.
    pub basis: CoordinateBasis,
    pub polygon: Vec<Point>,
}

/// Parent/child relation, versioned independently of polygon geometry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateLink {
    pub child_stable_id: String,
    /// None means the gate roots at the full event set ("UNGATED").
    pub parent_stable_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixQuality {
    pub invertible: bool,
    /// Reciprocal condition number estimate; 0.0 for an exactly singular
    /// matrix. Small (< 1e-6) means near-singular.
    pub rcond: f64,
    pub log10_condition: f64,
    pub rank: usize,
    pub smallest_pivot: f64,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InversionCandidates {
    /// Exactly one candidate for a well-conditioned matrix; several
    /// legitimate alternatives for a near-singular one; empty when singular.
    pub candidates: Vec<InversionCandidate>,
    pub quality: MatrixQuality,
    pub uniquely_determined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InversionCandidate {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub epsilon: Option<f64>,
    pub inverse: Vec<Vec<f64>>,
    pub amplification: f64,
    pub legal: bool,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryCandidates {
    /// Event indices that lie exactly on a polygon edge, with every legal
    /// membership convention kept until adjudicated.
    pub event_indices: Vec<u32>,
    pub candidates: Vec<BoundaryCandidate>,
    pub uniquely_determined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryCandidate {
    pub convention: String,
    pub label: String,
    /// Boundary events considered inside under this convention.
    pub inside_event_indices: Vec<u32>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PopResult {
    pub gate_stable_id: String,
    pub gate_version: String,
    pub link_version: String,
    pub transform_version: String,
    pub compensation_version: Option<String>,
    /// Version of the parent population this percentage denominator pins.
    pub parent_pop_version: Option<String>,
    pub parent_gate_stable_id: Option<String>,
    pub count: u64,
    pub parent_count: u64,
    pub percent_of_parent: f64,
    pub percent_of_parent_unit: String,
    pub member_event_indices: Vec<u32>,
    #[serde(default)]
    pub boundary_event_indices: Vec<u32>,
    pub boundary_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StaleReason {
    GateRevision {
        stable_id: String,
        old_version: String,
        new_version: String,
        affects: Vec<String>,
    },
    LinkRevision {
        old_version: String,
        new_version: String,
        affects: Vec<String>,
    },
    TransformRevision {
        channel: String,
        old_version: String,
        new_version: String,
        affects: Vec<String>,
    },
    CompensationSuperseded {
        old_version: String,
        new_version: Option<String>,
        affects: Vec<String>,
    },
    AdjudicationWithdrawn {
        scope: String,
        affects: Vec<String>,
    },
}

impl StaleReason {
    pub fn affected(&self) -> &[String] {
        match self {
            StaleReason::GateRevision { affects, .. }
            | StaleReason::LinkRevision { affects, .. }
            | StaleReason::TransformRevision { affects, .. }
            | StaleReason::CompensationSuperseded { affects, .. }
            | StaleReason::AdjudicationWithdrawn { affects, .. } => affects,
        }
    }
}

/// Immutable record appended to a branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub id: String,
    pub branch: String,
    /// Monotonic per-branch sequence number.
    pub seq: u64,
    /// Fixed logical clock from the case pack; makes replay deterministic.
    pub clock_ms: i64,
    pub actor: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub parent_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchInfo {
    pub name: String,
    pub tip_seq: u64,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensatedEvents {
    pub raw_version: String,
    pub matrix_version: String,
    pub candidate_kind: String,
    #[serde(default)]
    pub epsilon: Option<f64>,
    pub channels: Vec<String>,
    /// Compensated values per raw event index, in `channels` order.
    pub values: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixCandidateSet {
    pub matrix_version: String,
    pub candidates: InversionCandidates,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryCandidateSet {
    pub gate_version: String,
    pub candidates: BoundaryCandidates,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompAdjudication {
    pub matrix_version: String,
    pub candidate_kind: String,
    #[serde(default)]
    pub epsilon: Option<f64>,
    pub candidate_label: String,
    pub compensated_version: String,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryAdjudication {
    pub gate_version: String,
    pub convention: String,
    pub boundary_candidates_version: String,
    pub event_id: String,
}
