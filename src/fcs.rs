//! 最小可用的 FCS 3.1 文本/数据段读写器。
//!
//! 设计原则（对应验收要求“导入时保留原始表示”）：
//! - 不丢弃任何事件；NaN/Inf 以字符串形式保留在原始通道值层，仅在派生层才规范化；
//! - 所有量携带单位或坐标基准（`$PnE` 增益、`$PnT` 量程、`UNIT/` 关键词）；
//! - 解析失败只返回错误，由上层在一个数据库事务里回滚，不产生半份记录。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub enum FcsError {
    TooShort,
    NotFcs,
    BadOffset(&'static str),
    BadHeader(String),
    BadKeyword(String),
    Unsupported(&'static str),
    BadEvent,
}

impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcsError::TooShort => write!(f, "文件短于 FCS3.1 头部 58 字节"),
            FcsError::NotFcs => write!(f, "不是 FCS 文件（缺少 FCS3.1 魔数）"),
            FcsError::BadOffset(s) => write!(f, "段偏移非法: {s}"),
            FcsError::BadHeader(s) => write!(f, "FCS 头部错误: {s}"),
            FcsError::BadKeyword(s) => write!(f, "文本段关键词错误: {s}"),
            FcsError::Unsupported(s) => write!(f, "暂不支持的数据表示: {s}"),
            FcsError::BadEvent => write!(f, "事件数据长度与参数声明不一致"),
        }
    }
}
impl std::error::Error for FcsError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    /// 通道短名，如 FITC-A
    pub name: String,
    /// 检测器/荧光素描述（$PnS）
    pub label: Option<String>,
    /// 原始数据类型（F=float32, D=float64, I=integer）
    pub raw_type: char,
    /// (amplification gain, amplification type)，0,0=线性
    pub gain: (f64, f64),
    /// 量程（坐标基准上界）；NaN 时序列化为 null，不伪造数值
    #[serde(serialize_with = "serde_f64_opt", default)]
    pub range: f64,
    /// 单位（UNIT/Pn，缺省为 raw detector unit）
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsFile {
    pub version: String,
    /// 保留原始文本段键值（顺序不敏感，键统一大写）
    pub keywords: BTreeMap<String, String>,
    pub channels: Vec<ChannelSpec>,
    /// 每个事件每个通道的“原始表示”字符串（永远不丢值）
    pub raw_events: Vec<Vec<String>>,
    /// 与 raw_events 一一对应的 f64 解释；无法转换处为 NaN 并写入 diagnostics
    #[serde(with = "nan_matrix")]
    pub events: Vec<Vec<f64>>,
    /// 针对事件级别的诊断（下标 + 通道 + 原因）
    pub diagnostics: Vec<EventDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventDiagnostic {
    pub event_index: usize,
    pub channel: String,
    pub code: String,
    pub detail: String,
}

fn serde_f64_opt<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() {
        s.serialize_f64(*v)
    } else {
        s.serialize_none()
    }
}

