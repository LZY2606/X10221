//! Minimal FCS 3.1 reader/writer.
//!
//! The writer is used by the bundled case generator so the offline case pack
//! ships real domain material rather than generic tables. The reader preserves
//! the original textual representation of every cell; values that cannot be
//! converted stay at their position with a diagnostic.

use crate::domain::{Channel, CellQuality, ImportDiagnostic, RawCell, RawFile};
use std::collections::HashMap;
use std::io::Write;

#[derive(Clone)]
pub struct FcsWriterSpec {
    pub filename: String,
    pub channels: Vec<Channel>,
    /// f32 values per event per channel; NaN is legal and must round-trip.
    pub events: Vec<Vec<f32>>,
    pub spillover_text: Option<String>,
    pub spillover_detectors: Vec<String>,
    pub spillover_markers: Vec<String>,
    pub spillover_matrix: Vec<Vec<f64>>,
    pub extra_keywords: Vec<(String, String)>,
}

fn escape_text(s: &str) -> String {
    s.replace('\\', "\\\\")
}

pub fn write_fcs(spec: &FcsWriterSpec) -> Vec<u8> {
    let delimiter = b'/';
    let mut text: Vec<u8> = Vec::new();
    text.push(delimiter);
    let mut push_kv = |key: &str, val: &str, text: &mut Vec<u8>| {
        text.extend_from_slice(key.as_bytes());
        text.push(delimiter);
        text.extend_from_slice(escape_text(val).as_bytes());
        text.push(delimiter);
    };
    push_kv("BEGINANALYSIS", "0", &mut text);
    push_kv("ENDANALYSIS", "0", &mut text);
    push_kv("BEGINDATA", "0000000000", &mut text);
    push_kv("ENDDATA", "0000000000", &mut text);
    push_kv("$BEGINSTEXT", "0", &mut text);
    push_kv("$ENDSTEXT", "0", &mut text);
    push_kv("$FIL", &spec.filename, &mut text);
    push_kv("$TOT", &spec.events.len().to_string(), &mut text);
    push_kv("$PAR", &spec.channels.len().to_string(), &mut text);
    push_kv("$BYTEORD", "1,2,3,4", &mut text);
    push_kv("$DATATYPE", "F", &mut text);
    push_kv("$MODE", "L", &mut text);
    push_kv("$NEXTDATA", "0", &mut text);
    for (i, ch) in spec.channels.iter().enumerate() {
        let pn = (i + 1).to_string();
        push_kv(&format!("$P{}N", pn), &ch.name, &mut text);
        if let Some(d) = &ch.detector {
            push_kv(&format!("$P{}DET", pn), d, &mut text);
        }
        if let Some(f) = &ch.fluorophore {
            push_kv(&format!("$P{}FLOR", pn), f, &mut text);
        }
        push_kv(&format!("$P{}B", pn), "32", &mut text);
        push_kv(&format!("$P{}E", pn), "0,0", &mut text);
        push_kv(&format!("$P{}R", pn), &(ch.range_max as u32).to_string(), &mut text);
        if let Some(v) = ch.voltage {
            push_kv(&format!("$P{}V", pn), &v.to_string(), &mut text);
        }
        if let Some(g) = ch.gain {
            push_kv(&format!("$P{}G", pn), &g.to_string(), &mut text);
        }
    }
    if let Some(sp) = &spec.spillover_text {
        push_kv("$SPILLOVER", sp, &mut text);
    }
    for (k, v) in &spec.extra_keywords {
        push_kv(k, v, &mut text);
    }
    // Header: version(6) spaces(4) offsets 8 chars each (6 fields) then spaces.
    let mut header = Vec::new();
    header.extend_from_slice(b"FCS3.1");
    header.extend_from_slice(b"    ");
    let text_start = 258u64;
    let text_end = text_start + text.len() as u64 - 1;
    let data_start = (text_end + 1 + 7) & !7;
    let data_len = spec.events.len() as u64 * spec.channels.len() as u64 * 4;
    let data_end = data_start + data_len - 1;
    let off = format!(
        "{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
        text_start, text_end, data_start, data_end, 0, 0
    );
    header.extend_from_slice(off.as_bytes());
    while header.len() < text_start as usize {
        header.push(b' ');
    }
    // Patch offsets into TEXT segment (required keyword copies).
    let patch = |text: &mut Vec<u8>, key: &str, val: &str| {
        let marker = format!("/{}/", key);
        let pos = text
            .windows(marker.len())
            .position(|w| w == marker.as_bytes())
            .unwrap()
            + marker.len();
        let end_slash = text[pos..].iter().position(|&b| b == delimiter).unwrap() + pos;
        let new = format!("{:>width$}", val, width = end_slash - pos);
        text[pos..end_slash].copy_from_slice(new.as_bytes());
    };
    patch(&mut text, "BEGINDATA", &data_start.to_string());
    patch(&mut text, "ENDDATA", &data_end.to_string());

    let mut out = Vec::new();
    out.extend_from_slice(&header);
    out.extend_from_slice(&text);
    out.resize(data_start as usize, 0);
    for ev in &spec.events {
        for val in ev {
            out.write_all(&val.to_le_bytes()).unwrap();
        }
    }
    out
}

