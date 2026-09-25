//! 领域基础类型：带单位/坐标基准的量、稳定身份、机器可读的判定事件。
use serde::{Deserialize, Serialize};

/// 领域量：始终携带单位或坐标基准；raw 保留导入时的原始表示。
/// 无法转换的值 value=None，raw 留在原位置，诊断挂在所属记录上。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Qty {
    #[serde(default)]
    pub value: Option<f64>,
    pub unit: String,
    pub basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Qty {
    pub fn new(value: f64, unit: &str, basis: &str) -> Self {
        Qty { value: Some(value), unit: unit.into(), basis: basis.into(), raw: None }
    }
    pub fn unconvertible(raw: &str, unit: &str, basis: &str) -> Self {
        Qty { value: None, unit: unit.into(), basis: basis.into(), raw: Some(raw.into()) }
    }
}

/// 一条原始通道值（导入层，绝不做不可逆清洗）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub event_id: String,
    pub file_id: String,
    pub row: usize,
    /// 解析后的通道值（f64 层，未补偿未变换）。
    pub values: Vec<f64>,
    /// 原始表示（FCS 数据段的十六进制字节），派生层规范化前的凭据。
    pub raw_hex: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<Diag>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diag {
    pub severity: String, // "warn" | "error" | "info"
    pub code: String,
    pub message: String,
}

/// 依赖引用：派生对象钉住来源的 (kind, id, version)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct DepRef {
    pub kind: String, // "matrix" | "transform" | "gate" | "tree" | "file" | "derived" | "popstat" | "spillover"
    pub id: String,
    pub version: i64,
}

/// 机器可读的判定依据（写入事件日志的 payload 片段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgment {
    pub rule: String,
    pub status: String,
    #[serde(default)]
    pub details: serde_json::Value,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}

/// 事件日志条目（追加式，永不改写）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRow {
    pub event_id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}

pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}
