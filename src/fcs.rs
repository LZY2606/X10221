//! Minimal FCS 3.1 parser for the bundled offline case pack.
//!
//! Scope: TEXT + DATA segments, FLOAT32/FLOAT64/INT16/INT32 list-mode events,
//! big or little endian. Raw bytes and raw keyword representations are
//! preserved; values that cannot be converted stay out-of-band in
//! `diagnostics` and the event slot becomes NaN rather than being dropped.

use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct ParsedFcs {
    pub version: String,
    pub text_start: usize,
    pub text_end: usize,
    pub data_start: usize,
    pub data_end: usize,
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<String>,
    /// Raw events in channel order; NaN marks values that could not be decoded.
    pub events: Vec<Vec<f64>>,
    pub diagnostics: Vec<String>,
    pub sha256: String,
    pub byte_count: usize,
}

fn keyword<'a>(kws: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kws.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

fn header_num(bytes: &[u8], start: usize, end: usize) -> i64 {
    String::from_utf8_lossy(&bytes[start..=end]).trim().parse().unwrap_or(0)
}

pub fn parse_fcs(bytes: &[u8]) -> Result<ParsedFcs, String> {
    let sha256 = hex::encode(Sha256::digest(bytes));
    if bytes.len() < 58 {
        return Err("FCS file shorter than 58-byte header".to_string());
    }
    if &bytes[0..3] != b"FCS" {
        return Err("missing FCS magic".to_string());
    }
    let version = String::from_utf8_lossy(&bytes[3..6]).to_string();
    let text_start = header_num(bytes, 10, 17) as usize;
    let text_end = header_num(bytes, 18, 25) as usize;
    let data_start = header_num(bytes, 26, 33) as usize;
    let data_end = header_num(bytes, 34, 41) as usize;
    if text_start == 0 || text_end == 0 || text_end >= bytes.len() {
        return Err("invalid TEXT segment offsets".to_string());
    }

    let delim = bytes[text_start];
    let text_bytes = &bytes[text_start..=text_end];
    let parts: Vec<&[u8]> = text_bytes
        .split(|b| *b == delim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut keywords: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        keywords.push((
            String::from_utf8_lossy(parts[i]).to_string(),
            String::from_utf8_lossy(parts[i + 1]).to_string(),
        ));
        i += 2;
    }

    let mut diagnostics = Vec::new();
    let n_channels: usize = keyword(&keywords, "$PAR")
        .and_then(|v| v.parse().ok())
        .ok_or("missing $PAR")?;
    let n_events: usize = keyword(&keywords, "$TOT")
        .and_then(|v| v.parse().ok())
        .ok_or("missing $TOT")?;
    let datatype = keyword(&keywords, "$DATATYPE").unwrap_or("");
    let mode = keyword(&keywords, "$MODE").unwrap_or("L");
    if mode != "L" {
        diagnostics.push(format!("non-list mode '$MODE={mode}' retained verbatim; events not decoded"));
    }

    let mut channels = Vec::new();
    for c in 1..=n_channels {
        let name = keywords
            .iter()
            .find(|(k, _)| k == &format!("$P{c}N"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| format!("CH{c}"));
        channels.push(name);
    }

    let ds = keyword(&keywords, "$BEGINDATA").and_then(|v| v.parse::<usize>().ok()).unwrap_or(data_start);
    let de = keyword(&keywords, "$ENDDATA").and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.saturating_sub(1))
        .unwrap_or(data_end);
    let big_endian = keyword(&keywords, "$BYTEORD").map(|v| v.starts_with('4')).unwrap_or(false);
    let data = bytes.get(ds..=de).ok_or("DATA segment out of range")?;

    let mut events: Vec<Vec<f64>> = Vec::new();
    match datatype {
        "F" | "D" => {
            let width = if datatype == "F" { 4 } else { 8 };
            let need = n_events * n_channels * width;
            if data.len() < need {
                diagnostics.push(format!("DATA segment {} bytes shorter than expected {}", data.len(), need));
            }
            for e in 0..n_events {
                let mut row = Vec::with_capacity(n_channels);
                for c in 0..n_channels {
                    let off = (e * n_channels + c) * width;
                    row.push(match data.get(off..off + width) {
                        Some(ch) if width == 4 => {
                            let mut buf = [0u8; 4];
                            buf.copy_from_slice(ch);
                            if big_endian { f32::from_be_bytes(buf) as f64 } else { f32::from_le_bytes(buf) as f64 }
                        }
                        Some(ch) => {
                            let mut buf = [0u8; 8];
                            buf.copy_from_slice(ch);
                            if big_endian { f64::from_be_bytes(buf) } else { f64::from_le_bytes(buf) }
                        }
                        None => {
                            diagnostics.push(format!("event {e} channel {c}: truncated float; retained as null"));
                            f64::NAN
                        }
                    });
                }
                events.push(row);
            }
        }
        "I" => {
            let width: usize = keyword(&keywords, "$P1B")
                .and_then(|v| v.parse().ok())
                .unwrap_or(16) / 8;
            let width = width.clamp(1, 4);
            for e in 0..n_events {
                let mut row = Vec::with_capacity(n_channels);
                for c in 0..n_channels {
                    let off = (e * n_channels + c) * width;
                    row.push(match data.get(off..off + width) {
                        Some(ch) => {
                            let mut v: u64 = 0;
                            for (idx, &b) in ch.iter().enumerate() {
                                let shift = if big_endian {
                                    8 * (ch.len() - 1 - idx)
                                } else {
                                    8 * idx
                                };
                                v |= (b as u64) << shift;
                            }
                            v as f64
                        }
                        None => {
                            diagnostics.push(format!("event {e} channel {c}: truncated int; retained as null"));
                            f64::NAN
                        }
                    });
                }
                events.push(row);
            }
        }
        other => {
            diagnostics.push(format!("unsupported $DATATYPE '{other}'; raw DATA retained, no events decoded"));
        }
    }

    Ok(ParsedFcs {
        version,
        text_start,
        text_end,
        data_start,
        data_end,
        keywords,
        channels,
        events,
        diagnostics,
        sha256,
        byte_count: bytes.len(),
    })
}

