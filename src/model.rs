use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseBundle {
    pub schema_version: u32,
    pub case_id: String,
    pub title: String,
    pub sources: Vec<SourceRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub branches: Vec<BranchRecord>,
    pub events: Vec<EventRecord>,
    pub judgments: Vec<JudgmentRecord>,
    pub derivations: Vec<DerivationRecord>,
    pub diagnostics: Vec<DiagnosticRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRecord {
    pub id: String,
    pub kind: String,
    pub revision: u32,
    pub name: String,
    pub content_kind: String,
    pub raw: Value,
    pub sha256: String,
    pub created_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_id: Option<String>,
    pub payload: Value,
    pub sha256: String,
    pub created_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRecord {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_branch_id: Option<String>,
    pub base_event_seq: i64,
    pub head_seq: i64,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub seq: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch_id: Option<String>,
    pub event_type: String,
    pub basis: String,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub base_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub revision: Option<u32>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgmentRecord {
    pub event_id: String,
    pub branch_id: String,
    pub decision: String,
    pub artifact_id: String,
    pub basis_code: String,
    pub rationale: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationRecord {
    pub id: String,
    pub branch_id: String,
    pub population_id: String,
    pub gate_id: String,
    pub gate_version: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_population_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_gate_version: Option<u32>,
    pub transform_refs: BTreeMap<String, String>,
    pub compensation_id: String,
    pub compensation_version: u32,
    pub source_id: String,
    pub source_revision: u32,
    pub numerator: u32,
    pub denominator: u32,
    pub percent: f64,
    pub denominator_strategy: String,
    pub denominator_version: String,
    pub event_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stale_reason: Option<String>,
    pub latest_event_seq: i64,
    pub adopted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub branch_id: Option<String>,
    pub event_id: String,
    pub code: String,
    pub severity: String,
    pub subject: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub raw_value: Option<Value>,
    pub retained: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    pub id: String,
    pub fsc: f64,
    pub ssc: f64,
    pub cd3: Option<f64>,
    pub cd4: Option<f64>,
    pub cd8: Option<f64>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub id: String,
    pub label: String,
    pub fluorophore: Option<String>,
    pub unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformSpec {
    pub channel: String,
    pub family: String,
    pub cofactor: f64,
    pub input_unit: String,
    pub output_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSpec {
    pub id: String,
    pub name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: String,
    pub polygon: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSpec {
    pub parent_gate_id: Option<String>,
    pub child_gate_id: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixCandidate {
    pub method: String,
    pub legal: bool,
    pub status: String,
    pub determinant: f64,
    pub condition_number: Option<f64>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inverse: Option<Vec<Vec<f64>>>,
}
