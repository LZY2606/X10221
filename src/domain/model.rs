//! 机器可读事件模型。
//!
//! 六个“来源/解释”层分开版本化：
//! 1. 原始通道值（FCS 字节 + 解析结果）
//! 2. spillover 元数据
//! 3. 补偿矩阵（spillover 本身与其逆/候选的判定分开）
//! 4. 变换参数
//! 5. 门多边形（几何）
//! 6. 父子关系（门树，与几何独立，可改名/重挂）
//!
//! 所有事件只追加（append-only），任何裁定都生成新事件，旧事件永不被覆盖。

use crate::fcs::FcsFile;
use crate::domain::math::{MatrixVerdict, Point, TransformSpec};
use serde::{Deserialize, Serialize};

pub const ALGORITHM_VERSION: &str = "flowlineage-asinh-polygon-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    Sample,
    SingleStain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    ProjectCreated {
        project_id: String,
        title: String,
    },
    FcsImported {
        source_id: String,
        role: SourceRole,
        revision: u32,
        replaces: Option<String>,
        filename: String,
        sha256: String,
        byte_len: usize,
        fcs: FcsFile,
        /// 这次直接来源改动按依赖图算出的失效范围（局部源改动只影响局部）
        affected_objects: Vec<String>,
    },
    SpilloverRecorded {
        spillover_id: String,
        revision: u32,
        channel_order: Vec<String>,
        /// 行=受染通道，列=荧光通道；分数，单位 fraction
        matrix: Vec<Vec<f64>>,
        unit: String,
        basis: String,
        derived_from_sources: Vec<String>,
    },
    TransformSet {
        transform_id: String,
        revision: u32,
        channel: String,
        spec: TransformSpec,
    },
    GateGeometrySaved {
        gate_id: String,
        revision: u32,
        name: String,
        x_channel: String,
        y_channel: String,
        coordinate_basis: String,
        polygon: Vec<Point>,
    },
    GateRenamed {
        gate_id: String,
        new_name: String,
    },
    GateTreeSaved {
        tree_id: String,
        revision: u32,
        /// 子门 -> 父门（None = 挂根）
        edges: Vec<(String, Option<String>)>,
    },
    /// 近奇异/奇异矩阵的机器可读判定事件：保存输入片段 + 判定状态 + 全部候选
    CompensationAssessed {
        spillover_id: String,
        verdict: MatrixVerdict,
        affected_objects: Vec<String>,
    },
    /// 人工对补偿候选的接受/否定（可被后续 Revert 撤销）
    CompensationDecided {
        decision_id: String,
        spillover_id: String,
        outcome: DecisionOutcome,
        candidate_id: Option<String>,
        rationale: String,
    },
    /// 一次门几何修订：保存受影响（失效）对象范围，旧结论保留不覆盖
    GateRevised {
        gate_id: String,
        new_revision: u32,
        affected_objects: Vec<String>,
        reason: String,
    },
    /// 同版本并发修改：后提交者看到冲突与双方差异，事件留档
    VersionConflictRecorded {
        conflict_id: String,
        branch: String,
        expected_head: u64,
        actual_head: u64,
        attempted_action: String,
        left_draft: String,
        right_draft: String,
        diff: Vec<String>,
    },
    /// 分叉：对同一份材料重新解释
    ForkCreated {
        new_branch: String,
        display_name: String,
        forked_from_seq: u64,
    },
    /// 撤销一次裁定，恢复前一分支可见结论
    DecisionReverted {
        reverted_event_id: String,
        note: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    AcceptedAuto,
    AcceptedCandidate,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// 全局单调序号（稳定身份的一部分）
    pub seq: u64,
    /// 该事件所属分支
    pub branch: String,
    /// 分叉前继承自哪条分支
    pub parent_branch: Option<String>,
    /// 分支创建事件 seq（main = 0）
    pub branch_start_seq: u64,
    /// 事件确定性身份
    pub event_id: String,
    pub payload: DomainEvent,
}

impl DomainEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            DomainEvent::ProjectCreated { .. } => "project_created",
            DomainEvent::FcsImported { .. } => "fcs_imported",
            DomainEvent::SpilloverRecorded { .. } => "spillover_recorded",
            DomainEvent::TransformSet { .. } => "transform_set",
            DomainEvent::GateGeometrySaved { .. } => "gate_geometry_saved",
            DomainEvent::GateRenamed { .. } => "gate_renamed",
            DomainEvent::GateTreeSaved { .. } => "gate_tree_saved",
            DomainEvent::CompensationAssessed { .. } => "compensation_assessed",
            DomainEvent::CompensationDecided { .. } => "compensation_decided",
            DomainEvent::GateRevised { .. } => "gate_revised",
            DomainEvent::VersionConflictRecorded { .. } => "version_conflict_recorded",
            DomainEvent::ForkCreated { .. } => "fork_created",
            DomainEvent::DecisionReverted { .. } => "decision_reverted",
        }
    }

    /// 该事件直接改动的“直接来源”对象
    pub fn direct_source(&self) -> String {
        match self {
            DomainEvent::ProjectCreated { project_id, .. } => format!("project:{project_id}"),
            DomainEvent::FcsImported { source_id, .. } => format!("raw_channels:{source_id}"),
            DomainEvent::SpilloverRecorded { spillover_id, .. } => {
                format!("spillover:{spillover_id}")
            }
            DomainEvent::TransformSet { transform_id, .. } => format!("transform:{transform_id}"),
            DomainEvent::GateGeometrySaved { gate_id, .. }
            | DomainEvent::GateRenamed { gate_id, .. }
            | DomainEvent::GateRevised { gate_id, .. } => format!("gate:{gate_id}"),
            DomainEvent::GateTreeSaved { tree_id, .. } => format!("gate_tree:{tree_id}"),
            DomainEvent::CompensationAssessed { spillover_id, .. }
            | DomainEvent::CompensationDecided { spillover_id, .. } => {
                format!("compensation:{spillover_id}")
            }
            DomainEvent::VersionConflictRecorded { conflict_id, .. } => {
                format!("conflict:{conflict_id}")
            }
            DomainEvent::ForkCreated { new_branch, .. } => format!("branch:{new_branch}"),
            DomainEvent::DecisionReverted { reverted_event_id, .. } => {
                format!("decision:{reverted_event_id}")
            }
        }
    }
}
