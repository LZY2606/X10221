use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const KIND_FCS_EVENTS: &str = "fcs_events";
pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_COMP_MATRIX: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_GATE_EDGE: &str = "gate_edge";

pub const STATUS_CURRENT: &str = "current";
pub const STATUS_STALE: &str = "stale";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntityVersion {
    pub entity_id: String,
    pub version: i64,
    pub branch: String,
    pub kind: String,
    pub payload: Value,
    pub created_event: String,
    pub created_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub event_id: String,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Population {
    pub pop_id: String,
    pub gate_entity: String,
    pub gate_version: i64,
    pub branch: String,
    pub parent_gate: Option<String>,
    pub parent_version: Option<i64>,
    pub denominator: i64,
    pub count: i64,
    pub percent: f64,
    pub status: String,
    pub dependencies: Vec<(String, i64)>,
    pub basis_event: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub subject: String,
    pub detail: Value,
    pub decision: Option<String>,
    pub decided_event: Option<String>,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub diag_id: String,
    pub event_id: String,
    pub kind: String,
    pub subject: String,
    pub detail: Value,
}
