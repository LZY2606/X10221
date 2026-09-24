//! 追加事件：所有状态变化都落为事件；身份由规范化内容哈希决定。

use serde::{Deserialize, Serialize};

use crate::domain::{
    ChannelDataset, CompensationProposal, GateVersion, PopulationResult, SpilloverSpec,
    TransformVersion, TreeEdgeVersion,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewAction {
    Accept,
    Reject,
    Adjust,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubjectKind {
    Compensation,
    Population,
    Transform,
    Gate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaleMarkPayload {
    pub population_id: String,
    pub version: u32,
    pub reason: String,
    pub source_kind: String,
    pub source_id: String,
    pub source_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventPayload {
    BranchCreated {
        branch_id: String,
        forked_from_branch: Option<String>,
        fork_event_id: Option<String>,
        title: String,
    },
    BlobImported {
        blob_id: String,
        media_type: String,
        byte_length: usize,
    },
    DatasetRegistered {
        dataset: ChannelDataset,
    },
    SpilloverRegistered {
        spec: SpilloverSpec,
    },
    CompensationProposed {
        proposal: CompensationProposal,
    },
    TransformSet {
        transform: TransformVersion,
    },
    GateSet {
        gate: GateVersion,
    },
    TreeEdgeSet {
        edge: TreeEdgeVersion,
    },
    PopulationComputed {
        result: PopulationResult,
    },
    ReviewDecision {
        decision_id: String,
        subject_kind: SubjectKind,
        subject_id: String,
        subject_version: u32,
        action: ReviewAction,
        /// 接受补偿提案时显式选择的候选 id；无法唯一决定时必须人工指定。
        candidate_id: Option<String>,
        note: String,
    },
    DecisionReverted {
        decision_id: String,
        note: String,
    },
    StaleMarks {
        marks: Vec<StaleMarkPayload>,
    },
    ArchiveImported {
        export_id: String,
    },
}

impl EventPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            EventPayload::BranchCreated { .. } => "branch_created",
            EventPayload::BlobImported { .. } => "blob_imported",
            EventPayload::DatasetRegistered { .. } => "dataset_registered",
            EventPayload::SpilloverRegistered { .. } => "spillover_registered",
            EventPayload::CompensationProposed { .. } => "compensation_proposed",
            EventPayload::TransformSet { .. } => "transform_set",
            EventPayload::GateSet { .. } => "gate_set",
            EventPayload::TreeEdgeSet { .. } => "tree_edge_set",
            EventPayload::PopulationComputed { .. } => "population_computed",
            EventPayload::ReviewDecision { .. } => "review_decision",
            EventPayload::DecisionReverted { .. } => "decision_reverted",
            EventPayload::StaleMarks { .. } => "stale_marks",
            EventPayload::ArchiveImported { .. } => "archive_imported",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub branch_id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub payload: EventPayload,
    pub actor: String,
    /// 仅用于展示顺序；不参与身份哈希（身份只依赖内容与链）。
    pub created_at_unix: i64,
}

/// 事件身份：分支内序号 + 父事件 + 载荷 + 操作者。
pub fn event_identity(
    branch_id: &str,
    seq: i64,
    parent_id: &Option<String>,
    payload: &EventPayload,
    actor: &str,
) -> String {
    #[derive(Serialize)]
    struct IdInput<'a> {
        branch_id: &'a str,
        seq: i64,
        parent_id: &'a Option<String>,
        payload: &'a EventPayload,
        actor: &'a str,
    }
    crate::canonical::digest_hex(&IdInput {
        branch_id,
        seq,
        parent_id,
        payload,
        actor,
    })
}
