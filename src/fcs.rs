//! 最小 FCS 3.1 读取器（HEADER/TEXT/DATA 三段）。
//! 只负责读取，不做清洗：越界值、未转换增益都原样保留并附诊断。
use crate::domain::{Diag, Qty, RawEvent};

#[derive(Debug, Clone)]
pub struct FcsParam {
    pub index: usize, // 1-based Pn
    pub name: String,     // $PnN
    pub stain: String,    // $PnS
    pub range: Option<f64>, // $PnR
    pub gain: Qty,        // $PnG 原始表示 + 规范化值（无法转换时 None）
    pub amp: String,      // $PnE
    pub bits: i64,        // $PnB
}

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub text_pairs: Vec<(String, String)>,
    pub params: Vec<FcsParam>,
    pub events: Vec<RawEvent>,
}

#[derive(thiserror::Error, Debug)]
pub enum FcsError {
    #[error("not an FCS file: {0}")]
    NotFcs(String),
    #[error("FCS parse error: {0}")]
    Parse(String),
}

fn read_text_pairs(bytes: &[u8]) -> Vec<(String, String)> {
    let delim = bytes.first().copied().unwrap_or(b'|') as char;
    let body = &bytes[1..];
    let parts: Vec<&[u8]> = if delim.is_ascii() {
        body.split(|&b| b == delim as u8).collect()
    } else {
        body.split(|&b| b == delim as u8).collect()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        let key = String::from_utf8_lossy(parts[i]).trim().to_string();
        let mut val = String::from_utf8_lossy(parts[i + 1]).to_string();
        // FCS 转义：分隔符重复两次表示字面量
        if !key.is_empty() {
            out.push((key, std::mem::take(&mut val)));
        }
        i += 2;
    }
    out
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

fn bytes_from_f32(b: [u8; 4], big: bool) -> f64 {
    if big {
        f32::from_be_bytes(b) as f64
    } else {
        f32::from_le_bytes(b) as f64
    }
}
fn bytes_from_f64(b: [u8; 8], big: bool) -> f64 {
    if big {
        f64::from_be_bytes(b)
    } else {
        f64::from_le_bytes(b)
    }
}

pub fn parse(file_id: &str, bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 || &bytes[0..4] != b"FCS3" {
        return Err(FcsError::NotFcs(format!(
            "bad magic: {:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(6)])
        )));
    }
    let hdr = |a: usize, b: usize| -> Result<i64, FcsError> {
        String::from_utf8_lossy(&bytes[a..b])
            .trim()
            .parse::<i64>()
            .map_err(|e| FcsError::Parse(format!("offset header {}-{}: {}", a, b, e)))
    };
    // 优先使用 TEXT/DATA 段头部偏移；$BEGIN/$END 作为补充。
    let text_b = hdr(10, 18)?;
    let text_e = hdr(18, 26)?;
    let data_b = hdr(26, 34)?;
    let data_e = hdr(34, 42)?;

    let text_bytes = &bytes[text_b as usize..=text_e as usize];
    let pairs = read_text_pairs(text_bytes);

    let n_events: usize = get(&pairs, "$TOT")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FcsError::Parse("missing $TOT".into()))?;
    let n_par: usize = get(&pairs, "$PAR")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| FcsError::Parse("missing $PAR".into()))?;
    let dtype = get(&pairs, "$DATATYPE").unwrap_or("F");
    let byteord = get(&pairs, "$BYTEORD").unwrap_or("1,2,3,4");
    let big = byteord.starts_with('4');

    let mut params = Vec::with_capacity(n_par);
    for p in 1..=n_par {
        let name = get(&pairs, &format!("$P{}N", p)).unwrap_or("").to_string();
        let stain = get(&pairs, &format!("$P{}S", p)).unwrap_or("").to_string();
        let range = get(&pairs, &format!("$P{}R", p)).and_then(|s| s.parse().ok());
        let bits: i64 = get(&pairs, &format!("$P{}B", p)).and_then(|s| s.parse().ok()).unwrap_or(32);
        let amp = get(&pairs, &format!("$P{}E", p)).unwrap_or("0,0").to_string();
        let gain_raw = get(&pairs, &format!("$P{}G", p)).map(|s| s.to_string());
        let gain = match &gain_raw {
            Some(raw) => match raw.parse::<f64>() {
                Ok(v) => Qty::new(v, "fraction", format!("$P{}G amplifier gain", p)),
                Err(_) => Qty::unconvertible(raw, "fraction", format!("$P{}G amplifier gain", p)),
            },
            None => Qty::new(1.0, "fraction", "default linear gain 1.0"),
        };
        params.push(FcsParam { index: p, name, stain, range, gain, amp, bits });
    }

    let begin = get(&pairs, "$BEGINDATA").and_then(|s| s.parse::<i64>().ok()).unwrap_or(data_b) as usize;
    let end = get(&pairs, "$ENDDATA").and_then(|s| s.parse::<i64>().ok()).unwrap_or(data_e) as usize;
    let data = &bytes[begin..=end];

    let mut events = Vec::with_capacity(n_events);
    match dtype {
        "F" => {
            let stride = n_par * 4;
            for row in 0..n_events {
                let off = row * stride;
                if off + stride > data.len() {
                    return Err(FcsError::Parse(format!("truncated DATA at event {}", row)));
                }
                let mut vals = Vec::with_capacity(n_par);
                let mut raws = Vec::with_capacity(n_par);
                let mut diags = Vec::new();
                for (c, par) in params.iter().enumerate() {
                    let chunk: [u8; 4] = data[off + c * 4..off + c * 4 + 4].try_into().unwrap();
                    let raw_hex = format!("{:02X?}", chunk).replace(['[', ']', ' '], "");
                    let v = bytes_from_f32(chunk, big);
                    if !v.is_finite() {
                        diags.push(Diag {
                            severity: "error",
                            code: "event.nonfinite",
                            message: format!("{} 通道 {} 为非有限值，保留原始字节", par.name, par.index),
                        });
                    }
                    if let Some(range) = par.range {
                        if v.is_finite() && v > range {
                            diags.push(Diag {
                                severity: "warn",
                                code: "event.exceeds_range",
                                message: format!(
                                    "通道 {} 值 {} 超过 $P{}R={}，原始值保留，绘图需标注",
                                    par.name, v, par.index, range
                                ),
                            });
                        }
                    }
                    vals.push(v);
                    raws.push(raw_hex);
                }
                events.push(RawEvent {
                    event_id: format!("event:{}:{}", file_id, row),
                    file_id: file_id.to_string(),
                    row,
                    values: vals,
                    raw_hex: raws,
                    diagnostics: diags,
                });
            }
        }
        "D" => {
            let stride = n_par * 8;
            for row in 0..n_events {
                let off = row * stride;
                let mut vals = Vec::with_capacity(n_par);
                let mut raws = Vec::with_capacity(n_par);
                for c in 0..n_par {
                    let chunk: [u8; 8] = data[off + c * 8..off + c * 8 + 8].try_into().unwrap();
                    raws.push(format!("{:02X?}", chunk).replace(['[', ']', ' '], ""));
                    vals.push(bytes_from_f64(chunk, big));
                }
                events.push(RawEvent {
                    event_id: format!("event:{}:{}", file_id, row),
                    row,
                    values: vals,
                    raw_hex: raws,
                    diagnostics: Vec::new(),
                });
            }
        }
        other => return Err(FcsError::Parse(format!("unsupported $DATATYPE {}", other))),
    }

    Ok(FcsFile { text_pairs: pairs, params, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fcs(floats: &[f32], par: usize, tot: usize, extra: &str) -> Vec<u8> {
        let mut text = format!("|$BEGINANALYSIS|0|$ENDANALYSIS|0|$DATATYPE|F|$MODE|L|$PAR|{}|$TOT|{}|$BYTEORD|1,2,3,4|$P1N|FSC-A|$P1R|262144|$P1B|32|$P1E|0,0|$P2N|FL1-A|$P2R|262144|$P2B|32|$P2E|0,0{}", par, tot, extra);
        let delim = b'|';
        let _ = delim;
        if text.len() % 2 == 0 {
            text.push(' ');
        }
        let mut data = Vec::new();
        for f in floats {
            data.extend_from_slice(&f.to_le_bytes());
        }
        let tb = 258usize;
        let te = tb + text.len() - 1;
        let db = te + 1;
        let de = db + data.len() - 1;
        let mut out = vec![b' '; tb];
        out[0..6].copy_from_slice(b"FCS3.1");
        let s = format!("{:>8}{:>8}{:>8}{:>8}0", tb, te, db, de);
        out[10..42].copy_from_slice(s.as_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn parses_roundtrip_and_flags_range() {
        let bytes = make_fcs(&[100.0, 200.0, 300.0, 400000.0], 2, 2, "|$P2G|2.5");
        let f = parse("file:fcs:t", &bytes).unwrap();
        assert_eq!(f.events.len(), 2);
        assert_eq!(f.events[0].values, vec![100.0, 200.0]);
        assert_eq!(f.events[1].values, vec![300.0, 400000.0]);
        assert!(f.events[1].diagnostics.iter().any(|d| d.code == "event.exceeds_range"));
        assert_eq!(f.params[1].gain.value, Some(2.5));
        assert_eq!(f.params[0].gain.value, Some(1.0));
        // 原始字节凭据保留
        assert_eq!(f.events[0].raw_hex.len(), 2);
        assert_eq!(f.events[0].raw_hex[0], "0000C842");
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(parse("x", b"not an fcs file at all").is_err());
    }
}
