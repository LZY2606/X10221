//! Minimal, non-cleaning FCS 3.1 reader.
//!
//! Raw byte representations are preserved.  Values that cannot be converted
//! (NaN / infinity) stay in place and carry a diagnostic instead of being
//! dropped or clipped so plots can render everything.

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Channel {
    pub index: usize,
    pub name: String,
    pub detector: Option<String>,
    pub fluorophore: Option<String>,
    pub range: Option<f64>,
    pub unit: Option<String>,
    pub basis: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind")]
pub enum RawValue {
    /// Parseable finite value, stored both as f64 and the original token text.
    Finite { value: f64 },
    /// NaN bit pattern: kept in place, never rendered as zero.
    NaN,
    /// +/- infinity.
    Infinite { negative: bool },
}

impl RawValue {
    pub fn finite(value: f64) -> Self {
        if value.is_nan() {
            RawValue::NaN
        } else if value.is_infinite() {
            RawValue::Infinite { negative: value < 0.0 }
        } else {
            RawValue::Finite { value }
        }
    }

    pub fn as_finite(&self) -> Option<f64> {
        match self {
            RawValue::Finite { value } => Some(*value),
            _ => None,
        }
    }

    pub fn is_unconvertible(&self) -> bool {
        matches!(self, RawValue::NaN | RawValue::Infinite { .. })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub event_index: usize,
    pub channel: String,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FcsFile {
    pub source_identifier: Option<String>,
    pub total: usize,
    pub channels: Vec<Channel>,
    /// rows[event_index][channel_index]
    pub rows: Vec<Vec<RawValue>>,
    pub diagnostics: Vec<Diagnostic>,
    pub keywords: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum FcsError {
    TooShort,
    BadHeader(String),
    Unsupported(String),
    Parse(String),
}

impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcsError::TooShort => write!(f, "file shorter than FCS header"),
            FcsError::BadHeader(m) => write!(f, "bad FCS header: {m}"),
            FcsError::Unsupported(m) => write!(f, "unsupported FCS feature: {m}"),
            FcsError::Parse(m) => write!(f, "FCS parse error: {m}"),
        }
    }
}

impl std::error::Error for FcsError {}

fn parse_off(s: &str, field: &str) -> Result<usize, FcsError> {
    let t = s.trim();
    if t == "0" {
        return Ok(0);
    }
    t.parse::<usize>()
        .map_err(|_| FcsError::BadHeader(format!("offset {field}")))
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 {
        return Err(FcsError::TooShort);
    }
    if &bytes[0..6] != b"FCS3.0" && &bytes[0..6] != b"FCS3.1" {
        return Err(FcsError::BadHeader("magic".into()));
    }
    let text_start = parse_off(std::str::from_utf8(&bytes[10..18]).unwrap_or(""), "BEGINTEXT")?;
    let text_end = parse_off(std::str::from_utf8(&bytes[18..26]).unwrap_or(""), "ENDTEXT")?;
    let mut data_start =
        parse_off(std::str::from_utf8(&bytes[26..34]).unwrap_or(""), "BEGINDATA")?;
    let mut data_end =
        parse_off(std::str::from_utf8(&bytes[34..42]).unwrap_or(""), "ENDDATA")?;
    if text_start == 0 || text_end <= text_start {
        return Err(FcsError::BadHeader("text offsets".into()));
    }

    let delim = bytes[text_start] as char;
    let text = std::str::from_utf8(&bytes[text_start + 1..=text_end - 1])
        .map_err(|_| FcsError::Parse("text segment not ASCII/UTF-8".into()))?;
    let mut kv: Vec<&str> = text.split(delim).collect();
    if kv.last() == Some(&"") {
        kv.pop();
    }
    let mut keywords = BTreeMap::new();
    let mut pairs = kv.into_iter();
    while let Some(k) = pairs.next() {
        if let Some(v) = pairs.next() {
            keywords.insert(k.to_string(), v.to_string());
        }
    }

    // Offsets in TEXT take precedence when header overflows.
    if let Some(v) = keywords.get("$BEGINDATA") {
        if let Ok(n) = v.trim().parse::<usize>() {
            data_start = n;
        }
    }
    if let Some(v) = keywords.get("$ENDDATA") {
        if let Ok(n) = v.trim().parse::<usize>() {
            data_end = n;
        }
    }
    if data_end < data_start {
        return Err(FcsError::BadHeader("data offsets".into()));
    }

    let total: usize = keywords
        .get("$TOT")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| FcsError::Parse("$TOT missing".into()))?;
    let npar: usize = keywords
        .get("$PAR")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| FcsError::Parse("$PAR missing".into()))?;
    let datatype = keywords
        .get("$DATATYPE")
        .map(|s| s.trim())
        .ok_or_else(|| FcsError::Parse("$DATATYPE missing".into()))?;
    let byteord = keywords
        .get("$BYTEORD")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "1,2,3,4".into());

    let mut channels = Vec::with_capacity(npar);
    for i in 1..=npar {
        let name = keywords
            .get(&format!("$P{i}N"))
            .cloned()
            .unwrap_or_else(|| format!("P{i}"));
        channels.push(Channel {
            index: i - 1,
            name,
            detector: keywords.get(&format!("$P{i}S")).cloned(),
            fluorophore: keywords.get(&format!("$P{i}FLUOR")).cloned(),
            range: keywords
                .get(&format!("$P{i}R"))
                .and_then(|v| v.trim().parse().ok()),
            unit: keywords
                .get(&format!("FLOWLINE/$P{i}UNIT"))
                .cloned(),
            basis: keywords
                .get(&format!("FLOWLINE/$P{i}BASIS"))
                .cloned(),
        });
    }

    let data = &bytes[data_start..=data_end];
    let mut rows: Vec<Vec<RawValue>> = Vec::with_capacity(total);
    let mut diagnostics = Vec::new();

    match datatype {
        "F" => {
            let expected = total * npar * 4;
            if data.len() < expected {
                return Err(FcsError::Parse(format!(
                    "data segment {actual} < expected {expected}",
                    actual = data.len()
                )));
            }
            for ev in 0..total {
                let mut row = Vec::with_capacity(npar);
                for ch in 0..npar {
                    let o = (ev * npar + ch) * 4;
                    let bits = match byteord.as_str() {
                        "4,3,2,1" => u32::from_be_bytes([
                            data[o],
                            data[o + 1],
                            data[o + 2],
                            data[o + 3],
                        ]),
                        "1,2,3,4" => u32::from_le_bytes([
                            data[o],
                            data[o + 1],
                            data[o + 2],
                            data[o + 3],
                        ]),
                        other => {
                            return Err(FcsError::Unsupported(format!(
                                "$BYTEORD {other}"
                            )))
                        }
                    };
                    let v = f32::from_bits(bits) as f64;
                    let val = RawValue::finite(v);
                    if val.is_unconvertible() {
                        diagnostics.push(Diagnostic {
                            event_index: ev,
                            channel: channels[ch].name.clone(),
                            code: if matches!(val, RawValue::NaN) {
                                "VALUE_NAN".into()
                            } else {
                                "VALUE_INFINITE".into()
                            },
                            detail: "value preserved; normalized layer skips this channel"
                                .into(),
                        });
                    }
                    row.push(val);
                }
                rows.push(row);
            }
        }
        other => return Err(FcsError::Unsupported(format!("$DATATYPE {other}"))),
    }

    Ok(FcsFile {
        source_identifier: keywords.get("$SRC").cloned(),
        total,
        channels,
        rows,
        diagnostics,
        keywords,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_committed_case_pack() {
        let b = include_bytes!("../case_pack/events.fcs");
        let f = parse_fcs(b).expect("parse");
        assert_eq!(f.total, 24);
        assert_eq!(f.channels.len(), 9);
        assert_eq!(f.channels[2].name, "FITC-A");
        assert_eq!(f.channels[8].unit.as_deref(), Some("ms"));
        // saturated event
        match f.rows[21][2] {
            RawValue::Finite { value } => assert_eq!(value, 262143.0),
            _ => panic!("expected finite saturation value"),
        }
        // NaN event retained in place
        assert!(matches!(f.rows[23][3], RawValue::NaN));
        assert!(f.diagnostics.iter().any(|d| d.code == "VALUE_NAN"
            && d.event_index == 23 && d.channel == "PE-A"));
    }
}
