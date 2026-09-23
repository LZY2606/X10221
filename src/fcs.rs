//! 最小 FCS 3.1 读取器：解析 HEADER/TEXT/DATA 段。
//!
//! 只支持本仓库离线案例包需要的子集（MODE=L, DATATYPE=F 32-bit little-endian float），
//! 但严格保留每个测量的原始文本/二进制表示：无法转换的值留在原位置并附诊断，
//! 绝不为了画图而丢弃。

use std::collections::BTreeMap;
use crate::domain::{ChannelDef, Diagnostic, FlowEvent, Measurement};

#[derive(Debug)]
pub struct FcsFile {
    pub header: String,
    pub text: String,
    pub keywords: BTreeMap<String, String>,
    pub channels: Vec<ChannelDef>,
    pub events: Vec<FlowEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum FcsError {
    #[error("FCS 文件过短：{0} 字节")]
    TooShort(usize),
    #[error("不是 FCS 文件：{0:?}")]
    NotFcs(String),
    #[error("不支持的 FCS 版本：{0}")]
    Version(String),
    #[error("不支持的 DATATYPE={0}（仅 F 32-bit float）")]
    DataType(String),
    #[error("不支持的 MODE={0}（仅 L）")]
    Mode(String),
    #[error("TEXT 段偏移越界：begin={begin} end={end} len={len}")]
    TextOutOfRange { begin: usize, end: usize, len: usize },
    #[error("DATA 段偏移越界：begin={begin} end={end} len={len}")]
    DataOutOfRange { begin: usize, end: usize, len: usize },
    #[error("缺少关键字：{0}")]
    MissingKeyword(String),
    #[error("DATA 长度 {actual} 与事件数 {events}×通道数 {params}={expected} 不匹配")]
    DataLength { actual: usize, expected: usize, events: usize, params: usize },
}

fn uslice(s: &str, a: usize, b: usize) -> &str {
    let b = b.min(s.len());
    if a >= s.len() || a > b { "" } else { &s[a..b] }
}

/// 解析 TEXT 段：以分隔符（首字节）分割的 key/value 序列。
pub fn parse_keywords(text: &str) -> BTreeMap<String, String> {
    let bytes = text.as_bytes();
    let delim = bytes.first().copied().unwrap_or(b'|') as char;
    let mut out = BTreeMap::new();
    let body = text.trim_matches(|c| c == delim);
    let mut parts = body.split(delim);
    while let Some(key) = parts.next() {
        if key.is_empty() {
            continue;
        }
        let value = parts.next().unwrap_or("");
        out.insert(key.to_string(), value.to_string());
    }
    out
}

fn parse_usize(kw: &BTreeMap<String, String>, name: &str) -> Result<usize, FcsError> {
    let v = kw.get(name).ok_or_else(|| FcsError::MissingKeyword(name.to_string()))?;
    v.trim().parse::<usize>().map_err(|_| FcsError::MissingKeyword(name.to_string()))
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 {
        return Err(FcsError::TooShort(bytes.len()));
    }
    let header = String::from_utf8_lossy(&bytes[..58]).to_string();
    if !header.starts_with("FCS") {
        return Err(FcsError::NotFcs(header.chars().take(10).collect()));
    }
    let version = uslice(&header, 3, 6).trim().to_string();
    if !version.starts_with("3.1") && !version.starts_with("3.0") {
        return Err(FcsError::Version(version));
    }

    let text_begin: usize = uslice(&header, 10, 18).trim().parse().unwrap_or(0);
    let text_end_s: usize = uslice(&header, 18, 26).trim().parse().unwrap_or(0);
    let mut data_begin: usize = uslice(&header, 26, 34).trim().parse().unwrap_or(0);
    let mut data_end_s: usize = uslice(&header, 34, 42).trim().parse().unwrap_or(0);

    if text_begin >= bytes.len() || text_end_s > bytes.len() || text_begin > text_end_s {
        return Err(FcsError::TextOutOfRange { begin: text_begin, end: text_end_s, len: bytes.len() });
    }
    let text = String::from_utf8_lossy(&bytes[text_begin..=text_end_s.min(bytes.len() - 1)]).to_string();
    let keywords = parse_keywords(&text);

    // 大偏移段（FCS 3.1 允许 $BEGINSTEXT/$BEGINDATA 覆盖头偏移）
    if let Some(v) = keywords.get("$BEGINDATA") {
        if let Ok(v) = v.trim().parse::<usize>() { data_begin = v; }
    }
    if let Some(v) = keywords.get("$ENDDATA") {
        if let Ok(v) = v.trim().parse::<usize>() { data_end_s = v; }
    }

    let datatype = keywords.get("$DATATYPE").cloned().unwrap_or_default();
    if datatype != "F" {
        return Err(FcsError::DataType(datatype));
    }
    let mode = keywords.get("$MODE").cloned().unwrap_or_else(|| "L".to_string());
    if mode != "L" {
        return Err(FcsError::Mode(mode));
    }
    let n_params = parse_usize(&keywords, "$PAR")?;
    let n_events = parse_usize(&keywords, "$TOT")?;
    if data_begin >= bytes.len() || data_end_s >= bytes.len() || data_begin > data_end_s {
        return Err(FcsError::DataOutOfRange { begin: data_begin, end: data_end_s, len: bytes.len() });
    }
    let data = &bytes[data_begin..=data_end_s];
    let expected = n_events * n_params * 4;
    if data.len() < expected {
        return Err(FcsError::DataLength { actual: data.len(), expected, events: n_events, params: n_params });
    }

    let mut channels = Vec::with_capacity(n_params);
    for i in 1..=n_params {
        let pn = format!("$P{}N", i);
        let optical_name = keywords.get(&pn).cloned().ok_or_else(|| FcsError::MissingKeyword(pn))?;
        let short_name = keywords
            .get(&format!("$P{}SNAME", i))
            .cloned()
            .unwrap_or_else(|| optical_name.clone());
        let stain = keywords.get(&format!("$P{}S", i)).cloned();
        let detector = keywords.get(&format!("$P{}DET", i)).cloned().unwrap_or_else(|| short_name.clone());
        let unit = keywords.get(&format!("$P{}UNIT", i)).cloned().unwrap_or_else(|| "ADC counts".to_string());
        let basis = keywords.get(&format!("$P{}BASIS", i)).cloned().unwrap_or_else(|| "raw".to_string());
        let gain: f64 = keywords.get(&format!("$P{}G", i)).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let amplifier = keywords.get(&format!("$P{}AMP", i)).cloned().unwrap_or_else(|| "linear".to_string());
        let fluorescent = keywords
            .get(&format!("$P{}FL", i))
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|| detector.starts_with("FL"));
        channels.push(ChannelDef {
            short_name, optical_name, stain, detector, unit, basis, gain, amplifier, fluorescent,
        });
    }

