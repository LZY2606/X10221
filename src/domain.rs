//! 领域类型：原始通道值、spillover 元数据、补偿矩阵、变换参数、门多边形、
//! 父子关系全部作为独立、内容寻址、不可变的版本化制品（artifact）。
//!
//! 原始表示在导入时原样保留（字符串 + 单位/坐标基准），派生层才做数值化；
//! 无法转换的值留在原位置并附诊断（`EventValue::Unparsable`），绝不丢弃。

use serde::{Deserialize, Serialize};

/// 稳定身份：跨导出/删除/重导入保持不变。
/// 规则 = `kind:stem:v{n}`，内容哈希与分支不参与身份（内容靠 sha256，分支靠映射表）。
pub fn stable_id(kind: &str, stem: &str, version: u32) -> String {
    format!("{kind}:{stem}:v{version}")
}

/// 单个原始通道值：保留原始字符串表示、单位/坐标基准与诊断。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum EventValue {
    /// 可规范化的线性数值。
    Numeric { raw: String, value: f64, unit: String },
    /// 溢出/哨兵值：保留原始 token，数值层缺失但不删除。
    Sentinel { raw: String, unit: String },
    /// 无法转换：留在原位置，附机器可读诊断。
    Unparsable { raw: String, diagnostic: String, unit: String },
}

impl EventValue {
    pub fn numeric_opt(&self) -> Option<f64> {
        match self {
            EventValue::Numeric { value, .. } => Some(*value),
            _ => None,
        }
    }
    pub fn raw(&self) -> &str {
        match self {
            EventValue::Numeric { raw, .. }
            | EventValue::Sentinel { raw, .. }
            | EventValue::Unparsable { raw, .. } => raw,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    /// 短名（散点轴标识），例如 `FSC-A`。
    pub short: String,
    /// $PnN / 检测器长名，例如 `FITC-A`。
    pub detector: String,
    /// $PnE 增益（(a0,a1)），坐标基准的一部分。
    pub amplification: [f64; 2],
    /// 物理单位，例如 "channel"、"channel(log)"、"MEF"。
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowEvent {
    /// 事件在来源文件中的 1 基序号（跨版本稳定）。
    pub index: u64,
    pub values: Vec<EventValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsArtifact {
    pub channels: Vec<Channel>,
    pub events: Vec<FlowEvent>,
}

/// spillover 元数据：来自 FCS 的 $SPILL / SPILL / $SPILLOVER 关键字原文。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverArtifact {
    /// 原始关键字文本（逐字保留）。
    pub raw_keyword: String,
    /// 关键字名，例如 "$SPILLOVER"。
    pub keyword_name: String,
    pub detectors: Vec<String>,
    /// n x n，按 detectors 顺序；线性空间 spillover S（列=荧光染料源，行=检测器）。
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
}

/// 补偿矩阵：与 spillover 元数据分开版本化。可由单染对照人工确认后生成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationArtifact {
    pub detectors: Vec<String>,
    /// n x n 线性补偿矩阵 C。理想情况下 C ≈ S^{-1}。
    pub matrix: Vec<Vec<f64>>,
    /// 声明依据：spillover 版本、单染对照事件或人工。
    pub basis: String,
    pub unit: String,
}

/// 变换参数：分开版本化。所有派生坐标都标注坐标基准。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformArtifact {
    /// 通道 -> 参数。
    pub params: Vec<TransformParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParam {
    pub detector: String,
    /// `asinh`（x/cofactor）或 `linear`。
    pub method: String,
    pub cofactor: f64,
    pub coordinate_basis: String,
}

/// 门多边形：顶点坐标来自指定变换版本（坐标基准钉死）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatePolygonArtifact {
    pub name: String,
    pub x_detector: String,
    pub y_detector: String,
    /// 顶点所在坐标基准，例如 "asinh:cofactor=150"。
    pub coordinate_basis: String,
    pub vertices: Vec<[f64; 2]>,
    /// 边界点裁定策略：`inside`（边界算内）/ `outside`（边界算外）/
    /// `candidate`（保留全部合法候选，不唯一决定）。
    pub boundary_policy: String,
}

/// 父子关系：与门多边形分开版本化——只改名/重挂接不动几何。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateLinkArtifact {
    /// 子门（稳定 stem）。
    pub child: String,
    /// 父门 stable_id；根门父为 `root:all-events:v1`。
    pub parent: String,
    /// 展示名（门嵌套改名只动这里，不动几何 stem）。
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ArtifactData {
    Events(EventsArtifact),
    Spillover(SpilloverArtifact),
    Compensation(CompensationArtifact),
    Transform(TransformArtifact),
    GatePolygon(GatePolygonArtifact),
    GateLink(GateLinkArtifact),
}

impl ArtifactData {
    pub fn kind(&self) -> &'static str {
        match self {
            ArtifactData::Events(_) => "events",
            ArtifactData::Spillover(_) => "spillover",
            ArtifactData::Compensation(_) => "comp",
            ArtifactData::Transform(_) => "transform",
            ArtifactData::GatePolygon(_) => "gate-poly",
            ArtifactData::GateLink(_) => "gate-link",
        }
    }
}

