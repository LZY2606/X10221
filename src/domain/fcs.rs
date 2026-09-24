//! 最小但真实的 FCS 3.1 读取器：HEADER(256B) / TEXT / DATA。
//! 支持 `$DATATYPE = F`（$BYTEORD 1,2,3,4 大端 IEEE-754 单精度）。
//! 导入时保留所有原始表示，无法转换的值留原位并附诊断，不做任何不可逆清洗。

use sha2::{Digest, Sha256};

use crate::domain::error::{FlowError, Result};
use crate::domain::model::{ChannelSpec, RawEvents};
use crate::domain::units::{Measured, Unit};

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub raw: RawEvents,
    pub keywords: Vec<(String, String)>,
}

pub fn parse_fcs(bytes: &[u8], filename: &str) -> Result<ParsedFcs> {
    let magic = &bytes[0..6.min(bytes.len())];
    if bytes.len() < 256 || magic != b"FCS3.1" && magic != b"FCS3.0" {
        return Err(FlowError::Fcs("不是 FCS3.0/3.1 文件（前 6 字节不匹配）".into()));
    }
    let text_start = read_offset(&bytes[10..18])?;
    let text_end = read_offset(&bytes[18..26])?;
    let data_start = read_offset(&bytes[26..34])?;
    let data_end = read_offset(&bytes[34..42])?;
    if text_start == 0 || text_end <= text_start || text_end as usize > bytes.len() {
        return Err(FlowError::Fcs("TEXT 段偏移非法".into()));
    }

    let segment = &bytes[text_start as usize..text_end as usize];
    let delim = segment[0];
    let keywords = parse_text_segment(segment, delim)?;
    let get = |key: &str| -> Result<String> {
        keywords
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| FlowError::Fcs(format!("缺少关键字 {key}")))
    };

    let par: usize = get("$PAR")?.parse().map_err(|_| FlowError::Fcs("$PAR 非整数".into()))?;
    let total: usize = get("$TOT")?.parse().map_err(|_| FlowError::Fcs("$TOT 非整数".into()))?;
    let datatype = get("$DATATYPE")?;
    let byteord = get("$BYTEORD")?;
    if datatype != "F" {
        return Err(FlowError::Fcs(format!("案例包只提供 $DATATYPE=F，收到 {datatype}")));
    }
    if !byteord.starts_with("1,2,3,4") {
        return Err(FlowError::Fcs("案例包使用大端字节序 1,2,3,4".into()));
    }

    let mut channels = Vec::with_capacity(par);
    for n in 1..=par {
        let name = get(&format!("$P{n}N"))?;
        let long_name = get(&format!("$P{n}S")).ok();
        let amplification = get(&format!("$P{n}E")).unwrap_or_else(|_| "0,0".into());
        channels.push(ChannelSpec { index: n - 1, name, long_name, amplification });
    }

    let (ds, de) = if data_start == 0 {
        let b: usize = get("$BEGINDATA")?.parse().map_err(|_| FlowError::Fcs("$BEGINDATA".into()))?;
        let e_plus: usize = get("$ENDDATA")?.parse().map_err(|_| FlowError::Fcs("$ENDDATA".into()))?;
        (b, e_plus + 1)
    } else {
        (data_start as usize, data_end as usize)
    };
    if de <= ds || de > bytes.len() {
        return Err(FlowError::Fcs("DATA 段偏移非法".into()));
    }
    let data = &bytes[ds..de];
    let need = total * par * 4;
    if data.len() < need {
        return Err(FlowError::Fcs(format!("DATA 段过短：需要 {need} 字节，实际 {}", data.len())));
    }

    let mut events = Vec::with_capacity(total);
    for i in 0..total {
        let mut row = Vec::with_capacity(par);
        for c in 0..par {
            let off = (i * par + c) * 4;
            let bits = u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]);
            let unit = if channels[c].name.starts_with("FSC") || channels[c].name.starts_with("SSC") {
                Unit::ScatterAru
            } else {
                Unit::RawIntensity
            };
            row.push(Measured::raw_f32(bits, unit));
        }
        events.push(row);
    }

    Ok(ParsedFcs {
        raw: RawEvents {
            id: String::new(),
            source_sha: sha256_hex(bytes),
            filename: filename.to_string(),
            channels,
            events,
        },
        keywords,
    })
}

