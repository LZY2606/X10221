//! 版本化工件（artifact）与复核事件（event）。
//!
//! 六种来源/解释材料**分开版本化**：
//! 1. raw_events   原始通道值（含原始字面量与位表示）
//! 2. spillover     spillover 元数据（单染对照、SRC 通道、系数来源）
//! 3. matrix        补偿矩阵（由 spillover 推导或人工修订）
//! 4. transform     变换参数（asinh 的 a/cofactor、坐标基准）
//! 5. gate          门多边形（坐标基准下的顶点 + 边界规则）
//! 6. lineage       门的父子关系（改名只改 label，不动 node_id）

use serde::{Deserialize, Serialize};

use crate::units::{CoordBasis, RawNumber};

pub type Json = serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    RawEvents,
    Spillover,
    Matrix,
    Transform,
    Gate,
    Lineage,
}

impl ArtifactKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactKind::RawEvents => "raw_events",
            ArtifactKind::Spillover => "spillover",
            ArtifactKind::Matrix => "matrix",
            ArtifactKind::Transform => "transform",
            ArtifactKind::Gate => "gate",
            ArtifactKind::Lineage => "lineage",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "raw_events" => ArtifactKind::RawEvents,
            "spillover" => ArtifactKind::Spillover,
            "matrix" => ArtifactKind::Matrix,
            "transform" => ArtifactKind::Transform,
            "gate" => ArtifactKind::Gate,
            "lineage" => ArtifactKind::Lineage,
            _ => return None,
        })
    }
}

/// 通道声明：原始单位与坐标基准在导入时固定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub unit: String,
    /// 荧光通道（参与补偿）还是散射通道。
    pub detector: DetectorKind,
    /// 原始 f32 位十六进制，用于“原始表示不丢失”。
    pub exponent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorKind {
    Scatter,
    Fluorescence,
    Time,
}

/// 原始通道值集合。events 保存原始字面量 + 解析值；无法解析的值留在原位。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEventsDoc {
    pub source_file: String,
    pub fcs_version: String,
    pub channels: Vec<Channel>,
    /// 每个事件：按 channels 顺序的 RawNumber（NaN/Inf 保留）。
    pub events: Vec<Vec<RawNumber>>,
    /// 导入诊断（NaN、极端值、无法转换的字面量）。
    pub diagnostics: Vec<Diagnostic>,
    /// 原始字节摘要（原始字节另存 source_blobs，事件引用之）。
    pub source_blob_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub event_index: Option<usize>,
    pub channel: Option<String>,
    pub code: String,
    pub message: String,
    /// 原始表示（字面量或 f32 位 hex），保证“留在原位置”。
    pub raw: Option<String>,
}

/// spillover 元数据：单染对照与溢出系数的“解释”，与矩阵分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverDoc {
    pub channels: Vec<String>,
    /// rows[i] = 第 i 个单染对照在各通道上的溢出比例；
    /// 每个系数带 source（单染事件选择/人工标注），保留原始表示。
    pub rows: Vec<Vec<RawNumber>>,
    pub single_stains: Vec<SingleStainRef>,
    /// 元数据里的原始关键字字符串（例如 $SPILL / $COMP），导入时保留。
    pub raw_keywords: Vec<(String, String)>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SingleStainRef {
    pub channel: String,
    pub label: String,
    pub source_event_range: [usize; 2],
    pub raw_blob_id: Option<String>,
}

/// 补偿矩阵工件。它是 spillover 的**解释版本**：可以来自推导，也可以人工修订。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixDoc {
    pub channels: Vec<String>,
    pub entries: Vec<Vec<f64>>,
    pub derived_from: Option<String>,
    /// 数值健康：条件数、是否近奇异（由内核计算，写入时冻结）。
    pub conditioning: MatrixConditioning,
}

