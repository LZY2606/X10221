//! 最小 FCS 3.1 读取器：只解析案例包需要的关键字与 $DATATYPE=F/4 字节浮点事件。
//! 原始数值直接进入 raw_data，不在解析阶段做任何清洗。

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub keywords: HashMap<String, String>,
    /// 事件 × 通道的原始浮点值。
    pub events: Vec<Vec<f32>>,
    pub channel_names: Vec<String>,
    pub channel_ranges: Vec<f64>,
}

#[derive(Debug)]
pub struct FcsError(pub String);
impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS 解析失败: {}", self.0)
    }
}

fn read_be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn parse_text_segment(seg: &[u8], delimiter: u8) -> Vec<(String, String)> {
    let mut kvs = Vec::new();
    // 段首是分隔符
    let mut start = 1usize;
    let mut parts: Vec<&[u8]> = Vec::new();
    for (i, &b) in seg.iter().enumerate().skip(1) {
        if b == delimiter {
            parts.push(&seg[start..i]);
            start = i + 1;
        }
    }
    if start < seg.len() {
        parts.push(&seg[start..]);
    }
    let mut i = 0;
    while i + 1 < parts.len() {
        let k = String::from_trimmed(parts[i]);
        let v = String::from_trimmed(parts[i + 1]);
        if !k.is_empty() {
            kvs.push((k, v));
        }
        i += 2;
    }
    kvs
}

trait FromTrimmed {
    fn from_trimmed(b: &[u8]) -> Self;
}
impl FromTrimmed for String {
    fn from_trimmed(b: &[u8]) -> Self {
        String::from_utf8_lossy(b).trim().to_string()
    }
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(FcsError("不是 FCS3.1 文件".into()));
    }
    let delim = bytes[62];
    // 优先使用 TEXT 段内的 $BEGINDATA/$ENDDATA（允许数据段在 99 字节以后）
    let hdr_text_begin = read_be_u32(&bytes[10..14]) as usize;
    let hdr_text_end = read_be_u32(&bytes[14..18]) as usize;
    if hdr_text_end >= bytes.len() {
        return Err(FcsError("TEXT 段偏移越界".into()));
    }
    let text_seg = &bytes[hdr_text_begin..=hdr_text_end];
    let kvs = parse_text_segment(text_seg, delim);
    let map: HashMap<String, String> = kvs.into_iter().collect();

    let get = |k: &str| -> Result<&String, FcsError> {
        map.get(k).ok_or_else(|| FcsError(format!("缺少关键字 {k}")))
    };
    if get("$DATATYPE")? != "F" {
        return Err(FcsError("案例包仅生成/读取 $DATATYPE=F".into()));
    }
    if get("$BYTEORD")? != "4,3,2,1" {
        return Err(FcsError("案例包使用大端字节序 4,3,2,1".into()));
    }
    let par: usize = get("$PAR")?
        .parse()
        .map_err(|_| FcsError("$PAR 不是整数".into()))?;
    let total: usize = get("$TOT")?
        .parse()
        .map_err(|_| FcsError("$TOT 不是整数".into()))?;
    let data_begin: usize = get("$BEGINDATA")?
        .parse()
        .map_err(|_| FcsError("$BEGINDATA 不是整数".into()))?;
    let data_end: usize = get("$ENDDATA")?
        .parse()
        .map_err(|_| FcsError("$ENDDATA 不是整数".into()))?;

    let channel_names = (1..=par)
        .map(|n| map.get(&format!("$P{n}N")).cloned().unwrap_or_default())
        .collect();
    let channel_ranges = (1..=par)
        .map(|n| {
            map.get(&format!("$P{n}R"))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        })
        .collect();

    let need = total * par * 4;
    let seg = bytes
        .get(data_begin..=data_end)
        .ok_or_else(|| FcsError("DATA 段偏移越界".into()))?;
    if seg.len() < need {
        return Err(FcsError(format!("DATA 段长度不足：{need} > {}", seg.len())));
    }
    let mut events = Vec::with_capacity(total);
    let mut off = 0;
    for _ in 0..total {
        let mut row = Vec::with_capacity(par);
        for _ in 0..par {
            let bits = u32::from_be_bytes([seg[off], seg[off + 1], seg[off + 2], seg[off + 3]]);
            row.push(f32::from_bits(bits));
            off += 4;
        }
        events.push(row);
    }

    Ok(FcsFile {
        keywords: map,
        events,
        channel_names,
        channel_ranges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fcs_writer::write_fcs_31;

    #[test]
    fn roundtrip_small_fcs() {
        let names = vec!["FSC-A".to_string(), "FITC-A".to_string()];
        let ranges = vec![262144.0, 262144.0];
        let events = vec![vec![1.0f32, 2.0], vec![262143.0, -3.0]];
        let mut kws = HashMap::new();
        kws.insert("$FIL".to_string(), "roundtrip.fcs".to_string());
        let bytes = write_fcs_31(&names, &ranges, &events, &kws);
        let parsed = parse_fcs(&bytes).unwrap();
        assert_eq!(parsed.events, events);
        assert_eq!(parsed.channel_names, names);
        assert_eq!(parsed.channel_ranges, ranges);
        assert_eq!(parsed.keywords.get("$TOT"), Some(&"2".to_string()));
        assert_eq!(parsed.keywords.get("$PAR"), Some(&"2".to_string()));
    }
}