/// 一个已版本化的制品行。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub stem: String,
    pub version: u32,
    pub sha256: String,
    pub data: ArtifactData,
}

/// 制品类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Events,
    Spillover,
    Compensation,
    Transform,
    GatePolygon,
    GateLink,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Events => "events",
            ArtifactKind::Spillover => "spillover",
            ArtifactKind::Compensation => "comp",
            ArtifactKind::Transform => "transform",
            ArtifactKind::GatePolygon => "gate-poly",
            ArtifactKind::GateLink => "gate-link",
        }
    }
    pub fn parse(s: &str) -> Option<ArtifactKind> {
        Some(match s {
            "events" => ArtifactKind::Events,
            "spillover" => ArtifactKind::Spillover,
            "comp" => ArtifactKind::Compensation,
            "transform" => ArtifactKind::Transform,
            "gate-poly" => ArtifactKind::GatePolygon,
            "gate-link" => ArtifactKind::GateLink,
            _ => return None,
        })
    }
}

/// 结果状态：ok / blocked（不能默默继续，等人工）/ error（硬失败）/ stale（上游过期）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultStatus {
    Ok,
    Blocked,
    Error,
    Stale,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultStatus::Ok => "ok",
            ResultStatus::Blocked => "blocked",
            ResultStatus::Error => "error",
            ResultStatus::Stale => "stale",
        }
    }
}

/// 补偿评估结果（多候选 + 条件数；近奇异绝不默默求逆）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompensationResult {
    pub status: ResultStatus,
    pub condition_number: Option<f64>,
    pub determinant: Option<f64>,
    /// 事件 index -> 通道 -> 补偿后值（线性空间）。
    pub compensated: Vec<Vec<f64>>,
    /// blocked 时的全部合法候选标签（如 "reject"、"pseudoinverse"）。
    pub candidates: Vec<String>,
    pub diagnostics: Vec<String>,
}

/// 门评估结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateResult {
    pub gate_id: String,
    pub status: ResultStatus,
    /// 命中事件 index（采用结论；未裁定时为边界策略 inside 默认候选）。
    pub member_indices: Vec<u64>,
    /// 边界点：坐标恰在边上的事件，永远列出。
    pub boundary_indices: Vec<u64>,
    /// 多候选：policy=candidate 时每个合法候选（inside/outside）各一份成员集。
    pub candidates: Vec<GateCandidate>,
    /// 百分比分母：钉住父群版本（stable_id + 父计数快照）。
    pub denominator: Option<DenominatorRef>,
    pub percent: Option<f64>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateCandidate {
    pub label: String,
    pub member_indices: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DenominatorRef {
    /// 父群结果的稳定来源身份（父门 stable_id@事件版本）。
    pub parent_gate_id: String,
    pub parent_events_id: String,
    pub parent_count: u64,
}
