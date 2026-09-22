//! 领域模型：流式事件、补偿、变换、门与门控系谱。
//!
//! 约定：
//! * 导入时保留原始表示（字符串），派生层才解析；无法转换的值留在原位置并附诊断。
//! * 所有领域量携带单位或坐标基准。
//! * 原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形、父子关系分别版本化。

use serde::{Deserialize, Serialize};

/// 通道的物理测量量与单位。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelSpec {
    pub code: String,
    pub label: String,
    /// 单位，例如 `ADC-count`（脉冲面积的线性模数计数）。
    pub unit: String,
    /// 原始坐标基准，例如 `raw-linear-ADC`。
    pub basis: String,
    /// 通道线性上限（ADC 满量程），用于极端信号判定。
    pub linear_max: f64,
}

/// 单个通道值的原始表示与解析结果。无法解析时 `numeric` 为 None，保留诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawValue {
    /// 通道稳定代码，例如 `FITC`。
    pub channel: String,
    /// 原始字符串表示（导入原文），永不丢失。
    pub raw: String,
    /// 解析后的数值；无法转换时为 None。
    pub numeric: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

impl RawValue {
    pub fn new(raw: String, linear_max: f64) -> Self {
        let mut diags = Vec::new();
        let numeric = match raw.trim().parse::<f64>() {
            Ok(v) => {
                if !v.is_finite() {
                    diags.push(format!("non-finite raw value kept at source: {}", raw));
                    None
                } else {
                    if v < 0.0 {
                        diags.push(format!("negative raw count {v} kept in place (ADC-count)"));
                    }
                    if v > linear_max {
                        diags.push(format!(
                            "overflow: raw count {v} exceeds ADC linear_max {linear_max}; kept, not clipped"
                        ));
                    }
                    Some(v)
                }
            }
            Err(_) => {
                diags.push(format!("unconvertible value {:?} kept at original location", raw));
                None
            }
        };
        RawValue { channel: String::new(), raw, numeric, diagnostics: diags }
    }
}

/// 一条 FCS 事件：以事件为单位保留所有原始通道值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsEvent {
    /// 稳定事件身份，例如 `E01`。
    pub event_id: String,
    pub values: Vec<RawValue>,
}

/// 数据集（原始通道值）的一个不可变版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetVersion {
    pub version_id: String,
    pub supersedes: Option<String>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<FcsEvent>,
    pub note: String,
}

/// Spillover 元数据（试剂/仪器来源信息）的一个版本，与数值矩阵分开版本化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub version_id: String,
    pub supersedes: Option<String>,
    pub panel_name: String,
    pub fluorochromes: Vec<String>,
    pub single_stain_controls: Vec<SingleStainControl>,
    pub acquired_on: String,
    pub cytometer: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SingleStainControl {
    pub fluorochrome: String,
    /// 单染对照引用的数据集版本与事件范围（来源可追溯）。
    pub dataset_version: String,
    pub event_ids: Vec<String>,
    pub unit: String,
}

/// 补偿矩阵的一个不可变版本。单位为 fraction（无量纲溢漏比例）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationMatrix {
    pub version_id: String,
    pub supersedes: Option<String>,
    /// 行列顺序与 fluorochromes 一致；对角线为 1.0。
    pub channels: Vec<String>,
    /// 行主序方阵，fraction，坐标基准 `spillover-fraction`。
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
    pub note: String,
}

/// 变换参数的一个版本（对补偿后信号做 asinh 压缩）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    pub version_id: String,
    pub supersedes: Option<String>,
    pub kind: String,
    /// asinh(x / cofactor)，输入输出单位均标注。
    pub cofactor: f64,
    pub input_unit: String,
    pub output_unit: String,
    pub note: String,
}

