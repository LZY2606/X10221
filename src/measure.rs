//! 领域量：所有量都携带单位或坐标基准；原始表示在导入时保留，
//! 只有派生层才做规范化。无法转换的值留在原位置并附诊断。
//! 规则：不能为了画图而丢掉无法转换的值（CASE-MEAS-01）。

use serde::{Deserialize, Serialize};

/// 探测器信号的原始表示。导入时逐字保留，不做任何不可逆清洗。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawMeasure {
    /// 文件中出现的原始词法（如 "1234.5"、"-0.11"、"Infinity"、"OVFL"、"NaN"）。
    pub raw: String,
    /// 规范化后的数值与单位基准；无法转换时为 None，诊断写入 diagnostics。
    pub value: Option<f64>,
    /// 坐标/单位基准：探测器通道（例如 "channel:FITC"），不同于补偿后的荧光单位。
    pub basis: String,
    /// 规范化时的诊断代码（空表示无诊断）。
    pub diagnostics: Vec<String>,
}

impl RawMeasure {
    pub fn detector(raw: impl Into<String>, channel: impl Into<String>) -> Self {
        let raw = raw.into();
        let basis = format!("channel:{}", channel.into());
        let (value, mut diagnostics) = parse_detector_literal(&raw);
        if let Some(v) = value {
            if !v.is_finite() {
                value = None;
                diagnostics.push("EXTREME_NON_FINITE".to_string());
            } else if v < 0.0 {
                diagnostics.push("EXTREME_NEGATIVE_BASELINE".to_string());
            }
        }
        RawMeasure { raw, value, basis, diagnostics }
    }

    pub fn is_convertible(&self) -> bool {
        self.value.is_some()
    }
}

/// 解析探测器词法。非数值/越界词法返回 None 与诊断代码，绝不静默丢弃。
pub fn parse_detector_literal(raw: &str) -> (Option<f64>, Vec<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, vec!["EMPTY_VALUE".to_string()]);
    }
    let upper = trimmed.to_ascii_uppercase();
    if matches!(upper.as_str(), "OVFL" | "OVERFLOW" | "OOR>" | "INFINITY" | "+INFINITY" | "INF") {
        return (None, vec!["OVERFLOW".to_string()]);
    }
    if matches!(upper.as_str(), "NAN" | "-NAN") {
        return (None, vec!["NOT_A_NUMBER".to_string()]);
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() => (Some(v), vec![]),
        Ok(v) if v.is_infinite() => (Some(v), vec!["OVERFLOW".to_string()]),
        Ok(_) => (None, vec!["NOT_A_NUMBER".to_string()]),
        Err(_) => (None, vec!["UNPARSEABLE_LITERAL".to_string()]),
    }
}

/// 已规范化、带坐标基准的派生量（补偿/变换后的事件信号）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaledMeasure {
    pub value: f64,
    /// 坐标基准："channel:FITC"（补偿后仍以探测器通道为基准）或
    /// "transform:asinh:channel:FITC"（门控坐标基准）。
    pub basis: String,
    /// 单位标签，例如 "MEFL-like:f64"、"transformed:dimensionless"。
    pub unit: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_stays_in_place_with_diagnostic() {
        let m = RawMeasure::detector("OVFL", "FITC");
        assert_eq!(m.raw, "OVFL");
        assert!(m.value.is_none());
        assert!(m.diagnostics.contains(&"OVERFLOW".to_string()));
        assert_eq!(m.basis, "channel:FITC");
    }

    #[test]
    fn non_finite_infinity_is_marked_extreme() {
        let m = RawMeasure::detector("Infinity", "PE");
        assert!(m.value.is_none());
        assert!(m.diagnostics.iter().any(|d| d == "EXTREME_NON_FINITE"));
    }

    #[test]
    fn ordinary_value_keeps_basis_and_unit_reference() {
        let m = RawMeasure::detector("1234.5", "FITC");
        assert_eq!(m.value, Some(1234.5));
        assert!(m.diagnostics.is_empty());
        assert_eq!(m.basis, "channel:FITC");
    }

    #[test]
    fn negative_baseline_is_flagged_but_kept() {
        let m = RawMeasure::detector("-0.11", "FITC");
        assert_eq!(m.value, Some(-0.11));
        assert!(m.diagnostics.iter().any(|d| d == "EXTREME_NEGATIVE_BASELINE"));
    }
}
