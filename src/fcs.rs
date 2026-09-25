//! 最小 FCS 3.1 读写器：只解析本工具需要的 TEXT/DATA 段。
//! 导入失败向上传递错误，由导入层整体回滚，绝不留下半份记录。

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcsChannel {
    /// $PnN，短名，例如 FITC-A
    pub name: String,
    /// $PnS，长标记，可能缺失
    pub marker: Option<String>,
    /// $PnR，量程上界（原始表示字符串保留）
    pub range: String,
    /// $PnG，增益（原始表示字符串保留）
    pub gain: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FcsFile {
    pub version: String,
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<FcsChannel>,
    pub total_events: usize,
    /// 每个事件、每个通道的原始词法表示（导入时逐字保留）。
    pub events: Vec<Vec<String>>,
}

impl FcsFile {
    pub fn keyword(&self, key: &str) -> Option<&str> {
        self.keywords.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile> {
    if bytes.len() < 58 {
        return Err(Error::Fcs("header shorter than 58 bytes".into()));
    }
    let version = String::from_utf8_lossy(&bytes[0..6]).trim().to_string();
    if !version.starts_with("FCS3.") {
        return Err(Error::Fcs(format!("unsupported FCS version: {version}")));
    }
    let num = |start: usize, len: usize| -> Result<usize> {
        String::from_utf8_lossy(&bytes[start..start + len])
            .trim()
            .parse::<usize>()
            .map_err(|_| Error::Fcs(format!("bad header offset at {start}")))
    };
    let text_start = num(10, 8)?;
    let text_end = num(18, 8)?;
    let data_start = num(26, 8)?;
    let data_end = num(34, 8)?;
    if text_end <= text_start || text_end > bytes.len() {
        return Err(Error::Fcs("bad TEXT segment offsets".into()));
    }

    let text = &bytes[text_start..text_end];
    let delimiter = text[0] as char;
    let body = String::from_utf8_lossy(&text[1..]).to_string();
    let mut keywords: Vec<(String, String)> = Vec::new();
    for part in body.split(delimiter) {
        if part.is_empty() {
            continue;
        }
        let kv: Vec<&str> = part.splitn(2, '=').collect();
        if kv.len() == 2 {
            keywords.push((unescape(kv[0]), unescape(kv[1])));
        }
    }

    let par: usize = require_kw(&keywords, "$PAR")?;
    let tot: usize = require_kw(&keywords, "$TOT")?;
    let datatype = require_kw(&keywords, "$DATATYPE")?;
    let byteord = keywords.iter().find(|(k, _)| k == "$BYTEORD").map(|(_, v)| v.clone());
    let mode = keywords.iter().find(|(k, _)| k == "$MODE").map(|(_, v)| v.clone());
    if mode.as_deref() != Some("L") {
        return Err(Error::Fcs("only list-mode ($MODE=L) files supported".into()));
    }

    let mut channels = Vec::with_capacity(par);
    for i in 1..=par {
        let name = require_kw(&keywords, &format!("$P{i}N"))?;
        channels.push(FcsChannel {
            name,
            marker: kw(&keywords, &format!("$P{i}S")),
            range: require_kw(&keywords, &format!("$P{i}R"))?,
            gain: kw(&keywords, &format!("$P{i}G")),
        });
    }

    if data_end <= data_start || data_end > bytes.len() {
        return Err(Error::Fcs("bad DATA segment offsets".into()));
    }
    let data = &bytes[data_start..data_end];
    let mut events: Vec<Vec<String>> = Vec::with_capacity(tot);
    match datatype.as_str() {
        "F" => {
            if data.len() < tot * par * 4 {
                return Err(Error::Fcs("DATA shorter than $TOT*$PAR*4".into()));
            }
            for e in 0..tot {
                let mut row = Vec::with_capacity(par);
                for c in 0..par {
                    let off = (e * par + c) * 4;
                    let word = &data[off..off + 4];
                    let arr: [u8; 4] = word.try_into().unwrap();
                    let v = match byteord.as_deref() {
                        Some("2,1") | Some("2,1,3,4") => f32::from_le_bytes(arr),
                        Some("1,2") | Some("1,2,3,4") => f32::from_be_bytes(arr),
                        _ => f32::from_le_bytes(arr),
                    };
                    row.push(format_fcs_float(v));
                }
                events.push(row);
            }
        }
        "I" => {
            let p1b: usize = require_kw(&keywords, "$P1B")?;
            let width = p1b / 8;
            if !matches!(width, 1 | 2 | 4) || data.len() < tot * par * width {
                return Err(Error::Fcs("unsupported integer width or short DATA".into()));
            }
            let little = !matches!(byteord.as_deref(), Some("1,2") | Some("1,2,3,4"));
            for e in 0..tot {
                let mut row = Vec::with_capacity(par);
                for c in 0..par {
                    let off = (e * par + c) * width;
                    let val: u64 = match width {
                        1 => data[off] as u64,
                        2 => {
                            let mut a = [0u8; 2];
                            a.copy_from_slice(&data[off..off + 2]);
                            if little { u16::from_le_bytes(a) as u64 } else { u16::from_be_bytes(a) as u64 }
                        }
                        4 => {
                            let mut a = [0u8; 4];
                            a.copy_from_slice(&data[off..off + 4]);
                            if little { u32::from_le_bytes(a) as u64 } else { u32::from_be_bytes(a) as u64 }
                        }
                        _ => unreachable!(),
                    };
                    row.push(val.to_string());
                }
                events.push(row);
            }
        }
        other => return Err(Error::Fcs(format!("unsupported $DATATYPE: {other}"))),
    }

    Ok(FcsFile { version, keywords, channels, total_events: tot, events })
}

fn kw(kws: &[(String, String)], key: &str) -> Option<String> {
    kws.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}
fn require_kw(kws: &[(String, String)], key: &str) -> Result<String> {
    kw(kws, key).ok_or_else(|| Error::Fcs(format!("missing required keyword {key}")))
}
fn unescape(s: &str) -> String {
    s.replace("^^", "^").replace("^@", "\u{0}")
}

fn format_fcs_float(v: f32) -> String {
    if v.is_nan() {
        "NaN".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }
    } else {
        let s = format!("{v}");
        s
    }
}

/// 生成 FCS 3.1 浮点（$DATATYPE=F, $BYTEORD=1,2,3,4）文件，供离线案例包使用。
pub fn write_fcs_float(
    channels: &[(String, Option<String>)],
    ranges: &[u32],
    events: &[Vec<f32>],
) -> Vec<u8> {
    let par = channels.len();
    let tot = events.len();
    let mut kws: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTUDY".into(), "0".into()),
        ("$ENDSTUDY".into(), "0".into()),
        ("$MODE".into(), "L".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$NEXTDATA".into(), "0".into()),
        ("$PAR".into(), par.to_string()),
        ("$TOT".into(), tot.to_string()),
    ];
    for (i, (name, marker)) in channels.iter().enumerate() {
        let n = i + 1;
        kws.push((format!("$P{n}N"), name.clone()));
        if let Some(m) = marker {
            kws.push((format!("$P{n}S"), m.clone()));
        }
        kws.push((format!("$P{n}R"), ranges[i].to_string()));
        kws.push((format!("$P{n}B"), "32".into()));
        kws.push((format!("$P{n}E"), "0,0".into()));
    }
    let delim = '|';
    let mut text = String::new();
    text.push(delim);
    for (k, v) in &kws {
        text.push_str(k);
        text.push('=');
        text.push_str(&v.replace(delimiter, "^|"));
        text.push(delim);
    }
    let text_bytes = text.as_bytes();
    let header_len = 58usize;
    let text_start = header_len;
    let text_end = text_start + text_bytes.len();
    let data_start = text_end;
    let data_end = data_start + tot * par * 4;
    let mut out = Vec::with_capacity(data_end);
    let mut header = String::from("FCS3.1");
    while header.len() < 10 {
        header.push(' ');
    }
    let off = |s: usize, e: usize| -> String {
        format!("{:>width$}{:>width$}", s, e, width = 8)
    };
    header.push_str(&off(text_start, text_end));
    header.push_str(&off(data_start, data_end));
    header.push_str(&off(0, 0));
    while header.len() < 58 {
        header.push(' ');
    }
    debug_assert_eq!(header.len(), 58);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(text_bytes);
    for row in events {
        for v in row {
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    debug_assert_eq!(out.len(), data_end);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fcs_float_roundtrip() {
        let channels = vec![
            ("FITC-A".to_string(), Some("CD3".to_string())),
            ("PE-A".to_string(), None),
        ];
        let events = vec![vec![1.5f32, 2.5], vec![3.0, 4.0]];
        let bytes = write_fcs_float(&channels, &[262144, 262144], &events);
        let f = parse_fcs(&bytes).unwrap();
        assert_eq!(f.channels.len(), 2);
        assert_eq!(f.channels[0].name, "FITC-A");
        assert_eq!(f.channels[0].marker.as_deref(), Some("CD3"));
        assert_eq!(f.total_events, 2);
        assert_eq!(f.events[0], vec!["1.5".to_string(), "2.5".to_string()]);
    }

    #[test]
    fn bad_header_is_rejected_not_cleaned() {
        let err = parse_fcs(b"not an fcs file at all, padding padding").unwrap_err();
        assert!(matches!(err, Error::Fcs(_)));
    }
}
