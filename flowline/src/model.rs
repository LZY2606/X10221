//! 领域模型：来源与解释分离的原始材料类型。
//!
//! - 原始通道值导入时保留原始表示（字符串），派生层才解析为 f64；
//! - 无法转换的值留在原位置并附诊断，绝不为画图丢弃；
//! - 所有领域量携带单位或坐标基准。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    pub id: String,
    pub label: String,
    pub fluorophore: Option<String>,
    /// 原始单位基准，如 "linear_counts"。
    pub unit: String,
}

/// 单个事件（细胞）的原始通道值。raw 保留导入时的字符串表示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawEvent {
    pub id: String,
    pub values: Vec<RawCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RawCell {
    pub channel: String,
    /// 原始表示，例如 "1234.5" 或 "OVRFLW"。
    pub raw: String,
    /// 解析值；无法转换时为 None，诊断记录原因。
    pub value: Option<f64>,
    pub unit: String,
    pub diagnostic: Option<String>,
}

/// 单染对照测得的 spillover（S 矩阵）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpilloverSpec {
    pub id: String,
    pub channels: Vec<String>,
    /// 行=检测通道，列=荧光染料（与 channels 同序）。
    pub matrix: Vec<Vec<f64>>,
    /// 单染对照事件引用，说明每个矩阵元素的来源。
    pub single_stain_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformSpec {
    pub channel: String,
    pub basis: String,
    pub cofactor: f64,
    pub scale: f64,
}

/// 门多边形。vertices_basis 明确顶点坐标基准。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateDef {
    /// 稳定身份，改名不改变。
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub x_channel: String,
    pub y_channel: String,
    pub vertices_basis: String, // asinh_compensated | linear_counts
    pub vertices: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CasePack {
    pub case_id: String,
    pub title: String,
    pub channels: Vec<Channel>,
    pub raw_events: Vec<RawEvent>,
    pub spillover: SpilloverSpec,
    pub transforms: Vec<TransformSpec>,
    pub gates: Vec<GateDef>,
}

/// 解析一个原始表示。保留原值，失败时附诊断而非丢弃。
pub fn parse_raw_cell(channel: &str, raw: &str, unit: &str) -> RawCell {
    let trimmed = raw.trim();
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() => RawCell {
            channel: channel.into(),
            raw: raw.to_string(),
            value: Some(v),
            unit: unit.into(),
            diagnostic: None,
        },
        _ => RawCell {
            channel: channel.into(),
            raw: raw.to_string(),
            value: None,
            unit: unit.into(),
            diagnostic: Some(format!("无法把原始表示 {raw:?} 转换为 {} 下的数值", unit)),
        },
    }
}
