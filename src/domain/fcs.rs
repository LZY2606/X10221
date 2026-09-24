//! 最小可用的 FCS 3.1 读写器（list-mode，float32 big/little endian）。
//!
//! 导入时保留原始表示：通道原始值按文件中的 IEEE754 数值解析；无法解析的值不产生半份记录
//! （调用方在事务里整体回滚）。NaN / Inf 事件值原样读入并附诊断，而不是被清洗。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct FcsParseError(pub String);

impl std::fmt::Display for FcsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS 解析失败: {}", self.0)
    }
}
impl std::error::Error for FcsParseError {}

#[derive(Debug, Clone)]
pub struct FcsData {
    pub version: String,
    pub keywords: BTreeMap<String, String>,
    /// 参数名（$PnN），顺序即列顺序。
    pub channels: Vec<String>,
    /// 每个参数的单位/增益基准（$PnS 描述，没有则用 a.u.）。
    pub channel_labels: Vec<String>,
    /// 行=事件，列=通道；NaN/Inf 原样保留。
    pub events: Vec<Vec<f32>>,
    /// 逐事件诊断（NaN/非有限等），与 events 行对齐。
    pub event_diagnostics: Vec<Vec<String>>,
}

/// 生成一份 FCS 3.1 文件字节（TEXT + 单段 DATA，float32 little endian）。
pub fn write_fcs_3_1(
    channels: &[String],
    labels: &[String],
    events: &[Vec<f32>],
    extra_keywords: &BTreeMap<String, String>,
) -> Vec<u8> {
    assert_eq!(channels.len(), labels.len());
    let npar = channels.len();
    let nevents = events.len();

    // DATA
    let mut data = Vec::with_capacity(nevents * npar * 4);
    for row in events {
        assert_eq!(row.len(), npar);
        for v in row {
            data.extend_from_slice(&v.to_le_bytes());
        }
    }

    // TEXT：分隔符 '/'。先在未知 DATA 偏移的情况下构建 TEXT 一次以取得长度，
    // 由此确定 DATA 偏移，再用 8 位定宽偏移重建（偏移必须落在头部 8 位槽内）。
    let build_text = |data_begin: usize, data_end: usize| -> Vec<u8> {
        let mut text: Vec<u8> = Vec::new();
        let push = |text: &mut Vec<u8>, k: &str, v: &str| {
            text.push(b'/');
            text.extend(k.replace('/', "//").bytes());
            text.push(b'/');
            text.extend(v.replace('/', "//").bytes());
        };
        push(&mut text, "BEGINANALYSIS", "0");
        push(&mut text, "ENDANALYSIS", "0");
        push(&mut text, "BEGINDATA", &format!("{data_begin:08}"));
        push(&mut text, "ENDDATA", &format!("{data_end:08}"));
        push(&mut text, "DATATYPE", "F");
        push(&mut text, "MODE", "L");
        push(&mut text, "BYTEORD", "1,2,3,4");
        push(&mut text, "$NEXTDATA", "0");
        push(&mut text, "$TOT", &nevents.to_string());
        push(&mut text, "$PAR", &npar.to_string());
        for (i, name) in channels.iter().enumerate() {
            let n = i + 1;
            push(&mut text, &format!("$P{n}N"), name);
            push(&mut text, &format!("$P{n}S"), &labels[i]);
            push(&mut text, &format!("$P{n}B"), "32");
            push(&mut text, &format!("$P{n}E"), "0,0");
            push(&mut text, &format!("$P{n}R"), "262144");
        }
        for (k, v) in extra_keywords {
            push(&mut text, k, v);
        }
        push(&mut text, "ENDSTEXT", "0");
        text
    };
    let probe = build_text(0, 0);
    let data_begin = 58 + probe.len();
    let data_end = data_begin + data.len() - 1;
    if data_end > 99_999_999 {
        panic!("FCS 数据超过 8 位偏移槽上限，本案例包不支持");
    }
    let text = build_text(data_begin, data_end);

    let text_begin = 58usize;
    let text_end = text_begin + text.len() - 1;
    let mut header = String::new();
    header.push_str("FCS3.1    "); // 10
    header.push_str(&format!("{:>8}", text_begin)); // @10
    header.push_str(&format!("{:>8}", text_end)); // @18
    header.push_str(&format!("{:>8}", data_begin)); // @26
    header.push_str(&format!("{:>8}", data_end)); // @34
    header.push_str(&format!("{:>8}", 0));
    header.push_str(&format!("{:>8}", 0));
    assert_eq!(header.len(), 58);

    let mut out = header.into_bytes();
    out.extend(text);
    out.extend(data);
    out
}

fn parse_delimiters(bytes: &[u8], delim: u8) -> Vec<Vec<u8>> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == delim && i + 1 < bytes.len() && bytes[i + 1] == delim {
            cur.push(delim);
            i += 2;
            continue;
        }
        if bytes[i] == delim {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(bytes[i]);
        }
        i += 1;
    }
    parts.push(cur);
    parts
}

