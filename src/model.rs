//! 流式领域模型：来源材料与解释分开，原始表示原样保留。
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 规则版本：矩阵近奇异阈值、边界点规则等判定逻辑都钉在此版本上。
pub const RULES_VERSION: &str = "flowline-rules/2026.09.1";
/// 1 范数条件数倒数（rcond）低于该阈值即判定为近奇异，禁止默默求逆。
pub const NEAR_SINGULAR_RCOND: f64 = 1e-8;
/// 精确奇异判定阈值。
pub const SINGULAR_DET: f64 = 1e-12;

/// 指向某类材料的某个版本。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatRef {
    pub kind: String,
    pub id: String,
    pub version: u32,
}

impl std::fmt::Display for MatRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}@v{}", self.kind, self.id, self.version)
    }
}

/// 通道规格：单位/坐标基准必须随数据携带。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub name: String,
    pub detector: String,
    #[serde(default)]
    pub fluor: Option<String>,
    pub unit: String,
    pub scale: String,
}

/// 单个细胞事件；值保留原始 JSON 表示（数字/字符串均允许），派生层才规范化。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CellEvent {
    pub id: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventFile {
    pub format: String,
    #[serde(default)]
    pub coordinate_basis: Option<String>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<CellEvent>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spillover {
    pub id: String,
    pub version: u32,
    pub source: String,
    pub unit_dimension: String,
    pub channels: Vec<String>,
    /// 行=检测器输出，列=荧光素真实信号，对角归一为 1。
    pub spillover: Vec<Vec<f64>>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Compensation {
    pub id: String,
    pub version: u32,
    pub spillover_ref: MatRef,
    /// 显式给出的逆矩阵；省略时由引擎在可安全求逆时推导。
    #[serde(default)]
    pub inverse: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AxisMap {
    pub channel: String,
    pub output_axis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transform {
    pub id: String,
    pub version: u32,
    pub method: String, // linear | asinh | log10
    pub input_unit: String,
    pub output_unit: String,
    #[serde(default)]
    pub cofactor: Option<f64>,
    #[serde(default)]
    pub decades: Option<f64>,
    pub axes: Vec<AxisMap>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct GateAxis {
    pub channel: String,
    /// linear=原始线性计数；transformed=引用 transform_ref 的输出坐标基准。
    pub space: String,
    #[serde(default)]
    pub transform_ref: Option<MatRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gate {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub x: GateAxis,
    pub y: GateAxis,
    pub polygon: Vec<[f64; 2]>,
    /// v1：边界点判定规则，记录规则版本。
    pub boundary_rule: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Population {
    pub id: String,
    pub name: String,
    /// null 表示根群（全部事件）。
    pub parent: Option<String>,
    pub gate: MatRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lineage {
    pub id: String,
    pub version: u32,
    pub populations: Vec<Population>,
}

/// 一份版本化的来源材料。raw 保留导入时的原始 JSON 字节。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Material {
    pub kind: String,
    pub id: String,
    pub version: u32,
    #[serde(default)]
    pub replaces: Option<u32>,
    pub title: String,
    pub raw: Value,
    #[serde(default)]
    pub source_file: Option<String>,
}

/// 事件分类（机器可读）。
pub mod evkind {
    pub const IMPORT: &str = "material.imported";
    pub const COMP_DIAG: &str = "compensation.diagnostic";
    pub const COMP_DECISION: &str = "compensation.decision";
    pub const VALUE_DIAG: &str = "value.diagnostic";
    pub const STALE: &str = "result.stale";
    pub const REFRESH: &str = "result.refresh";
    pub const RENAME: &str = "gate.renamed";
    pub const UNDO: &str = "decision.undone";
    pub const FORK: &str = "branch.forked";
    pub const CONFLICT: &str = "version.conflict";
    pub const ROLLBACK: &str = "import.rolled_back";
}
