//! FCS 3.0/3.1 解析与写入。
//!
//! 关键原则：
//! - 导入时保留原始表示（关键字按原始字符串存储；事件保留整数位模式
//!   或浮点 NaN/Inf，不做不可逆清洗）；
//! - 派生层（归一化、坐标解释）单独进行；
//! - 解析 `$SPILL`/`$SPILLOVER` 元数据为 spillover 材料，但原始字符串
//!   仍保留在元数据的 raw_repr 中。

use serde::{Deserialize, Serialize};

use super::matrix::{ChannelRef, SpilloverMatrix};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsFile {
    pub version: String,
    pub keywords: Vec<(String, String)>,
    pub parameters: Vec<FcsParam>,
    /// 事件：每个事件长度为 parameters.len()。
    /// 保留原始数值表示（整数即整数，非有限浮点保留 NaN/Inf 位模式）。
    pub events: Vec<Vec<RawValue>>,
    pub raw_keyword_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FcsParam {
    pub index: usize,
    pub name: String,
    /// 探测器/通道名（$PnN 即 name；$PnS 为附加标记）。
    pub label: Option<String>,
    pub unit: String,
    pub bits: i64,
    pub amplification: Option<String>,
}

/// 原始通道值：不丢失整数与特殊浮点的区别。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum RawValue {
    Integer(i64),
    /// 允许 NaN/Inf，因此“极端信号”在原位置保留。
    Float(f64),
}

impl RawValue {
    pub fn as_f64(self) -> f64 {
        match self {
            RawValue::Integer(i) => i as f64,
            RawValue::Float(f) => f,
        }
    }

    pub fn is_finite(self) -> bool {
        match self {
            RawValue::Integer(_) => true,
            RawValue::Float(f) => f.is_finite(),
        }
    }
}

/// 解析失败：仅描述问题，调用方负责整次导入回滚。
#[derive(Debug, Clone)]
pub struct FcsError(pub String);

impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FcsError {}

fn keyword<'a>(keywords: &'a [(String, String)], key: &str) -> Option<&'a str> {
    keywords
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 {
        return Err(FcsError("FCS 文件短于 58 字节头".to_string()));
    }
    let version = String::from_utf8_lossy(&bytes[0..6]).trim().to_string();
    if !version.starts_with("FCS") {
        return Err(FcsError(format!("不是 FCS 文件：{version}")));
    }

    let text_start = read_offset(&bytes[10..18])? as usize;
    let text_end = read_offset(&bytes[18..26])? as usize;
    let mut data_start = read_offset(&bytes[26..34])? as usize;
    let mut data_end = read_offset(&bytes[34..42])? as usize;
    if text_end >= bytes.len() || text_start >= text_end {
        return Err(FcsError("TEXT 段偏移非法".to_string()));
    }

    let delimiter = bytes[text_start] as char;
    let raw_keyword_text = String::from_utf8_lossy(&bytes[text_start + 1..text_end]).to_string();
    let keywords = parse_text_segment(&raw_keyword_text, delimiter);

    // FCS3.1 允许 DATA 偏移为 0，真值放在 $BEGINDATA/$ENDDATA。
    if let Some(v) = keyword(&keywords, "$BEGINDATA") {
        data_start = parse_offset_value(v).map_err(|_| {
            FcsError(format!("$BEGINDATA 非法：{v}"))
        })?;
    }
    if let Some(v) = keyword(&keywords, "$ENDDATA") {
        data_end = parse_offset_value(v).map_err(|_| {
            FcsError(format!("$ENDDATA 非法：{v}"))
        })?;
    }
    if data_start == 0 || data_end <= data_start || data_end > bytes.len() {
        return Err(FcsError(format!(
            "DATA 段偏移非法：{data_start}..{data_end}（文件长度 {}）",
            bytes.len()
        )));
    }

    let n_params: usize = keyword(&keywords, "$PAR")
        .ok_or_else(|| FcsError("缺少 $PAR".to_string()))?
        .trim()
        .parse()
        .map_err(|_| FcsError("$PAR 非法".to_string()))?;
    let n_events: usize = keyword(&keywords, "$TOT")
        .ok_or_else(|| FcsError("缺少 $TOT".to_string()))?
        .trim()
        .parse()
        .map_err(|_| FcsError("$TOT 非法".to_string()))?;
    let datatype = keyword(&keywords, "$DATATYPE")
        .ok_or_else(|| FcsError("缺少 $DATATYPE".to_string()))?
        .trim()
        .to_string();
    let mode = keyword(&keywords, "$MODE").map(|s| s.trim()).unwrap_or("L");
    if mode != "L" {
        return Err(FcsError(format!("仅支持 list 模式($MODE=L)，得到 {mode}")));
    }
    let byteorder = keyword(&keywords, "$BYTEORD").map(|s| s.trim()).unwrap_or("1,2,3,4");

    let parameters = build_params(&keywords, n_params)?;
    let data = &bytes[data_start..data_end];
    let events = parse_events(data, n_params, n_events, &datatype, byteorder)?;

    Ok(FcsFile {
        version,
        keywords,
        parameters,
        events,
        raw_keyword_text,
    })
}

