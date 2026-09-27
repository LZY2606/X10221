//! 最小 FCS 3.1 读取器。只解析离线案例包需要的部分：
//! TEXT 关键字 + 事件矩阵。原始表示（$PnG / $PnE / 饱和位）在领域层保留。
//!
//! 关键约定：
//! - $DATATYPE F=float32, D=float64, I=integer(big endian, $PnB bits)
//! - $BYTEORD 1,2,3,4 = little endian；4,3,2,1 = big endian
//! - $BEGINSTEXT 存在且非 0 时跳过补充 TEXT 段
//! - $TIMESTEP 时间通道线性；本案例不使用时间通道

use crate::domain::{
    Channel, ChannelValue, CompensationMatrix, FlowEvent, ScaleUnit, SpilloverMeta, ValueStatus,
};
use crate::error::{AppError, AppResult};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub channels: Vec<Channel>,
    pub events: Vec<FlowEvent>,
    pub keywords: BTreeMap<String, String>,
    pub spillover: Option<(SpilloverMeta, CompensationMatrix)>,
}

fn kw(keywords: &BTreeMap<String, String>, key: &str) -> AppResult<String> {
    keywords
        .get(key)
        .cloned()
        .ok_or_else(|| AppError::Fcs(format!("FCS 缺少必需关键字 {key}")))
}

/// 解析 TEXT 段：以分隔符开头，键值成对，重复分隔符表示转义。
fn parse_text(buf: &[u8]) -> AppResult<BTreeMap<String, String>> {
    if buf.is_empty() {
        return Err(AppError::Fcs("TEXT 段为空".into()));
    }
    let delim = buf[0];
    let body = &buf[1..];
    let mut fields: Vec<Vec<u8>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == delim {
            if i + 1 < body.len() && body[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(body[i]);
        }
        i += 1;
    }
    if !cur.is_empty() || fields.last().map(|f| f.is_empty()).unwrap_or(false) {
        // 尾随分隔符产生的空串丢弃
    }
    if fields.last().map(|f| f.is_empty()) == Some(true) {
        fields.pop();
    }
    let mut map = BTreeMap::new();
    let mut k = 0;
    while k + 1 < fields.len() {
        let key = String::from_utf8_lossy(&fields[k]).trim().to_string();
        let val = String::from_utf8_lossy(&fields[k + 1]).to_string();
        map.insert(key, val);
        k += 2;
    }
    Ok(map)
}

fn parse_spillover(
    keywords: &BTreeMap<String, String>,
) -> Option<(SpilloverMeta, CompensationMatrix)> {
    let raw = keywords.get("$SPILL")?;
    let parts: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return None;
    }
    let n: usize = parts[0].parse().ok()?;
    if parts.len() < 1 + n + n * n {
        return None;
    }
    let mut idx = 1;
    let names: Vec<String> = parts[idx..idx + n].iter().map(|s| (*s).to_string()).collect();
    idx += n;
    let mut rows = Vec::with_cap(n);
    for _ in 0..n {
        let mut row = Vec::with_cap(n);
        for _ in 0..n {
            row.push(parts[idx].parse::<f64>().unwrap_or(f64::NAN));
            idx += 1;
        }
        rows.push(row);
    }
    let meta = SpilloverMeta {
        fluorochromes: names.clone(),
        detectors: names.clone(),
        source_label: keywords.get("$INST").cloned().unwrap_or_else(|| "flowline-bench".into()),
        fcs_keyword: "$SPILL".into(),
    };
    let matrix = CompensationMatrix {
        channel_names: names,
        rows,
        basis: "fcs-$SPILL,v1".into(),
    };
    Some((meta, matrix))
}

