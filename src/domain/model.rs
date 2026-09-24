//! 版本化领域对象与追加式事件。
//!
//! 来源与解释分开保存：原始通道值 / spillover 元数据是「来源」，
//! 补偿矩阵 / 变换参数 / 门多边形 / 父子关系是「解释」，各自独立版本化。
//! 所有修改都以不可变事件追加；投影（population 等）可随时从事件重放重建。

use serde::{Deserialize, Serialize};

use crate::domain::units::Measured;

/// 通道描述（来自 FCS $PnN/$PnS/$PnE 等，导入时保留原始表示）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub index: usize,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
    /// FCS $PnE 原值，例如 "0,0" 表示对数，"0.0,4.096" 表示线性。
    pub amplification: String,
}

/// 一次 FCS 导入的原始事件表。逐事件逐通道保留原始表示与诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvents {
    pub id: String,
    /// 源 FCS 字节的 sha256，事件身份钉在原始字节上。
    pub source_sha: String,
    pub filename: String,
    pub channels: Vec<ChannelSpec>,
    /// events[event_index][channel_index]
    pub events: Vec<Vec<Measured>>,
}

/// spillover 元数据（FCS $SPILL / 单染对照记录），保持来源形态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub id: String,
    pub source_sha: String,
    /// 矩阵中各列（通道）的顺序，来自 $SPILL 头。
    pub channels: Vec<String>,
    /// 行优先的原始字符串，例如 "1.0,0.03,..."；不做任何隐式换算。
    pub matrix_raw: Vec<Vec<String>>,
    /// 单染对照名（若有），与矩阵行顺序一致。
    pub controls: Vec<String>,
}

/// 补偿矩阵（解释层）。单独版本化，可被参数修订替换而不动来源。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub id: String,
    pub revision: i64,
    pub supersedes: Option<String>,
    pub channels: Vec<String>,
    /// 行优先；S[i][j] = 通道 j 信号泄漏到通道 i 的比例（对角=1）。
    pub spill: Vec<Vec<f64>>,
    /// 判定阈值：rcond 低于该值即视为近奇异，不允许默默求逆。
    pub near_singular_threshold: f64,
}

/// 变换参数（解释层），按通道独立版本化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub id: String,
    pub revision: i64,
    pub supersedes: Option<String>,
    /// 缺省 cofactor（asinh(x / cofactor)）。
    pub default_cofactor: f64,
    /// 通道级覆盖；未列出的通道使用 default_cofactor。
    pub per_channel_cofactor: Vec<(String, f64)>,
}

/// 多边形门（解释层）。坐标基准显式声明，边界点按半开区间归属。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub id: String,
    pub revision: i64,
    pub supersedes: Option<String>,
    pub name: String,
    /// 轴基准，例如 {coord: "transformed", channel: "FITC-A"}。
    pub x_axis: AxisRef,
    pub y_axis: AxisRef,
    /// 顶点（变换坐标）。
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxisRef {
    pub coordinate: String,
    pub channel: String,
}

/// 门的父子关系，独立于多边形本身版本化（改名 / 重新挂载只改这里）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateLink {
    pub id: String,
    pub revision: i64,
    pub supersedes: Option<String>,
    pub gate_id: String,
    /// None 表示挂在根群（全部事件）下。
    pub parent_gate_id: Option<String>,
    pub display_name: String,
}

