//! Minimal FCS 3.0 parser: HEADER + TEXT + DATA segments.
//! Supports $DATATYPE F (float32) and D (float64), little/big endian,
//! and extracts $SPILLOVER metadata. Raw bytes are always preserved by
//! the caller; this parser only produces a parsed view.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsChannel {
    pub index: usize,
    pub name: String,          // $PnN
    pub stain: String,         // $PnS (may be empty)
    pub bits: u32,             // $PnB
    pub range: f64,            // $PnR  (detector max coordinate basis)
    pub gain: Option<f64>,     // $PnG
    pub unit: String,          // domain unit label, e.g. "fluorescence (a.u.)"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpilloverMeta {
    pub channels: Vec<String>,
    /// row-major n x n values
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsFile {
    pub version: String,
    pub keywords: BTreeMap<String, String>,
    pub channels: Vec<FcsChannel>,
    pub total_events: usize,
    /// events[event][channel] raw values as stored in DATA segment
    pub events: Vec<Vec<f64>>,
    pub spillover: Option<SpilloverMeta>,
}

fn parse_offset(header: &[u8], from: usize, to: usize) -> Result<Option<usize>> {
    let s = std::str::from_utf8(&header[from..=to])?.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let v: usize = s.parse().context("bad header offset")?;
    if v == 0 {
        Ok(None)
    } else {
        Ok(Some(v))
    }
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile> {
    if bytes.len() < 58 {
        bail!("file too small for FCS header");
    }
    let version = std::str::from_utf8(&bytes[0..6])?.trim().to_string();
    if !version.starts_with("FCS") {
        bail!("not an FCS file (magic={version})");
    }
    let text_start = parse_offset(bytes, 10, 17)?.context("missing TEXT start")?;
    let text_end = parse_offset(bytes, 18, 25)?.context("missing TEXT end")?;
    if text_end >= bytes.len() || text_start > text_end {
        bail!("TEXT segment out of bounds");
    }
    let text = &bytes[text_start..=text_end];
    let delim = text[0];
    let text_str = std::str::from_utf8(&text[1..])?;
    // key/value pairs, escaped delimiter is doubled
    let mut keywords = BTreeMap::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let b = text_str.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == delim {
            if i + 1 < b.len() && b[i + 1] == delim {
                cur.push(delim as char);
                i += 2;
                continue;
            }
            tokens.push(std::mem::take(&mut cur));
        } else {
            cur.push(b[i] as char);
        }
        i += 1;
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut it = tokens.into_iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        keywords.insert(k.to_ascii_uppercase(), v);
    }

    let par: usize = req(&keywords, "$PAR")?.parse()?;
    let tot: usize = req(&keywords, "$TOT")?.parse()?;
    let datatype = req(&keywords, "$DATATYPE")?.to_string();
    let byteord = req(&keywords, "$BYTEORD")?.to_string();
    let little = byteord.trim() == "1,2,3,4";

    let mut channels = Vec::new();
    for p in 1..=par {
        let name = keywords
            .get(&format!("$P{p}N"))
            .cloned()
            .unwrap_or_else(|| format!("P{p}"));
        let stain = keywords.get(&format!("$P{p}S")).cloned().unwrap_or_default();
        let bits: u32 = keywords
            .get(&format!("$P{p}B"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(32);
        let range: f64 = keywords
            .get(&format!("$P{p}R"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(262144.0);
        let gain = keywords
            .get(&format!("$P{p}G"))
            .and_then(|s| s.parse().ok());
        let unit = if name.starts_with("FSC") || name.starts_with("SSC") {
            "scatter (a.u.)".to_string()
        } else {
            "fluorescence (a.u.)".to_string()
        };
        channels.push(FcsChannel {
            index: p - 1,
            name,
            stain,
            bits,
            range,
            gain,
            unit,
        });
    }

    // DATA segment offsets: header first, then $BEGINDATA/$ENDDATA keywords
    let data_start = match parse_offset(bytes, 26, 33)? {
        Some(v) => v,
        None => req(&keywords, "$BEGINDATA")?.parse()?,
    };
    let data_end = match parse_offset(bytes, 34, 41)? {
        Some(v) => v,
        None => req(&keywords, "$ENDDATA")?.parse()?,
    };
    if data_end >= bytes.len() || data_start > data_end {
        bail!("DATA segment out of bounds");
    }
    let data = &bytes[data_start..=data_end];
    let width = match datatype.as_str() {
        "F" => 4,
        "D" => 8,
        other => bail!("unsupported $DATATYPE {other}"),
    };
    let need = tot * par * width;
    if data.len() < need {
        bail!("DATA segment truncated: have {}, need {need}", data.len());
    }
    let mut events = Vec::with_capacity(tot);
    let mut off = 0usize;
    for _ in 0..tot {
        let mut row = Vec::with_capacity(par);
        for _ in 0..par {
            let v = match (width, little) {
                (4, true) => f32::from_le_bytes(data[off..off + 4].try_into()?) as f64,
                (4, false) => f32::from_be_bytes(data[off..off + 4].try_into()?) as f64,
                (8, true) => f64::from_le_bytes(data[off..off + 8].try_into()?),
                (8, false) => f64::from_be_bytes(data[off..off + 8].try_into()?),
                _ => unreachable!(),
            };
            row.push(v);
            off += width;
        }
        events.push(row);
    }

    let spillover = keywords.get("$SPILLOVER").and_then(|s| {
        let parts: Vec<&str> = s.split(',').collect();
        let n: usize = parts.first()?.parse().ok()?;
        if parts.len() < 1 + n + n * n {
            return None;
        }
        let channels = parts[1..1 + n].iter().map(|s| s.to_string()).collect();
        let values: Vec<f64> = parts[1 + n..1 + n + n * n]
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();
        if values.len() != n * n {
            return None;
        }
        Some(SpilloverMeta { channels, values })
    });

    Ok(FcsFile {
        version,
        keywords,
        channels,
        total_events: tot,
        events,
        spillover,
    })
}

fn req<'a>(kw: &'a BTreeMap<String, String>, k: &str) -> Result<&'a String> {
    kw.get(k).with_context(|| format!("missing required keyword {k}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny FCS3.0 file in memory (float32 LE, 2 params, 3 events).
    pub fn build_test_fcs(spillover: Option<&str>) -> Vec<u8> {
        let mut text = String::new();
        let kv = [
            ("$BEGINANALYSIS", "0"), ("$ENDANALYSIS", "0"),
            ("$BYTEORD", "1,2,3,4"), ("$DATATYPE", "F"), ("$MODE", "L"),
            ("$NEXTDATA", "0"), ("$PAR", "2"), ("$TOT", "3"),
            ("$P1B", "32"), ("$P1R", "1024"), ("$P1N", "FSC-A"), ("$P1S", ""),
            ("$P2B", "32"), ("$P2R", "1024"), ("$P2N", "FITC-A"), ("$P2S", "CD3"),
        ];
        for (k, v) in kv {
            text.push('/');
            text.push_str(k);
            text.push('/');
            text.push_str(v);
        }
        if let Some(sp) = spillover {
            text.push_str("/$SPILLOVER/");
            text.push_str(sp);
        }
        text.push('/');
        let header_len = 58;
        let text_start = header_len;
        let text_end = text_start + text.len() - 1;
        let data_start = text_end + 1;
        let data_len = 3 * 2 * 4;
        let data_end = data_start + data_len - 1;
        let mut out = Vec::new();
        out.extend_from_slice(b"FCS3.0    ");
        out.extend_from_slice(format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
            text_start, text_end, data_start, data_end, 0, 0).as_bytes());
        out.extend_from_slice(text.as_bytes());
        let vals: [f32; 6] = [10.0, 100.0, 20.0, 1024.0, 30.0, -5.0];
        for v in vals {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn parses_roundtrip() {
        let bytes = build_test_fcs(Some("2,FITC-A,PE-A,1.0,0.15,0.02,1.0"));
        let f = parse_fcs(&bytes).unwrap();
        assert_eq!(f.version, "FCS3.0");
        assert_eq!(f.channels.len(), 2);
        assert_eq!(f.channels[1].name, "FITC-A");
        assert_eq!(f.channels[1].range, 1024.0);
        assert_eq!(f.total_events, 3);
        assert_eq!(f.events[1][1], 1024.0); // extreme value preserved raw
        assert_eq!(f.events[2][1], -5.0);   // negative preserved raw
        let sp = f.spillover.unwrap();
        assert_eq!(sp.channels, vec!["FITC-A", "PE-A"]);
        assert_eq!(sp.values, vec![1.0, 0.15, 0.02, 1.0]);
    }
}
