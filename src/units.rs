//! 坐标基准与单位。
//!
//! 规则：所有领域量都携带单位或坐标基准；导入时保留原始表示，
//! 只有派生层才做规范化。无法转换的值留在原位置并附诊断。

use serde::{Deserialize, Serialize};

/// 一个物理量的原始表示 + 解析结果。
/// `raw` 保留导入时的字面量（例如 FCS 事件中的 NaN bit pattern、
/// `FCS` 关键字字符串、"1.0e4" 文本），`value` 是派生层解析结果；
/// 解析失败时 value=None 且诊断写入导入事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawNumber {
    pub raw: String,
    pub value: Option<f64>,
}

impl RawNumber {
    pub fn from_str(s: &str) -> Self {
        let t = s.trim();
        let value = match t.to_ascii_uppercase().as_str() {
            "NAN" | "NA" => Some(f64::NAN),
            "INF" | "+INF" => Some(f64::INF),
            "-INF" => Some(f64::NEG_INFINITY),
            _ => t.parse::<f64>().ok(),
        };
        RawNumber { raw: s.to_string(), value }
    }
    pub fn finite(s: impl Into<String>, v: f64) -> Self {
        RawNumber { raw: s.into(), value: Some(v) }
    }
}

/// 坐标基准（coordinate basis）：每个散点轴、多边形顶点都必须声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "snake_case")]
pub enum CoordBasis {
    /// 仪器原始线性信号（FCS 通道值，未经任何变换/补偿）。
    Raw { channel: String, unit: String },
    /// 补偿后的线性信号。
    Compensated { channel: String, unit: String, matrix_version: String },
    /// 补偿后再经变换（如 asinh）。
    Transformed {
        channel: String,
        transform_version: String,
    },
}

impl CoordBasis {
    pub fn channel(&self) -> &str {
        match self {
            CoordBasis::Raw { channel, .. }
            | CoordBasis::Compensated { channel, .. }
            | CoordBasis::Transformed { channel, .. } => channel,
        }
    }
    pub fn label(&self) -> String {
        match self {
            CoordBasis::Raw { channel, unit } => format!("{} (raw {})", channel, unit),
            CoordBasis::Compensated { channel, unit, matrix_version } => {
                format!("{} (comp {} {})", channel, unit, &matrix_version[..10])
            }
            CoordBasis::Transformed { channel, transform_version } => {
                format!("{} (asinh {})", channel, &transform_version[..10])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_repr_is_preserved() {
        let n = RawNumber::from_str("  1.0e4 ");
        assert_eq!(n.raw, "  1.0e4 ");
        assert_eq!(n.value, Some(1.0e4));
        let bad = RawNumber::from_str("OVERFLOW");
        assert_eq!(bad.value, None);
        assert_eq!(bad.raw, "OVERFLOW");
        let nan = RawNumber::from_str("NaN");
        assert!(nan.value.unwrap().is_nan());
    }
}
