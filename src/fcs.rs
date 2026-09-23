//! Minimal FCS 3.1 reader: header offsets, delimited TEXT segment, float DATA.
//!
//! Raw detector values are preserved; values that cannot be converted stay in
//! place and carry diagnostics rather than being dropped.

use crate::domain::{ChannelSpec, RawEvent, RawEvents};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct FcsDiagnostic {
    pub code: String,
    pub event_id: Option<String>,
    pub channel_id: Option<String>,
    pub message: String,
}

#[derive(Debug)]
pub enum FcsError {
    BadHeader(String),
    BadText(String),
    BadData(String),
}

impl fmt::Display for FcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FcsError::BadHeader(m) => write!(f, "FCS header error: {m}"),
            FcsError::BadText(m) => write!(f, "FCS TEXT error: {m}"),
            FcsError::BadData(m) => write!(f, "FCS DATA error: {m}"),
        }
    }
}
impl std::error::Error for FcsError {}

pub struct ParsedFcs {
    pub keywords: BTreeMap<String, String>,
    pub raw: RawEvents,
    pub diagnostics: Vec<FcsDiagnostic>,
}

fn read_u32_be(buf: &[u8], at: usize) -> Result<usize, FcsError> {
    if at + 4 > buf.len() {
        return Err(FcsError::BadData("offset beyond file".into()));
    }
    Ok(u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize)
}

pub fn parse_fcs(bytes: &[u8]) -> Result<ParsedFcs, FcsError> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(FcsError::BadHeader("not an FCS 3.1 file".into()));
    }
    let data_start = read_u32_be(bytes, 26)?;
    let data_end = read_u32_be(bytes, 34)?;

    let delim = bytes[58];
    if delim == b' ' || delim == 0 {
        return Err(FcsError::BadText("invalid primary delimiter".into()));
    }
    let text_end = data_start;
    let text = &bytes[58..text_end];
    let parts: Vec<&[u8]> = text.split(|b| *b == delim).collect();
    let mut keywords = BTreeMap::new();
    let mut pair = parts.iter().skip(1);
    while let (Some(k), Some(v)) = (pair.next(), pair.next()) {
        let key = String::from_utf8_lossy(k).trim().to_string();
        let val = String::from_utf8_lossy(v).trim().to_string();
        if !key.is_empty() {
            keywords.insert(key, val);
        }
    }

    let npar: usize = keywords
        .get("$PAR")
        .ok_or_else(|| FcsError::BadText("missing $PAR".into()))?
        .parse()
        .map_err(|_| FcsError::BadText("bad $PAR".into()))?;
    let tot: usize = keywords
        .get("$TOT")
        .ok_or_else(|| FcsError::BadText("missing $TOT".into()))?
        .parse()
        .map_err(|_| FcsError::BadText("bad $TOT".into()))?;
    let byteord = keywords
        .get("$BYTEORD")
        .ok_or_else(|| FcsError::BadText("missing $BYTEORD".into()))?;
    let big_endian = byteord.starts_with("4,3,2,1");
    let datatype = keywords
        .get("$DATATYPE")
        .ok_or_else(|| FcsError::BadText("missing $DATATYPE".into()))?;
    if datatype != "F" {
        return Err(FcsError::BadText(format!("unsupported $DATATYPE {datatype}")));
    }

    let mut channels = Vec::new();
    for i in 1..=npar {
        let short_name = keywords
            .get(&format!("$P{i}N"))
            .cloned()
            .unwrap_or_else(|| format!("P{i}"));
        let channel_id = keywords
            .get(&format!("$P{i}N_LONG"))
            .cloned()
            .unwrap_or_else(|| short_name.clone());
        let range: f64 = keywords
            .get(&format!("$P{i}R"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(f64::NAN);
        channels.push(ChannelSpec {
            channel_id,
            short_name,
            range,
            unit: "a.u.".into(),
            basis: "raw detector signal".into(),
        });
    }

    let need = tot * npar * 4;
    if data_end < data_start || data_end + 1 > bytes.len() || data_end + 1 - data_start < need {
        return Err(FcsError::BadData(format!(
            "DATA segment truncated: need {need} bytes"
        )));
    }
    let data = &bytes[data_start..=data_end];
    let mut events = Vec::with_capacity(tot);
    let mut diagnostics = Vec::new();
    for e in 0..tot {
        let mut values = BTreeMap::new();
        for (ci, ch) in channels.iter().enumerate() {
            let at = (e * npar + ci) * 4;
            let word = [data[at], data[at + 1], data[at + 2], data[at + 3]];
            let raw_bits = if big_endian {
                u32::from_be_bytes(word)
            } else {
                u32::from_le_bytes(word)
            };
            let value = f32::from_bits(raw_bits) as f64;
            if value.is_nan() {
                diagnostics.push(FcsDiagnostic {
                    code: "unconvertible_nan".into(),
                    event_id: Some(format!("ev_{e:03d}")),
                    channel_id: Some(ch.channel_id.clone()),
                    message: "raw float word is NaN; kept at original position".into(),
                });
            } else if !value.is_finite() {
                diagnostics.push(FcsDiagnostic {
                    code: "unconvertible_infinite".into(),
                    event_id: Some(format!("ev_{e:03d}")),
                    channel_id: Some(ch.channel_id.clone()),
                    message: "raw float word is infinite; kept at original position".into(),
                });
            }
            values.insert(ch.channel_id.clone(), value);
        }
        events.push(RawEvent {
            event_id: format!("ev_{e:03d}"),
            tag: String::new(),
            values,
        });
    }

    let source_file = keywords.get("$FIL").cloned().unwrap_or_else(|| "memory.fcs".into());
    let raw = RawEvents {
        artifact_id: "art_events".into(),
        version: "v1".into(),
        source_file,
        representation: "float32 FCS 3.1 DATA segment, original byte order".into(),
        channels,
        events,
    };
    Ok(ParsedFcs { keywords, raw, diagnostics })
}

/// Parse the FCS `$SPILLOVER` keyword: n, names..., row-major n*n values.
pub fn parse_spillover_keyword(spec: &str) -> Option<(Vec<String>, Vec<Vec<f64>>)> {
    let fields: Vec<&str> = spec.split(',').map(|s| s.trim()).collect();
    if fields.is_empty() {
        return None;
    }
    let n: usize = fields[0].parse().ok()?;
    if fields.len() < 1 + n + n * n {
        return None;
    }
    let names = fields[1..=n].iter().map(|s| s.to_string()).collect();
    let mut rows = Vec::with_capacity(n);
    let mut k = 1 + n;
    for _ in 0..n {
        let mut row = Vec::with_capacity(n);
        for _ in 0..n {
            row.push(fields[k].parse().ok()?);
            k += 1;
        }
        rows.push(row);
    }
    Some((names, rows))
}