/// 追加式事件（机器可读的判断依据）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// 一次完整导入：内含原始字节 sha、FCS 与 spillover；失败则整体回滚。
    Import {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        fcs_sha: String,
        fcs_filename: String,
        fcs_json: String,
        spill_json: Option<String>,
        diagnostics: Vec<String>,
        /// 本次相对前一版源数据实际改动的通道；用于精确计算失效范围（局部传播）。
        #[serde(default)]
        affected_channels: Vec<String>,
    },
    /// 采纳 / 替换补偿矩阵（可能指向一次近奇异判定后的人工裁定）。
    CompensationSet {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        matrix_json: String,
    },
    /// 近奇异判定本身也是事件：记录输入片段、rcond、全部合法候选，不做自动求逆。
    NearSingularDetected {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        matrix_id: String,
        rcond: f64,
        threshold: f64,
        /// 候选描述（正则化逆 / 伪逆 / 人工提供），均标记为未采纳候选。
        candidates_json: String,
        affected_objects: Vec<String>,
    },
    TransformSet {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        transform_json: String,
    },
    GateUpsert {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        gate_json: String,
        link_json: String,
    },
    /// 门嵌套改名 / 重新挂载（只改 link，多边形保留旧版可读）。
    GateRename {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        link_json: String,
    },
    /// 边界点复核：对落在多边形边界上的事件做显式归属决定。
    BoundaryDecision {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        gate_id: String,
        /// event_index -> include。
        decisions_json: String,
    },
    /// 接受 / 否定一个派生结果或候选；reason 是机器可读判断依据的一部分。
    Adjudicate {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        target: String,
        accepted: bool,
        reason_code: String,
        note: String,
    },
    /// 从某个事件分叉，恢复当时的依赖版本与可见结论。
    BranchOpen {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        new_branch: String,
        from_event_id: String,
    },
    /// 撤销本分支最近一次裁定（恢复前一分支状态），不删除历史。
    Undo {
        id: String,
        seq: i64,
        branch: String,
        parent_event_id: Option<String>,
        base_version: i64,
        timestamp: String,
        undone_event_id: String,
    },
}

impl Event {
    pub fn id(&self) -> &str {
        match self {
            Event::Import { id, .. }
            | Event::CompensationSet { id, .. }
            | Event::NearSingularDetected { id, .. }
            | Event::TransformSet { id, .. }
            | Event::GateUpsert { id, .. }
            | Event::GateRename { id, .. }
            | Event::BoundaryDecision { id, .. }
            | Event::Adjudicate { id, .. }
            | Event::BranchOpen { id, .. }
            | Event::Undo { id, .. } => id,
        }
    }
    pub fn seq(&self) -> i64 {
        match self {
            Event::Import { seq, .. }
            | Event::CompensationSet { seq, .. }
            | Event::NearSingularDetected { seq, .. }
            | Event::TransformSet { seq, .. }
            | Event::GateUpsert { seq, .. }
            | Event::GateRename { seq, .. }
            | Event::BoundaryDecision { seq, .. }
            | Event::Adjudicate { seq, .. }
            | Event::BranchOpen { seq, .. }
            | Event::Undo { seq, .. } => *seq,
        }
    }
    pub fn branch(&self) -> &str {
        match self {
            Event::Import { branch, .. }
            | Event::CompensationSet { branch, .. }
            | Event::NearSingularDetected { branch, .. }
            | Event::TransformSet { branch, .. }
            | Event::GateUpsert { branch, .. }
            | Event::GateRename { branch, .. }
            | Event::BoundaryDecision { branch, .. }
            | Event::Adjudicate { branch, .. }
            | Event::BranchOpen { branch, .. }
            | Event::Undo { branch, .. } => branch,
        }
    }
    pub fn base_version(&self) -> i64 {
        match self {
            Event::Import { base_version, .. }
            | Event::CompensationSet { base_version, .. }
            | Event::NearSingularDetected { base_version, .. }
            | Event::TransformSet { base_version, .. }
            | Event::GateUpsert { base_version, .. }
            | Event::GateRename { base_version, .. }
            | Event::BoundaryDecision { base_version, .. }
            | Event::Adjudicate { base_version, .. }
            | Event::BranchOpen { base_version, .. }
            | Event::Undo { base_version, .. } => *base_version,
        }
    }
    pub fn parent_event_id(&self) -> Option<&str> {
        match self {
            Event::Import { parent_event_id, .. }
            | Event::CompensationSet { parent_event_id, .. }
            | Event::NearSingularDetected { parent_event_id, .. }
            | Event::TransformSet { parent_event_id, .. }
            | Event::GateUpsert { parent_event_id, .. }
            | Event::GateRename { parent_event_id, .. }
            | Event::BoundaryDecision { parent_event_id, .. }
            | Event::Adjudicate { parent_event_id, .. }
            | Event::BranchOpen { parent_event_id, .. }
            | Event::Undo { parent_event_id, .. } => parent_event_id.as_deref(),
        }
    }
}
