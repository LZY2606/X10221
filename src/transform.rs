//! 变换参数：每个通道携带单位与坐标基准。
//! 无法转换的值留在原位置并附诊断，不能为了画图而丢掉。

use serde_json::json;

/// 对单个通道应用变换。返回 (变换后序列, 诊断)。
/// 未知基准 → 原样保留并逐通道附诊断。
pub fn apply_channel(values: &[f64], basis: &str) -> (Vec<f64>, Vec<String>) {
    match basis {
        "linear" => (values.to_vec(), vec![]),
        "log" => (
            values
                .iter()
                .map(|&v| if v > 0.0 { v.ln() } else { 0.0 })
                .collect(),
            vec![],
        ),
        "sqrt" => (
            values
                .iter()
                .map(|&v| if v >= 0.0 { v.sqrt() } else { 0.0 })
                .collect(),
            vec![],
        ),
        other => (
            values.to_vec(),
            vec![format!("未知坐标基准 '{other}'：值保留原始表示，未做规范化")],
        ),
    }
}

/// 从变换版本载荷中取某通道的基准；缺省 linear。
pub fn basis_for(transform_payload: &serde_json::Value, channel: &str) -> String {
    transform_payload
        .get("channels")
        .and_then(|c| c.get(channel))
        .and_then(|c| c.get("basis"))
        .and_then(|b| b.as_str())
        .unwrap_or("linear")
        .to_string()
}

pub fn diagnostics_event_payload(channel: &str, diagnostics: &[String]) -> serde_json::Value {
    json!({"channel": channel, "diagnostics": diagnostics})
}
