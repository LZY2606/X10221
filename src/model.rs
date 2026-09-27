use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A domain quantity always carries its unit and coordinate basis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
    pub basis: String,
}

/// Material kinds are versioned independently of each other.
pub const MATERIAL_KINDS: &[&str] = &[
    "raw_events",
    "spillover",
    "comp_matrix",
    "transform",
    "gate",
    "hierarchy",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Material {
    pub id: String,
    pub kind: String,
    pub version: i64,
    pub content: Value,
    /// Original representation exactly as imported; never normalized away.
    pub raw: Option<String>,
    pub source_event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derived {
    pub id: String,
    pub branch: String,
    pub version: i64,
    pub kind: String,
    /// Pinned inputs, e.g. {"gate:Live": 1, "hierarchy:main": 1}.
    pub inputs: Value,
    pub content: Value,
    pub source_event: i64,
    /// Filled by replaying invalidations; not stored on the row.
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub invalidated_by: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub branch: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: i64,
    pub branch: String,
    /// accept | reject | adjust
    pub verdict: String,
    pub target: String,
    pub payload: Value,
    pub event_id: i64,
    pub undone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub parent: Option<String>,
    pub head_event: i64,
}

/// Machine-readable judgement record attached to boundary scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgement {
    pub rule: String,
    pub status: String,
    pub affected: Vec<String>,
    pub detail: Value,
}
