//! 最小 FCS 3.0/3.1 解析器：导入时只保留原始表示，不做不可逆清洗。
//!
//! 支持：BEGINDATA/ENDDATA（优先 TEXT 段关键字回退）、ASCII 文本段定界符转义、
//! $DATATYPE = F(4)/D(8)/I(1,2,4)、$BYTEORD 1234/4321、$MODE L、
//! $PnN/$PnB/$PnR/$PnE、$SPILL/$SPILLOVER/SPILL 关键字逐字保留。
//!
//! 数值原始表示以字符串保留在 `EventValue.raw`；超出 $PnR 的整数值记为
//! `Sentinel`（如 "OVFLO" 或饱和）；无法读取的记 `Unparsable` + 诊断。

use crate::domain::{Channel, EventValue, EventsArtifact, FlowEvent, SpilloverArtifact};

#[derive(Debug)]
pub struct FcsParseError(pub String);

impl std::fmt::Display for FcsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS parse error: {}", self.0)
    }
}
impl std::error::Error for FcsParseError {}

impl From<&str> for FcsParseError {
    fn from(s: &str) -> Self { FcsParseError(s.to_string()) }
}
impl From<String> for FcsParseError {
    fn from(s: String) -> Self { FcsParseError(s) }
}

/// 解析出的事件制品 + 逐字 spillover 元数据（若文件中存在）。
pub struct ParsedFcs {
    pub events: EventsArtifact,
    pub spillover: Option<SpilloverArtifact>,
}

struct Header {
    begin_data: Option<usize>,
    end_data: Option<usize>,
    begin_text: usize,
    end_text: usize,
}

fn read_usize(bytes: &[u8], lo: usize, hi: usize) -> Option<usize> {
    let s = std::str::from_utf8(bytes.get(lo..hi)?).ok()?;
    s.trim().parse().ok()
}

