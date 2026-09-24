//! 量纲与原始表示：导入时保留原表示，派生层才解析；无法转换留在原位并附诊断。

use super::ChannelValue;

/// 严格解析：只接受有限数；NaN/Inf/越界/空/符号错误都算无法转换。
pub fn parse_strict(raw: &str) -> Result<f64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty".to_string());
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        Ok(v) => Err(format!("non-finite value: {}", v)),
        Err(e) => Err(format!("unparseable: {}", e)),
    }
}

/// 把原始字符串变成 ChannelValue：解析失败时 value=None 且诊断附在原位。
pub fn channel_value(channel_id: &str, raw: &str, unit: &str) -> ChannelValue {
    let mut diagnostics = Vec::new();
    let value = match parse_strict(raw) {
        Ok(v) => Some(v),
        Err(d) => {
            diagnostics.push(format!("原始值无法规范化，保留原表示 `{}`: {}", raw, d));
            None
        }
    };
    ChannelValue {
        channel_id: channel_id.to_string(),
        raw: raw.to_string(),
        value,
        unit: unit.to_string(),
        diagnostics,
    }
}

/// 典型流式 ADC 饱和上界（2^18），仅用于生成“极端信号”诊断，不丢值。
pub const ADC_SATURATION: f64 = 262_144.0;

pub fn extreme_diagnostic(value: f64) -> Option<&'static str> {
    if value >= ADC_SATURATION {
        Some("极端信号：达到/超过 ADC 饱和上界 262144，保留但不参与不可验证的外推")
    } else if value < 0.0 {
        Some("极端信号：负值，保留并在补偿后复核")
    } else {
        None
    }
}
