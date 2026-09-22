//! 机器可读取事件。事件 append-only；每个事件带 refs（依赖版本钉住）与 result（当时结论）。
//! 关键判断依据以显式 kind 保存：
//! - compensation.inversion-refused：近奇异矩阵不得静默求逆，附 kappa 与全部候选
//! - appraisal.committed：群体百分比的分母钉住父群版本 denominator_ref
//! - appraisal.stale-marked：上游修订后下游只标记过期，不覆盖旧结论

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EventKind {
    CaseLoaded,
    MaterialsIngested,
    ImportRejected,
    CompensationAssessed,
    InversionRefused,
    CandidateAdopted,
    CandidateRejected,
    TransformRevised,
    SourcePatched,
    GateRenamed,
    GateRevised,
    AppraisalCommitted,
    StaleMarked,
    BranchCreated,
    DecisionReverted,
    ConflictRejected,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::CaseLoaded => "case.loaded",
            EventKind::MaterialsIngested => "materials.ingested",
            EventKind::ImportRejected => "import.rejected",
            EventKind::CompensationAssessed => "compensation.assessed",
            EventKind::InversionRefused => "compensation.inversion-refused",
            EventKind::CandidateAdopted => "candidate.adopted",
            EventKind::CandidateRejected => "candidate.rejected",
            EventKind::TransformRevised => "transform.revised",
            EventKind::SourcePatched => "source.patched",
            EventKind::GateRenamed => "gate.renamed",
            EventKind::GateRevised => "gate.revised",
            EventKind::AppraisalCommitted => "appraisal.committed",
            EventKind::StaleMarked => "appraisal.stale-marked",
            EventKind::BranchCreated => "branch.created",
            EventKind::DecisionReverted => "decision.reverted",
            EventKind::ConflictRejected => "conflict.rejected",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "case.loaded" => EventKind::CaseLoaded,
            "materials.ingested" => EventKind::MaterialsIngested,
            "import.rejected" => EventKind::ImportRejected,
            "compensation.assessed" => EventKind::CompensationAssessed,
            "compensation.inversion-refused" => EventKind::InversionRefused,
            "candidate.adopted" => EventKind::CandidateAdopted,
            "candidate.rejected" => EventKind::CandidateRejected,
            "transform.revised" => EventKind::TransformRevised,
            "source.patched" => EventKind::SourcePatched,
            "gate.renamed" => EventKind::GateRenamed,
            "gate.revised" => EventKind::GateRevised,
            "appraisal.committed" => EventKind::AppraisalCommitted,
            "appraisal.stale-marked" => EventKind::StaleMarked,
            "branch.created" => EventKind::BranchCreated,
            "decision.reverted" => EventKind::DecisionReverted,
            "conflict.rejected" => EventKind::ConflictRejected,
            _ => return None,
        })
    }
}

/// 一个分支视图钉住的版本引用。None 表示该类材料在本视图尚未引入。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Refs {
    pub raw_events: Option<String>,
    pub spillover_meta: Option<String>,
    pub compensation: Option<String>,
    pub transform: Option<String>,
    pub gates: std::collections::BTreeMap<String, String>,
    pub hierarchy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredEvent {
    pub event_id: String,
    pub seq: i64,
    pub branch_id: String,
    pub parent_event_id: Option<String>,
    pub kind: String,
    pub payload: serde_json::Value,
    pub refs: Refs,
    pub result: serde_json::Value,
}