/// Encode FCS 3.1 little-endian float32 list data for the case generator.
pub fn encode_fcs_float32(
    channel_names: &[String],
    events: &[Vec<f64>],
    extra_keywords: &[(String, String)],
) -> Vec<u8> {
    let mut kws: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTEXT".into(), "0".into()),
        ("$ENDSTEXT".into(), "0".into()),
        ("$MODE".into(), "L".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$PAR".into(), channel_names.len().to_string()),
        ("$TOT".into(), events.len().to_string()),
    ];
    for (i, name) in channel_names.iter().enumerate() {
        let p = i + 1;
        kws.push((format!("$P{p}N"), name.clone()));
        kws.push((format!("$P{p}B"), "32".into()));
        kws.push((format!("$P{p}E"), "0,0".into()));
        kws.push((format!("$P{p}R"), "262144".into()));
    }
    kws.extend(extra_keywords.iter().cloned());

    let data_len = events.len() * channel_names.len() * 4;
    let header_len = 58usize;
    let build_text = |data_off: usize| {
        let mut full = kws.clone();
        full.push(("$BEGINDATA".into(), data_off.to_string()));
        full.push(("$ENDDATA".into(), (data_off + data_len - 1).to_string()));
        let delim = b'\\';
        let mut text = vec![delim];
        for (k, v) in &full {
            text.extend_from_slice(k.as_bytes());
            text.push(delim);
            text.extend_from_slice(v.as_bytes());
            text.push(delim);
        }
        text
    };
    let text = build_text(header_len + build_text(0).len());
    let data_off = header_len + text.len();
    let text = build_text(data_off);

    let header = format!(
        "FCS3.1    {h:>8}{h:>8}{s:>8}{e:>8}{d0:>8}{d1:>8}0       0       0       0       ",
        h = header_len,
        s = header_len,
        e = header_len + text.len() - 1,
        d0 = data_off,
        d1 = data_off + data_len - 1,
    );
    debug_assert_eq!(header.len(), 58, "header must be exactly 58 bytes");
    let mut out = header.into_bytes();
    out.extend_from_slice(&text);
    for row in events {
        for v in row {
            out.extend_from_slice(&(*v as f32).to_le_bytes());
        }
    }
    out
}
