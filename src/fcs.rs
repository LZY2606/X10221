//! 最小 FCS 3.1 读取器（离线案例包所需的子集）。
//!
//! 支持：TEXT 段（分隔符可被双写转义）、DATA 段 32 位浮点
//! （$DATATYPE=F，$BYTEORD=4,3,2,1 大端或 1,2,3,4 小端），
//! 事件按 $PAR 列、$TOT 行连续排布。
//!
//! 解析原则：不做任何不可逆清洗。无法识别的值不丢弃，
//! 以原始位表示形式保留在 `diagnostics` 与调用方的 RawNumber.raw 中。

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub keywords: BTreeMap<String, String>,
    pub channels: Vec<String>,
    /// 行=事件，列=通道；值为 f32（所有 f32 都能被 f64 精确表示）。
    pub events: Vec<Vec<f32>>,
    pub text_segment: Vec<u8>,
    pub diagnostics: Vec<String>,
}

fn offset_field(hdr: &[u8], start: usize, end: usize) -> Option<u64> {
    let s = std::str::from_utf8(hdr.get(start..end)?).ok()?.trim();
    if s.is_empty() { None } else { s.parse().ok() }
}

pub fn parse_fcs(bytes: &[u8]) -> anyhow::Result<ParsedFcs> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" && &bytes[0..6] != b"FCS3.0" {
        anyhow::bail!("不是受支持的 FCS 3.0/3.1 文件（前 6 字节应为 FCS3.x）");
    }
    let hdr = &bytes[..58];
    let mut text_start = offset_field(hdr, 10, 18);
    let mut text_end = offset_field(hdr, 18, 26);
    let mut data_start = offset_field(hdr, 26, 34);
    let mut data_end = offset_field(hdr, 34, 42);

    // 先读 TEXT，可能需要从关键字里取真实偏移（大文件时头部偏移写 0）。
    let (ts, te) = match (text_start, text_end) {
        (Some(a), Some(b)) if b > a => (a as usize, b as usize),
        _ => anyhow::bail!("TEXT 段偏移缺失"),
    };
    let text_bytes = bytes.get(ts..=te).unwrap_or(&[]).to_vec();
    let delim = *text_bytes.first().unwrap_or(&b'|') as char;
    let body = &text_bytes[1..text_bytes.len().saturating(1)];
    let text = std::str::from_utf8(body)
        .map_err(|e| anyhow::anyhow!("TEXT 段不是合法 UTF-8: {e}"))?;
    let keywords = parse_text_pairs(text, delim);
    let kw = |k: &str| keywords.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());

    if data_start.is_none() {
        data_start = kw("$BEGINDATA").and_then(|v| v.trim().parse().ok());
    }
    if data_end.is_none() {
        data_end = kw("$ENDDATA").and_then(|v| v.trim().parse().ok());
    }
    if text_start.is_none() {
        text_start = kw("$BEGINTEXT").and_then(|v| v.trim().parse().ok());
    }
    if text_end.is_none() {
        text_end = kw("$ENDTEXT").and_then(|v| v.trim().parse().ok());
    }

    let par: usize = kw("$PAR")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow::anyhow!("缺少 $PAR"))?;
    let tot: usize = kw("$TOT")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow::anyhow!("缺少 $TOT"))?;
    let dtype = kw("$DATATYPE").unwrap_or_default();
    if dtype.trim() != "F" {
        anyhow::bail!("案例包仅使用 $DATATYPE=F（32 位浮点），实际: {dtype:?}");
    }
    let byteord = kw("$BYTEORD").unwrap_or_default();
    let big_endian = byteord.trim() == "4,3,2,1";
    if !big_endian && byteord.trim() != "1,2,3,4" {
        anyhow::bail!("不支持的 $BYTEORD: {byteord:?}");
    }
    let mut channels = Vec::new();
    for i in 1..=par {
        channels.push(kw(&format!("$P{}N", i)).unwrap_or_else(|| format!("CH{}", i)));
    }

    let (ds, de) = match (data_start, data_end) {
        (Some(a), Some(b)) => (a as usize, b as usize),
        _ => anyhow::bail!("DATA 段偏移缺失（$BEGINDATA/$ENDDATA）"),
    };
    let data = bytes
        .get(ds..=de)
        .ok_or_else(|| anyhow::anyhow!("DATA 段超出文件范围"))?;
    let need = tot * par * 4;
    let mut diagnostics = Vec::new();
    if data.len() < need {
        anyhow::bail!("DATA 段字节不足：需要 {}，实际 {}", need, data.len());
    }
    let mut events = Vec::with_capacity(tot);
    for e in 0..tot {
        let mut row = Vec::with_capacity(par);
        for c in 0..par {
            let off = (e * par + c) * 4;
            let raw: [u8; 4] = data[off..off + 4].try_into().unwrap();
            let bits = if big_endian {
                u32::from_be_bytes(raw)
            } else {
                u32::from_le_bytes(raw)
            };
            let v = f32::from_bits(bits);
            if v.is_nan() {
                diagnostics.push(format!(
                    "事件 {} 通道 {} 原始位 {} 为 NaN：保留原值，不参与计算",
                    e,
                    channels[c],
                    &format!("f32{}:{:08x}", if big_endian { "be" } else { "le" }, bits)
                ));
            }
            row.push(v);
        }
        events.push(row);
    }

    let mut map = BTreeMap::new();
    for (k, v) in keywords {
        map.insert(k, v);
    }
    Ok(ParsedFcs {
        keywords: map,
        channels,
        events,
        text_segment: text_bytes,
        diagnostics,
    })
}

fn parse_text_pairs(text: &str, delim: char) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut current = String::new();
    // 双写分隔符是对分隔符的转义。
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == delim {
            if i + 1 < chars.len() && chars[i + 1] == delim {
                current.push(delim);
                i += 2;
                continue;
            }
            pairs.push(std::mem::take(&mut current));
            i += 1;
        } else {
            current.push(chars[i]);
            i += 1;
        }
    }
    if !current.is_empty() || pairs.len() % 2 == 1 {
        pairs.push(current);
    }
    pairs
        .chunks_exact(2)
        .map(|kv| (kv[0].clone(), kv[1].clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fcs(channels: &[&str], events: &[Vec<f32>]) -> Vec<u8> {
        crate::fcs_write::write_fcs_3_1(channels, events)
    }

    #[test]
    fn roundtrips_events_and_keywords() {
        let evs = vec![vec![1.0f32, 2.0, 3.0, 4.0], vec![0.5, -2.0, 1e4, 0.0]];
        let bytes = make_fcs(&["FSC-A", "SSC-A", "FITC-A", "PE-A"], &evs);
        let p = parse_fcs(&bytes).unwrap();
        assert_eq!(p.channels, vec!["FSC-A", "SSC-A", "FITC-A", "PE-A"]);
        assert_eq!(p.events, evs);
        assert_eq!(p.keywords["$TOT"], "2");
        assert_eq!(p.keywords["$P3N"], "FITC-A");
    }

    #[test]
    fn nan_is_kept_and_diagnosed_not_dropped() {
        let evs = vec![vec![1.0f32, 2.0, f32::NAN, 4.0]];
        let bytes = make_fcs(&["FSC-A", "SSC-A", "FITC-A", "PE-A"], &evs);
        let p = parse_fcs(&bytes).unwrap();
        assert!(p.events[0][2].is_nan());
        assert!(p.diagnostics[0].contains("NaN"));
    }

    #[test]
    fn rejects_non_fcs() {
        assert!(parse_fcs(b"not an fcs file at all!!").is_err());
    }
}
