use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Id = String;

/// 领域量：值 + 单位 + 坐标基准 + 原始表示。导入层保留 raw，派生层才规范化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
    pub basis: String,
    pub raw: String,
}

/// 分开版本化的六类对象。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RawChannels,
    SpilloverMeta,
    CompMatrix,
    TransformParams,
    GatePolygon,
    GateHierarchy,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RawChannels => "raw_channels",
            Self::SpilloverMeta => "spillover_meta",
            Self::CompMatrix => "comp_matrix",
            Self::TransformParams => "transform_params",
            Self::GatePolygon => "gate_polygon",
            Self::GateHierarchy => "gate_hierarchy",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Id,
    pub version: u64,
    pub kind: ArtifactKind,
    pub branch: String,
    pub payload: Value,
    /// 原始字节（base64），导入时原样保留，任何分支/裁定都可回读。
    pub raw_b64: Option<String>,
    pub created_by: Id,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    CaseImported,
    ArtifactCommitted,
    DerivationComputed,
    DiagnosticRaised,
    Adjudicated,
    AdjudicationUndone,
    Invalidated,
    ConflictRecorded,
    BranchCreated,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CaseImported => "case_imported",
            Self::ArtifactCommitted => "artifact_committed",
            Self::DerivationComputed => "derivation_computed",
            Self::DiagnosticRaised => "diagnostic_raised",
            Self::Adjudicated => "adjudicated",
            Self::AdjudicationUndone => "adjudication_undone",
            Self::Invalidated => "invalidated",
            Self::ConflictRecorded => "conflict_recorded",
            Self::BranchCreated => "branch_created",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "case_imported" => Self::CaseImported,
            "artifact_committed" => Self::ArtifactCommitted,
            "derivation_computed" => Self::DerivationComputed,
            "diagnostic_raised" => Self::DiagnosticRaised,
            "adjudicated" => Self::Adjudicated,
            "adjudication_undone" => Self::AdjudicationUndone,
            "invalidated" => Self::Invalidated,
            "conflict_recorded" => Self::ConflictRecorded,
            "branch_created" => Self::BranchCreated,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub id: Id,
    pub ts: String,
    pub actor: String,
    pub branch: String,
    pub kind: EventKind,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagStatus {
    Open,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub id: Id,
    pub code: String,
    pub subject: String,
    pub detail: Value,
    /// 全部合法候选（含最终被采纳与未采纳的）都留档。
    pub candidates: Vec<Value>,
    pub status: DiagStatus,
    pub resolution: Option<Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DerivedStatus {
    Valid,
    Stale,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derived {
    pub id: Id,
    pub version: u64,
    pub kind: String,
    pub branch: String,
    /// 钉住的依赖版本：(artifact_or_derived_id, version)。
    pub inputs: Vec<(Id, u64)>,
    pub status: DerivedStatus,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub id: Id,
    pub artifact: Id,
    pub branch: String,
    pub base_version: u64,
    pub head_version: u64,
    /// 双方差异（机器可读）。
    pub diff: Vec<Value>,
    pub attempted_payload: Value,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invalidation {
    pub id: Id,
    pub cause_event: Id,
    pub affected: Vec<Id>,
    pub scope: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub case: Option<Value>,
    pub artifacts: BTreeMap<(Id, u64), Artifact>,
    pub heads: BTreeMap<(Id, String), u64>,
    pub derived: BTreeMap<(Id, u64), Derived>,
    pub derived_heads: BTreeMap<(Id, String), u64>,
    pub diagnostics: BTreeMap<Id, Diagnostic>,
    pub conflicts: Vec<Conflict>,
    pub invalidations: Vec<Invalidation>,
    pub branches: Vec<String>,
}

impl State {
    pub fn head(&self, id: &str, branch: &str) -> Option<&Artifact> {
        self.heads
            .get(&(id.to_string(), branch.to_string()))
            .and_then(|v| self.artifacts.get(&(id.to_string(), *v)))
    }
    pub fn artifact(&self, id: &str, version: u64) -> Option<&Artifact> {
        self.artifacts.get(&(id.to_string(), version))
    }
    pub fn derived_head(&self, id: &str, branch: &str) -> Option<&Derived> {
        self.derived_heads
            .get(&(id.to_string(), branch.to_string()))
            .and_then(|v| self.derived.get(&(id.to_string(), *v)))
    }
}