impl FcsFile {
    pub fn parse(bytes: &[u8]) -> Result<FcsFile, FcsError> {
        if bytes.len() < 58 {
            return Err(FcsError::TooShort);
        }
        if &bytes[0..6] != b"FCS3.1" && &bytes[0..6] != b"FCS3.0" {
            return Err(FcsError::NotFcs);
        }
        let version = String::from_utf8_lossy(&bytes[0..6]).to_string();
        let text_start = read_offset(&bytes[10..18], "text start")?;
        let text_end = read_offset(&bytes[18..26], "text end")?;
        let data_start = read_offset(&bytes[26..34], "data start")?;
        let data_end = read_offset(&bytes[34..42], "data end")?;
        if text_end < text_start || data_end < data_start || text_start >= bytes.len() {
            return Err(FcsError::BadOffset("段范围越界或逆序"));
        }
        let delim = bytes[text_start];
        if delim == 0 || delim == b' ' {
            return Err(FcsError::BadHeader("文本分隔符非法".into()));
        }
        let text_body = &bytes[text_start + 1..=text_end.min(bytes.len() - 1)];
        let parts: Vec<&[u8]> = text_body.split(|b| *b == delim).collect();
        let mut keywords = BTreeMap::new();
        let mut pairs = parts.iter();
        while let Some(k) = pairs.next() {
            if k.is_empty() {
                continue;
            }
            let v = pairs.next().ok_or_else(|| {
                FcsError::BadKeyword("关键词没有配对值".into())
            })?;
            let key = std::str::from_utf8(k)
                .map_err(|_| FcsError::BadKeyword("非 ASCII 键".into()))?
                .to_string();
            let val = sanitize(std::str::from_utf8(v).unwrap_or_default());
            keywords.insert(key.to_uppercase(), val);
        }

        let n_par: usize = keywords
            .get("$PAR")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| FcsError::BadKeyword("$PAR 缺失".into()))?;
        let total: usize = keywords
            .get("$TOT")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| FcsError::BadKeyword("$TOT 缺失".into()))?;
        let data_type = keywords
            .get("$DATATYPE")
            .map(|s| s.as_str())
            .unwrap_or("");
        let byteord = keywords.get("$BYTEORD").cloned().unwrap_or_else(|| "1,2,3,4".into());

        let mut channels = Vec::with_capacity(n_par);
        for i in 1..=n_par {
            let name = keywords
                .get(&format!("$P{i}N"))
                .cloned()
                .ok_or_else(|| FcsError::BadKeyword(format!("$P{i}N 缺失")))?;
            let label = keywords.get(&format!("$P{i}S")).cloned();
            let raw_type = data_type.chars().next().unwrap_or('F');
            let gain = keywords
                .get(&format!("$P{i}E"))
                .and_then(|s| s.split_once(','))
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                .unwrap_or((0.0, 0.0));
            let range = keywords
                .get(&format!("$P{i}R"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(f64::NAN);
            let unit = keywords
                .get(&format!("UNIT/P{i}"))
                .cloned()
                .unwrap_or_else(|| "raw detector unit".into());
            channels.push(ChannelSpec {
                name,
                label,
                raw_type,
                gain,
                range,
                unit,
            });
        }

        let data = &bytes[data_start..=data_end.min(bytes.len() - 1)];
        let (events_f, raw_events, diagnostics) =
            decode_events(data, &channels, total, &byteord)?;
        Ok(FcsFile {
            version,
            keywords,
            channels,
            raw_events,
            events: events_f,
            diagnostics,
        })
    }
}

fn sanitize(s: &str) -> String {
    s.trim_matches('\u{0}').trim().to_string()
}

fn read_offset(slice: &[u8], what: &'static str) -> Result<usize, FcsError> {
    std::str::from_utf8(slice)
        .map_err(|_| FcsError::BadOffset(what))?
        .trim()
        .parse::<usize>()
        .map_err(|_| FcsError::BadOffset(what))
}

fn decode_events(
    data: &[u8],
    channels: &[ChannelSpec],
    total: usize,
    byteord: &str,
) -> Result<(Vec<Vec<f64>>, Vec<Vec<String>>, Vec<EventDiagnostic>), FcsError> {
    let little = byteord.starts_with("1,");
    let n = channels.len();
    let mut events = Vec::with_capacity(total);
    let mut raw = Vec::with_capacity(total);
    let mut diags = Vec::new();
    match channels.first().map(|c| c.raw_type) {
        Some('F') => {
            let stride = 4 * n;
            if data.len() < total * stride {
                return Err(FcsError::BadEvent);
            }
            for ei in 0..total {
                let mut row = Vec::with_capacity(n);
                let mut raws = Vec::with_capacity(n);
                for ci in 0..n {
                    let off = ei * stride + ci * 4;
                    let mut buf = [0u8; 4];
                    if little {
                        buf.copy_from_slice(&data[off..off + 4]);
                    } else {
                        for (k, b) in data[off..off + 4].iter().enumerate() {
                            buf[3 - k] = *b;
                        }
                    }
                    let v = f32::from_bits(u32::from_le_bytes(buf)) as f64;
                    raws.push(format_float_original(v));
                    if !v.is_finite() {
                        diags.push(EventDiagnostic {
                            event_index: ei,
                            channel: channels[ci].name.clone(),
                            code: if v.is_nan() { "NAN_VALUE".to_string() } else { "INFINITE_VALUE".to_string() },
                            detail: "原始信号非有限值，保留原位，不参与派生计算".into(),
                        });
                    } else if v < 0.0 {
                        diags.push(EventDiagnostic {
                            event_index: ei,
                            channel: channels[ci].name.clone(),
                            code: "NEGATIVE_RAW".to_string(),
                            detail: "原始信号为负（电子基线偏移），保留原位".into(),
                        });
                    } else if channels[ci].range.is_finite() && v > channels[ci].range {
                        diags.push(EventDiagnostic {
                            event_index: ei,
                            channel: channels[ci].name.clone(),
                            code: "SATURATED".to_string(),
                            detail: "超过声明量程，视为极端信号但不丢弃".into(),
                        });
                    }
                    row.push(v);
                }
                events.push(row);
                raw.push(raws);
            }
        }
        _ => return Err(FcsError::Unsupported("$DATATYPE（本离线包仅写/读 F 浮点）")),
    }
    Ok((events, raw, diags))
}

