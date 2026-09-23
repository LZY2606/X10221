//! Minimal FCS 3.1 reader/writer.
//!
//! Only what the offline case pack needs: HEADER/TEXT/DATA segments, float32
//! event matrices, $SPILLOVER keyword. Original textual representations are
//! preserved; unconvertible values stay in place with diagnostics.

use crate::domain::{
    Basis, Channel, EventsDoc, Raw, ValueDiagnostic,
};
use crate::error::{Error, Result};
use std::collections::HashMap;

pub struct FcsFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct TextSeg {
    delimiter: u8,
    pairs: Vec<(String, String)>,
}

fn parse_text(bytes: &[u8], start: u64, end: u64) -> Result<TextSeg> {
    let (start, end) = (start as usize, end as usize);
    if end <= start || end > bytes.len() {
        return Err(Error::Fcs("bad TEXT segment".into()));
    }
    let delimiter = bytes[start];
    let body = &bytes[start + 1..end - 1];
    let text = String::from_utf8_lossy(body);
    let parts: Vec<&str> = text.split(delimiter as char).collect();
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        pairs.push((parts[i].trim().to_string(), parts[i + 1].to_string()));
        i += 2;
    }
    Ok(TextSeg { delimiter, pairs })
}

impl TextSeg {
    fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_uppercase();
        self.pairs
            .iter()
            .find(|(k, _)| k.to_ascii_uppercase() == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parse event data plus embedded metadata from an FCS blob. `blob_id` is the
/// stored-bytes identity.
pub fn parse(name: &str, blob_id: &str, bytes: &[u8]) -> Result<EventsDoc> {
    let (text_s, text_e, data_s, data_e) = parse_header_offsets(bytes)?;
    let text = parse_text(bytes, text_s, text_e)?;

    let get = |k: &str| -> Result<&str> {
        text.get(k)
            .ok_or_else(|| Error::Fcs(format!("missing keyword {k}")))
    };
    let mode = get("$MODE")?;
    if mode != "L" {
        return Err(Error::Fcs(format!("unsupported $MODE {mode}")));
    }
    let datatype = get("$DATATYPE")?;
    if datatype != "F" {
        return Err(Error::Fcs(format!("unsupported $DATATYPE {datatype}")));
    }
    let byteord = get("$BYTEORD")?;
    let big = match byteord {
        "4,3,2,1" => true,
        "1,2,3,4" => false,
        _ => return Err(Error::Fcs(format!("unsupported $BYTEORD {byteord}"))),
    };
    let n_events: usize = get("$TOT")?.parse().map_err(|_| Error::Fcs("$TOT".into()))?;
    let n_par: usize = get("$PAR")?.parse().map_err(|_| Error::Fcs("$PAR".into()))?;

    let mut channels = Vec::with_capacity(n_par);
    for i in 1..=n_par {
        let short_name = text.get(&format!("$P{i}N")).unwrap_or("").to_string();
        let long = text
            .get(&format!("$P{i}S"))
            .unwrap_or(short_name.as_str())
            .to_string();
        let gain: f64 = text.get(&format!("$P{i}G")).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let range: f64 = text.get(&format!("$P{i}R")).and_then(|v| v.parse().ok()).unwrap_or(262144.0);
        let unit = text.get(&format!("$P{i}U")).unwrap_or("channel").to_string();
        channels.push(Channel {
            index: i - 1,
            name: long,
            short_name,
            unit,
            basis: Basis::RawSignal,
            gain,
            range,
        });
    }

    let seg = if data_s == 0 || data_e == 0 {
        let s: u64 = get("$BEGINDATA")?.parse().map_err(|_| Error::Fcs("$BEGINDATA".into()))?;
        let e: u64 = get("$ENDDATA")?.parse().map_err(|_| Error::Fcs("$ENDDATA".into()))?;
        &bytes[s as usize..e as usize]
    } else {
        &bytes[data_s as usize..data_e as usize]
    };
    let need = n_events * n_par * 4;
    if seg.len() < need {
        return Err(Error::Fcs(format!(
            "DATA segment too short: {} < {need}",
            seg.len()
        )));
    }

    let mut rows = Vec::with_capacity(n_events);
    let mut diagnostics = Vec::new();
    for ev in 0..n_events {
        let mut row = Vec::with_capacity(n_par);
        for ch in 0..n_par {
            let off = (ev * n_par + ch) * 4;
            let bits = if big {
                u32::from_be_bytes(seg[off..off + 4].try_into().unwrap())
            } else {
                u32::from_le_bytes(seg[off..off + 4].try_into().unwrap())
            };
            let f = f32::from_bits(bits) as f64;
            let raw = if f.is_nan() {
                diagnostics.push(ValueDiagnostic {
                    event_index: ev,
                    channel: channels[ch].short_name.clone(),
                    code: "not_a_number".into(),
                    message: "FCS float decodes to NaN; value kept in place, excluded from gates"
                        .into(),
                });
                Raw::Unconvertible {
                    raw: "NaN".into(),
                    diagnostic: "not_a_number".into(),
                }
            } else if f.is_infinite() {
                diagnostics.push(ValueDiagnostic {
                    event_index: ev,
                    channel: channels[ch].short_name.clone(),
                    code: "infinite_signal".into(),
                    message: "FCS float decodes to infinity; retained as extreme signal".into(),
                });
                Raw::Unconvertible {
                    raw: if f > 0.0 { "Inf".into() } else { "-Inf".into() },
                    diagnostic: "infinite_signal".into(),
                }
            } else {
                Raw::number(f)
            };
            row.push(raw);
        }
        rows.push(row);
    }

    Ok(EventsDoc {
        fcs_blob_id: blob_id.into(),
        fcs_file_name: name.into(),
        channels,
        rows,
        diagnostics,
    })
}

fn parse_header_offsets(bytes: &[u8]) -> Result<(u64, u64, u64, u64)> {
    let num = |a: usize, b: usize| -> Result<u64> {
        Ok(std::str::from_utf8(&bytes[a..b])
            .map_err(|e| Error::Fcs(e.to_string()))?
            .trim()
            .parse::<u64>()
            .unwrap_or(0))
    };
    Ok((num(10, 18)?, num(18, 26)?, num(26, 34)?, num(34, 42)?))
}

/// Writer used by the case generator: builds an FCS3.1 blob with TEXT (pipe
/// delimited) + float32 DATA. `extra_keywords` (e.g. $SPILLOVER) are embedded.
pub fn write_fcs(
    channels: &[Channel],
    rows: &[Vec<Raw>],
    extra_keywords: &HashMap<String, String>,
) -> Vec<u8> {
    let n_par = channels.len();
    let n_events = rows.len();
    let mut pairs: Vec<(String, String)> = vec![
        ("$BEGINANAL".to_string(), "0".into()),
        ("$ENDANAL".to_string(), "0".into()),
        ("$BEGINSTEXT".to_string(), "0".into()),
        ("$ENDSTEXT".to_string(), "0".into()),
        ("$MODE".to_string(), "L".into()),
        ("$DATATYPE".to_string(), "F".into()),
        ("$BYTEORD".to_string(), "1,2,3,4".into()),
        ("$NEXTDATA".to_string(), "0".into()),
        ("$TOT".to_string(), n_events.to_string()),
        ("$PAR".to_string(), n_par.to_string()),
    ];
    for (i, ch) in channels.iter().enumerate() {
        let i = i + 1;
        pairs.push((format!("$P{i}N"), ch.short_name.clone()));
        pairs.push((format!("$P{i}S"), ch.name.clone()));
        pairs.push((format!("$P{i}B"), "32".into()));
        pairs.push((format!("$P{i}E"), "0,0".into()));
        pairs.push((format!("$P{i}G"), format!("{}", ch.gain)));
        pairs.push((format!("$P{i}R"), format!("{}", ch.range as i64)));
        pairs.push((format!("$P{i}U"), ch.unit.clone()));
    }
    for (k, v) in extra_keywords {
        pairs.push((k.clone(), v.clone()));
    }

    let build_text = |pairs: &[(String, String)], begin_data: u64, end_data: u64| -> Vec<u8> {
        let mut all = pairs.to_vec();
        all.push(("$BEGINDATA".to_string(), begin_data.to_string()));
        all.push(("$ENDDATA".to_string(), end_data.to_string()));
        let mut out = Vec::new();
        out.push(b'|');
        for (k, v) in &all {
            out.extend_from_slice(k.as_bytes());
            out.push(b'|');
            out.extend_from_slice(v.as_bytes());
            out.push(b'|');
        }
        if out.len() % 2 == 0 {
            out.extend_from_slice(b" |");
        }
        out
    };

    let data_len = n_events * n_par * 4;
    // Fixed 58-byte header; text starts at 58.
    let text_start: u64 = 58;
    // Iterate once to get lengths (offsets do not change the text length here
    // because BEGINDATA/ENDDATA are fixed-width padded in the header, and the
    // TEXT keyword numbers lengths only grow logarithmically: iterate).
    let mut text = build_text(&pairs, 0, 0);
    let mut begin_data = text_start + text.len() as u64;
    loop {
        text = build_text(&pairs, begin_data, begin_data + data_len as u64 - 1);
        let new_begin = text_start + text.len() as u64;
        if new_begin == begin_data {
            break;
        }
        begin_data = new_begin;
    }
    let end_data = begin_data + data_len as u64 - 1;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"FCS3.1     ");
    bytes.extend_from_slice(format!("{:>8}", text_start).as_bytes());
    bytes.extend_from_slice(format!("{:>8}", begin_data - 1).as_bytes()); // TEXT end
    bytes.extend_from_slice(format!("{:>8}", begin_data).as_bytes());
    bytes.extend_from_slice(format!("{:>8}", end_data).as_bytes());
    bytes.extend_from_slice(b"0       0       0       0");
    debug_assert_eq!(bytes.len(), 58);
    bytes.extend_from_slice(&text);

    for row in rows {
        for raw in row {
            let f = match raw {
                Raw::Number { f, .. } => *f as f32,
                Raw::Unconvertible { raw, .. } if raw == "Inf" => f32::INFINITY,
                Raw::Unconvertible { raw, .. } if raw == "-Inf" => f32::NEG_INFINITY,
                _ => f32::NAN,
            };
            bytes.extend_from_slice(&f.to_le_bytes());
        }
    }
    bytes
}
