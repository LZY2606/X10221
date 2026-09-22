//! Minimal FCS 3.0/3.1 parser (list mode, DATATYPE F/D/I).
//! Raw channel values are preserved exactly as stored in the DATA segment;
//! no scaling, clamping or cleaning is applied at import time.

use std::collections::BTreeMap;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FcsParam {
    pub index: usize,
    pub name: String,        // $PnN
    pub stain: String,       // $PnS (may be empty)
    pub range: f64,          // $PnR
    pub bits: u32,           // $PnB
    pub unit: String,        // domain unit label, defaults to "ADC channels"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FcsFile {
    pub version: String,
    pub text: BTreeMap<String, String>,
    pub params: Vec<FcsParam>,
    /// events[event_index][param_index] = raw channel value (as stored)
    pub events: Vec<Vec<f64>>,
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile, String> {
    if bytes.len() < 58 {
        return Err("file too small for FCS header".into());
    }
    let version = std::str::from_utf8(&bytes[0..6])
        .map_err(|_| "bad version".to_string())?
        .trim()
        .to_string();
    if !version.starts_with("FCS") {
        return Err(format!("not an FCS file (magic={version})"));
    }
    let read_off = |a: usize, b: usize| -> usize {
        std::str::from_utf8(&bytes[a..b])
            .unwrap_or("")
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
    };
    let mut text_start = read_off(10, 18);
    let mut text_end = read_off(18, 26);
    let mut data_start = read_off(26, 34);
    let mut data_end = read_off(34, 42);

    // Some writers put offsets in the TEXT segment when > 99,999,999.
    if text_start == 0 || text_end == 0 {
        return Err("missing TEXT segment offsets".into());
    }
    if text_end >= bytes.len() {
        text_end = bytes.len() - 1;
    }
    let text_seg = &bytes[text_start..=text_end.min(bytes.len() - 1)];
    let delim = text_seg[0] as char;
    let text_str = String::from_utf8_lossy(&text_seg[1..]).to_string();
    let mut text = BTreeMap::new();
    {
        let parts: Vec<&str> = text_str.split(delim).collect();
        let mut i = 0;
        while i + 1 < parts.len() {
            text.insert(parts[i].to_string(), parts[i + 1].to_string());
            i += 2;
        }
    }
    let get = |k: &str| -> Option<String> { text.get(k).cloned() };
    if data_start == 0 {
        data_start = get("$BEGINDATA").and_then(|v| v.parse().ok()).unwrap_or(0);
        data_end = get("$ENDDATA").and_then(|v| v.parse().ok()).unwrap_or(0);
    }
    let _ = &mut text_start;
    let tot: usize = get("$TOT").and_then(|v| v.parse().ok()).unwrap_or(0);
    let par: usize = get("$PAR").and_then(|v| v.parse().ok()).unwrap_or(0);
    if par == 0 {
        return Err("$PAR is 0".into());
    }
    let datatype = get("$DATATYPE").unwrap_or_else(|| "F".into());
    let byteord = get("$BYTEORD").unwrap_or_else(|| "1,2,3,4".into());
    let little = byteord.starts_with('1');

    let mut params = Vec::new();
    for i in 1..=par {
        let name = get(&format!("$P{i}N")).unwrap_or_else(|| format!("P{i}"));
        let stain = get(&format!("$P{i}S")).unwrap_or_default();
        let range = get(&format!("$P{i}R")).and_then(|v| v.parse().ok()).unwrap_or(262144.0);
        let bits = get(&format!("$P{i}B")).and_then(|v| v.parse().ok()).unwrap_or(32);
        params.push(FcsParam {
            index: i - 1,
            name,
            stain,
            range,
            bits,
            unit: "ADC channels".into(),
        });
    }

    let n_values = tot * par;
    let mut raw: Vec<f64> = Vec::with_capacity(n_values);
    let seg = &bytes[data_start..=data_end.min(bytes.len() - 1)];
    match datatype.as_str() {
        "F" => {
            for chunk in seg.chunks_exact(4) {
                let b: [u8; 4] = chunk.try_into().unwrap();
                raw.push(if little { f32::from_le_bytes(b) } else { f32::from_be_bytes(b) } as f64);
            }
        }
        "D" => {
            for chunk in seg.chunks_exact(8) {
                let b: [u8; 8] = chunk.try_into().unwrap();
                raw.push(if little { f64::from_le_bytes(b) } else { f64::from_be_bytes(b) });
            }
        }
        "I" => {
            // Use max $PnB to choose width; mixed widths are not supported.
            let width = params.iter().map(|p| p.bits).max().unwrap_or(16);
            match width {
                0..=16 => {
                    for chunk in seg.chunks_exact(2) {
                        let b: [u8; 2] = chunk.try_into().unwrap();
                        raw.push(if little { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) } as f64);
                    }
                }
                _ => {
                    for chunk in seg.chunks_exact(4) {
                        let b: [u8; 4] = chunk.try_into().unwrap();
                        raw.push(if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) } as f64);
                    }
                }
            }
        }
        other => return Err(format!("unsupported $DATATYPE {other}")),
    }
    raw.truncate(n_values);
    if raw.len() < n_values {
        return Err(format!("DATA segment short: got {} values, want {n_values}", raw.len()));
    }
    let events: Vec<Vec<f64>> = raw.chunks(par).map(|c| c.to_vec()).collect();
    Ok(FcsFile { version, text, params, events })
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_generated_file() {
        let bytes = crate::fcswrite::write(
            &["FSC-A", "FL1-A"],
            &[262144.0, 262144.0],
            &[vec![100.0, 200.0], vec![300.0, 400.0]],
        );
        let f = super::parse(&bytes).unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.events.len(), 2);
        assert_eq!(f.events[1][1], 400.0);
        assert_eq!(f.params[1].range, 262144.0);
    }
}
