use serde::{Deserialize, Serialize};

/// Coordinate basis a quantity is expressed in. Every derived layer records
/// which basis it works in so the UI can always show where a number came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Raw channel reading as decoded from the FCS event matrix.
    RawSignal,
    /// Linearized, scale-corrected physical signal (gain/range applied).
    ScaleLinear,
    /// Compensation-corrected linear signal.
    Compensated,
    /// Non-linear display transform (asinh/linear).
    Transformed,
}

/// A channel/parameter descriptor. Units live on the descriptor and are never
/// dropped during import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub index: usize,
    pub name: String,
    /// $PnN short name from FCS.
    pub short_name: String,
    pub unit: String,
    pub basis: Basis,
    /// $PnG gain and $PnR range; preserve original representation.
    pub gain: f64,
    pub range: f64,
}

/// A single stored event value. Unconvertible values (NaN text, out of range)
/// stay in place as `Raw::Unconvertible` with a diagnostic instead of being
/// dropped or fabricated for plotting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Raw {
    /// Exact original representation from FCS (decimal text), plus parsed f64.
    Number { text: String, f: f64 },
    Unconvertible { raw: String, diagnostic: String },
}

impl Raw {
    pub fn number(f: f64) -> Raw {
        Raw::Number {
            text: format_float(f),
            f,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Raw::Number { f, .. } if f.is_finite() => Some(*f),
            _ => None,
        }
    }
    pub fn display(&self) -> String {
        match self {
            Raw::Number { text, .. } => text.clone(),
            Raw::Unconvertible { raw, .. } => raw.clone(),
        }
    }
}

pub(crate) fn format_float(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e15 {
        format!("{:.1}", f)
    } else {
        let s = format!("{:.6}", f);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Versioned original channel values imported from one FCS file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsDoc {
    pub fcs_blob_id: String,
    pub fcs_file_name: String,
    pub channels: Vec<Channel>,
    /// rows[event_index][channel_index]; original representation preserved.
    pub rows: Vec<Vec<Raw>>,
    pub diagnostics: Vec<ValueDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueDiagnostic {
    pub event_index: usize,
    pub channel: String,
    pub code: String,
    pub message: String,
}

/// Spillover metadata *as reported by the platform* — separated from the
/// compensation matrix that is actually applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverDoc {
    pub source: String,
    pub detector_names: Vec<String>,
    pub fluorochromes: Vec<String>,
    /// SPILLOVER matrix verbatim (rows detectors, cols fluorochromes).
    pub matrix: Vec<Vec<f64>>,
    pub single_stain_refs: Vec<String>,
}

/// The compensation matrix selected for application, versioned independently
/// of the spillover metadata it was derived from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompensationDoc {
    pub channels: Vec<String>,
    /// S rows; compensated = S^{-1} * raw (when invertible).
    pub matrix: Vec<Vec<f64>>,
    pub derived_from: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformKind {
    Linear,
    Asinh,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTransform {
    pub channel: String,
    pub kind: TransformKind,
    /// asinh: asinh(x / cofactor); cofactor carries the channel unit.
    pub cofactor: f64,
    pub unit: String,
    pub output_unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformDoc {
    pub channels: Vec<ChannelTransform>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateShape {
    Polygon,
    Rectangle,
}

/// Vertex with explicit coordinate basis. Coordinates are *not* moved between
/// bases silently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vertex {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDoc {
    /// Stable gate id, survives renames.
    pub gate_id: String,
    pub name: String,
    pub shape: GateShape,
    pub x_channel: String,
    pub y_channel: String,
    pub x_basis: Basis,
    pub y_basis: Basis,
    pub vertices: Vec<Vertex>,
}

/// Parent/child relations versioned separately from gate geometry and names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageDoc {
    /// (child_gate_id, parent_gate_id); root populations have parent `None`.
    pub edges: Vec<(String, Option<String>)>,
}
