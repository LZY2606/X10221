use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    let digest = h.finalize();
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}_{}", prefix, &hex[..16])
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RawChannels,
    SpilloverMeta,
    CompensationMatrix,
    TransformParams,
    GatePolygon,
    GateHierarchy,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RawChannels => "raw_channels",
            Self::SpilloverMeta => "spillover_meta",
            Self::CompensationMatrix => "compensation_matrix",
            Self::TransformParams => "transform_params",
            Self::GatePolygon => "gate_polygon",
            Self::GateHierarchy => "gate_hierarchy",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_channels" => Self::RawChannels,
            "spillover_meta" => Self::SpilloverMeta,
            "compensation_matrix" => Self::CompensationMatrix,
            "transform_params" => Self::TransformParams,
            "gate_polygon" => Self::GatePolygon,
            "gate_hierarchy" => Self::GateHierarchy,
            _ => return None,
        })
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Artifact {
    pub id: String,
    pub stream: String,
    pub kind: String,
    pub version: i64,
    pub payload: Value,
    pub raw_repr: String,
    pub units: Option<String>,
    pub basis: Option<String>,
    pub created_event: String,
    pub created_seq: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Event {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Derived {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub branch_id: String,
    pub deps: Value,
    pub payload: Value,
    pub created_event: String,
    pub created_seq: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub status: String,
    pub parent: Option<String>,
    pub created_seq: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Adjudication {
    pub id: String,
    pub target: String,
    pub decision: String,
    pub params: Value,
    pub branch_id: String,
    pub supersedes: Option<String>,
    pub active: bool,
    pub event_id: String,
    pub created_seq: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Draft {
    pub id: String,
    pub name: String,
    pub version: i64,
    pub state: Value,
    pub updated_event: String,
    pub created_seq: i64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Diagnostic {
    pub id: String,
    pub subject: String,
    pub severity: String,
    pub message: String,
    pub payload: Value,
    pub created_seq: i64,
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
