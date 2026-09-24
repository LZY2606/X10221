use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub unit: String,
    pub range: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsEvent {
    pub file: String,
    pub index: u64,
    /// Raw channel values kept verbatim as strings (original representation).
    pub values: BTreeMap<String, String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub channel: String,
    pub spill_into: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spillover {
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    #[serde(default)]
    pub source_controls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transform {
    pub kind: String,
    pub params: Value,
    #[serde(default)]
    pub basis: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDef {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scenario {
    ParamRevision {
        artifact: String,
        params: Value,
        #[serde(default)]
        comment: Option<String>,
    },
    MatrixSingular {
        artifact: String,
        matrix: Vec<Vec<f64>>,
        #[serde(default)]
        comment: Option<String>,
    },
    ExtremeSignal {
        events: Vec<FcsEvent>,
        #[serde(default)]
        comment: Option<String>,
    },
    BoundaryPoint {
        gate_id: String,
        point: [f64; 2],
        #[serde(default)]
        comment: Option<String>,
    },
    LocalSourceChange {
        control: String,
        spill_into: BTreeMap<String, f64>,
        #[serde(default)]
        comment: Option<String>,
    },
    GateRenameNested {
        gate_id: String,
        new_name: String,
        #[serde(default)]
        comment: Option<String>,
    },
    UpstreamStale {
        gate_id: String,
        new_polygon: Vec<[f64; 2]>,
        #[serde(default)]
        comment: Option<String>,
    },
    ConcurrentEdit {
        branch: String,
        #[serde(default)]
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CasePack {
    pub case_id: String,
    pub title: String,
    pub narrative: String,
    pub coordinate_basis: String,
    pub channels: Vec<Channel>,
    pub fcs_events: Vec<FcsEvent>,
    pub single_stain_controls: Vec<Control>,
    pub spillover: Spillover,
    pub transform: Transform,
    pub gates: Vec<GateDef>,
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
}

pub const CASEPACK_JSON: &str = include_str!("../casepack/boundary_chamber.json");

pub fn load_casepack() -> serde_json::Result<CasePack> {
    serde_json::from_str(CASEPACK_JSON)
}
