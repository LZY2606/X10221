use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 原始通道值：导入时保留原始字符串表示，派生层才做数值化。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RawChannels {
    pub unit: String,
    pub coordinate_basis: String,
    pub detector_max: f64,
    pub channels: Vec<String>,
    pub events: Vec<RawEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompMatrix {
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub derived_from: String,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransformParams {
    pub kind: String,
    pub cofactor: BTreeMap<String, f64>,
    pub output_basis: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gate {
    pub id: String,
    pub channels: [String; 2],
    pub space: String,
    pub boundary_policy: String,
    pub polygon: Vec<[f64; 2]>,
}

/// 父子关系与显示名（与门多边形分开版本化）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateTree {
    pub nodes: Vec<TreeNode>,
    pub edges: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub gate_id: String,
    pub name: String,
}