fn read_offset(slice: &[u8]) -> Result<i64, FcsError> {
    let s = String::from_utf8_lossy(slice);
    s.trim()
        .parse::<i64>()
        .map_err(|_| FcsError(format!("偏移字段非法：{s}")))
}

fn parse_offset_value(v: &str) -> Result<usize, std::num::ParseIntError> {
    // 偏移值可能被零填充为等宽（如 00000353）。
    v.trim().trim_start_matches('0').parse::<usize>().or_else(|_| {
        if v.trim().chars().all(|c| c == '0') {
            Ok(0)
        } else {
            v.trim().parse::<usize>()
        }
    })
}

fn parse_text_segment(text: &str, delimiter: char) -> Vec<(String, String)> {
    // 逐字符解析：分隔符成对出现表示字面分隔符；单个分隔符分隔键值。
    // 段以 K\\V\\K\\V\\ 组织（调用前已去掉首分隔符）。
    let chars: Vec<char> = text.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == delimiter && i + 1 < chars.len() && chars[i + 1] == delimiter {
            cur.push(delimiter);
            i += 2;
        } else if chars[i] == delimiter {
            tokens.push(std::mem::take(&mut cur));
            i += 1;
        } else {
            cur.push(chars[i]);
            i += 1;
        }
    }
    if !cur.is_empty() || !tokens.is_empty() {
        tokens.push(cur);
    }
    // 去掉由首尾分隔符产生的空 token。
    let tokens: Vec<String> = tokens
        .into_iter()
        .enumerate()
        .filter(|(i, t)| !(t.is_empty() && (*i == 0)))
        .map(|(_, t)| t)
        .collect();
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        pairs.push((tokens[i].clone(), tokens[i + 1].clone()));
        i += 2;
    }
    pairs
}