pub fn parse_fcs(data: &[u8]) -> Result<ParsedFcs, FcsParseError> {
    if data.len() < 58 || &data[..6] != b"FCS3.0" && &data[..6] != b"FCS3.1" {
        return Err(FcsParseError("不是 FCS3.0/3.1 文件（头 6 字节不匹配）".into()));
    }
    let header = Header {
        begin_text: read_usize(data, 10, 18).ok_or("缺少 BEGINSTEXT")?,
        end_text: read_usize(data, 18, 26).ok_or("缺少 ENDSTEXT")?,
        begin_data: read_usize(data, 26, 34),
        end_data: read_usize(data, 34, 42),
    };

    let text_bytes = data
        .get(header.begin_text..=header.end_text)
        .ok_or("TEXT 段越界")?;
    let delimiter = text_bytes.first().copied().ok_or("空 TEXT 段")?;
    let keywords = parse_text_segment(text_bytes, delimiter)?;
    let kw = |name: &str| keywords.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    let require = |name: &str| -> Result<String, FcsParseError> {
        kw(name).ok_or_else(|| FcsParseError(format!("缺少关键字 {name}")))
    };

    let datatype = require("$DATATYPE")?;
    let byteord = require("$BYTEORD")?;
    let mode = require("$MODE")?;
    if mode != "L" {
        return Err(FcsParseError(format!("仅支持 $MODE=L，得到 {mode}")));
    }
    let n_params: usize = require("$PAR")?
        .parse()
        .map_err(|_| FcsParseError("$PAR 不是整数".into()))?;
    let total: usize = require("$TOT")?
        .parse()
        .map_err(|_| FcsParseError("$TOT 不是整数".into()))?;
    let big_endian = match byteord.as_str() {
        "1,2,3,4" | "1,2" => false,
        "4,3,2,1" | "2,1" => true,
        other => return Err(FcsParseError(format!("不支持的 $BYTEORD {other}"))),
    };

    let mut channels = Vec::with_capacity(n_params);
    let mut bit_widths = Vec::with_capacity(n_params);
    let mut ranges = Vec::with_capacity(n_params);
    for n in 1..=n_params {
        let name = kw(&format!("$P{n}N")).unwrap_or_else(|| format!("P{n}"));
        let pnb: usize = kw(&format!("$P{n}B"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| FcsParseError(format!("缺少/非法 $P{n}B")))?;
        let pnr: f64 = kw(&format!("$P{n}R"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| FcsParseError(format!("缺少/非法 $P{n}R")))?;
        let pne = kw(&format!("$P{n}E")).unwrap_or_else(|| "0,0".into());
        let mut parts = pne.split(',');
        let a0: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let a1: f64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let unit = kw(&format!("$P{n}U")).unwrap_or_else(|| "channel".into());
        channels.push(Channel {
            short: name.clone(),
            detector: name,
            amplification: [a0, a1],
            unit,
        });
        bit_widths.push(pnb);
        ranges.push(pnr);
    }

    // 大文件时 BEGINDATA 可能放 0，以关键字为准。
    let begin_data = header
        .begin_data
        .or_else(|| kw("BEGINDATA").and_then(|v| v.parse().ok()))
        .ok_or("无法定位 BEGINDATA")?;
    let end_data = header
        .end_data
        .or_else(|| kw("ENDDATA").and_then(|v| v.parse().ok()))
        .ok_or("无法定位 ENDDATA")?;
    let data_seg = data
        .get(begin_data..=end_data)
        .ok_or("DATA 段越界")?;

    let events = parse_data_segment(
        data_seg,
        datatype.as_str(),
        big_endian,
        n_params,
        total,
        &bit_widths,
        &ranges,
        &channels,
    )?;

    let spillover = parse_spillover(&keywords);

    Ok(ParsedFcs {
        events: EventsArtifact { channels, events },
        spillover,
    })
}

fn parse_text_segment(bytes: &[u8], delimiter: u8) -> Result<Vec<(String, String)>, FcsParseError> {
    // 跳过首字节（即定界符本身），按未转义定界符切分，双定界符表示字面量。
    let mut out: Vec<(String, String)> = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == delimiter {
            if i + 1 < bytes.len() && bytes[i + 1] == delimiter {
                cur.push(delimiter as char);
                i += 2;
                continue;
            }
            fields.push(std::mem::take(&mut cur));
            i += 1;
            continue;
        }
        cur.push(b as char);
        i += 1;
    }
    if !cur.is_empty() || bytes.last() == Some(&delimiter) {
        // 末尾分隔符不产生新字段；若有残余非空则保留
        if !cur.is_empty() {
            fields.push(cur);
        }
    }
    let mut it = fields.into_iter();
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        if !k.is_empty() {
            out.push((k, v));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn parse_data_segment(
    seg: &[u8],
    datatype: &str,
    big_endian: bool,
    n_params: usize,
    total: usize,
    bit_widths: &[usize],
    ranges: &[f64],
    channels: &[Channel],
) -> Result<Vec<FlowEvent>, FcsParseError> {
    let rec_size: usize = match datatype {
        "F" => n_params * 4,
        "D" => n_params * 8,
        "I" => bit_widths.iter().sum::<usize>() / 8,
        other => return Err(FcsParseError(format!("不支持的 $DATATYPE {other}"))),
    };
    let expected = total
        .checked_mul(rec_size)
        .ok_or("DATA 段尺寸溢出")?;
    if seg.len() < expected {
        return Err(FcsParseError(format!(
            "DATA 段过短：需要 {expected} 字节，实际 {}",
            seg.len()
        )));
    }
    let mut events = Vec::with_capacity(total);
    for (ev_idx, chunk) in seg.chunks_exact(rec_size).take(total).enumerate() {
        let mut values = Vec::with_capacity(n_params);
        let mut cursor = 0;
        for p in 0..n_params {
            let ch = &channels[p];
            let val = match datatype {
                "F" => {
                    let mut buf = [0u8; 4];
                    buf.copy_from_slice(&chunk[cursor..cursor + 4]);
                    if big_endian {
                        buf.reverse();
                    }
                    cursor += 4;
                    let v = f32::from_le_bytes(buf) as f64;
                    float_value(v, &format!("{v}"), ch)
                }
                "D" => {
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(&chunk[cursor..cursor + 8]);
                    if big_endian {
                        buf.reverse();
                    }
                    cursor += 8;
                    let v = f64::from_le_bytes(buf);
                    float_value(v, &format!("{v}"), ch)
                }
                _ => {
                    let bytes_n = bit_widths[p] / 8;
                    let mut raw_int: u64 = 0;
                    for b in 0..bytes_n {
                        let byte = chunk[cursor + b];
                        raw_int = if big_endian {
                            (raw_int << 8) | byte as u64
                        } else {
                            raw_int | ((byte as u64) << (8 * b))
                        };
                    }
                    cursor += bytes_n;
                    let max = ranges[p];
                    if (raw_int as f64) >= max {
                        // 饱和/溢出：保留哨兵，不静默截断后画图。
                        EventValue::Sentinel {
                            raw: raw_int.to_string(),
                            unit: ch.unit.clone(),
                        }
                    } else {
                        EventValue::Numeric {
                            raw: raw_int.to_string(),
                            value: raw_int as f64,
                            unit: ch.unit.clone(),
                        }
                    }
                }
            };
            values.push(val);
        }
        events.push(FlowEvent {
            index: (ev_idx + 1) as u64,
            values,
        });
    }
    Ok(events)
}

fn float_value(v: f64, raw: &str, ch: &Channel) -> EventValue {
    if v.is_nan() || v.is_infinite() {
        EventValue::Unparsable {
            raw: raw.to_string(),
            diagnostic: "非有限浮点值（NaN/Inf），保留原位置不参与派生计算".into(),
            unit: ch.unit.clone(),
        }
    } else if v < 0.0 {
        EventValue::Sentinel {
            raw: raw.to_string(),
            unit: ch.unit.clone(),
        }
    } else {
        EventValue::Numeric {
            raw: raw.to_string(),
            value: v,
            unit: ch.unit.clone(),
        }
    }
}

fn parse_spillover(keywords: &[(String, String)]) -> Option<SpilloverArtifact> {
    for name in ["$SPILLOVER", "$SPILL", "SPILL"] {
        if let Some((_, raw)) = keywords.iter().find(|(k, _)| k == name) {
            if let Some(parsed) = parse_spill_text(raw) {
                return Some(SpilloverArtifact {
                    raw_keyword: raw.clone(),
                    keyword_name: name.to_string(),
                    detectors: parsed.0,
                    matrix: parsed.1,
                    unit: "fraction(linear)".into(),
                });
            }
        }
    }
    None
}

/// 解析逗号分隔 spillover：n,n labels..., n*n values（FCS 3.1 顺序：列=源染料）。
fn parse_spill_text(raw: &str) -> Option<(Vec<String>, Vec<Vec<f64>>)> {
    let tokens: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    let n: usize = tokens.first()?.parse().ok()?;
    if tokens.len() < 1 + n + n * n {
        return None;
    }
    let detectors = tokens[1..1 + n].iter().map(|s| s.to_string()).collect();
    let mut matrix = Vec::with_capacity(n);
    let mut k = 1 + n;
    for _ in 0..n {
        let mut row = Vec::with_capacity(n);
        for _ in 0..n {
            row.push(tokens[k].parse().ok()?);
            k += 1;
        }
        matrix.push(row);
    }
    Some((detectors, matrix))
}
