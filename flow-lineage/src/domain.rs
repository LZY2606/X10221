//! Domain model for the flow cytometry boundary chamber.
//!
//! The pipeline is: raw FCS events -> spillover metadata -> compensation
//! matrix -> per-channel transform -> gate polygons (with parent/child
//! relations) -> populations whose percentages are pinned to a parent
//! population version.
//!
//! All derived quantities record the identity of every input version.

use serde::{Deserialize, Serialize};

/// Material kinds that are versioned independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Events,
    Spillover,
    Matrix,
    Transform,
    Gate,
    Tree,
}

impl Kind {
    pub fn id_prefix(self) -> &'static str {
        match self {
            Kind::Events => "evt",
            Kind::Spillover => "sp",
            Kind::Matrix => "mat",
            Kind::Transform => "xform",
            Kind::Gate => "gate",
            Kind::Tree => "tree",
        }
    }
}

/// A channel declaration. Units / coordinate basis are carried explicitly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    /// e.g. "FITC-A", raw acquisition units like "raw fluorescence count"
    #[serde(default)]
    pub detector: Option<String>,
    #[serde(default)]
    pub fluorophore: Option<String>,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventDocument {
    pub kind: &'static str,
    pub fcs_sha256: String,
    pub channels: Vec<Channel>,
    /// rows align with `channels`; `null` keeps an unconvertible value in place.
    pub values: Vec<Vec<Option<f64>>>,
    pub diagnostics: Vec<ValueDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueDiagnostic {
    pub event_index: usize,
    pub channel: String,
    pub raw: String,
    pub reason: String,
}

/// Spillover as reported by the instrument / single-stain controls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverDocument {
    pub kind: &'static str,
    pub channels: Vec<String>,
    /// Fraction of channel i signal spilling into channel j.
    pub spillover: Vec<Vec<f64>>,
    #[serde(default)]
    pub source: String,
}

/// A compensation matrix proposal (typically inverse spillover).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixDocument {
    pub kind: &'static str,
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    /// How the matrix was produced ("spillover_inverse", "single_stain_fit",
    /// "manual", "regularized_candidate", ...).
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransformSpec {
    /// x' = (x - offset) * scale ; unit carried on output.
    Linear {
        channel: String,
        scale: f64,
        offset: f64,
        output_unit: String,
    },
    /// x' = asinh((x - offset) / cofactor) ; dimensionless scale name kept.
    Asinh {
        channel: String,
        cofactor: f64,
        offset: f64,
        output_unit: String,
    },
    /// x' = (log10(max(x, floor)) - log10(floor)) * decades
    Log10 {
        channel: String,
        floor: f64,
        decades: f64,
        output_unit: String,
    },
}

impl TransformSpec {
    pub fn channel(&self) -> &str {
        match self {
            TransformSpec::Linear { channel, .. }
            | TransformSpec::Asinh { channel, .. }
            | TransformSpec::Log10 { channel, .. } => channel,
        }
    }
    pub fn apply(&self, x: f64) -> f64 {
        match *self {
            TransformSpec::Linear { scale, offset, .. } => (x - offset) * scale,
            TransformSpec::Asinh { cofactor, offset, .. } => {
                ((x - offset) / cofactor).asinh()
            }
            TransformSpec::Log10 { floor, decades, .. } => {
                (x.max(floor).log10() - floor.log10()) * decades
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformDocument {
    pub kind: &'static str,
    pub transforms: Vec<TransformSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryPolicy {
    /// Points exactly on an edge follow the half-open edge rule (deterministic
    /// but may differ from a reviewer's intent).
    HalfOpen,
    Include,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub x_channel: String,
    pub y_channel: String,
    /// coordinate basis, e.g. "compensated:asinh" or "compensated:linear"
    pub coordinate_basis: String,
    pub vertices: Vec<[f64; 2]>,
    pub boundary_policy: BoundaryPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDocument {
    pub kind: &'static str,
    pub gate_id: String,
    pub display_name: String,
    pub polygon: Polygon,
}

/// Parent/child relations versioned separately from gate geometry.
/// `parent_gate: null` means the gate is rooted on the whole event set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeDocument {
    pub kind: &'static str,
    pub parent_of: std::collections::BTreeMap<String, Option<String>>,
}