fn read_offset(b: &[u8]) -> Result<usize> {
    let t = std::str::from_utf8(b).map_err(|_| FlowError::Fcs("偏移非 ASCII"))?.trim();
    if t.is_empty() {
        return Ok(0);
    }
    t.parse().map_err(|_| FlowError::Fcs(format!("偏移非整数: {t:?}")))
}

fn parse_text_segment(seg: &[u8], delim: u8) -> Result<Vec<(String, String)>> {
    let bytes: Vec<u8> = seg.to_vec();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != delim {
            return Err(FlowError::Fcs("TEXT 段分隔符不成对".into()));
        }
        i += 1;
        let mut token = Vec::new();
        while i < bytes.len() {
            if bytes[i] == delim {
                if i + 1 < bytes.len() && bytes[i + 1] == delim {
                    token.push(delim);
                    i += 2;
                } else {
                    i += 1;
                    break;
                }
            } else {
                token.push(bytes[i]);
                i += 1;
            }
        }
        tokens.push(String::from_utf8_lossy(&token).into_owned());
    }
    let mut pairs = Vec::new();
    let mut j = 1; // tokens[0] 为空（首分隔符之前）
    while j + 1 < tokens.len() {
        pairs.push((tokens[j].clone(), tokens[j + 1].clone()));
        j += 2;
    }
    Ok(pairs)
}

/// 构造 FCS 3.1 字节（案例包生成器与测试使用）。
/// 偏移在 TEXT 中以 10 位定宽书写，布局与数值无关，一次成型。
pub fn build_fcs(channels: &[(&str, &str)], matrix: &[f32], extra_keywords: &[(&str, &str)]) -> Vec<u8> {
    let par = channels.len();
    let total = matrix.len() / par;
    let off = |v: usize| format!("{v:010}");

    let make_text = |data_start: usize, data_end_m1: usize| {
        let mut text: Vec<(String, String)> = vec![
            ("$BEGINANALYSIS".into(), "0".into()),
            ("$ENDANALYSIS".into(), "0".into()),
            ("$BEGINDATA".into(), off(data_start)),
            ("$ENDDATA".into(), off(data_end_m1)),
            ("$TOT".into(), total.to_string()),
            ("$PAR".into(), par.to_string()),
            ("$DATATYPE".into(), "F".into()),
            ("$BYTEORD".into(), "1,2,3,4".into()),
            ("$MODE".into(), "L".into()),
            ("$NEXTDATA".into(), "0".into()),
        ];
        for (n, (name, amp)) in channels.iter().enumerate() {
            let n = n + 1;
            text.push((format!("$P{n}N"), (*name).to_string()));
            text.push((format!("$P{n}E"), (*amp).to_string()));
            text.push((format!("$P{n}B"), "32".into()));
            text.push((format!("$P{n}R"), "262144".into()));
        }
        for (k, v) in extra_keywords {
            text.push(((*k).to_string(), (*v).to_string()));
        }
        let delim = b'/';
        let mut b = Vec::new();
        for (k, v) in &text {
            b.push(delim);
            b.extend_from_slice(k.as_bytes());
            b.push(delim);
            b.extend_from_slice(v.as_bytes());
        }
        b.push(delim);
        b
    };

    let text_start = 256usize;
    // 定宽 10 位保证 text 长度不随偏移变化。
    let probe = make_text(text_start + 1, text_start);
    let data_start = text_start + probe.len();
    let data_end = data_start + matrix.len() * 4;
    let text_bytes = make_text(data_start, data_end - 1);
    debug_assert_eq!(text_bytes.len(), probe.len());

    let mut header = vec![b' '; 256];
    header[0..6].copy_from_slice(b"FCS3.1");
    header[10..18].copy_from_slice(format!("{:>8}", text_start).as_bytes());
    header[18..26].copy_from_slice(format!("{:>8}", text_start + text_bytes.len()).as_bytes());
    // DATA 超过 99,999,999 才会超出定宽，案例数据远小于此；HEADER 置 0，读 $BEGINDATA。
    header[26..34].copy_from_slice(b"       0");
    header[34..42].copy_from_slice(b"       0");

    let mut data = Vec::with_capacity(matrix.len() * 4);
    for v in matrix {
        data.extend_from_slice(&v.to_be_bytes());
    }

    let mut out = Vec::with_capacity(data_end);
    out.extend_from_slice(&header);
    out.extend_from_slice(&text_bytes);
    out.extend_from_slice(&data);
    out
}
