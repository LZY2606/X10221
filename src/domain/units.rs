//! 量纲与坐标基准。
//!
//! 所有领域量都必须携带单位或坐标基准：原始通道强度使用仪器原始量纲，
//! 补偿后通道使用通道单位，变换后通道使用变换坐标。派生层才做规范化，
//! 任何无法转换的值都留在原位置并附诊断。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 一个数值可以同时保留「原始表示」与「规范化表示」。
/// 无法转换时 `normalized` 为 None，并在 `diagnostic` 记录原因。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Measured {
    /// 原始表示，例如 FCS 中按大端 f32 读出的数字，或无法解析的原始 token。
    pub raw: String,
    /// 规范化为 f64 后的值；不可转换时为 None（NaN / Inf 也保留 None 还是 Some？见下）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized: Option<f64>,
    pub unit: Unit,
    /// 非空表示该值无法被正常使用（如 NaN、溢出、文本非法），但值不丢弃。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

impl Measured {
    pub fn raw_f32(bits: u32, unit: Unit) -> Self {
        let v = f32::from_bits(bits);
        let raw = format!("{v}");
        if v.is_nan() {
            Measured { raw, normalized: None, unit, diagnostic: Some("NaN：保留原始位型，不参与补偿/门控".into()) }
        } else if v.is_infinite() {
            Measured { raw, normalized: None, unit, diagnostic: Some("Inf：超出 f32 有限范围，保留原位".into()) }
        } else {
            Measured { raw, normalized: Some(v as f64), unit, diagnostic: None }
        }
    }

    pub fn finite(&self) -> Option<f64> {
        self.normalized.filter(|v| v.is_finite())
    }
}

/// 单位 / 坐标基准。坐标系的轴在每个门里显式声明 `x_axis/y_axis`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// 仪器原始通道计数（FCS $PnE 的线性原始强度）。
    RawIntensity,
    /// 补偿后通道强度，单位与对应单染对照一致。
    Compensated,
    /// 双曲反正弦（或线性）变换后的无量纲坐标。
    Transformed,
    /// 散射光任意单位（FSC/SSC 仪器相关单位）。
    ScatterAru,
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Unit::RawIntensity => "raw_intensity",
            Unit::Compensated => "compensated",
            Unit::Transformed => "transformed",
            Unit::ScatterAru => "scatter_aru",
        };
        f.write_str(s)
    }
}
