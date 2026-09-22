//! FCS 3.1 读取与写入（整数模式 $DATATYPE/I，$MODE/L）。
//!
//! 导入时保留原始词法表示：
//! - 正常整数 -> numeric + token 文本；
//! - 满量程值若带 OVF/SAT 语义（由调用方依据诊断标注提供）仍保留数值与 token；
//! - 任何无法解析的字节段 -> bad_token，事件保留在原位置，绝不丢弃。

use std::collections::BTreeMap;

use crate::error::FlowError;
use crate::Result;

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub keywords: BTreeMap<String, String>,
    /// 每条事件的原始 token 字符串（$PnR 十进制文本）。
    pub raw_tokens: Vec<Vec<String>>,
    /// 每条事件解析出的数值；失败位置为 None。
    pub numerics: Vec<Vec<Option<f64>>>,
    pub raw_bytes: Vec<u8>,
}

impl FcsFile {
    /// 返回 ($PnN, $PnR) 通道列表。
    pub fn channels(&self) -> Vec<(String, f64)> {
        let n_par: usize = self.keywords.get("$PAR").and_then(|v| v.parse().ok()).unwrap_or(0);
        (1..=n_par)
            .map(|i| {
                let name = self.keywords.get(&format!("$P{i}N")).cloned().unwrap_or_default();
                let range = self
                    .keywords
                    .get(&format!("$P{i}R"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(262144.0);
                (name, range)
            })
            .collect()
    }
}

fn is_upper_alnum_key(k: &str) -> bool {
    k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'$' || b == b'_')
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(FlowError::Msg("不是 FCS3.1 文件".into()));
    }
    let text_start: usize = std::str::from_utf8(&bytes[10..18])
        .map_err(|_| FlowError::Msg("TEXT 起始偏移非法".into()))?
        .trim()
        .parse()
        .map_err(|_| FlowError::Msg("TEXT 起始偏移非法".into()))?;
    let text_end: usize = std::str::from_utf8(&bytes[18..26])
        .map_err(|_| FlowError::Msg("TEXT 结束偏移非法".into()))?
        .trim()
        .parse()
        .map_err(|_| FlowError::Msg("TEXT 结束偏移非法".into()))?;
    let data_start: usize = std::str::from_utf8(&bytes[26..34])
        .map_err(|_| FlowError::Msg("DATA 起始偏移非法".into()))?
        .trim()
        .parse()
        .map_err(|_| FlowError::Msg("DATA 起始偏移非法".into()))?;
    let data_end: usize = std::str::from_utf8(&bytes[34..42])
        .map_err(|_| FlowError::Msg("DATA 结束偏移非法".into()))?
        .trim()
        .parse()
        .map_err(|_| FlowError::Msg("DATA 结束偏移非法".into()))?;
    if text_start >= bytes.len() || text_end > bytes.len() || text_end <= text_start {
        return Err(FlowError::Msg("TEXT 段偏移越界（文件被截断）".into()));
    }
    let delimiter = bytes[text_start] as char;
    let text_slice = &bytes[text_start + 1..=text_end - 1];