fn format_float_original(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v == f64::INFINITY {
        "Infinity".into()
    } else if v == f64::NEG_INFINITY {
        "-Infinity".into()
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 离线案例包生成器：把结构化事件编码成合法 FCS3.1 字节（小端 float32）。
pub fn write_fcs31(
    channel_names: &[String],
    events: &[Vec<f64>],
    extra_keywords: &BTreeMap<String, String>,
) -> Vec<u8> {
    let n = channel_names.len();
    let mut kws: BTreeMap<String, String> = BTreeMap::new();
    kws.insert("BEGINANALYSIS".into(), "0".into());
    kws.insert("ENDANALYSIS".into(), "0".into());
    kws.insert("BEGINSTEXT".into(), "0".into());
    kws.insert("ENDSTEXT".into(), "0".into());
    kws.insert("$MODE".into(), "L".into());
    kws.insert("$BYTEORD".into(), "1,2,3,4".into());
    kws.insert("$DATATYPE".into(), "F".into());
    kws.insert("$PAR".into(), n.to_string());
    kws.insert("$TOT".into(), events.len().to_string());
    for (i, name) in channel_names.iter().enumerate() {
        let p = i + 1;
        kws.insert(format!("$P{p}N"), name.clone());
        kws.insert(format!("$P{p}S"), format!("detector {name}"));
        kws.insert(format!("$P{p}E"), "0,0".into());
        kws.insert(format!("$P{p}R"), "262144".into());
        kws.insert(format!("UNIT/P{p}"), "raw detector unit".into());
    }
    for (k, v) in extra_keywords {
        kws.insert(k.clone(), v.clone());
    }
    let delim = b'\\';
    let mut text = Vec::new();
    text.push(delim);
    for (k, v) in &kws {
        text.extend_from_slice(k.as_bytes());
        text.push(delim);
        text.extend_from_slice(v.as_bytes());
        text.push(delim);
    }
    let data_len = events.len() * n * 4;
    let header_len = 256usize;
    let text_start = header_len;
    let text_end = text_start + text.len() - 1;
    let data_start = text_end + 1;
    let data_end = data_start + data_len - 1;
    let mut out = vec![0u8; data_end + 1];
    out[0..6].copy_from_slice(b"FCS3.1");
    out[6..10].copy_from_slice(b"    ");
    write_field(&mut out[10..18], text_start);
    write_field(&mut out[18..26], text_end);
    write_field(&mut out[26..34], data_start);
    write_field(&mut out[34..42], data_end);
    out[42..58].copy_from_slice(b"0       0       ");
    out[text_start..=text_end].copy_from_slice(&text);
    for (ei, row) in events.iter().enumerate() {
        for (ci, v) in row.iter().enumerate() {
            let off = data_start + (ei * n + ci) * 4;
            let bits = (*v as f32).to_bits().to_le_bytes();
            out[off..off + 4].copy_from_slice(&bits);
        }
    }
    out
}

fn write_field(slice: &mut [u8], v: usize) {
    let s = format!("{v:>8}");
    slice[..8].copy_from_slice(s.as_bytes());
}


mod nan_matrix {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Cell {
        Num(f64),
        Special(String),
        Null,
    }

    pub fn serialize<S: Serializer>(m: &[Vec<f64>], s: S) -> Result<S::Ok, S::Error> {
        let rows: Vec<Vec<Cell>> = m
            .iter()
            .map(|r| {
                r.iter()
                    .map(|v| {
                        if v.is_nan() {
                            Cell::Special("NaN".into())
                        } else if *v == f64::INFINITY {
                            Cell::Special("Infinity".into())
                        } else if *v == f64::NEG_INFINITY {
                            Cell::Special("-Infinity".into())
                        } else {
                            Cell::Num(*v)
                        }
                    })
                    .collect()
            })
            .collect();
        rows.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<f64>>, D::Error> {
        let rows = <Vec<Vec<Cell>>>::deserialize(d)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|c| match c {
                        Cell::Num(v) => v,
                        Cell::Special(s) => match s.as_str() {
                            "NaN" => f64::NAN,
                            "Infinity" => f64::INFINITY,
                            "-Infinity" => f64::NEG_INFINITY,
                            _ => f64::NAN,
                        },
                        Cell::Null => f64::NAN,
                    })
                    .collect()
            })
            .collect())
    }
}
