//! Minimal FCS 3.1 reader used by the boundary-chamber case pack.
//!
//! The parser keeps the *raw* representation of every event exactly as the
//! platform encoded it (including out-of-range / NaN sentinels) and records
//! diagnostics instead of dropping values. Normalisation happens only in the
//! derived layer.
use crate::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelMeta>,
    pub events: Vec<Vec<f64>>,
    pub byte_order_big_endian: bool,
    pub data_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelMeta {
    pub index: usize,
    pub name: String,
    pub detector: Option<String>,
    pub fluor: Option<String>,
    pub unit: String,
    pub gain: Option<f64>,
    pub voltage: Option<f64>,
    pub range_min: f64,
    pub range_max: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParseDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
}

fn keyword<'a>(keywords: &'a [(String, String)], key: &str) -> Option<&'a str> {
    keywords.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Parse the TEXT segment of an FCS file honoring the per-file delimiter.
fn parse_text(segment: &[u8], delimiter: u8) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(segment);
    let parts: Vec<&str> = text.split(delimiter as char).collect();
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < parts.len() {
        let key = parts[i].trim();
        let value = parts[i + 1].trim();
        if !key.is_empty() {
            out.push((key.to_string(), value.to_string()));
        }
        i += 2;
    }
    out
}

fn read_f64(bytes: &[u8], big: bool) -> f64 {
    let mut arr = [0u8; 8];
    arr.copy_from_slice(bytes);
    if big {
        f64::from_be_bytes(arr)
    } else {
        f64::from_le_bytes(arr)
    }
}

fn read_f32(bytes: &[u8], big: bool) -> f64 {
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    let v = if big {
        f32::from_be_bytes(arr)
    } else {
        f32::from_le_bytes(arr)
    };
    v as f64
}

