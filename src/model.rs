use serde::{Deserialize, Serialize};

pub const ARCHIVE_FORMAT: &str = "flow-lineage-archive";
pub const ARCHIVE_VERSION: u32 = 1;

/// Boundary tolerance (in ADC counts of the compensated display space) within
/// which an event is treated as lying *on* a gate edge and therefore cannot be
/// uniquely assigned without a human adjudication.
pub const BOUNDARY_EPS: f64 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    RawChannels,
    SpilloverMeta,
    CompensationMatrix,
    TransformParams,
    GatePolygon,
    GateHierarchy,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::RawChannels => "raw_channels",
            SourceKind::SpilloverMeta => "spillover_meta",
            SourceKind::CompensationMatrix => "compensation_matrix",
            SourceKind::TransformParams => "transform_params",
            SourceKind::GatePolygon => "gate_polygon",
            SourceKind::GateHierarchy => "gate_hierarchy",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_channels" => SourceKind::RawChannels,
            "spillover_meta" => SourceKind::SpilloverMeta,
            "compensation_matrix" => SourceKind::CompensationMatrix,
            "transform_params" => SourceKind::TransformParams,
            "gate_polygon" => SourceKind::GatePolygon,
            "gate_hierarchy" => SourceKind::GateHierarchy,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SourceRow {
    pub id: String,
    pub version: i64,
    pub seq: i64,
    pub kind: SourceKind,
    pub raw_payload: String,
    pub normalized: Option<serde_json::Value>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct DerivedRow {
    pub result_id: String,
    pub branch_id: String,
    pub seq: i64,
    pub kind: String,
    pub status: String, // valid | stale | contested
    pub inputs: serde_json::Value,
    pub value: Option<serde_json::Value>,
}

#[derive(Clone, Debug)]
pub struct CandidateRow {
    pub set_id: String,
    pub branch_id: String,
    pub created_seq: i64,
    pub kind: String,
    pub context: serde_json::Value,
    pub options: Vec<serde_json::Value>,
    pub status: String, // pending | adjudicated
    pub decision: Option<serde_json::Value>,
    pub decision_seq: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct EventRow {
    pub seq: i64,
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
