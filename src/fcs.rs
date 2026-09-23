//! Minimal FCS 3.0 parser (HEADER + TEXT + DATA, $DATATYPE F, little endian).
//! Raw values are preserved exactly as parsed; no cleaning happens here.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub text: HashMap<String, String>,
    pub channels: Vec<String>,
    /// events[event_index][channel_index]
    pub events: Vec<Vec<f32>>,
}

fn parse_offset(header: &[u8], start: usize) -> Result<usize> {
    let s = std::str::from_utf8(&header[start..start + 8])
        .context("header offset not ascii")?;
    s.trim().parse::<usize>().with_context(|| format!("bad offset '{s}'"))
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile> {
    if bytes.len() < 58 {
        bail!("file too small for FCS header");
    }
    if !bytes.starts_with(b"FCS3.") {
        bail!("unsupported FCS version (only FCS3.x supported)");
    }
    let text_start = parse_offset(bytes, 10)?;
    let text_end = parse_offset(bytes, 18)?;
    let data_start = parse_offset(bytes, 26)?;
    let data_end = parse_offset(bytes, 34)?;
    if text_end >= bytes.len() || data_end >= bytes.len() {
        bail!("segment offsets out of range");
    }
    let text_raw = &bytes[text_start..=text_end];
    let delim = text_raw[0] as char;
    let text_str = String::from_utf8_lossy(&text_raw[1..]);
    let mut text = HashMap::new();
    let parts: Vec<&str> = text_str.split(delim).collect();
    for kv in parts.chunks(2) {
        if kv.len() == 2 && !kv[0].is_empty() {
            text.insert(kv[0].to_string(), kv[1].to_string());
        }
    }
    let par: usize = text
        .get("$PAR")
        .ok_or_else(|| anyhow!("missing $PAR"))?
        .parse()?;
    let tot: usize = text
        .get("$TOT")
        .ok_or_else(|| anyhow!("missing $TOT"))?
        .parse()?;
    let datatype = text.get("$DATATYPE").map(|s| s.as_str()).unwrap_or("F");
    if datatype != "F" {
        bail!("unsupported $DATATYPE '{datatype}' (only 32-bit float)");
    }
    let byteord = text.get("$BYTEORD").map(|s| s.as_str()).unwrap_or("1,2,3,4");
    if byteord != "1,2,3,4" {
        bail!("unsupported $BYTEORD '{byteord}' (only little endian)");
    }
    let mut channels = Vec::with_capacity(par);
    for i in 1..=par {
        let name = text
            .get(&format!("$P{i}N"))
            .cloned()
            .ok_or_else(|| anyhow!("missing $P{i}N"))?;
        channels.push(name);
    }
    let need = data_start + tot * par * 4;
    if need > bytes.len() || data_start + (data_end - data_start + 1) > bytes.len() {
        bail!("DATA segment truncated");
    }
    let mut events = Vec::with_capacity(tot);
    let mut cursor = data_start;
    for _ in 0..tot {
        let mut ev = Vec::with_capacity(par);
        for _ in 0..par {
            let v = f32::from_le_bytes(bytes[cursor..cursor + 4].try_into()?);
            ev.push(v);
            cursor += 4;
        }
        events.push(ev);
    }
    Ok(FcsFile { text, channels, events })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_generated_fixture() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/cases/flow-boundary-chamber/fcs/sample_a.fcs"
        );
        let bytes = std::fs::read(path).unwrap();
        let fcs = parse(&bytes).unwrap();
        assert_eq!(fcs.channels, vec!["FSC-A", "SSC-A", "FITC-A", "PE-A"]);
        assert_eq!(fcs.events.len(), 302);
        assert_eq!(fcs.events[0].len(), 4);
        // saturated extreme events are preserved, not clamped away
        let saturated = fcs
            .events
            .iter()
            .filter(|e| e[2] == 262144.0)
            .count();
        assert_eq!(saturated, 8);
        let nan_count = fcs.events.iter().filter(|e| e[2].is_nan()).count();
        assert_eq!(nan_count, 2);
    }

    #[test]
    fn rejects_truncated_file() {
        assert!(parse(b"FCS3.0").is_err());
    }
}