    // 关键字段优先，允许 TEXT 中的 $BEGINDATA/$ENDDATA 覆盖头部（FCS3.1 要求）。
    let text = parse_text_segment(text_slice, delimiter)?;
    let begindata = text
        .get("$BEGINDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(data_start);
    let enddata = text
        .get("$ENDDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(data_end);

    let n_tot: usize = must_key(&text, "$TOT")?;
    let n_par: usize = must_key(&text, "$PAR")?;
    let dtype = must_key_str(&text, "$DATATYPE")?.to_string();
    let mode = must_key_str(&text, "$MODE")?.to_string();
    if mode != "L" {
        return Err(FlowError::Msg(format!("案例包仅支持 $MODE=L，收到 {mode}")));
    }
    if dtype != "I" && dtype != "A" {
        return Err(FlowError::Msg(format!("仅支持 $DATATYPE=I 或 A，收到 {dtype}")));
    }
    let byteord = text.get("$BYTEORD").cloned().unwrap_or_else(|| "1,2,3,4".into());
    let big_endian = byteord.starts_with('4');
    let bytes_per: Vec<usize> = (1..=n_par)
        .map(|i| {
            let bits: usize = must_key(&text, &format!("$P{i}B"))?;
            if bits % 8 != 0 {
                return Err(FlowError::Msg(format!("$P{i}B={bits} 不是 8 的倍数")));
            }
            Ok(bits / 8)
        })
        .collect::<Result<Vec<_>>>()?;
    if begindata >= bytes.len() || enddata > bytes.len() || enddata <= begindata {
        return Err(FlowError::Msg("DATA 段偏移越界（文件被截断）".into()));
    }
    let data = &bytes[begindata..=enddata - 1];

    let mut raw_tokens: Vec<Vec<String>> = Vec::with_capacity(n_tot);
    let mut numerics: Vec<Vec<Option<f64>>> = Vec::with_capacity(n_tot);
    if dtype == "A" {
        let s = std::str::from_utf8(data).map_err(|_| FlowError::Msg("ASCII DATA 非 UTF-8".to_string()))?;
        let rows: Vec<&str> = s.split(',').collect();
        if rows.len() < n_tot {
            return Err(FlowError::Msg(format!(
                "ASCII DATA 事件数 {} 小于 $TOT={}（导入回滚，不保留半份记录）",
                rows.len(), n_tot
            )));
        }
        for row in rows.iter().take(n_tot) {
            let toks: Vec<String> = row.split_whitespace().map(|t| t.to_string()).collect();
            if toks.len() < n_par {
                return Err(FlowError::Msg(format!(
                    "ASCII DATA 某事件通道数 {} 小于 $PAR={}（导入回滚）", toks.len(), n_par
                )));
            }
            let nums = toks.iter().map(|t| t.parse::<f64>().ok()).collect();
            raw_tokens.push(toks);
            numerics.push(nums);
        }
    } else {
        let rec_width: usize = bytes_per.iter().sum();
        let expect = n_tot * rec_width;
        if data.len() < expect {
            return Err(FlowError::Msg(format!(
                "DATA 段长度 {} 小于期望 {}（导入回滚，不保留半份记录）",
                data.len(), expect
            )));
        }
        for ev in 0..n_tot {
            let mut toks = Vec::with_capacity(n_par);
            let mut nums = Vec::with_capacity(n_par);
            let mut off = ev * rec_width;
            for width in &bytes_per {
                let chunk = &data[off..off + width];
                off += width;
                let mut val: u64 = 0;
                for (k, b) in chunk.iter().enumerate() {
                    let byte = if big_endian { *b } else { chunk[width - 1 - k] };
                    val = (val << 8) | byte as u64;
                }
                let token = val.to_string();
                nums.push(Some(val as f64));
                toks.push(token);
            }
            raw_tokens.push(toks);
            numerics.push(nums);
        }
    }

    Ok(FcsFile { keywords: text, raw_tokens, numerics, raw_bytes: bytes.to_vec() })
}

fn must_key(text: &BTreeMap<String, String>, key: &str) -> Result<usize> {
    text.get(key)
        .ok_or_else(|| FlowError::Msg(format!("缺少必需关键字 {key}")))?
        .parse()
        .map_err(|_| FlowError::Msg(format!("关键字 {key} 不是整数")))
}

fn must_key_str<'a>(text: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    text.get(key).map(|s| s.as_str()).ok_or_else(|| FlowError::Msg(format!("缺少关键字 {key}")))
}

fn parse_text_segment(slice: &[u8], delim: char) -> Result<BTreeMap<String, String>> {
    // 双分隔符转义；键值交替。
    let s = std::str::from_utf8(slice).map_err(|_| FlowError::Msg("TEXT 段非 UTF-8".into()))?;
    let mut out = BTreeMap::new();
    let mut parts: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == delim {
            if chars.peek() == Some(&delim) {
                buf.push(c);
                chars.next();
            } else {
                parts.push(std::mem::take(&mut buf));
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        parts.push(buf);
    }
    let mut i = 0;
    while i + 1 < parts.len() {
        let k = parts[i].trim().to_string();
        let v = parts[i + 1].clone();
        i += 2;
        if k.is_empty() {
            continue;
        }
        // 必需关键字按大写比较，自定义键原样保留。
        let key = if is_upper_alnum_key(&k) { k.to_uppercase() } else { k };
        out.insert(key, v);
    }
    Ok(out)
}

/// 写出 FCS3.1（小端 $BYTEORD=1,2,3,4，2 字节/通道整数）。
/// `token_diagnostics` 为每事件每通道可选标记，仅影响调用方后续标注，不改变字节。
pub fn write_fcs(
    channels: &[FcsChannelSpec],
    values: &[Vec<u16>],
    extra_keywords: &BTreeMap<String, String>,
) -> Vec<u8> {
    let n_par = channels.len();
    let n_tot = values.len();
    let mut text: Vec<String> = Vec::new();
    let push = |text: &mut Vec<String>, k: &str, v: String| {
        text.push(k.to_string());
        text.push(v);
    };
    push(&mut text, "$BEGINANALYSIS", "0".into());
    push(&mut text, "$ENDANALYSIS", "0".into());
    push(&mut text, "$BEGINSTEXT", "0".into());
    push(&mut text, "$ENDSTEXT", "0".into());
    push(&mut text, "$TOT", n_tot.to_string());
    push(&mut text, "$PAR", n_par.to_string());
    push(&mut text, "$DATATYPE", "I".into());
    push(&mut text, "$MODE", "L".into());
    push(&mut text, "$BYTEORD", "1,2,3,4".into());
    push(&mut text, "$NEXTDATA", "0".into());
    push(&mut text, "$FIL", "flow-boundary-chamber.fcs".into());
    push(&mut text, "$CYT", "FlowlineFixture".into());
    for (i, ch) in channels.iter().enumerate() {
        let n = i + 1;
        push(&mut text, &format!("$P{n}N"), ch.name.clone());
        push(&mut text, &format!("$P{n}S"), ch.long_name.clone());
        push(&mut text, &format!("$P{n}B"), "16".into());
        push(&mut text, &format!("$P{n}R"), ch.range.to_string());
        push(&mut text, &format!("$P{n}E"), "0,0".into());
        push(&mut text, &format!("$P{n}G"), "1".into());
    }
    for (k, v) in extra_keywords {
        push(&mut text, k, v.clone());
    }
    // 组装 TEXT 段（分隔符 '/'），偏移随后回填。
    let delim = b'/';
    let mut text_bytes = vec![delim];
    for (i, part) in text.iter().enumerate() {
        let escaped = part.replace('/', "//");
        text_bytes.extend_from_slice(escaped.as_bytes());
        if i + 1 < text.len() {
            text_bytes.push(delim);
        }
    }
    text_bytes.push(delim);
    let header_len = 58usize;
    let text_start = 58usize;
    let text_end = text_start + text_bytes.len();
    // 偶数对齐 DATA。
    let mut data_start = text_end;
    if data_start % 2 != 0 {
        text_bytes.push(b' ');
        data_start += 1;
    }
    let mut data = Vec::with_capacity(n_tot * n_par * 2);
    for row in values {
        for v in row {
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let data_end = data_start + data.len();

    let mut full = Vec::with_capacity(data_end);
    full.extend_from_slice(b"FCS3.1");
    full.extend_from_slice(b"    "); // 字节 6..10（4 空格补充文本），TEXT 偏移段从字节 10 开始
    let field = |v: usize, w: usize| format!("{v:>w$}");
    full.extend_from_slice(field(text_start, 8).as_bytes());
    full.extend_from_slice(field(text_end, 8).as_bytes());
    full.extend_from_slice(field(data_start, 8).as_bytes());
    full.extend_from_slice(field(data_end, 8).as_bytes());
    full.extend_from_slice(b"       0       0"); // $BEGINDATA 占位已给，这里是 analysis 两个 8 宽字段
    assert_eq!(full.len(), header_len, "header must be 58 bytes, got {}", full.len());
    full.extend_from_slice(&text_bytes);
    full.extend_from_slice(&data);
    full
}

#[derive(Debug, Clone)]
pub struct FcsChannelSpec {
    pub name: String,
    pub long_name: String,
    pub range: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channels() -> Vec<FcsChannelSpec> {
        vec![
            FcsChannelSpec { name: "FSC-A".into(), long_name: "FSC-A".into(), range: 262144 },
            FcsChannelSpec { name: "SSC-A".into(), long_name: "SSC-A".into(), range: 262144 },
            FcsChannelSpec { name: "FITC-A".into(), long_name: "CD3 FITC".into(), range: 262144 },
        ]
    }

    #[test]
    fn roundtrip_preserves_values_and_keywords() {
        let vals = vec![vec![100u16, 200, 300], vec![65535, 1, 0], vec![12345, 54321, 1000]];
        let mut extra = BTreeMap::new();
        extra.insert("$OP".into(), "flowline".into());
        let bytes = write_fcs(&channels(), &vals, &extra);
        let f = parse_fcs(&bytes).unwrap();
        assert_eq!(f.keywords.get("$TOT").unwrap(), "3");
        assert_eq!(f.keywords.get("$PAR").unwrap(), "3");
        assert_eq!(f.keywords.get("FITC-A"), None);
        assert_eq!(f.keywords.get("$P3N").unwrap(), "FITC-A");
        assert_eq!(f.keywords.get("$OP").unwrap(), "flowline");
        assert_eq!(f.numerics[1][0], Some(65535.0));
        assert_eq!(f.raw_tokens[2][1], "54321");
        assert_eq!(f.numerics[0], vec![Some(100.0), Some(200.0), Some(300.0)]);
    }

    #[test]
    fn rejects_truncated_data_segment_atomically() {
        let vals = vec![vec![1u16, 2, 3], vec![4, 5, 6]];
        let bytes = write_fcs(&channels(), &vals, &BTreeMap::new());
        let truncated = bytes[..bytes.len() - 2].to_vec();
        let err = parse_fcs(&truncated).unwrap_err().to_string();
        assert!(err.contains("DATA 段"), "{err}");
    }

    #[test]
    fn escapes_delimiter_in_keyword_values() {
        let vals = vec![vec![1u16, 2, 3]];
        let mut extra = BTreeMap::new();
        extra.insert("$SRC".into(), "plate/A/well".into());
        let bytes = write_fcs(&channels(), &vals, &extra);
        let f = parse_fcs(&bytes).unwrap();
        assert_eq!(f.keywords.get("$SRC").unwrap(), "plate/A/well");
    }
}
