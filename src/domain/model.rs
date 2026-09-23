//! 来源层与解释层共享的数据模型。
//!
//! 所有载荷均以 JSON 规范表示存入 SQLite；原始 FCS 字节另外存入 blob 表，
//! 载荷只保存“导入时保留的原始表示”，规范化只发生在派生层。

use serde::{Deserialize, Serialize};

/// 坐标基准 / 单位。所有领域量都必须显式携带。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Basis {
    /// 探测器原始线性计数（整数 u16，未补偿、未变换）。
    RawCounts {
        bits: u32,
        #[serde(rename = "max_code")]
        max_code: f64,
        gain: f64,
    },
    /// 补偿后线性计数，仍在探测器量纲上。
    CompensatedCounts {
        comp_matrix: String,
        comp_version: u32,
        candidate: Option<String>,
    },
    /// 变换后的无量纲坐标，钉住变换参数工件版本。
    Transformed {
        transform: String,
        transform_version: u32,
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSpec {
    /// 短名（$PnN），例如 FITC-A
    pub name: String,
    pub long_name: Option<String>,
    pub detector: Option<String>,
    pub fluor: Option<String>,
    /// 是否参与补偿
    pub compensated: bool,
    pub basis: Basis,
}

/// 单个事件在单个通道上的原始值标记。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawFlag {
    /// 整数编码达到该位宽满量程（饱和）
    Saturated,
    /// 浮点 NaN/Inf，无法解释，留在原位置
    NonFinite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcsDocument {
    pub filename: String,
    pub fcs_version: String,
    pub data_type: String,
    pub byte_order: String,
    /// 原样保留的 TEXT 段关键字（不做改名、不做单位换算）
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    /// 每行一个事件；整数量化为 f64 仍精确表示 u16/u32
    pub events: Vec<Vec<f64>>,
    /// 与 events 等长等宽；无标记为空数组（稀疏表示时用 null）
    #[serde(default)]
    pub flags: Vec<Vec<Option<RawFlag>>>,
    pub role: TubeRole,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TubeRole {
    Specimen { tube: String },
    SingleStain { control_channel: String },
}

/// spillover 元数据：与补偿矩阵分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub fluor_channels: Vec<String>,
    /// S：行=探测器，列=荧光；comp = S^{-1}（条件良好时）
    pub spillover: Vec<Vec<f64>>,
    pub controls: Vec<ControlRef>,
    pub convention: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRef {
    pub channel: String,
    pub events_artifact: String,
    pub events_version: u32,
    pub median_raw: f64,
}

/// 补偿矩阵工件。近奇异时 matrix 为 None，只保留候选与判定依据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompMatrix {
    pub fluor_channels: Vec<String>,
    pub spillover_artifact: String,
    pub spillover_version: u32,
    pub scaled_determinant: f64,
    pub near_singular: bool,
    pub singular_threshold: f64,
    /// 条件良好时为 S^{-1}；近奇异时为 None（不允许默默求逆）
    pub matrix: Option<Vec<Vec<f64>>>,
    /// 近奇异时保留的全部合法候选
    pub candidates: Vec<CompCandidate>,
    pub judgement: CompJudgement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompCandidate {
    pub id: String,
    pub rationale: String,
    /// 该候选实际使用的补偿矩阵（identity 或 ridge 正则逆）
    pub matrix: Vec<Vec<f64>>,
    pub ridge_lambda: Option<f64>,
    pub scaled_determinant: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum CompJudgement {
    Ok,
    NearSingularNoUniqueInverse { reason: String },
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum TransformSpec {
    /// y = (a*z + (1-a)*asinh(z)) / denom_at_xmax * m，z=(x-x0)/b
    Asinh { m: f64, a: f64, b: f64, x0: f64 },
    /// y = log10((x-x0)/b)；x-x0<=0 不可转换，保留原值并附诊断，不裁剪
    Log10 { b: f64, x0: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformParams {
    pub applies_to: Vec<String>,
    pub x_max: f64,
    pub spec: TransformSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    /// 闭合顶点（按顺序），坐标由 coordinate_basis 解释
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatePolygon {
    pub display_name: String,
    pub x_channel: String,
    pub y_channel: String,
    pub coordinate_basis: Basis,
    pub polygon: Polygon,
    /// 边界判定规则：射线法半开区间（下边/左边含，上边/右边不含）
    pub boundary_rule: String,
}

/// 父子关系与门多边形分开版本化。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateHierarchy {
    /// node id -> 节点描述；根为 root:all-events
    pub nodes: Vec<HierarchyNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HierarchyNode {
    pub node: String,
    pub display_name: String,
    pub gate_artifact: Option<String>,
    pub gate_version: Option<u32>,
    pub parent: Option<String>,
    /// 若该版本由改名而来，记录旧身份，保证谱系可追溯
    #[serde(default)]
    pub renamed_from: Option<String>,
}

/// 工件种类与版本载荷的和类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactPayload {
    FcsEvents(FcsDocument),
    SpilloverMeta(SpilloverMeta),
    CompMatrix(CompMatrix),
    TransformParams(TransformParams),
    GatePolygon(GatePolygon),
    GateHierarchy(GateHierarchy),
}

impl ArtifactPayload {
    pub fn kind_name(&self) -> &'static str {
        match self {
            ArtifactPayload::FcsEvents(_) => "fcs_events",
            ArtifactPayload::SpilloverMeta(_) => "spillover_meta",
            ArtifactPayload::CompMatrix(_) => "comp_matrix",
            ArtifactPayload::TransformParams(_) => "transform_params",
            ArtifactPayload::GatePolygon(_) => "gate_polygon",
            ArtifactPayload::GateHierarchy(_) => "gate_hierarchy",
        }
    }
}
