//! Core domain model: stable identities, versioned objects, units.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Entity kinds that are versioned *separately* from each other.
pub const KIND_RAW_EVENTS: &str = "raw_events";
pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_COMP_MATRIX: &str = "comp_matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_GATE_TREE: &str = "gate_tree";
pub const KIND_POPULATION: &str = "population";
pub const KIND_DRAFT: &str = "draft";

pub const ALL_KINDS: [&str; 8] = [
    KIND_RAW_EVENTS,
    KIND_SPILLOVER,
    KIND_COMP_MATRIX,
    KIND_TRANSFORM,
    KIND_GATE,
    KIND_GATE_TREE,
    KIND_POPULATION,
    KIND_DRAFT,
];

/// Content-addressed version hash: stable identity for one version of one object.
pub fn content_hash(kind: &str, logical_id: &str, version: i64, payload: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(payload).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"|");
    h.update(logical_id.as_bytes());
    h.update(b"|");
    h.update(version.to_string().as_bytes());
    h.update(b"|");
    h.update(canonical.as_bytes());
    hex(&h.finalize())
}

/// Stable identity for an event in the journal.
pub fn event_id(seq: i64, kind: &str, payload: &serde_json::Value) -> String {
    let canonical = serde_json::to_string(payload).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(b"evt|");
    h.update(seq.to_string().as_bytes());
    h.update(b"|");
    h.update(kind.as_bytes());
    h.update(b"|");
    h.update(canonical.as_bytes());
    format!("evt-{}", &hex(&h.finalize())[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A versioned object row as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedObject {
    pub kind: String,
    pub id: String,
    pub version: i64,
    pub content_hash: String,
    pub payload: serde_json::Value,
    pub supersedes: Option<String>,
    pub seq: i64,
}

/// A domain quantity: value + unit / coordinate basis. Raw representation kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quantity {
    /// Original token exactly as imported (never rewritten).
    pub raw: String,
    /// Normalized numeric value, computed only in the derived layer.
    pub value: Option<f64>,
    pub unit: String,
    /// Coordinate basis, e.g. channel name "FL1-A".
    pub basis: String,
    /// Diagnostic when the value could not be normalized; value stays in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

/// Population status in the provenance graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PopStatus {
    Current,
    Stale,
    Blocked,
    Ambiguous,
}

impl PopStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PopStatus::Current => "current",
            PopStatus::Stale => "stale",
            PopStatus::Blocked => "blocked",
            PopStatus::Ambiguous => "ambiguous",
        }
    }
}
