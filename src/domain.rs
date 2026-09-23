//! Domain types for the flow-cytometry lineage archive.
//!
//! Sources and interpretations are kept separate: raw channel values,
//! spillover metadata, compensation matrices, transform parameters, gate
//! polygons and parent/child relations all carry their own stable identities
//! and are versioned independently.

use serde::{Deserialize, Serialize};

/// A raw channel declaration. `unit` and the declared ADC range are kept with
/// the channel so every domain quantity carries its unit / coordinate basis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub id: String,
    pub name: String,
    pub detector: String,
    pub fluorophore: Option<String>,
    pub unit: String,
    pub range_min: f64,
    pub range_max: f64,
}

/// One observed raw value. The original textual representation is always
/// retained; a value that cannot be converted stays in place with a diagnostic
/// instead of being deleted for plotting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelValue {
    pub channel_id: String,
    pub raw_text: String,
    pub numeric: Option<f64>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventRecord {
    pub id: String,
    pub values: Vec<ChannelValue>,
}

/// Spillover metadata describes how the matrix was derived (single-stain
/// controls). It is stored separately from the numeric matrix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub id: String,
    pub description: String,
    /// Matrix channel labels in row/column order.
    pub labels: Vec<String>,
    /// Single-stain control references: channel id -> control event ids.
    pub controls: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MatrixVersion {
    pub id: String,
    pub spillover_id: String,
    /// Spillover matrix S (observed = S * emitted), row-major n x n.
    pub coefficients: Vec<Vec<f64>>,
    pub basis_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub id: String,
    pub kind: String,
    pub cofactor: f64,
    pub output_unit: String,
    pub applies_to: Vec<String>,
}

/// Coordinate reference for one gate axis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AxisSpace {
    Raw,
    Compensated,
    CompensatedAsinh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisRef {
    pub channel_id: String,
    pub space: AxisSpace,
    /// Identity of the transform / matrix this axis was authored against.
    pub matrix_id: Option<String>,
    pub transform_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDef {
    pub logical_id: String,
    pub version_id: String,
    pub name: String,
    pub x: AxisRef,
    pub y: AxisRef,
    pub polygon: Vec<[f64; 2]>,
    /// How points exactly on the polygon edge are treated.
    pub boundary_rule: BoundaryRule,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BoundaryRule {
    Include,
    Exclude,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRelation {
    pub child_logical: String,
    pub parent_logical: Option<String>,
}

/// The immutable baseline package: raw material plus the first version of each
/// interpretation layer. Importing it is all-or-nothing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CasePackage {
    pub package_id: String,
    pub label: String,
    pub coordinate_convention: String,
    pub channels: Vec<ChannelSpec>,
    pub spillover: SpilloverMeta,
    pub matrix: MatrixVersion,
    pub transform: TransformParams,
    pub gates: Vec<GateDef>,
    pub relations: Vec<GateRelation>,
    pub events: Vec<EventRecord>,
}

/// One step of a scripted boundary scenario stored with the offline case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ScenarioStep {
    ReviseMatrix { matrix: MatrixVersion },
    ReviseTransform { transform: TransformParams },
    ReviseGate { gate: GateDef },
    PatchSource {
        changes: Vec<PatchChange>,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatchChange {
    pub event_id: String,
    pub channel_id: String,
    pub new_raw_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scenario {
    pub id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseFile {
    pub case_format: u32,
    pub title: String,
    pub package: CasePackage,
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
}

// ---------------------------------------------------------------------------
// Event log (append-only, branched)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EventPayload {
    PackageImported {
        package: CasePackage,
    },
    SourcePatched {
        changes: Vec<PatchChange>,
        reason: String,
    },
    MatrixRevised {
        matrix: MatrixVersion,
    },
    TransformRevised {
        transform: TransformParams,
    },
    GateRevised {
        gate: GateDef,
    },
    CompensationChosen {
        matrix_id: String,
        choice: CompensationChoiceKind,
        rationale: String,
    },
    PopulationRuled {
        gate_logical: String,
        verdict: RulingVerdict,
        rationale: String,
    },
    RulingReverted {
        annulled_event: String,
        note: String,
    },
    GateRedriven {
        gate_logical: String,
        note: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CompensationChoiceKind {
    Refuse,
    Regularized,
    Passthrough,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RulingVerdict {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_id: Option<String>,
    /// Head event the client observed when editing; used for optimistic
    /// concurrency control.
    pub base_event: Option<String>,
    pub actor: String,
    pub created_ms: i64,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent_branch: Option<String>,
    pub forked_from_event: Option<String>,
}
