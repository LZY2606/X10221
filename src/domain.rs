//! Domain model for the flow cytometry boundary vault.
//!
//! Every physical quantity carries either a unit (e.g. detector counts) or a
//! named coordinate basis (e.g. asinh dimension-less transformed space).
//! Import keeps the original representation; normalization only happens in the
//! derived computation layer. Values that cannot be converted stay in place as
//! [`EventValue::Literal`] with an attached diagnostic.

use serde::{Deserialize, Serialize};

/// Kind of a separately versioned artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RawEvents,
    Spillover,
    Compensation,
    Transform,
    GatePolygon,
    GateTree,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::RawEvents => "raw_events",
            ArtifactKind::Spillover => "spillover",
            ArtifactKind::Compensation => "compensation",
            ArtifactKind::Transform => "transform",
            ArtifactKind::GatePolygon => "gate_polygon",
            ArtifactKind::GateTree => "gate_tree",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_events" => ArtifactKind::RawEvents,
            "spillover" => ArtifactKind::Spillover,
            "compensation" => ArtifactKind::Compensation,
            "transform" => ArtifactKind::Transform,
            "gate_polygon" => ArtifactKind::GatePolygon,
            "gate_tree" => ArtifactKind::GateTree,
            _ => return None,
        })
    }
}

/// Stable identity reference: the content id *is* the immutable version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ArtifactRef {
    pub kind: ArtifactKind,
    pub id: String,
}

impl std::fmt::Display for ArtifactRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.id)
    }
}

/// Pointer to the original bytes a fact came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct SourceRef {
    /// Original file name as presented by the platform (no renaming on import).
    pub file_name: String,
    pub media_type: String,
    pub byte_length: u64,
    pub sha256: String,
    /// Keyword / section inside the source, e.g. `$SPILLOVER` or `DATA`.
    pub locator: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChannelRole {
    ForwardScatter,
    SideScatter,
    Time,
    Fluorescence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub marker: Option<String>,
    /// Physical unit of raw detector values, normally `counts`.
    pub unit: String,
    pub role: ChannelRole,
    pub range_min: f64,
    pub range_max: f64,
    /// Original representation, e.g. the FCS `$PnR` keyword value.
    pub raw_repr: String,
}

/// Event value preserving the original representation. [`EventValue::Literal`]
/// holds values that could not be normalized (overflow marker, NaN sentinel...)
/// without discarding them for plotting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventValue {
    Number(f64),
    Literal { literal: String, diagnostic: String },
}

impl EventValue {
    pub fn as_number(&self) -> Option<f64> {
        match self {
            EventValue::Number(v) if v.is_finite() => Some(*v),
            _ => None,
        }
    }
    pub fn label(&self) -> String {
        match self {
            EventValue::Number(v) if v.is_finite() => format!("{v}"),
            EventValue::Number(v) if v.is_nan() => "NaN".to_string(),
            EventValue::Number(_) => "Inf".to_string(),
            EventValue::Literal { literal, .. } => literal.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    /// Stable acquisition order index (0-based), never renumbered.
    pub index: u32,
    pub values: Vec<EventValue>,
}

/// 1) Original channel values, versioned separately from all interpretation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvents {
    pub source: SourceRef,
    pub channels: Vec<Channel>,
    pub events: Vec<RawEvent>,
}

/// 2) Spillover metadata as stated by the platform (FCS keyword etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub source: SourceRef,
    pub raw_keyword: String,
    /// Exact original keyword value; never overwritten by parsed numbers.
    pub raw_keyword_value: String,
    /// Detector rows, in matrix order.
    pub detectors: Vec<String>,
    /// Fluorochrome columns, in matrix order.
    pub fluorochromes: Vec<String>,
    /// S where `detector_counts = S * fluorochrome_counts`; unit `fraction`.
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub note: Option<String>,
}

/// 3) Compensation matrix adopted for a derivation, pinned to spillover source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compensation {
    pub derived_from: ArtifactRef,
    pub detectors: Vec<String>,
    /// Adopted spillover matrix S. The inverse S^-1 is never stored as fact;
    /// it is (or is refused to be) derived in the computation layer.
    pub matrix: Vec<Vec<f64>>,
    pub orientation: String,
    pub unit: String,
}

