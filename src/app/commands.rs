//! HTTP/测试侧命令载荷（反序列化）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ImportMeta {
    /// FCS 文件名（空则用内置名）。
    pub filename: Option<String>,
    /// 附带的单染对照/spillover 草案等 blob（可选）。
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdjudicateComp {
    pub branch: String,
    /// 乐观并发：客户端读到的分支头 hash。
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub matrix_id: String,
    /// accept | reject
    pub decision: String,
    /// accept 时的候选 kind：exact_inverse / ridge_regularized。
    #[serde(default)]
    pub candidate: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReviseTransform {
    pub branch: String,
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub channel: String,
    pub spec: TransformIn,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformIn {
    Linear { offset: f64, scale: f64, unit: String },
    ArcSinh { cofactor: f64, unit: String },
}

#[derive(Debug, Deserialize)]
pub struct PointIn {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Deserialize)]
pub struct ReviseGate {
    pub branch: String,
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub gate_id: String,
    /// 改名（身份不变）。None 表示不改。
    #[serde(default)]
    pub new_name: Option<String>,
    /// 新多边形（None 表示只改名）。
    #[serde(default)]
    pub polygon: Option<Vec<PointIn>>,
    /// 改变父门（用于“门嵌套改名/重挂”场景）。
    #[serde(default)]
    pub new_parent: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BoundaryCall {
    pub branch: String,
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub gate_id: String,
    pub event_id: String,
    /// include | exclude
    pub call: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForkDraft {
    pub branch: String,
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub new_branch: String,
    pub label: String,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UndoCall {
    pub branch: String,
    #[serde(default)]
    pub base_head_hash: Option<String>,
    pub adjudication_id: String,
    #[serde(default)]
    pub actor: Option<String>,
}
