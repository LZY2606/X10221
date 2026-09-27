//! Minimal, dependency-free FCS 3.1 reader/writer for the bundled case pack.
//!
//! Supports float32 little-endian event matrices with the TEXT keywords used
//! by flow cytometry platforms ($TOT, $PAR, $PnN, ...). Original numeric
//! representations are preserved: values are read as stored in the file.

use crate::types::{ChannelSpec, CoordinateBasis, RawEvent, RawEvents, ValueDiagnostic};
use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone)]
pub struct FcsData {
    pub text: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<RawEvent>,
}

fn keyword<'a>(text: &'a [(String, String)], key: &str) -> Option<&'a str> {
    text.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

pub fn parse_fcs(bytes: &[u8], file_name: &str, sha: &str) -> Result<RawEvents> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        bail!("not an FCS 3.1 file (bad header)");
    }
    let num = |start: usize, end: usize| -> Result<usize> {
        let s = std::str::from_utf8(&bytes[start..end])?.trim();
        Ok(s.parse::<usize>()?)
    };
    let text_start = num(10, 18)?;
    let text_end = num(18, 26)?;
    let data_start = num(26, 34)?;
    let data_end = num(34, 42)?;

    let text_bytes = &bytes[text_start..=text_end];
    let delimiter = text_bytes[0] as char;
    let text_str = std::str::from_utf8(&text_bytes[1..])?;
    let parts: Vec<&str> = text_str.split(delimiter).collect();
    let mut text: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        let key = parts[i].trim();
        let val = parts[i + 1];
        if !key.is_empty() {
            text.push((key.to_string(), val.to_string()));
        }
        i += 2;
    }

    let n_par: usize = keyword(&text, "$PAR")
        .ok_or_else(|| anyhow!("missing $PAR"))?
        .parse()?;
    let n_tot: usize = keyword(&text, "$TOT")
        .ok_or_else(|| anyhow!("missing $TOT"))?
        .parse()?;
    let datatype = keyword(&text, "$DATATYPE").ok_or_else(|| anyhow!("missing $DATATYPE"))?;
    let byteord = keyword(&text, "$BYTEORD").unwrap_or("1,2,3,4");

    let mut channels = Vec::with_capacity(n_par);
    for n in 1..=n_par {
        let name = keyword(&text, &format!("$P{}N", n))
            .unwrap_or("UNKNOWN")
            .to_string();
        let detector = keyword(&text, &format!("$P{}S", n))
            .unwrap_or(&name)
            .to_string();
        let range: f64 = keyword(&text, &format!("$P{}R", n))
            .and_then(|v| v.parse().ok())
            .unwrap_or(262144.0);
        channels.push(ChannelSpec {
            name,
            detector,
            fluorophore: None,
            unit: "channel-units".to_string(),
            coordinate_basis: CoordinateBasis {
                space: "raw-pulse-area".to_string(),
                unit: "channel-units".to_string(),
            },
            declared_min: 0.0,
            declared_max: range,
        });
    }

    if datatype != "F" {
        bail!("case pack only encodes $DATATYPE=F float32 events");
    }
    let little = byteord.starts_with("1,2");
    let mut events = Vec::with_capacity(n_tot);
    let chunk = &bytes[data_start..=data_end];
    if chunk.len() < n_tot * n_par * 4 {
        bail!("DATA segment too short");
    }
    for e in 0..n_tot {
        let mut values = Vec::with_capacity(n_par);
        let mut diagnostics = Vec::new();
        for c in 0..n_par {
            let off = (e * n_par + c) * 4;
            let raw: [u8; 4] = chunk[off..off + 4].try_into().unwrap();
            let bits = if little {
                u32::from_le_bytes(raw)
            } else {
                u32::from_be_bytes(raw)
            };
            let value = f32::from_bits(bits) as f64;
            let spec = &channels[c];
            if !value.is_finite() {
                diagnostics.push(ValueDiagnostic {
                    channel_index: c,
                    code: "NON_FINITE".to_string(),
                    message: format!("{} is not finite", spec.name),
                });
            } else if value < spec.declared_min {
                diagnostics.push(ValueDiagnostic {
                    channel_index: c,
                    code: "BELOW_DECLARED_RANGE".to_string(),
                    message: format!(
                        "{}={} below declared minimum {}",
                        spec.name, value, spec.declared_min
                    ),
                });
            } else if value > spec.declared_max {
                diagnostics.push(ValueDiagnostic {
                    channel_index: c,
                    code: "ABOVE_DECLARED_RANGE".to_string(),
                    message: format!(
                        "{}={} above declared maximum {}",
                        spec.name, value, spec.declared_max
                    ),
                });
            }
            values.push(value);
        }
        events.push(RawEvent {
            index: e as u32,
            values,
            diagnostics,
        });
    }

    Ok(RawEvents {
        fcs_file: file_name.to_string(),
        fcs_sha256: sha.to_string(),
        channels,
        events,
    })
}

/// Build a deterministic FCS 3.1 byte blob (float32, little-endian).
pub fn write_fcs(channel_names: &[&str], ranges: &[f64], rows: &[Vec<f64>]) -> Vec<u8> {
    let par = channel_names.len();
    let tot = rows.len();
    let mut pairs: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".to_string(), "0".to_string()),
        ("$ENDANALYSIS".to_string(), "0".to_string()),
        ("$BEGINSTEXT".to_string(), "0".to_string()),
        ("$ENDSTEXT".to_string(), "0".to_string()),
        ("$MODE".to_string(), "L".to_string()),
        ("$DATATYPE".to_string(), "F".to_string()),
        ("$BYTEORD".to_string(), "1,2,3,4".to_string()),
        ("$NEXTDATA".to_string(), "0".to_string()),
        ("$PAR".to_string(), par.to_string()),
        ("$TOT".to_string(), tot.to_string()),
    ];
    for (i, name) in channel_names.iter().enumerate() {
        let n = i + 1;
        pairs.push((format!("$P{}N", n), name.to_string()));
        pairs.push((format!("$P{}S", n), name.to_string()));
        pairs.push((format!("$P{}B", n), "32".to_string()));
        pairs.push((format!("$P{}E", n), "0,0".to_string()));
        pairs.push((format!("$P{}R", n), format!("{}", ranges[i] as u64)));
        pairs.push((format!("$P{}G", n), "1".to_string()));
    }
    let mut text = String::from('|');
    for (k, v) in &pairs {
        text.push_str(k);
        text.push('|');
        text.push_str(v);
        text.push('|');
    }
    // Header offsets are filled in once sizes are known; TEXT starts at 58.
    let text_start = 58usize;
    let text_bytes = text.as_bytes();
    let text_end = text_start + text_bytes.len() - 1;
    let data_start = text_end + 1;
    let data_len = tot * par * 4;
    let data_end = data_start + data_len - 1;

    let mut out = Vec::with_capacity(data_end + 1);
    out.extend_from_slice(b"FCS3.1");
    out.extend_from_slice(b"    ");
    let field = |v: usize| format!("{:>8}", v);
    out.extend_from_slice(field(text_start).as_bytes());
    out.extend_from_slice(field(text_end).as_bytes());
    out.extend_from_slice(field(data_start).as_bytes());
    out.extend_from_slice(field(data_end).as_bytes());
    out.extend_from_slice(b"       0       0");
    debug_assert_eq!(out.len(), 58);
    out.extend_from_slice(text_bytes);
    for row in rows {
        for value in row {
            out.extend_from_slice(&(*value as f32).to_le_bytes());
        }
    }
    out
}
