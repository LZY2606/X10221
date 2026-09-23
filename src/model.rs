// 领域模型：原始来源与解释分层保存。
// 所有领域量携带 unit（单位）与 basis（坐标基准）；无法转换的值保留原始 token + 诊断。

use serde::{Deserialize, Serialize};

pub const KIND_RAW: &str = "raw";
pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_COMP: &str = "comp";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_HIERARCHY: &str = "hierarchy";

pub const ALL_KINDS: [&str; 6] = [
    KIND_RAW,
    KIND_SPILLOVER,
    KIND_COMP,
    KIND_TRANSFORM,
    KIND_GATE,
    KIND_HIERARCHY,
];

/// 工件引用：六类材料各自独立版本化的稳定身份。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ArtifactRef {
    pub kind: String,
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub fingerprint: String,
}

impl ArtifactRef {
    pub fn new(kind: &str, name: &str, version: u32, fingerprint: impl Into<String>) -> Self {
        Self { kind: kind.to_string(), name: name.to_string(), version, fingerprint: fingerprint.into() }
    }
    pub fn id(&self) -> String {
        format!("{}:{}@v{}", self.kind, self.name, self.version)
    }
}

/// 带单位与坐标基准的通道测量值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelValue {
    pub channel: String,
    /// 原始表示（导入时逐字节/逐 token 保留），例如 "2.62143E5"、"NaN"。
    pub raw_token: String,
    /// 规范化后的数值；无法转换时为 None，值留在 raw_token 原位置。
    #[serde(default)]
    pub value: Option<f64>,
    pub unit: String,
    /// 坐标基准，例如 "FCS3.1 $PnR=262144 ADC count, $PnE=0,0"。
    pub basis: String,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    /// 事件在原始来源中的稳定下标。
    pub index: u32,
    pub channels: Vec<ChannelValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvents {
    pub source_file: String,
    pub events: Vec<RawEvent>,
}

/// FCS $SPILL / spillover 元数据：与补偿矩阵分开保存。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverMeta {
    pub keyword: String,
    pub channels: Vec<String>,
    /// 行：发射通道；列：荧光素通道，单位 spillover_fraction。
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub unit: String,
    pub basis: String,
    /// 来源：spillover 派生 / 人工修订。
    pub origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelTransform {
    pub method: String,
    #[serde(default)]
    pub cofactor: Option<f64>,
    pub unit: String,
    pub basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformSpec {
    pub channels: std::collections::BTreeMap<String, ChannelTransform>,
}

/// 门多边形/矩形；坐标显式给出基准（哪个变换版本下的坐标）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateSpec {
    pub node: String,
    pub display_name: String,
    pub shape: String,
    pub x_channel: String,
    pub y_channel: String,
    /// 多边形顶点（矩形也以 4 顶点表示），闭区间，边界点计入门内。
    pub vertices: Vec<[f64; 2]>,
    pub coordinate_unit: String,
    pub coordinate_basis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchySpec {
    /// node -> 父节点；根节点父为 None。
    pub parents: std::collections::BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactContent {
    Raw(RawEvents),
    Spillover(SpilloverMeta),
    Comp(CompMatrix),
    Transform(TransformSpec),
    Gate(GateSpec),
    Hierarchy(HierarchySpec),
}

impl ArtifactContent {
    pub fn kind(&self) -> &'static str {
        match self {
            ArtifactContent::Raw(_) => KIND_RAW,
            ArtifactContent::Spillover(_) => KIND_SPILLOVER,
            ArtifactContent::Comp(_) => KIND_COMP,
            ArtifactContent::Transform(_) => KIND_TRANSFORM,
            ArtifactContent::Gate(_) => KIND_GATE,
            ArtifactContent::Hierarchy(_) => KIND_HIERARCHY,
        }
    }
    pub fn name(&self) -> String {
        match self {
            ArtifactContent::Raw(r) => r.source_file.clone(),
            ArtifactContent::Spillover(_) => "fcs-spill".to_string(),
            ArtifactContent::Comp(_) => "comp".to_string(),
            ArtifactContent::Transform(_) => "transform".to_string(),
            ArtifactContent::Gate(g) => g.node.clone(),
            ArtifactContent::Hierarchy(_) => "hierarchy".to_string(),
        }
    }
}

// ---------- 事件（分支日志，只追加） ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub rationale: String,
    /// 采纳后将引用/产生的工件（可为空）。
    #[serde(default)]
    pub artifact: Option<ArtifactRef>,
    pub status: String, // open | accepted | rejected
}

/// 机器可读取的规则判定依据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleFinding {
    pub rule: String,
    pub severity: String, // info | warning | blocked
    pub status: String,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EventPayload {
    /// 初始导入：一次性写入六类材料的首个版本。
    CaseLoaded {
        case_id: String,
        refs: Vec<ArtifactRef>,
        bytes_fingerprint: String,
    },
    /// 参数修订（补偿矩阵 / 变换 / 门 / 层级均可，refs 为新版本）。
    Revision {
        reason: String,
        refs: Vec<ArtifactRef>,
    },
    /// 局部源数据改动：只修补给定下标的事件。
    SourcePatch {
        reason: String,
        patched_event_indices: Vec<u32>,
        new_ref: ArtifactRef,
    },
    /// 门嵌套改名 / 移动：节点身份不变。
    RenameNode {
        node: String,
        old_name: String,
        new_name: String,
        new_parent: Option<String>,
        gate_ref: Option<ArtifactRef>,
        hierarchy_ref: Option<ArtifactRef>,
    },
    /// 近奇异等场景下保存全部合法候选。
    CandidatesRecorded {
        scope: String,
        subject: ArtifactRef,
        candidates: Vec<Candidate>,
        findings: Vec<RuleFinding>,
    },
    /// 人工裁定：接受或否定某个候选；不直接改写原数据，只追加事件。
    Decision {
        scope: String,
        subject: ArtifactRef,
        candidate_id: String,
        verdict: String, // accepted | rejected
        note: String,
        /// 采纳后产生的新版本引用（可选）。
        applied_ref: Option<ArtifactRef>,
    },
    /// 上游修订后，下游结果标记过期，不覆盖旧结果。
    StalePropagation {
        caused_by_event: String,
        stale: Vec<StaleTarget>,
    },
    /// 撤销：从旧事件处分叉恢复前一分支。
    ForkUndo {
        forked_from_branch: String,
        forked_from_event: String,
        undone_event: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaleTarget {
    pub node: String,
    pub last_result_ref: String,
    pub outdated_deps: Vec<ArtifactRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub branch: String,
    pub seq: u32,
    pub parent: Option<String>,
    pub base_event_id: Option<String>,
    pub payload: EventPayload,
    pub author: String,
    pub created_at: String,
}

// ---------- 案例包 ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelChannel {
    pub name: String,
    pub label: String,
    pub unit: String,
    pub basis: String,
    pub kind: String, // scatter | fluorescence
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaseFile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub fcs_file: String,
    pub panel: Vec<PanelChannel>,
    pub spillover: SpilloverMeta,
    pub matrices: std::collections::BTreeMap<String, CompMatrix>,
    pub initial_matrix: String,
    pub transform: TransformSpec,
    pub gates: Vec<GateSpec>,
    pub hierarchy: HierarchySpec,
    pub narrative: Vec<String>,
}