/// 4) Transform parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParams {
    pub method: String,
    pub channels: Vec<String>,
    pub derived_from: ArtifactRef,
    /// Input unit of compensated detector values.
    pub input_unit: String,
    /// Named output coordinate basis.
    pub output_basis: String,
    /// Top of scale T in compensated counts.
    pub top_scale_counts: f64,
    /// Linearization width c in compensated counts.
    pub linear_width_counts: f64,
}

/// Coordinate system of a 2-D gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAxis {
    pub channel: String,
    /// Coordinate basis label, must match the transform output basis.
    pub basis: String,
    pub unit_or_basis: String,
}

/// 5) Gate polygon, versioned independently of parent-child relationships.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatePolygon {
    /// Stable lineage identity across renames and vertex edits.
    pub gate_id: String,
    pub name: String,
    pub x: GateAxis,
    pub y: GateAxis,
    /// Vertices in transformed coordinates, stored order is significant.
    pub polygon: Vec<[f64; 2]>,
    /// Absolute tolerance (in gate coordinates) for boundary classification.
    pub boundary_epsilon: f64,
    pub transform_ref: ArtifactRef,
}

/// One parent-child edge. `parent` is `"root"` or another gate id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEdge {
    pub gate_id: String,
    pub parent: String,
    /// Stable display order among siblings.
    pub order: u32,
}

/// 6) Parent-child relationships, versioned independently of polygons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateTree {
    pub root_name: String,
    pub edges: Vec<GateEdge>,
}

/// Versioned artifact envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Artifact {
    RawEvents(RawEvents),
    Spillover(SpilloverMeta),
    Compensation(Compensation),
    Transform(TransformParams),
    GatePolygon(GatePolygon),
    GateTree(GateTree),
}

impl Artifact {
    pub fn kind(&self) -> ArtifactKind {
        match self {
            Artifact::RawEvents(_) => ArtifactKind::RawEvents,
            Artifact::Spillover(_) => ArtifactKind::Spillover,
            Artifact::Compensation(_) => ArtifactKind::Compensation,
            Artifact::Transform(_) => ArtifactKind::Transform,
            Artifact::GatePolygon(_) => ArtifactKind::GatePolygon,
            Artifact::GateTree(_) => ArtifactKind::GateTree,
        }
    }
}

/// Non-fatal or blocking diagnostic produced while importing / deriving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String, // error | warning
    pub at: String,       // structured locator, e.g. "events[3].FITC-H"
    pub message: String,
    /// Original token kept for traceability.
    pub raw: Option<String>,
}

/// Population result for one gate at one pinned input set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopulationResult {
    pub population: String,
    pub gate_id: Option<String>,
    /// Pinned parent result: percentage denominator is frozen to this version.
    pub parent_result_id: Option<String>,
    pub parent_population: String,
    pub count: u64,
    /// Parent count at the time the result was produced (denominator).
    pub parent_count: u64,
    pub percentage: Option<f64>,
    /// Acquistion indices classified inside.
    pub inside: Vec<u32>,
    /// Indices exactly on the boundary (under `boundary_epsilon`).
    pub boundary: Vec<u32>,
    /// Indices unusable for this gate (literal / missing axis value).
    pub unusable: Vec<u32>,
    /// Event-level membership labels kept for the scatter view.
    pub membership: Vec<EventMembership>,
    pub status: String, // current | stale
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMembership {
    pub event_index: u32,
    pub label: String, // inside | outside | boundary | unusable
}

/// Complete pinned snapshot of everything a derivation depended on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct InputsRef {
    pub raw: ArtifactRef,
    pub spillover: ArtifactRef,
    pub compensation: ArtifactRef,
    pub transform: ArtifactRef,
    pub gate_tree: ArtifactRef,
    /// Gate polygon versions actually used, keyed by stable gate id.
    pub gates: std::collections::BTreeMap<String, ArtifactRef>,
    /// Parent result version pinned per gate id.
    pub parent_results: std::collections::BTreeMap<String, String>,
    /// Accepted decisions (e.g. boundary policy) factored into the result.
    pub decisions: Vec<String>,
}
