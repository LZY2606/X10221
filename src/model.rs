//! 领域模型：所有领域量都携带单位或坐标基准；原始表示与派生解释分开保存。
use serde::{Deserialize, Serialize};

/// 通道元数据：单位 + 坐标基准（如 18-bit 检测器原始刻度）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMeta {
    pub name: String,
    pub unit: String,
    pub basis: String,
}

/// 数据集元数据（与原始行分开版本化）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasetMeta {
    pub id: String,
    pub description: String,
    pub channels: Vec<ChannelMeta>,
    pub source_file: String,
}

/// 原始事件行：raw_text 逐字保留导入时的表示，values 为未解析的原始字符串。
/// 派生层（transform/comp）才做数值化；无法转换的值留在原位置并附诊断。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawRow {
    pub index: usize,
    pub raw_text: String,
    pub values: Vec<String>,
}

/// spillover 元数据（单染对照来源）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpilloverFile {
    pub logical_id: String,
    pub version: u32,
    pub channels: Vec<String>,
    pub controls: Vec<serde_json::Value>,
    pub unit: String,
    pub basis: String,
}

/// 补偿矩阵：行=检测器，列=荧光素；补偿 = inv(S) * raw。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompMatrixFile {
    pub logical_id: String,
    pub version: u32,
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub source: String,
    pub unit: String,
    pub basis: String,
}

/// 变换参数（log10 演示变换；min/max 为可转换范围，越界留原值+诊断）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformFile {
    pub logical_id: String,
    pub version: u32,
    pub kind: String,
    pub channels: Vec<String>,
    pub min_signal: f64,
    pub max_signal: f64,
    pub unit: String,
    pub basis: String,
}

/// 单个门：多边形顶点以 raw 信号单位给出，评估时用所引用的变换同时变换顶点与事件，
/// 保证边界判定在同一坐标基准下进行。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateDef {
    pub name: String,
    pub parent: Option<String>,
    pub channels: Option<(String, String)>,
    pub polygon: Option<Vec<(f64, f64)>>,
}

/// 门树（含父子关系），整体作为一个可版本化 artifact。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateTreeFile {
    pub logical_id: String,
    pub version: u32,
    pub basis: String,
    pub gates: Vec<GateDef>,
}

/// 变换诊断：值留在原位置，绝不为了画图丢掉。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum TransformDiag {
    /// 低于可转换范围（如 <=0 无法取 log10）：保留原始值。
    BelowRange { kept_raw: f64 },
    /// 极端信号（超出检测器量程）：仍可转换，但标记。
    ExtremeSignal { value: f64 },
    /// 原始文本无法解析为数值。
    Unparseable { raw: String },
}

impl TransformDiag {
    pub fn kind(&self) -> &'static str {
        match self {
            TransformDiag::BelowRange { .. } => "below_transform_range",
            TransformDiag::ExtremeSignal { .. } => "extreme_signal",
            TransformDiag::Unparseable { .. } => "unparseable_raw",
        }
    }
}

/// 点的多边形分类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointClass {
    Inside,
    Outside,
    /// 恰好落在边界上：无法唯一决定，必须保留 include/exclude 两个合法候选。
    Boundary,
}
