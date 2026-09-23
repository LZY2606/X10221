//! Domain value types.
//!
//! Rule: every domain quantity carries a unit or coordinate basis. The import
//! layer keeps the *raw representation*; normalization (e.g. channel index ->
//! name) happens only in the derived layer. Values that cannot be converted
//! stay in place with a diagnostic instead of being dropped for plotting.

use serde::{Deserialize, Serialize};

pub type FcsValue = f64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    /// Raw detector signal as stored in the FCS $DATATYPE word.
    #[serde(rename = "raw_fcs_pulse")]
    RawFcsPulse,
    /// Linear compensation output, same scale family as the input channel.
    #[serde(rename = "compensated_channel_unit")]
    Compensated,
    /// Dimensionless transformed value, basis recorded in TransformParams.
    #[serde(rename = "asinh_dimensionless")]
    AsinhDimensionless,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitBasis {
    pub unit: Unit,
    /// Free text coordinate basis, e.g. "$PnE = 0,0 (linear)".
    pub basis: String,
}

/// One coordinate of a polygon point. `basis` names the channel/transform the
/// number is expressed in, so a gate point never loses its coordinate basis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coord {
    pub basis: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: Coord,
    pub y: Coord,
}

/// Diagnostic attached to a value that could not be normalized. The raw value
/// is retained on the parent record; it is never silently removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub raw_repr: String,
}

/// NaN/extreme signals are preserved. JSON has no NaN, so the payload layer
/// encodes them explicitly instead of relying on serde's lossy behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "v")]
pub enum Signal {
    Finite(f64),
    PosInf,
    NegInf,
    NaN,
}

impl Signal {
    pub fn new(v: f64) -> Self {
        if v.is_nan() {
            Signal::NaN
        } else if v == f64::INFINITY {
            Signal::PosInf
        } else if v == f64::NEG_INFINITY {
            Signal::NegInf
        } else {
            Signal::Finite(v)
        }
    }

    pub fn finite(&self) -> Option<f64> {
        match self {
            Signal::Finite(v) => Some(*v),
            _ => None,
        }
    }
}