fn build_params(
    keywords: &[(String, String)],
    n_params: usize,
) -> Result<Vec<FcsParam>, FcsError> {
    let mut params = Vec::with_capacity(n_params);
    for n in 1..=n_params {
        let name = keyword(keywords, &format!("$P{n}N"))
            .ok_or_else(|| FcsError(format!("缺少 $P{n}N")))?
            .trim()
            .to_string();
        let label = keyword(keywords, &format!("$P{n}S")).map(|s| s.trim().to_string());
        let bits = keyword(keywords, &format!("$P{n}B"))
            .ok_or_else(|| FcsError(format!("缺少 $P{n}B")))?
            .trim()
            .parse::<i64>()
            .map_err(|_| FcsError(format!("$P{n}B 非法")))?;
        let amplification = keyword(keywords, &format!("$P{n}E")).map(|s| s.to_string());
        let unit = keyword(keywords, &format!("$P{n}U"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "channel".to_string());
        params.push(FcsParam {
            index: n - 1,
            name,
            label,
            unit,
            bits,
            amplification,
        });
    }
    Ok(params)
}

fn parse_events(
    data: &[u8],
    n_params: usize,
    n_events: usize,
    datatype: &str,
    byteorder: &str,
) -> Result<Vec<Vec<RawValue>>, FcsError> {
    let be = match byteorder {
        "1,2,3,4" => false,
        "4,3,2,1" => true,
        other => return Err(FcsError(format!("不支持的 $BYTEORD：{other}"))),
    };
    let mut events = Vec::with_capacity(n_events);
    match datatype {
        "I" => {
            // 位宽由 $PnB 决定，本案例包统一使用 32 位整数。
            let need = n_events * n_params * 4;
            if data.len() < need {
                return Err(FcsError(format!(
                    "DATA 长度不足：需要 {need}，实际 {}",
                    data.len()
                )));
            }
            for e in 0..n_events {
                let mut row = Vec::with_capacity(n_params);
                for p in 0..n_params {
                    let off = (e * n_params + p) * 4;
                    let raw: [u8; 4] = data[off..off + 4].try_into().unwrap();
                    let v = if be {
                        i32::from_be_bytes(raw)
                    } else {
                        i32::from_le_bytes(raw)
                    };
                    row.push(RawValue::Integer(v as i64));
                }
                events.push(row);
            }
        }
        "F" | "D" => {
            let width = if datatype == "F" { 4 } else { 8 };
            let need = n_events * n_params * width;
            if data.len() < need {
                return Err(FcsError(format!(
                    "DATA 长度不足：需要 {need}，实际 {}",
                    data.len()
                )));
            }
            for e in 0..n_events {
                let mut row = Vec::with_capacity(n_params);
                for p in 0..n_params {
                    let off = (e * n_params + p) * width;
                    let v: f64 = if width == 4 {
                        let raw: [u8; 4] = data[off..off + 4].try_into().unwrap();
                        let bits = if be {
                            u32::from_be_bytes(raw)
                        } else {
                            u32::from_le_bytes(raw)
                        };
                        f32::from_bits(bits) as f64
                    } else {
                        let raw: [u8; 8] = data[off..off + 8].try_into().unwrap();
                        let bits = if be {
                            u64::from_be_bytes(raw)
                        } else {
                            u64::from_le_bytes(raw)
                        };
                        f64::from_bits(bits)
                    };
                    row.push(RawValue::Float(v));
                }
                events.push(row);
            }
        }
        _ => unreachable!(),
    }
    Ok(events)
}

/// 解析 `$SPILL`（FCS3.1）/`$SPILLOVER`（FCS3.0）CSV。
/// 形如：n, name1, ..., nameN, S11, ..., Snn
pub fn parse_spill(raw: &str) -> Result<SpilloverMatrix, FcsError> {
    let fields: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    if fields.len() < 2 {
        return Err(FcsError("spillover 字段过少".to_string()));
    }
    let n: usize = fields[0]
        .parse()
        .map_err(|_| FcsError(format!("spillover 维度非法：{}", fields[0])))?;
    if fields.len() != 1 + n + n * n {
        return Err(FcsError(format!(
            "spillover 字段数不匹配：维度 {n}，字段 {}",
            fields.len()
        )));
    }
    let names = &fields[1..1 + n];
    let mut entries = Vec::with_capacity(n * n);
    for token in &fields[1 + n..] {
        entries.push(
            token
                .parse::<f64>()
                .map_err(|_| FcsError(format!("spillover 数值非法：{token}")))?,
        );
    }
    Ok(SpilloverMatrix {
        channels: names
            .iter()
            .map(|name| ChannelRef {
                name: name.clone(),
                fluorochrome: None,
                unit: "fluorescence".to_string(),
            })
            .collect(),
        entries,
        raw_repr: Some(raw.to_string()),
    })
}

/// 从解析后的 FCS 中提取 spillover（若存在），保留原始字符串。
pub fn extract_spillover(fcs: &FcsFile) -> Option<Result<SpilloverMatrix, FcsError>> {
    let raw = keyword(&fcs.keywords, "$SPILL")
        .or_else(|| keyword(&fcs.keywords, "$SPILLOVER"))?;
    Some(parse_spill(raw))
}

/// FCS 写出（小端；$DATATYPE=F 使用 f32，可承载 NaN/Inf）。
pub fn write_fcs(fcs: &FcsFile, datatype: FcsWriteType) -> Vec<u8> {
    let n_params = fcs.parameters.len();
    let n_events = fcs.events.len();
    let delim = b'\\';

    let mut keywords: Vec<(String, String)> = fcs
        .keywords
        .iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "$BEGINDATA"
                    | "$ENDDATA"
                    | "$BEGINSTEXT"
                    | "$ENDSTEXT"
                    | "$BEGINANALYSIS"
                    | "$ENDANALYSIS"
                    | "$PAR"
                    | "$TOT"
                    | "$DATATYPE"
                    | "$BYTEORD"
            )
        })
        .cloned()
        .collect();
    upsert(&mut keywords, "$BEGINANALYSIS", "0");
    upsert(&mut keywords, "$ENDANALYSIS", "0");
    upsert(&mut keywords, "$BEGINSTEXT", "0");
    upsert(&mut keywords, "$ENDSTEXT", "0");
    upsert(&mut keywords, "$PAR", &n_params.to_string());
    upsert(&mut keywords, "$TOT", &n_events.to_string());
    upsert(&mut keywords, "$MODE", "L");
    upsert(&mut keywords, "$BYTEORD", "1,2,3,4");
    for (i, p) in fcs.parameters.iter().enumerate() {
        let n = i + 1;
        upsert(&mut keywords, &format!("$P{n}N"), &p.name);
        if let Some(label) = &p.label {
            upsert(&mut keywords, &format!("$P{n}S"), label);
        }
        upsert(&mut keywords, &format!("$P{n}B"), &p.bits.to_string());
        upsert(&mut keywords, &format!("$P{n}E"), p.amplification.as_deref().unwrap_or("0,0"));
        upsert(&mut keywords, &format!("$P{n}U"), &p.unit);
        if let Some(label) = &p.label {
            upsert(&mut keywords, &format!("$P{n}S"), label);
        }
    }
    let (dt, width): (&str, usize) = match datatype {
        FcsWriteType::Float32 => {
            upsert(&mut keywords, "$DATATYPE", "F");
            ("F", 4)
        }
        FcsWriteType::Integer32 => {
            upsert(&mut keywords, "$DATATYPE", "I");
            ("I", 4)
        }
    };

    let data_len = n_events * n_params * width;
    let header_len = 58;
    let text_start = header_len;
    // FCS3.1：头部 DATA 偏移置 0，真值保存在 $BEGINDATA/$ENDDATA。
    // 迭代到不动点：data_start 依赖 TEXT 长度，TEXT 中写入的偏移用
    // 零填充到等宽，因此长度只取决于偏移的位数，两步即收敛。
    let mut data_start = header_len;
    let text = loop {
        let width = (data_start + 1).to_string().len().max(8);
        let text = rebuild_text_fixed_width(&keywords, delim, data_start, data_start + data_len, width);
        let next_start = header_len + text.len();
        if next_start == data_start {
            break text;
        }
        data_start = next_start;
    };
    let data_end = data_start + data_len;
    let text_end = text_start + text.len();

    let mut out = Vec::with_capacity(data_end);
    let mut header = vec![b' '; header_len];
    header[0..6].copy_from_slice(b"FCS3.1");
    write_offsets(&mut header, text_start, text_end, 0, 0);
    out.extend_from_slice(&header);
    out.extend_from_slice(&text);

    for row in &fcs.events {
        for (p, val) in row.iter().enumerate() {
            let _ = p;
            match (dt, *val) {
                ("F", RawValue::Float(f)) => out.extend_from_slice(&(f as f32).to_le_bytes()),
                ("F", RawValue::Integer(i)) => {
                    out.extend_from_slice(&(i as f32).to_le_bytes())
                }
                ("I", RawValue::Integer(i)) => out.extend_from_slice(&(i as i32).to_le_bytes()),
                ("I", RawValue::Float(f)) => {
                    out.extend_from_slice(&(f as i32).to_le_bytes())
                }
                _ => unreachable!(),
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub enum FcsWriteType {
    Float32,
    Integer32,
}

fn rebuild_text(keywords: &[(String, String)], delim: u8, ds: usize, de: usize) -> Vec<u8> {
    let mut text = Vec::new();
    text.push(delim);
    for (k, v) in keywords {
        text.extend_from_slice(k.as_bytes());
        text.push(delim);
        text.extend_from_slice(v.as_bytes());
        text.push(delim);
    }
    text.extend_from_slice(b"$BEGINDATA");
    text.push(delim);
    text.extend_from_slice(ds.to_string().as_bytes());
    text.push(delim);
    text.extend_from_slice(b"$ENDDATA");
    text.push(delim);
    text.extend_from_slice(de.to_string().as_bytes());
    text.push(delim);
    text
}

fn rebuild_text_fixed_width(
    keywords: &[(String, String)],
    delim: u8,
    ds: usize,
    de: usize,
    width: usize,
) -> Vec<u8> {
    let fixed = |v: usize| format!("{v:0width$}");
    let mut text = Vec::new();
    text.push(delim);
    for (k, v) in keywords {
        text.extend_from_slice(k.as_bytes());
        text.push(delim);
        text.extend_from_slice(v.as_bytes());
        text.push(delim);
    }
    text.extend_from_slice(b"$BEGINDATA");
    text.push(delim);
    text.extend_from_slice(fixed(ds).as_bytes());
    text.push(delim);
    text.extend_from_slice(b"$ENDDATA");
    text.push(delim);
    text.extend_from_slice(fixed(de).as_bytes());
    text.push(delim);
    text
}

fn upsert(keywords: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(slot) = keywords.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_string();
    } else {
        keywords.push((key.to_string(), value.to_string()));
    }
}

fn write_offsets(
    header: &mut [u8],
    text_start: usize,
    text_end: usize,
    data_start: usize,
    data_end: usize,
) {
    let put = |buf: &mut [u8], off: usize, val: usize| {
        let s = format!("{val:>8}");
        buf[off..off + 8].copy_from_slice(s.as_bytes());
    };
    put(header, 10, text_start);
    put(header, 18, text_end);
    put(header, 26, data_start);
    put(header, 34, data_end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_i32() -> FcsFile {
        FcsFile {
            version: "FCS3.1".to_string(),
            keywords: vec![
                ("$FIL".to_string(), "sample.fcs".to_string()),
                ("$SRC".to_string(), "unit-test".to_string()),
            ],
            parameters: vec![
                FcsParam {
                    index: 0,
                    name: "FL1-A".to_string(),
                    label: Some("FITC".to_string()),
                    unit: "channel".to_string(),
                    bits: 32,
                    amplification: Some("0,0".to_string()),
                },
                FcsParam {
                    index: 1,
                    name: "FL2-A".to_string(),
                    label: Some("PE".to_string()),
                    unit: "channel".to_string(),
                    bits: 32,
                    amplification: Some("0,0".to_string()),
                },
            ],
            events: vec![
                vec![RawValue::Integer(100), RawValue::Integer(200)],
                vec![RawValue::Integer(0), RawValue::Integer(-5)],
            ],
            raw_keyword_text: String::new(),
        }
    }

    #[test]
    fn integer_roundtrip_preserves_values() {
        let bytes = write_fcs(&sample_i32(), FcsWriteType::Integer32);
        let parsed = parse(&bytes).expect("parse");
        assert_eq!(parsed.version, "FCS3.1");
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.events[0][0], RawValue::Integer(100));
        assert_eq!(parsed.events[1][1], RawValue::Integer(-5));
        assert_eq!(parsed.parameters[0].name, "FL1-A");
    }

    #[test]
    fn float_nan_and_inf_roundtrip() {
        let mut fcs = sample_i32();
        fcs.events = vec![vec![
            RawValue::Float(f64::NAN),
            RawValue::Float(f64::INFINITY),
        ]];
        let bytes = write_fcs(&fcs, FcsWriteType::Float32);
        let parsed = parse(&bytes).expect("parse float");
        match parsed.events[0][0] {
            RawValue::Float(f) => assert!(f.is_nan()),
            other => panic!("expected float NaN, got {other:?}"),
        }
        match parsed.events[0][1] {
            RawValue::Float(f) => assert_eq!(f, f64::INFINITY),
            other => panic!("expected float inf, got {other:?}"),
        }
    }

    #[test]
    fn spill_csv_roundtrip_and_parse() {
        let csv = "2,FL1-A,FL2-A,1,0.1,0.02,1";
        let m = parse_spill(csv).expect("parse spill");
        assert_eq!(m.dim(), 2);
        assert_eq!(m.entries, vec![1.0, 0.1, 0.02, 1.0]);
        assert_eq!(m.raw_repr.as_deref(), Some(csv));
    }

    #[test]
    fn malformed_header_is_reported_not_partial_import() {
        assert!(parse(b"not an fcs file at all").is_err());
    }
}
