//! Domain model.
//!
//! Provenance rule enforced everywhere:
//! original representation (`RawEvents`, `SpilloverMeta`) is stored verbatim,
//! normalization happens only in derived layers, and unconvertible values stay
//! in place with diagnostics instead of being silently dropped.

use serde::{Deserialize, Serialize};

/// Marker of the coordinate basis a numeric column is expressed in.
/// Every derived numeric quantity must carry one of these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Basis {
    /// e.g. "fcs-linear", "compensated", "asinh5"
    pub space: String,
    /// Physical unit where applicable, e.g. "fluorophore-area", "fsc-area",
    /// or "dimensionless" for ratio/compensated channels.
    pub unit: String,
}

impl Basis {
    pub fn new(space: impl Into<String>, unit: impl Into<String>) -> Self {
        Self { space: space.into(), unit: unit.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub index: usize,
    /// Short detector name, e.g. "FITC-A".
    pub short_name: String,
    /// Full $PnS / detector label as imported, verbatim.
    pub long_name: String,
    /// $PnE gain/timestep pair exactly as encoded in the FCS file.
    pub pne: String,
    /// $PnR range exactly as encoded.
    pub pnr: i64,
    /// Amplification type as encoded: "L" (linear) or other verbatim letter.
    pub amplification: String,
    pub unit: String,
    pub basis: Basis,
}

/// One raw event. The exact f32 bit pattern of every channel is preserved
/// (`bits`); special values (NaN / Inf) never disappear on import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    /// 0-based event ordinal inside the FCS DATA segment. Stable for life.
    pub ordinal: usize,
    /// f32 bit patterns per channel in channel order.
    pub bits: Vec<u32>,
    /// Per-cell diagnostics collected during import (never triggers cleaning).
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvents {
    pub format: String,
    /// Verbatim copy of every TEXT keyword from the FCS file (or synthetic
    /// generator), including delimiter conventions.
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
    /// Byte offsets/length of the DATA segment in the originating file,
    /// kept for source-chain queries.
    pub source_name: String,
    pub data_offset: usize,
    pub data_len: usize,
}

/// Single-stain control description. It points at raw event data but never
/// mutates it; the interpretation is stored separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SingleStainControl {
    pub name: String,
    pub channel: String,
    pub events_ref: String,
    pub lot: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub description: String,
    /// Channel order used by `matrix`.
    pub channels: Vec<String>,
    /// Row-major spillover matrix S such that measured = S * true.
    /// Stored as f64 textual matrix to keep the imported representation
    /// explicit; diagonal is 1.0.
    pub matrix: Vec<Vec<f64>>,
    /// How the matrix was obtained: "fcs-keyword", "manual", "single-stain".
    pub origin: String,
    /// Verbatim $SPILL / $SPILLOVER keyword if present.
    pub raw_keyword: String,
    pub controls: Vec<SingleStainControl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompCandidate {
    /// Stable candidate label, e.g. "inverse" / "pinv-minnorm" /
    /// "pinv-nullshift+/-" / "ridge-1e-04".
    pub label: String,
    /// "matrix" applies `matrix` directly; "pinv-nullshift" applies the
    /// minimum-norm pseudoinverse plus `null_shift_alpha` * `null_vector`
    /// (one representative of a whole family of equally-legal solutions).
    pub mode: String,
    /// Row-major compensation matrix C = candidate(S^{-1}) (the linear part).
    pub matrix: Vec<Vec<f64>>,
    pub null_vector: Option<Vec<f64>>,
    pub null_shift_alpha: Option<f64>,
    /// Why this candidate is offered (machine readable code + human text).
    pub reason_code: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompMatrix {
    pub spillover_ref: String,
    pub channels: Vec<String>,
    /// Ordered list of *all* legal compensation candidates. A well
    /// conditioned matrix yields exactly one ("inverse"). A (near) singular
    /// matrix yields several; nothing is inverted silently.
    pub candidates: Vec<CompCandidate>,
    pub condition_number: f64,
    pub near_singular: bool,
    pub singular: bool,
    pub threshold: f64,
    pub diagnostics: Vec<String>,
    pub rule_version: String,
}

/// One compensated event. Special raw values propagate as tagged entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedValue {
    /// Raw f32 bit pattern this value descends from (per channel, for this
    /// derived row the corresponding raw event ordinal is in `ordinal`).
    pub value: f64,
    pub special: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompResult {
    pub raw_events_ref: String,
    pub spillover_ref: String,
    pub comp_matrix_ref: String,
    /// Candidate label actually applied. None when no unique legal choice
    /// exists and the reviewer has not picked one yet.
    pub applied_candidate: Option<String>,
    pub channels: Vec<String>,
    pub basis: Basis,
    /// compensated[ordinal][channel]
    pub compensated: Vec<Vec<DerivedValue>>,
    /// When the spillover matrix is singular, `family_span[ordinal]` is the
    /// |null-space coefficient span| implied by this event's magnitude; the
    /// true compensated vector is only determined up to that family.
    pub family_span: Vec<f64>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParams {
    pub rule_version: String,
    /// Channels transformed and, for each, {cofactor, m, a} for the
    /// flow-compatible asinh transform x -> asinh(x/cofactor)/asinh(1/cofactor).
    pub kind: String,
    pub cofactor: f64,
    pub channel_overrides: Vec<(String, f64)>,
    /// Channels this transform applies to.
    pub channels: Vec<String>,
    pub input_basis: Basis,
    pub output_basis: Basis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformResult {
    pub comp_result_ref: String,
    pub params_ref: String,
    pub channels: Vec<String>,
    pub basis: Basis,
    /// transformed[ordinal][channel index in `channels`]
    pub transformed: Vec<Vec<DerivedValue>>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateShape {
    /// "polygon" | "rectangle"
    pub kind: String,
    pub x_channel: String,
    pub y_channel: Option<String>,
    pub x_basis: Basis,
    pub y_basis: Option<Basis>,
    /// Polygon vertices in counter-clockwise/any order; rectangles store
    /// two corners as first two points.
    pub points: Vec<(f64, f64)>,
    /// Boundary inclusion tolerance in transformed-coordinate units. Points
    /// within are flagged `boundary`.
    pub boundary_epsilon: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateNode {
    pub node_id: String,
    pub label: String,
    pub parent: Option<String>,
    pub shape_ref: String,
    pub order: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateTree {
    pub rule_version: String,
    pub root_label: String,
    pub nodes: Vec<GateNode>,
}

/// Denominator of a gate percentage, pinned to the exact parent result
/// version (or raw event set for root-level gates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Denominator {
    /// "raw-events" | "gate-result"
    pub kind: String,
    pub ref_id: String,
    pub parent_node: Option<String>,
    /// Number of parent-population events as of the pinned version.
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub tree_ref: String,
    pub node_id: String,
    pub node_label: String,
    pub transform_result_ref: String,
    pub denominator: Denominator,
    /// Included event ordinals (from the pinned parent population).
    pub inside: Vec<usize>,
    /// Ordinals lying exactly (within epsilon) on the gate boundary.
    pub boundary: Vec<usize>,
    /// Ordinals skipped because a coordinate could not be transformed.
    pub unplottable: Vec<usize>,
    pub numerator: usize,
    pub percentage: f64,
    pub diagnostics: Vec<String>,
}