fn unescape_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_text_segment(seg: &[u8]) -> HashMap<String, String> {
    let delim = seg[0] as char;
    let s = String::from_utf8_lossy(&seg[1..]);
    let parts: Vec<&str> = s.split(delim).collect();
    let mut map = HashMap::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        let key = parts[i].trim().to_uppercase();
        if key.is_empty() {
            i += 1;
            continue;
        }
        let val = unescape_text(parts[i + 1]);
        map.insert(key, val);
        i += 2;
    }
    map
}

pub struct ParsedFcs {
    pub raw: RawFile,
    pub spillover: Option<(String, Vec<String>, Vec<String>, Vec<Vec<f64>>)>,
}

pub fn parse_fcs(filename: &str, bytes: &[u8]) -> Result<ParsedFcs, String> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" && &bytes[0..6] != b"FCS3.0" {
        return Err("not an FCS 3.0/3.1 file".into());
    }
    let ver = std::str::from_utf8(&bytes[0..6]).map_err(|e| e.to_string())?.to_string();
    let off = |start: usize| -> u64 {
        std::str::from_utf8(&bytes[start..start + 8])
            .unwrap_or("0")
            .trim()
            .parse()
            .unwrap_or(0)
    };
    let text_start = off(10) as usize;
    let text_end = off(18) as usize;
    let data_start_h = off(26) as usize;
    let data_end_h = off(34) as usize;
    if text_start == 0 || text_end <= text_start {
        return Err("missing TEXT segment".into());
    }
    let kv = parse_text_segment(&bytes[text_start..=text_end]);
    let data_start = kv
        .get("BEGINDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(data_start_h);
    let data_end = kv
        .get("ENDDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(data_end_h);

    let npar: u32 = kv.get("$PAR").ok_or("missing $PAR")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let ntot: u32 = kv.get("$TOT").ok_or("missing $TOT")?.parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    let byteord = kv.get("$BYTEORD").map(|s| s.as_str()).unwrap_or("1,2,3,4");
    let datatype = kv.get("$DATATYPE").map(|s| s.as_str()).unwrap_or("F");
    if datatype != "F" {
        return Err(format!("unsupported $DATATYPE {} (only F/float32 in case pack)", datatype));
    }

    let mut channels = Vec::new();
    for i in 1..=npar {
        let pn = i.to_string();
        let name = kv.get(&format!("$P{}N", pn)).cloned().ok_or_else(|| format!("missing $P{}N", i))?;
        let r: f64 = kv.get(&format!("$P{}R", pn)).map(|s| s.parse().unwrap_or(0.0)).unwrap_or(0.0);
        channels.push(Channel {
            index: i - 1,
            name,
            detector: kv.get(&format!("$P{}DET", pn)).cloned(),
            fluorophore: kv.get(&format!("$P{}FLOR", pn)).cloned(),
            unit: "detector-counts".into(),
            voltage: kv.get(&format!("$P{}V", pn)).and_then(|s| s.parse().ok()),
            gain: kv.get(&format!("$P{}G", pn)).and_then(|s| s.parse().ok()),
            range_min: 0.0,
            range_max: r,
        });
    }

    let mut events: Vec<Vec<RawCell>> = Vec::new();
    let mut diagnostics = Vec::new();
    let little = byteord.starts_with("1,2");
    let step = (npar as usize) * 4;
    for idx in 0..ntot as usize {
        let base = data_start + idx * step;
        if base + step > bytes.len().min(data_end + 1) {
            diagnostics.push(ImportDiagnostic {
                code: "event-truncated".into(),
                event_index: Some(idx as u32),
                channel: None,
                message: "event bytes missing".into(),
            });
            continue;
        }
        let mut row = Vec::new();
        for c in 0..npar as usize {
            let p = base + c * 4;
            let raw4 = [bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]];
            let bits = if little {
                u32::from_le_bytes(raw4)
            } else {
                u32::from_be_bytes(raw4)
            };
            let f = f32::from_bits(bits);
            let raw_repr = format!("f32:{}", f);
            let (value, quality) = if f.is_nan() {
                diagnostics.push(ImportDiagnostic {
                    code: "non-finite".into(),
                    event_index: Some(idx as u32),
                    channel: Some(channels[c].name.clone()),
                    message: "NaN retained at source position".into(),
                });
                (None, CellQuality::NonFinite)
            } else if f.is_infinite() {
                diagnostics.push(ImportDiagnostic {
                    code: "non-finite".into(),
                    event_index: Some(idx as u32),
                    channel: Some(channels[c].name.clone()),
                    message: "infinite value retained at source position".into(),
                });
                (None, CellQuality::NonFinite)
            } else if f < channels[c].range_min as f32 || f > channels[c].range_max as f32 {
                diagnostics.push(ImportDiagnostic {
                    code: "out-of-range".into(),
                    event_index: Some(idx as u32),
                    channel: Some(channels[c].name.clone()),
                    message: format!("{} outside [0,{}]", f, channels[c].range_max),
                });
                (Some(f as f64), CellQuality::OutOfRange)
            } else {
                (Some(f as f64), CellQuality::Ok)
            };
            row.push(RawCell {
                raw: raw_repr,
                value,
                quality,
            });
        }
        events.push(row);
    }

    let sha = crate::ids::sha256_hex(&bytes[..]);
    let raw = RawFile {
        filename: filename.to_string(),
        fcs_version: ver,
        sha256: sha,
        channels,
        events,
        diagnostics,
    };

    let spillover = kv.get("$SPILLOVER").and_then(|text| parse_spillover(text).ok());
    Ok(ParsedFcs { raw, spillover })
}

fn parse_spillover(text: &str) -> Result<(String, Vec<String>, Vec<String>, Vec<Vec<f64>>), String> {
    let parts: Vec<&str> = text.split(',').map(|s| s.trim()).collect();
    let n: usize = parts
        .get(0)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "bad $SPILLOVER header".to_string())?;
    if parts.len() < 1 + 2 * n + n * n {
        return Err("$SPILLOVER too short".into());
    }
    let detectors = parts[1..1 + n].iter().map(|s| s.to_string()).collect();
    let markers = parts[1 + n..1 + 2 * n].iter().map(|s| s.to_string()).collect();
    let mut matrix = Vec::new();
    let mut k = 1 + 2 * n;
    for _ in 0..n {
        let mut row = Vec::new();
        for _ in 0..n {
            row.push(parts[k].parse::<f64>().map_err(|e: std::num::ParseFloatError| e.to_string())?);
            k += 1;
        }
        matrix.push(row);
    }
    Ok(("$SPILLOVER".into(), detectors, markers, matrix))
}