/// 边界点策略：如何处理恰好落在门边界上的事件。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BoundaryPolicy {
    /// 含边界（闭区间/闭多边形）。
    Inclusive,
    /// 不含边界（开区间）。
    Exclusive,
    /// 边界点视为歧义，保留 in/out 两个候选，等待人工裁定。
    Ambiguous,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 门的一个不可变版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateVersion {
    pub gate_id: String,
    pub version_id: String,
    pub supersedes: Option<String>,
    pub name: String,
    pub kind: GateKind,
    /// 该门在哪一坐标空间评估。
    pub space: GateSpace,
    pub boundary: BoundaryPolicy,
    pub polygon: Vec<Point>,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GateKind {
    /// 根门，恒包含全部事件。
    Root,
    Polygon,
}

/// 门评估的坐标空间。引用具体的数据集/补偿/变换版本，保证可复现。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateSpace {
    pub x_channel: String,
    pub y_channel: String,
    /// `raw-linear-ADC` | `compensated-linear` | `asinh-compensated`
    pub basis: String,
    pub x_unit: String,
    pub y_unit: String,
    pub dataset_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform_version: Option<String>,
}

/// 父子关系（门控系谱）的一个独立版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateRelationVersion {
    pub version_id: String,
    pub supersedes: Option<String>,
    /// `(child_gate_id, parent_gate_id)`；根门无父。
    pub edges: Vec<(String, String)>,
    pub note: String,
}

/// 修订包：一次导入/参数修订/局部源数据改动提交的全部新版本。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RevisionBundle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<DatasetVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spillover_meta: Option<SpilloverMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<CompensationMatrix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<TransformParams>,
    #[serde(default)]
    pub gates: Vec<GateVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<GateRelationVersion>,
}

impl RevisionBundle {
    pub fn is_empty(&self) -> bool {
        self.dataset.is_none()
            && self.spillover_meta.is_none()
            && self.compensation.is_none()
            && self.transform.is_none()
            && self.gates.is_empty()
            && self.relations.is_none()
    }
}

/// 事件类型（append-only 事件日志的负载）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    /// 初始导入：六类材料的 v1 基线。
    Imported {
        bundle: RevisionBundle,
        source_blob: String,
        source_sha256: String,
    },
    /// 参数修订（补偿或变换新版本）。
    ParameterRevised { bundle: RevisionBundle, reason: String },
    /// 局部源数据改动（只影响局部依赖：仅替换数据集版本）。
    SourcePatched { bundle: RevisionBundle, reason: String },
    /// 修订门（形状/边界策略），下游结果将被标记过期而不覆盖。
    GateRevised { bundle: RevisionBundle, reason: String },
    /// 门嵌套改名：稳定 gate_id 不变，仅名称变化。
    GateRenamed { bundle: RevisionBundle },
    /// 人工裁定：从候选集中选择（接受/否定某候选）。
    Decision {
        candidate_set_id: String,
        /// 被采纳的候选 id；若整体否定该集合则为 None。
        accepted_candidate_id: Option<String>,
        rejected: Vec<String>,
        rationale: String,
    },
    /// 显式重算下游：把给定门重新钉到当前输入版本；旧结果保留，另存新结果。
    Recomputed { bundle: RevisionBundle, reason: String },
    /// 撤销上一条事件（软撤回；撤回本身也记录为事件）。
    Retracted { target_event_id: String, reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchEvent {
    pub event_id: String,
    /// 分支内单调序号，从 1 开始。
    pub seq: i64,
    pub branch: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub payload: EventPayload,
}

/// 候选（歧义点的一个合法解释）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub candidate_id: String,
    pub label: String,
    pub detail: String,
}

/// 多候选集合：无法唯一决定时保留全部合法候选，等待裁定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateSet {
    pub candidate_set_id: String,
    pub created_by_event: String,
    /// 歧义种类：`near-singular-compensation` | `boundary-membership`
    pub kind: String,
    pub subject: String,
    pub candidates: Vec<Candidate>,
    /// 被采纳候选；未裁定为 None，整体否定则为 Some(None)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<Option<String>>,
    #[serde(default)]
    pub rejected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}