    let overflow_token = keywords.get("$P13CAL") // 自定义：极端/无法转换标记通道的原始 token
        .cloned();
    let bad_tokens: BTreeMap<String, String> = keywords
        .iter()
        .filter(|(k, _)| k.starts_with("$FLOWBADSPIKE "))
        .map(|(k, v)| (k.trim_start_matches("$FLOWBADSPIKE ").to_string(), v.clone()))
        .collect();
    let _ = overflow_token;

    let mut events = Vec::with_capacity(n_events);
    let mut floats = Vec::with_capacity(n_events * n_params);
    for chunk in data[..expected].chunks_exact(4) {
        floats.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64);
    }
    for (idx, row) in floats.chunks_exact(n_params).enumerate() {
        let mut map = BTreeMap::new();
        let mut diagnostics = Vec::new();
        for (ci, channel) in channels.iter().enumerate() {
            let raw_bits = &data[(idx * n_params + ci) * 4..(idx * n_params + ci) * 4 + 4];
            let raw_token = format_raw_token(raw_bits, row[ci]);
            // 自定义关键字：$FLOWBADSPIKE "<eventidx>:<channelshort>" = "<原始坏 token>"
            let spike_key = format!("{}:{}", idx + 1, channel.short_name);
            if let Some(bad_token) = bad_tokens.get(&spike_key) {
                diagnostics.push(Diagnostic {
                    channel: channel.short_name.clone(),
                    code: "unconvertible".to_string(),
                    message: format!("原始表示 {:?} 无法转换为 f64，已保留在原位置（不参与几何计算）", bad_token),
                    raw_token: bad_token.clone(),
                });
                map.insert(channel.short_name.clone(), Measurement::Unconvertible {
                    raw_token: bad_token.clone(),
                    diagnostic: "unconvertible token in FCS TEXT keyword".to_string(),
                });
            } else {
                let mut value = row[ci];
                let mut code: Option<&'static str> = None;
                if value.is_nan() {
                    code = Some("nan");
                } else if value.is_infinite() {
                    code = Some("overflow");
                    value = value.clamp(-1.0e6, 1.0e6);
                } else if value < 0.0 {
                    code = Some("negative_voltage");
                } else if value >= 262_143.0 {
                    code = Some("saturated");
                }
                if let Some(c) = code {
                    diagnostics.push(Diagnostic {
                        channel: channel.short_name.clone(),
                        code: c.to_string(),
                        message: match c {
                            "overflow" => "超出量程（Inf），按钳制保留位置，不丢点".to_string(),
                            "saturated" => "达到 ADC 满量程（262143），饱和事件".to_string(),
                            "negative_voltage" => "原始信号为负（基线以下电压），保留负值".to_string(),
                            _ => "非有限值，保留位置与诊断".to_string(),
                        },
                        raw_token: raw_token.clone(),
                    });
                }
                map.insert(channel.short_name.clone(), Measurement::Value { value, raw_token });
            }
        }
        let id_stem = format!("evt-{:04}", idx + 1);
        events.push(FlowEvent { id: id_stem, index: idx, channels: map, diagnostics });
    }

    Ok(FcsFile { header, text, keywords, channels, events })
}

fn format_raw_token(bits: &[u8], value: f64) -> String {
    let u = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
    if value.is_nan() {
        format!("f32:NaN(0x{:08X})", u)
    } else if value.is_infinite() {
        if value > 0.0 { format!("f32:+Inf(0x{:08X})", u) } else { format!("f32:-Inf(0x{:08X})", u) }
    } else {
        let f = f32::from_bits(u);
        format!("f32:{}", f)
    }
}
