//! 流式细胞术领域模型：原始通道值、spillover、补偿矩阵、变换、门、谱系
//! 以及只追加的事件（材料版本、派生结果、失效标记、人工裁定）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MaterialKind {
    RawEvents,
    Spillover,
    Compensation,
    Transform,
    Gate,
    Lineage,
}

impl MaterialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MaterialKind::RawEvents => "raw_events",
            MaterialKind::Spillover => "spillover",
            MaterialKind::Compensation => "compensation",
            MaterialKind::Transform => "transform",
            MaterialKind::Gate => "gate",
            MaterialKind::Lineage => "lineage",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_events" => MaterialKind::RawEvents,
            "spillover" => MaterialKind::Spillover,
            "compensation" => MaterialKind::Compensation,
            "transform" => MaterialKind::Transform,
            "gate" => MaterialKind::Gate,
            "lineage" => MaterialKind::Lineage,
            _ => return None,
        })
    }
}

/// 单个通道的一个原始读数：保留原始表示，非有限值不丢弃。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventValue {
    /// 导入时的原始词法表示（如 "NaN"、"3.4028235E38"）
    pub raw: String,
    /// 解析后的有限值；无法转换时为 None，诊断留在 diagnostics
    pub v: Option<f64>,
    /// 量纲/坐标基准，例如 "fluorescence_linear"
    pub unit: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelDef {
    pub index: usize,
    pub name: String,
    pub short: String,
    /// 该通道读数的量纲
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub event_index: usize,
    pub channel: String,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RawEvents {
    pub blob_id: String,
    pub filename: String,
    pub channels: Vec<ChannelDef>,
    /// events[i] 与 channels 对齐；非有限/不可转换值保留原位
    pub events: Vec<Vec<EventValue>>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    /// FCS 中声明 spillover 的关键字，例如 $SPILLOVER
    pub keyword: String,
    pub channels: Vec<String>,
    /// S[i][j]：通道 j 的信号溢入通道 i 的比例（无量纲）
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    /// 与 spillover 同形的方阵；补偿通过其逆矩阵进行
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    /// 当前固定为 "asinh"
    pub kind: String,
    /// 每通道 cofactor（无量纲），按通道短名索引
    pub cofactors: Vec<(String, f64)>,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GatePolygon {
    pub gate_id: String,
    pub label: String,
    pub x_channel: String,
    pub y_channel: String,
    /// raw | compensated | transformed
    pub basis: String,
    /// 顶点坐标，单位随 basis
    pub vertices: Vec<[f64; 2]>,
    /// 边界约定：halfopen（右/上开，确定性）| closed（边界视为候选）
    pub boundary_rule: String,
    pub coordinate_unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LineageNode {
    pub gate_id: String,
    /// 根门父级为 null
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lineage {
    pub nodes: Vec<LineageNode>,
}

/// 派生结果的稳定身份
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    CompensationResult,
    GateMembership,
    Population,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::CompensationResult => "compensation_result",
            ArtifactKind::GateMembership => "gate_membership",
            ArtifactKind::Population => "population",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "compensation_result" => ArtifactKind::CompensationResult,
            "gate_membership" => ArtifactKind::GateMembership,
            "population" => ArtifactKind::Population,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RefVersion {
    pub material_id: String,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedStatus {
    /// 唯一确定
    Determined,
    /// 存在多个合法候选，等待人工裁定（近奇异/边界点）
    Indeterminate,
    /// 奇异，无法得到合法结果
    Singular,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub label: String,
    /// 机器可读的判定依据
    pub rationale_code: String,
    pub detail: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ArtifactResult {
    pub kind: ArtifactKind,
    /// compensation_result 固定 "comp"；其余为 gate_id
    pub key: String,
    pub status: DerivedStatus,
    /// 触发计算时钉住的依赖版本
    pub basis_versions: Vec<RefVersion>,
    pub candidates: Vec<Candidate>,
    /// 被采纳候选（人工或自动）；Indeterminate 时为 None
    pub chosen: Option<String>,
    /// 人口学结果等结构化载荷
    pub data: Value,
}

/// 只追加事件。所有状态都是这些事件的投影。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event_type")]
pub enum EventPayload {
    /// 登记一个原始字节 blob（FCS 文件）
    BlobStored {
        blob_id: String,
        filename: String,
        sha256: String,
        size: usize,
    },
    FcsImported {
        raw_material_id: String,
        version: u32,
        blob_id: String,
        filename: String,
        channel_count: usize,
        event_count: usize,
        diagnostics: Vec<Diagnostic>,
        content: RawEvents,
    },
    /// 一次局部源数据改动（只影响局部依赖）
    RawEventsPatched {
        raw_material_id: String,
        version: u32,
        base_version: u32,
        edits: Vec<RawEdit>,
        content: RawEvents,
    },
    MaterialSaved {
        kind: MaterialKind,
        material_id: String,
        version: u32,
        base_version: Option<u32>,
        content: Value,
    },
    BranchCreated {
        branch: String,
        parent_branch: Option<String>,
        fork_event_seq: Option<u32>,
        label: String,
    },
    DerivedComputed {
        branch: String,
        result: ArtifactResult,
        /// 规则版本，例如 "comp.singularity.v1"
        rule: String,
    },
    /// 上游变化导致下游结果过期：只标记，不覆盖
    MarkedStale {
        branch: String,
        kind: ArtifactKind,
        key: String,
        because_branch_event_seq: u32,
        changed_refs: Vec<RefVersion>,
    },
    Adjudication {
        branch: String,
        kind: ArtifactKind,
        key: String,
        decision: String,
        candidate_id: Option<String>,
        rationale: String,
    },
    CaseLoaded {
        case_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RawEdit {
    pub event_index: usize,
    pub channel: String,
    pub raw_before: String,
    pub raw_after: String,
}