#[derive(Debug,Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixConditioning {
    pub invertible: bool,
    pub condition_number: Option<f64>,
    pub min_singular_value: Option<f64>,
    pub threshold: f64,
    pub verdict: String,
}

/// 变换参数（案例包使用 asinh：x' = asinh(a*x + b)）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformDoc {
    pub channel: String,
    pub family: String,
    pub a: f64,
    pub b: f64,
    pub input_basis: CoordBasis,
    pub output_unit: String,
}

/// 门多边形。顶点坐标落在 `basis` 声明的坐标基准中；边界规则固定。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDoc {
    pub node_id: String,
    pub label: String,
    pub x: GateAxis,
    pub y: GateAxis,
    pub polygon: Vec<[f64; 2]>,
    /// 边界点归属规则（固定为包含边界），同时记录用于断言的判定式。
    pub boundary_rule: BoundaryRule,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateAxis {
    pub channel: String,
    pub basis: CoordBasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryRule {
    /// 边界点（多边形边上）计入门内 —— 确定性、无浮动手抖空间。
    EdgeInclusive,
}

/// 门系谱：父子关系单独版本化。改名只影响 gate.label，
/// 这里的 edges 以稳定 node_id 表达，因此改名不破坏父子链。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageDoc {
    /// node_id -> 父 node_id（根为 None）。
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEdge {
    pub child_node_id: String,
    pub parent_node_id: Option<String>,
}

/// 工件信封：某材料的一个具体不可变版本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub kind: ArtifactKind,
    pub logical: String,
    pub version: u32,
    pub supersedes: Option<String>,
    pub created_from_event: String,
    pub doc: Json,
}

// ---------- 事件 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    CaseCreated,
    ImportCommitted,
    ImportRejected,
    ArtifactProposed,
    CompensationRequested,
    CompensationResolved,
    GateRevised,
    LineageRenamed,
    CandidateChosen,
    Accepted,
    Rejected,
    ParamAdjusted,
    Undo,
    ConflictObserved,
    ExportSnapshot,
}

impl EventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventKind::CaseCreated => "case_created",
            EventKind::ImportCommitted => "import_committed",
            EventKind::ImportRejected => "import_rejected",
            EventKind::ArtifactProposed => "artifact_proposed",
            EventKind::CompensationRequested => "compensation_requested",
            EventKind::CompensationResolved => "compensation_resolved",
            EventKind::GateRevised => "gate_revised",
            EventKind::LineageRenamed => "lineage_renamed",
            EventKind::CandidateChosen => "candidate_chosen",
            EventKind::Accepted => "accepted",
            EventKind::Rejected => "rejected",
            EventKind::ParamAdjusted => "param_adjusted",
            EventKind::Undo => "undo",
            EventKind::ConflictObserved => "conflict_observed",
            EventKind::ExportSnapshot => "export_snapshot",
        }
    }
}

/// 一个复核事件：append-only 账本里的一行。
/// seq 在分支内稠密递增；parent_event_id 构成 DAG（撤销/冲突场景）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub case_id: String,
    pub branch: String,
    pub seq: u64,
    pub parent_event_id: Option<String>,
    pub kind: EventKind,
    pub actor: String,
    pub summary: String,
    pub payload: Json,
    /// 该事件读取的依赖版本（钉住版本，绝不“跟最新”）。
    pub refs: EventRefs,
    /// 该事件使哪些下游对象失效（内核计算的失效范围）。
    pub invalidation: Vec<Invalidation>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRefs {
    pub artifacts: Vec<String>,
    pub derived: Vec<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invalidation {
    /// 被失效对象的稳定 id（derived_result / gate population）。
    pub target_id: String,
    pub target_kind: String,
    pub reason: String,
    /// 失效而非覆盖：旧记录保留，状态置 stale。
    pub mode: String,
}

/// 机器可读判定（近奇异拒绝、分母钉版、过期传播）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub code: String,
    pub passed: bool,
    pub message: String,
    pub evidence: Json,
}