pub fn parse_fcs(bytes: &[u8]) -> AppResult<(FcsFile, Vec<ParseDiagnostic>)> {
    let mut diagnostics = Vec::new();
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(AppError::Validation(
            "not an FCS 3.1 file (missing FCS3.1 header)".into(),
        ));
    }
    let at = |start: usize, end: usize| bytes[start..end].iter().any(|b| *b != b' ');
    let text_start: usize = std::str::from_utf8(&bytes[10..18])
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);
    let text_end: usize = std::str::from_utf8(&bytes[18..26])
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);
    let data_start: usize = std::str::from_utf8(&bytes[26..34])
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);
    let data_end: usize = std::str::from_utf8(&bytes[34..42])
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);

    if text_start == 0 || text_end == 0 || !at(10, 26) {
        return Err(AppError::Validation("FCS header lacks TEXT offsets".into()));
    }
    let delimiter = bytes[text_start];
    let keywords = parse_text(&bytes[text_start..=text_end], delimiter);

    let byte_order = keyword(&keywords, "$BYTEORD").unwrap_or("1,2,3,4");
    let big = byte_order.starts_with('4');
    let datatype = keyword(&keywords, "$DATATYPE").unwrap_or("F").to_string();
    let par: usize = keyword(&keywords, "$PAR")
        .ok_or_else(|| AppError::Validation("missing $PAR".into()))?
        .parse()
        .map_err(|_| AppError::Validation("$PAR not numeric".into()))?;
    let n_events: usize = keyword(&keywords, "$TOT")
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);

    let mut channels = Vec::new();
    for index in 1..=par {
        let name = keyword(&keywords, &format!("$P{index}N")).unwrap_or("UNKNOWN").to_string();
        let detector = keyword(&keywords, &format!("$P{index}S")).map(|s| s.to_string());
        let fluor = keyword(&keywords, &format!("$P{index}F")).map(|s| s.to_string());
        let unit = keyword(&keywords, &format!("$P{index}U")).unwrap_or("channel").to_string();
        let gain = keyword(&keywords, &format!("$P{index}G")).and_then(|s| s.parse().ok());
        let voltage = keyword(&keywords, &format!("$P{index}V")).and_then(|s| s.parse().ok());
        let range: Vec<f64> = keyword(&keywords, &format!("$P{index}R"))
            .unwrap_or("1024")
            .split(',')
            .map(|s| s.trim().parse().unwrap_or(1024.0))
            .collect();
        channels.push(ChannelMeta {
            index: index - 1,
            name,
            detector,
            fluor,
            unit,
            gain,
            voltage,
            range_min: 0.0,
            range_max: range[0],
        });
    }

    let word_bytes = match datatype.as_str() {
        "F" => 4,
        "D" => 8,
        other => {
            return Err(AppError::Validation(format!(
                "unsupported $DATATYPE `{other}` (boundary chamber uses F/D)"
            )))
        }
    };

    let mut events: Vec<Vec<f64>> = Vec::new();
    if n_events > 0 && data_end > data_start {
        let stride = par * word_bytes;
        for event_idx in 0..n_events {
            let base = data_start + event_idx * stride;
            if base + stride > bytes.len() {
                diagnostics.push(ParseDiagnostic {
                    code: "FCS_TRUNCATED_EVENT".into(),
                    severity: "error".into(),
                    message: format!("event {event_idx} extends past file end; row kept with nulls"),
                });
                events.push(vec![f64::NAN; par]);
                continue;
            }
            let mut row = Vec::with_capacity(par);
            for ch in 0..par {
                let off = base + ch * word_bytes;
                let value = if word_bytes == 8 {
                    read_f64(&bytes[off..off + 8], big)
                } else {
                    read_f32(&bytes[off..off + 4], big)
                };
                if value.is_nan() {
                    diagnostics.push(ParseDiagnostic {
                        code: "FCS_RAW_NAN".into(),
                        severity: "warning".into(),
                        message: format!("event {event_idx} channel `{}` is NaN; retained in place", channels[ch].name),
                    });
                }
                row.push(value);
            }
            events.push(row);
        }
    }

    // Spillover keyword presence is a domain requirement, not a parser failure.
    if keyword(&keywords, "$SPILLOVER").is_none()
        && keyword(&keywords, "SPILL").is_none()
        && keyword(&keywords, "$SPILLOVER2").is_none()
    {
        diagnostics.push(ParseDiagnostic {
            code: "FCS_NO_SPILLOVER_KEYWORD".into(),
            severity: "info".into(),
            message: "no $SPILLOVER / SPILL keyword in file; spillover supplied separately".into(),
        });
    }

    Ok((
        FcsFile {
            keywords,
            channels,
            events,
            byte_order_big_endian: big,
            data_type: datatype,
            bytes: bytes.to_vec(),
        },
        diagnostics,
    ))
}

/// Serialise the parsed file as the canonical `raw_events` artifact content.
/// NaN is emitted as JSON null so the value stays *in place* but is never
/// silently coerced to zero.
pub fn raw_content(file: &FcsFile) -> Value {
    let mut map = Map::new();
    map.insert("format".into(), json("FCS3.1"));
    map.insert("byte_order".into(), json(if file.byte_order_big_endian { "big-endian" } else { "little-endian" }));
    map.insert("data_type".into(), json(file.data_type));
    map.insert(
        "coordinate_basis".into(),
        json("raw instrument channel numbers, linear ADC scale"),
    );
    map.insert(
        "channels".into(),
        Value::Array(
            file.channels
                .iter()
                .map(|c| {
                    let mut m = serde_json::to_value(c).unwrap();
                    m.as_object_mut().unwrap().insert(
                        "raw_representation".into(),
                        json(format!("$P{}N + $P{}U keywords preserved verbatim", c.index + 1, c.index + 1)),
                    );
                    m
                })
                .collect(),
        ),
    );
    map.insert(
        "events".into(),
        Value::Array(
            file.events
                .iter()
                .map(|row| {
                    Value::Array(
                        row.iter()
                            .map(|v| if v.is_nan() { Value::Null } else => Value::from(*v) })
                            .collect(),
                    )
                })
                .collect(),
        ),
    );
    map.insert(
        "keywords".into(),
        Value::Array(
            file.keywords
                .iter()
                .map(|(k, v)| json(format!("{k}={v}")))
                .collect(),
        ),
    );
    Value::Object(map)
}

fn json(s: &str) -> Value {
    Value::String(s.to_string())
}
