use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Json = serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInput {
    pub expected_head_seq: Option<i64>,
    pub event_id: Option<String>,
    pub event_type: String,
    pub branch_id: Option<String>,
    pub payload: Json,
    pub sources: Option<Vec<SourceRef>>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub kind: String,
    pub id: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub event_id: String,
    pub event_type: String,
    pub branch_id: String,
    pub parent_event_id: Option<String>,
    pub seq_in_branch: i64,
    pub payload: Json,
    pub sources: Json,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent_branch_id: Option<String>,
    pub fork_seq: i64,
    pub head_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialVersion {
    pub kind: String,
    pub id: String,
    pub version: u32,
    pub event_id: String,
    pub payload: Json,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowEvent {
    pub id: String,
    pub channels: BTreeMap<String, ChannelValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelValue {
    pub value: Option<f64>,
    pub representation: String,
    pub unit: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformedPoint {
    pub event_id: String,
    pub values: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopulationResult {
    pub id: String,
    pub version: u32,
    pub gate_id: String,
    pub parent_population_id: Option<String>,
    pub parent_gate_version: Option<u32>,
    pub transform_version: u32,
    pub included_event_ids: Vec<String>,
    pub denominator_event_ids: Vec<String>,
    pub numerator: usize,
    pub denominator: usize,
    pub percent: f64,
    pub percent_unit: String,
    pub boundary_policy: String,
    pub candidates: Vec<PopulationResult>,
    pub valid: bool,
    pub stale: bool,
    pub reason: Option<String>,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgment {
    pub decision_id: String,
    pub accepted: bool,
    pub rationale: String,
    pub event_id: String,
    pub conflicted: bool,
}

#[derive(Debug, Clone)]
pub struct GateState {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub parent_gate_id: Option<String>,
    pub x_channel: String,
    pub y_channel: String,
    pub transform_id: String,
    pub polygon: Vec<[f64; 2]>,
    pub boundary_policy: String,
    pub event_id: String,
    pub relation_event_id: String,
    pub relation_version: u32,
}

#[derive(Debug, Clone)]
pub struct ReplayState {
    pub branches: BTreeMap<String, Branch>,
    pub branch_events: BTreeMap<String, Vec<Event>>,
    pub events: Vec<Event>,
    pub raw_versions: Vec<MaterialVersion>,
    pub spillover_versions: Vec<MaterialVersion>,
    pub compensation_versions: Vec<MaterialVersion>,
    pub transform_versions: Vec<MaterialVersion>,
    pub gate_polygons: BTreeMap<String, Vec<MaterialVersion>>,
    pub gate_relations: Vec<MaterialVersion>,
    pub populations: BTreeMap<String, Vec<PopulationResult>>,
    pub judgments: BTreeMap<String, Vec<Judgment>>,
    pub diagnostics: Vec<Json>,
}

impl ReplayState {
    pub fn new() -> Self {
        Self {
            branches: BTreeMap::new(),
            branch_events: BTreeMap::new(),
            events: Vec::new(),
            raw_versions: Vec::new(),
            spillover_versions: Vec::new(),
            compensation_versions: Vec::new(),
            transform_versions: Vec::new(),
            gate_polygons: BTreeMap::new(),
            gate_relations: Vec::new(),
            populations: BTreeMap::new(),
            judgments: BTreeMap::new(),
            diagnostics: Vec::new(),
        }
    }
}

