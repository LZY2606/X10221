use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub created_event: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    pub entity_id: String,
    pub seq: i64,
    pub unit: String,
    pub basis: String,
    pub payload: serde_json::Value,
    pub created_event: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRec {
    pub seq: i64,
    pub id: String,
    pub kind: String,
    pub branch_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResultRec {
    pub id: String,
    pub branch_id: String,
    pub kind: String,
    pub subject: String,
    pub status: String,
    pub denominator_version: Option<String>,
    pub payload: serde_json::Value,
    pub created_event: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub created_event: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub branch_id: String,
    pub target_id: String,
    pub action: String,
    pub payload: serde_json::Value,
    pub undone: bool,
    pub created_event: i64,
}

pub mod kinds {
    pub const RAW_CHANNELS: &str = "raw_channels";
    pub const SPILLOVER: &str = "spillover";
    pub const COMP_MATRIX: &str = "comp_matrix";
    pub const TRANSFORM: &str = "transform";
    pub const GATE_POLYGON: &str = "gate_polygon";
    pub const GATE_TREE_EDGE: &str = "gate_tree_edge";
}

pub mod rules {
    pub const NEAR_SINGULAR: &str = "NEAR_SINGULAR_NO_SILENT_INVERSE";
    pub const DENOMINATOR_PINNED: &str = "DENOMINATOR_PINNED_TO_PARENT_VERSION";
    pub const UPSTREAM_STALE: &str = "UPSTREAM_REVISION_MARKS_STALE_NO_OVERWRITE";
    pub const EXTREME_KEPT_RAW: &str = "EXTREME_SIGNAL_KEPT_RAW";
    pub const BOUNDARY_DETERMINISTIC: &str = "BOUNDARY_POINT_DETERMINISTIC";
    pub const CONCURRENT_CONFLICT: &str = "CONCURRENT_SAME_VERSION_CONFLICT";
    pub const RENAME_NO_INVALIDATION: &str = "RENAME_GEOMETRY_UNCHANGED_NO_INVALIDATION";
}