pub fn parse_fcs(buf: &[u8]) -> Result<FcsData, FcsParseError> {
    if buf.len() < 58 {
        return Err(FcsParseError("文件短于 58 字节头".into()));
    }
    let version = String::from_utf8_lossy(&buf[0..6]).trim().to_string();
    if !version.starts_with("FCS") {
        return Err(FcsParseError("缺少 FCS 魔数".into()));
    }
    let hdr_num = |start: usize| -> Result<usize, FcsParseError> {
        let s = String::from_utf8_lossy(&buf[start..start + 8]).trim().to_string();
        s.parse::<usize>().map_err(|_| FcsParseError(format!("头偏移非法: {s}")))
    };
    let tb = hdr_num(10)?;
    let te = hdr_num(18)?;
    let db_hdr = hdr_num(26)?;
    let de_hdr = hdr_num(34)?;

    let text_bytes = if te + 1 <= buf.len() { &buf[tb..te + 1] } else { &buf[tb..] };
    let delim = text_bytes.first().copied().ok_or(FcsParseError("TEXT 段为空".into()))?;
    let parts = parse_delimiters(&text_bytes[1..], delim);
    let mut keywords = BTreeMap::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        let k = String::from_utf8_lossy(&parts[i]).trim().to_string();
        let v = String::from_utf8_lossy(&parts[i + 1]).trim().to_string();
        if !k.is_empty() {
            keywords.insert(k, v);
        }
        i += 2;
    }

    let data_begin = keywords
        .get("BEGINDATA")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v >= 58)
        .unwrap_or(db_hdr);
    let data_end = keywords
        .get("ENDDATA")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|v| *v >= data_begin)
        .unwrap_or(de_hdr);
    if data_end + 1 < data_begin || data_end >= buf.len() {
        return Err(FcsParseError("DATA 段偏移非法".into()));
    }
    let raw = &buf[data_begin..data_end + 1];
    let datatype = keywords.get("DATATYPE").map(|s| s.as_str()).unwrap_or("F");
    if datatype != "F" {
        return Err(FcsParseError(format!("本案例包仅支持 DATATYPE=F，读到 {datatype}")));
    }
    let byteord = keywords.get("BYTEORD").map(|s| s.as_str()).unwrap_or("1,2,3,4");
    let little = byteord.starts_with('1');
    let npar: usize = keywords
        .get("$PAR")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FcsParseError("缺少 $PAR".into()))?;
    let tot: usize = keywords
        .get("$TOT")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FcsParseError("缺少 $TOT".into()))?;

    let mut channels = Vec::with_capacity(npar);
    let mut labels = Vec::with_capacity(npar);
    for n in 1..=npar {
        let name = keywords
            .get(&format!("$P{n}N"))
            .cloned()
            .ok_or_else(|| FcsParseError(format!("缺少 $P{n}N")))?;
        let label = keywords.get(&format!("$P{n}S")).cloned().unwrap_or_else(|| "a.u.".into());
        channels.push(name);
        labels.push(label);
    }

    if raw.len() != tot * npar * 4 {
        return Err(FcsParseError(format!(
            "DATA 长度 {} 与 $TOT*$PAR*4={} 不符",
            raw.len(),
            tot * npar * 4
        )));
    }
    let mut events = Vec::with_capacity(tot);
    let mut event_diagnostics = Vec::with_capacity(tot);
    for r in 0..tot {
        let mut row = Vec::with_capacity(npar);
        let mut diag = Vec::new();
        for c in 0..npar {
            let o = (r * npar + c) * 4;
            let arr: [u8; 4] = raw[o..o + 4].try_into().unwrap();
            let v = if little { f32::from_le_bytes(arr) } else { f32::from_be_bytes(arr) };
            if v.is_nan() {
                diag.push(format!("{}=NaN（原始值保留，不参与派生）", channels[c]));
            } else if v.is_infinite() {
                diag.push(format!("{}=±Inf（原始值保留）", channels[c]));
            } else if v.abs() > 262_143.0 {
                diag.push(format!("{}={v} 超过 18-bit ADC 量程（极端信号）", channels[c]));
            }
            row.push(v);
        }
        events.push(row);
        event_diagnostics.push(diag);
    }

    Ok(FcsData {
        version,
        keywords,
        channels,
        channel_labels: labels,
        events,
        event_diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_nan_and_extremes() {
        let ch = vec!["FSC-A".into(), "FL1-A".into()];
        let lb = vec!["units".into(), "FITC a.u.".into()];
        let ev = vec![
            vec![100.0, 200.0],
            vec![f32::NAN, 262_144.0],
            vec![50.0, f32::INFINITY],
        ];
        let buf = write_fcs_3_1(&ch, &lb, &ev, &BTreeMap::new());
        assert_eq!(&buf[0..6], b"FCS3.1");
        let d = parse_fcs(&buf).unwrap();
        assert_eq!(d.channels, ch);
        assert_eq!(d.events.len(), 3);
        assert!(d.events[1][0].is_nan());
        assert_eq!(d.events[1][1], 262_144.0);
        assert!(d.events[2][1].is_infinite());
        assert!(d.event_diagnostics[1].iter().any(|s| s.contains("NaN")));
        assert!(d.event_diagnostics[2].iter().any(|s| contains_inf(s)));
    }

    fn contains_inf(s: &str) -> bool {
        s.contains("Inf")
    }

    #[test]
    fn rejects_truncated_and_bad_magic_atomically() {
        assert!(parse_fcs(&[0u8; 10]).is_err());
        let mut bad = vec![b'X'; 58];
        assert!(parse_fcs(&bad).is_err());
        bad[0..6].copy_from_slice(b"FCS3.1");
        assert!(parse_fcs(&bad).is_err());
    }
}



