//! 离线案例包（cases/flow-chamber）与 API 共用的数据模型。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseManifest {
    pub case_id: String,
    pub title: String,
    pub description: String,
    /// 按顺序导入的材料文件（相对案例目录）
    pub materials: Vec<MaterialFile>,
    /// 初始分支与基线头版本
    pub branches: Vec<BranchSpec>,
    /// 固定重放脚本
    pub replay: Vec<ReplayStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialFile {
    pub kind: String, // events|spillover|matrix|transform|gate|lineage
    pub path: String,
    pub lineage_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSpec {
    pub branch_id: String,
    pub label: String,
    pub parent_branch: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStep {
    pub step: String,
    pub kind: String,
    pub branch: String,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub candidate: Option<String>,
    #[serde(default)]
    pub expect_status: Option<String>,
    #[serde(default)]
    pub expect_stale: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsFile {
    pub source: String,
    pub fcs: String,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub index: usize,
    pub name: String,
    pub detector: String,
    pub fluorochrome: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    /// 原始数值（无法转换为 None，原始表示在 raw_repr 中保留）
    pub values: Vec<Option<f64>>,
    pub raw_repr: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub event_id: Option<String>,
    pub channel: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub lineage_id: String,
    pub source: String,
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub single_stain_controls: Vec<SingleStain>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleStain {
    pub fluorochrome: String,
    pub control_events: String,
    pub fcs_keyword: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixFile {
    pub lineage_id: String,
    pub matrix: MatrixData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixData {
    pub detectors: Vec<String>,
    pub fluorochromes: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformFile {
    pub lineage_id: String,
    pub channel: String,
    pub family: String,
    pub cofactor: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateFile {
    pub lineage_id: String,
    pub name: String,
    pub parent_gate: Option<String>,
    pub basis: String,
    pub x_axis: AxisSpec,
    pub y_axis: AxisSpec,
    pub vertices: Vec<[f64; 2]>,
    pub boundary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisSpec {
    pub channel: String,
    pub transform_id: String,
    pub transform_mvid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageFile {
    pub lineage_id: String,
    pub root_population: String,
    pub edges: Vec<LineageEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageEdge {
    pub parent_gate: Option<String>,
    pub child_gate: String,
    pub child_population: String,
}
