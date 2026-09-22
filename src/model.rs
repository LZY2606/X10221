//! Domain model: separately-versioned provenance layers, machine-readable
//! judgment events, candidates, derived results and drafts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LAYER_KINDS: [&str; 6] = [
    "raw_channels",
    "spillover",
    "comp_matrix",
    "transform_params",
    "gate_polygon",
    "hierarchy",
];

pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, uuid::Uuid::new_v4().simple())
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn sha256_hex(text: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// One immutable version of a provenance layer. `raw_repr` keeps the
/// original imported representation verbatim; `content` is the parsed view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerVersion {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub supersedes: Option<String>,
    pub raw_repr: String,
    pub content: Value,
    pub units: Value,
    pub hash: String,
    pub created_at: i64,
    pub created_seq: i64,
}

/// Machine-readable judgment / provenance event. `seq` is the global order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    pub subject_id: Option<String>,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub base_event_seq: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub branch_id: String,
    pub label: String,
    pub payload: Value,
    pub status: String, // pending | accepted | rejected
    pub decided_event_id: Option<String>,
}

/// A derived artifact (compensated event list, gated population...).
/// `inputs` pins the exact layer/derived versions it was computed from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derived {
    pub id: String,
    pub kind: String, // compensated_events | population
    pub branch_id: String,
    pub inputs: Value, // {layer_kind: layer_id, "parent_population": derived_id, ...}
    pub payload: Value,
    pub status: String, // valid | stale | blocked | retracted | superseded
    pub stale_reason: Option<Value>,
    pub created_event_seq: i64,
    pub stale_event_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub name: String,
    pub rev: i64,
    pub content: Value,
    pub updated_at: i64,
}
