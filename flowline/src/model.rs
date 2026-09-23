//! Versioned source layer: raw channel values, spillover metadata,
//! compensation candidates, transform parameters, gate polygons and
//! parent/child relations are all stored and versioned independently.

use serde::{Deserialize, Serialize};

pub const STABLE_NAMESPACE: &str = "flowline-boundary-chamber-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SourceKind {
    Events,
    Spillover,
    Compensation,
    Transforms,
    Gates,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Events => "events",
            SourceKind::Spillover => "spillover",
            SourceKind::Compensation => "compensation",
            SourceKind::Transforms => "transforms",
            SourceKind::Gates => "gates",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    Polygon { points: Vec<[f64; 2]> },
    Rectangle {
        x_min: f64,
        x_max: f64,
        y_min: f64,
        y_max: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisRef {
    pub channel: String,
    /// "raw" | "asinh_compensated"
    pub space: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDef {
    pub id: String,
    pub label: String,
    pub geometry: Geometry,
    pub axes: [AxisRef; 2],
    /// gate id of parent; None = root
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParam {
    pub channel: String,
    pub family: String,
    #[serde(default)]
    pub cofactor: Option<f64>,
    #[serde(default)]
    pub scale: Option<f64>,
    pub unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompCandidate {
    pub id: String,
    pub source_spillover: String,
    pub matrix_order: Vec<String>,
    /// candidate matrix payload.  Semantics of the payload are given by
    /// `matrix_kind`: "compensation_gain" (applied directly) or
    /// "spillover_proposal" (must be inverted first).
    pub matrix: Vec<Vec<f64>>,
    pub matrix_kind: String,
    pub unit: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub case_id: String,
    pub title: String,
    pub created: String,
    pub files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub adc_max: Option<f64>,
    #[serde(default)]
    pub narrative_stations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverDoc {
    pub id: String,
    pub matrix_order: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub row_basis: Option<String>,
    #[serde(default)]
    pub column_basis: Option<String>,
    #[serde(default)]
    pub derivation: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompensationDoc {
    pub id: String,
    pub candidates: Vec<CompCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformsDoc {
    pub id: String,
    pub parameters: Vec<TransformParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatesDoc {
    pub id: String,
    pub gates: Vec<GateDef>,
}

// ---- on-disk case-pack shapes --------------------------------------------

#[derive(Deserialize)]
struct RawGeometry {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    coordinates: Option<Vec<[f64; 2]>>,
    #[serde(default)]
    x_min: Option<f64>,
    #[serde(default)]
    x_max: Option<f64>,
    #[serde(default)]
    y_min: Option<f64>,
    #[serde(default)]
    y_max: Option<f64>,
}

#[derive(Deserialize)]
struct RawGate {
    id: String,
    label: String,
    geometry: RawGeometry,
    axes: Vec<AxisRef>,
    parent: Option<String>,
}

#[derive(Deserialize)]
struct RawGatesDoc {
    id: String,
    gates: Vec<RawGate>,
}

#[derive(Deserialize)]
struct RawCompCandidate {
    id: String,
    source_spillover: String,
    matrix_order: Vec<String>,
    matrix: Vec<Vec<f64>>,
    unit: String,
    #[serde(default)]
    matrix_kind: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    expect: Option<String>,
}

#[derive(Deserialize)]
struct RawCompensationDoc {
    id: String,
    candidates: Vec<RawCompCandidate>,
}

#[derive(Deserialize)]
struct RawManifest {
    case_id: String,
    title: String,
    created: String,
    files: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    adc_max: Option<f64>,
    #[serde(default)]
    narrative_stations: Vec<String>,
}

impl GatesDoc {
    pub fn from_pack_json(s: &str) -> Result<Self, String> {
        let raw: RawGatesDoc = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let mut gates = Vec::new();
        for g in raw.gates {
            let geometry = match g.geometry.kind.as_str() {
                "polygon" => Geometry::Polygon {
                    points: g
                        .geometry
                        .coordinates
                        .ok_or("polygon missing coordinates")?,
                },
                "rectangle" => Geometry::Rectangle {
                    x_min: g.geometry.x_min.ok_or("rectangle missing x_min")?,
                    x_max: g.geometry.x_max.ok_or("rectangle missing x_max")?,
                    y_min: g.geometry.y_min.ok_or("rectangle missing y_min")?,
                    y_max: g.geometry.y_max.ok_or("rectangle missing y_max")?,
                },
                other => return Err(format!("unknown geometry type {other}")),
            };
            if g.axes.len() != 2 {
                return Err(format!("gate {} needs exactly 2 axes", g.id));
            }
            let mut axes = g.axes.into_iter();
            gates.push(GateDef {
                id: g.id,
                label: g.label,
                geometry,
                axes: [axes.next().unwrap(), axes.next().unwrap()],
                parent: g.parent,
            });
        }
        Ok(GatesDoc {
            id: raw.id,
            gates,
        })
    }
}

impl CompensationDoc {
    pub fn from_pack_json(s: &str) -> Result<Self, String> {
        let raw: RawCompensationDoc = serde_json::from_str(s).map_err(|e| e.to_string())?;
        Ok(CompensationDoc {
            id: raw.id,
            candidates: raw
                .candidates
                .into_iter()
                .map(|c| CompCandidate {
                    id: c.id,
                    source_spillover: c.source_spillover,
                    matrix_order: c.matrix_order,
                    matrix: c.matrix,
                    matrix_kind: c.matrix_kind.unwrap_or("compensation_gain"),
                    unit: c.unit,
                    note: c.note.or(c.expect.map(|e| format!("expect: {e}"))),
                })
                .collect(),
        })
    }
}

impl PackManifest {
    pub fn from_pack_json(s: &str) -> Result<Self, String> {
        let raw: RawManifest = serde_json::from_str(s).map_err(|e| e.to_string())?;
        Ok(PackManifest {
            case_id: raw.case_id,
            title: raw.title,
            created: raw.created,
            files: raw.files,
            adc_max: raw.adc_max,
            narrative_stations: raw.narrative_stations,
        })
    }
}
