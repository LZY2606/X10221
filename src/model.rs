//! Domain model.
//!
//! Every domain quantity carries a unit or a coordinate basis. Source materials
//! keep their *original representation* (raw strings for channel values);
//! normalization happens only in derived layers. Unconvertible values are never
//! dropped: they stay in place together with diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatKind {
    Events,
    Spillover,
    Matrix,
    Transform,
    Gates,
}

impl MatKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatKind::Events => "events",
            MatKind::Spillover => "spillover",
            MatKind::Matrix => "matrix",
            MatKind::Transform => "transform",
            MatKind::Gates => "gates",
        }
    }
    pub fn parse(s: &str) -> Option<MatKind> {
        Some(match s {
            "events" => MatKind::Events,
            "spillover" => MatKind::Spillover,
            "matrix" => MatKind::Matrix,
            "transform" => MatKind::Transform,
            "gates" => MatKind::Gates,
            _ => return None,
        })
    }
}

/// Channel / detector definition. `unit` is mandatory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelDef {
    pub id: String,
    pub label: String,
    pub unit: String,
}

/// One observed event. Channel values are kept as their original textual
/// representation (e.g. "1234", "OVF", "1e400"); parsing is a derived step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowEvent {
    pub id: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventsData {
    pub origin: String,
    pub coordinate_basis: String,
    pub channels: Vec<ChannelDef>,
    pub events: Vec<FlowEvent>,
}

/// Spillover metadata: S is square, rows are detectors, columns are
/// fluorochromes, diagonal is 1. Values are fractions of the column signal
/// spilling into the row detector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpilloverData {
    pub origin: String,
    pub basis: String,
    pub unit: String,
    pub channels: Vec<String>,
    pub spillover: Vec<Vec<f64>>,
    pub single_stain_controls: BTreeMap<String, String>,
}

/// A compensation matrix C applied as `compensated = C * raw` on the listed
/// channels. Provenance records which spillover version it was derived from
/// (or "manual" for a hand-supplied matrix).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatrixData {
    pub origin: String,
    pub basis: String,
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub derived_from: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AsinhParams {
    pub m: f64,
    pub t: f64,
    pub a: f64,
    pub w: f64,
}

impl AsinhParams {
    /// Documented transform used by the bundled case:
    /// `y = (t / m) * asinh((x - a) / w)`.
    /// Input unit: linear compensated detector values. Output unit:
    /// dimensionless asinh-transformed display units.
    pub fn apply(&self, x: f64) -> f64 {
        (self.t / self.m) * ((x - self.a) / self.w).asinh()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransformData {
    pub origin: String,
    pub input_basis: String,
    pub output_unit: String,
    /// Map of channel id -> parameter set; absent channel means untransformed.
    pub params: BTreeMap<String, AsinhParams>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AxisBasis {
    pub channel: String,
    /// Pinned coordinate basis, e.g. "raw:ADC counts", "compensated:matrix#v1",
    /// "transformed:asinh#v2". Version pinning is what keeps gating results
    /// stable when upstream materials are revised.
    pub basis: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum GateShape {
    Rect {
        x: AxisBasis,
        y: AxisBasis,
        xmin: f64,
        xmax: f64,
        ymin: f64,
        ymax: f64,
    },
    Polygon {
        x: AxisBasis,
        y: AxisBasis,
        points: Vec<[f64; 2]>,
    },
}

impl GateShape {
    pub fn axes(&self) -> [&AxisBasis; 2] {
        match self {
            GateShape::Rect { x, y, .. } => [x, y],
            GateShape::Polygon { x, y, .. } => [x, y],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub id: String,
    /// Human label. Renaming the label never changes the stable id nor the
    /// parent-child lineage.
    pub name: String,
    #[serde(flatten)]
    pub shape: GateShape,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatesData {
    pub origin: String,
    pub gates: Vec<Gate>,
    /// gate id -> parent gate id (None means the gate hangs off the root
    /// event population).
    pub parents: BTreeMap<String, Option<String>>,
}

/// Result of attempting to convert one original channel value into a finite
/// number usable by a derived layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NumericValue {
    Finite { value: f64 },
    Unconvertible { raw: String, reason: String },
}

impl NumericValue {
    /// Parse while preserving the original representation. Tokens that do not
    /// parse and parses that yield non-finite numbers stay in place as
    /// diagnostics instead of being silently dropped or clipped.
    pub fn from_raw(raw: &str) -> NumericValue {
        match raw.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => NumericValue::Finite { value: v },
            Ok(v) => NumericValue::Unconvertible {
                raw: raw.to_string(),
                reason: format!("parsed value is not finite: {}", v),
            },
            Err(_) => NumericValue::Unconvertible {
                raw: raw.to_string(),
                reason: "not a finite numeric token".to_string(),
            },
        }
    }
    pub fn finite(&self) -> Option<f64> {
        match self {
            NumericValue::Finite { value } => Some(*value),
            _ => None,
        }
    }
}