pub fn parse_fcs(bytes: &[u8]) -> AppResult<ParsedFcs> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(AppError::Fcs("不是 FCS3.1 文件（头段不匹配）".into()));
    }
    let header_text_end = bytes[10..18]
        .iter()
        .any(|&b| b == b'0')
        .then(|| ());
    let _ = header_text_end;
    let at = |a: usize, b: usize| String::from_utf8_lossy(&bytes[a..b]).trim().to_string();
    let text_start: usize = at(10, 18).parse().map_err(|_| AppError::Fcs("TEXT 起始非法".into()))?;
    let text_end: usize = at(18, 26).parse().map_err(|_| AppError::Fcs("TEXT 结束非法".into()))?;
    let data_start_hdr: i64 = at(26, 34).parse().unwrap_or(0);
    let data_end_hdr: i64 = at(34, 42).parse().unwrap_or(0);

    let text = parse_text(&bytes[text_start..=text_end])?;
    let begin_stext = text
        .get("$BEGINSTEXT")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let end_stext = text
        .get("$ENDSTEXT")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let _ = (begin_stext, end_stext);

    let data_start = text
        .get("$BEGINDATA")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(data_start_hdr as usize);
    let data_end = text
        .get("$ENDDATA")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(data_end_hdr as usize);

    let n_params: usize = kw(&text, "$PAR")?.parse().map_err(|_| AppError::Fcs("$PAR 非法".into()))?;
    let n_events: usize = kw(&text, "$TOT")?.parse().map_err(|_| AppError::Fcs("$TOT 非法".into()))?;
    let datatype = kw(&text, "$DATATYPE")?;
    let byteord = kw(&text, "$BYTEORD")?;
    let big_endian = byteord.starts_with("4");
    let mode = kw(&text, "$MODE")?;
    if mode != "L" {
        return Err(AppError::Fcs(format!("仅支持 list mode ($MODE=L)，收到 {mode}")));
    }

    let mut channels = Vec::with_cap(n_params);
    let mut pnb = Vec::with_cap(n_params);
    for p in 1..=n_params {
        let name = text.get(&format!("$P{p}N")).cloned().unwrap_or_else(|| format!("P{p}"));
        let short = text.get(&format!("$P{p}S")).cloned().unwrap_or_else(|| name.clone());
        let pne = text.get(&format!("$P{p}E")).cloned().unwrap_or_else(|| "0,0".into());
        let gain = text
            .get(&format!("$P{p}G"))
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(1.0);
        let bits: usize = text
            .get(&format!("$P{p}B"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(32);
        pnb.push(bits);
        let unit = if pne == "0,0" {
            ScaleUnit::Linear
        } else if pne.starts_with("0,") {
            ScaleUnit::LogDecades
        } else {
            ScaleUnit::ChannelRaw
        };
        channels.push(Channel {
            id: crate::domain::ChannelId(format!("ch-{name}")),
            name,
            short,
            unit,
            fcs_gain: gain,
            fcs_pne: pne,
        });
    }

    let data = &bytes[data_start..=data_end];
    let mut events = Vec::with_cap(n_events);
    let mut offset = 0usize;
    let read_num = |off: usize, bits: usize| -> AppResult<f64> {
        match datatype.as_str() {
            "F" => {
                let raw: [u8; 4] = data
                    .get(off..off + 4)
                    .ok_or_else(|| AppError::Fcs("事件数据被截断".into()))?
                    .try_into()
                    .unwrap();
                let raw = if big_endian { raw } else { [raw[3], raw[2], raw[1], raw[0]] };
                Ok(f32::from_be_bytes(raw) as f64)
            }
            "D" => {
                let raw: [u8; 8] = data
                    .get(off..off + 8)
                    .ok_or_else(|| AppError::Fcs("事件数据被截断".into()))?
                    .try_into()
                    .unwrap();
                let raw = if big_endian {
                    raw
                } else {
                    [raw[7], raw[6], raw[5], raw[4], raw[3], raw[2], raw[1], raw[0]]
                };
                Ok(f64::from_be_bytes(raw))
            }
            "I" => {
                let bytes = bits / 8;
                let slice = data
                    .get(off..off + bytes)
                    .ok_or_else(|| AppError::Fcs("事件数据被截断".into()))?;
                let mut v: u64 = 0;
                if big_endian {
                    for &b in slice {
                        v = (v << 8) | b as u64;
                    }
                } else {
                    for (i, &b) in slice.iter().enumerate() {
                        v |= (b as u64) << (8 * i);
                    }
                }
                Ok(v as f64)
            }
            other => Err(AppError::Fcs(format!("不支持的 $DATATYPE={other}"))),
        }
    };

    for e in 0..n_events {
        let mut values = BTreeMap::new();
        for (ci, ch) in channels.iter().enumerate() {
            let bits = if datatype == "I" { pnb[ci] } else if datatype == "F" { 32 } else { 64 };
            let raw = read_num(offset, bits)?;
            offset += bits / 8;
            // 检测器饱和：平台标记（$PnR-1）或非有限值。无效值保留 NaN 原位置。
            let (status, diagnostic) = if raw.is_nan() {
                (ValueStatus::Invalid, Some("事件原始值为 NaN，保留原位置".into()))
            } else if raw.is_infinite() {
                (ValueStatus::Invalid, Some("事件原始值为 ±Inf，保留原位置".into()))
            } else {
                let range_max = text
                    .get(&format!("$P{}R", ci + 1))
                    .and_then(|s| s.parse::<f64>().ok());
                match range_max {
                    Some(max) if max > 0.0 && raw >= max => (
                        ValueStatus::Saturated,
                        Some(format!("极端信号：raw={raw} 达到 $P{}R={max}（检测器饱和）", ci + 1)),
                    ),
                    _ => (ValueStatus::Ok, None),
                }
            };
            values.insert(
                ch.short.clone(),
                ChannelValue {
                    raw,
                    status,
                    diagnostic,
                },
            );
        }
        events.push(FlowEvent {
            id: crate::domain::EventId(format!("evt-{:04}", e + 1)),
            channels: values,
        });
    }

    Ok(ParsedFcs {
        channels,
        events,
        keywords: text,
        spillover: parse_spillover(&keywords),
    })
}
