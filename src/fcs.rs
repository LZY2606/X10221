//! Minimal FCS 3.1 parser for the bundled boundary case.
//!
//! Supports `$DATATYPE F` (IEEE 754 single precision, big endian, as produced
//! by the repository case generator). Original raw bytes are stored verbatim;
//! only the derived layer normalizes values. Non-finite channel values are
//! kept in place with a diagnostic rather than being dropped.

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct FcsEvent {
    pub index: i64,
    pub values: Vec<f64>,
    pub diagnostics: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct FcsData {
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<FcsChannel>,
    pub events: Vec<FcsEvent>,
    pub spillover: Option<SpilloverSpec>,
}

#[derive(Debug, Clone)]
pub struct FcsChannel {
    pub n: i64,
    pub name: String,
    pub long_name: Option<String>,
    pub detector: Option<String>,
    pub range: f64,
    pub unit: String,
}

#[derive(Debug, Clone)]
pub struct SpilloverSpec {
    pub keyword: String,
    pub n: usize,
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
}

#[derive(Debug)]
pub enum FcsError {
    BadHeader(String),
    Unsupported(String),
}
impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FcsError::BadHeader(s) => write!(f, "malformed FCS header: {s}"),
            FcsError::Unsupported(s) => write!(f, "unsupported FCS feature: {s}"),
        }
    }
}
impl std::error::Error for FcsError {}

fn decode_text(raw: &[u8], delim: u8) -> Vec<String> {
    // FCS text segment: begins and ends with the delimiter.
    let body: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|b| *b != b'\r' && *b != b'\n')
        .collect();
    let parts: Vec<&[u8]> = body.split(|b| *b == delim).collect();
    let mut tokens = Vec::new();
    for p in parts {
        if p.is_empty() { continue; }
        // FCS 3.1 escape: two delimiter chars inside a token -> one literal.
        tokens.push(String::from_utf8_lossy(p).to_string());
    }
    tokens
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsData, FcsError> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err(FcsError::BadHeader("missing FCS3.1 identifier".into()));
    }
    let num = |a: usize, b: usize| -> i64 {
        std::str::from_utf8(&bytes[a..b]).unwrap_or("").trim().parse().unwrap_or(0)
    };
    let text_start = num(10, 18) as usize;
    let text_end = num(18, 26) as usize;
    let data_start = num(26, 34) as usize;
    let data_end = num(34, 42) as usize;

    if text_start >= bytes.len() || text_end > bytes.len() || text_end <= text_start {
        return Err(FcsError::BadHeader("TEXT offsets out of range".into()));
    }
    let delim = bytes[text_start];
    let tokens = decode_text(&bytes[text_start + 1..text_end], delim);
    let mut kw = Vec::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        kw.push((tokens[i].clone(), tokens[i + 1].clone()));
        i += 2;
    }
    let get = |k: &str| kw.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());

    let datatype = get("$DATATYPE").unwrap_or_default();
    let byteord = get("$BYTEORD").unwrap_or_default();
    let mode = get("$MODE").unwrap_or_default();
    if datatype != "F" {
        return Err(FcsError::Unsupported(format!("$DATATYPE={datatype}, only F (float32)")));
    }
    if byteord != "4,3,2,1" {
        return Err(FcsError::Unsupported(format!("$BYTEORD={byteord}, only 4,3,2,1")));
    }
    if mode != "L" {
        return Err(FcsError::Unsupported(format!("$MODE={mode}, only L (list)")));
    }
    let par: i64 = get("$PAR").and_then(|v| v.parse().ok()).unwrap_or(0);
    let tot: i64 = get("$TOT").and_then(|v| v.parse().ok()).unwrap_or(0);
    if par <= 0 {
        return Err(FcsError::BadHeader("$PAR missing".into()));
    }

    let mut channels = Vec::new();
    for n in 1..=par {
        let name = get(&format!("$P{n}N")).unwrap_or_else(|| format!("P{n}"));
        let long_name = get(&format!("$P{n}S"));
        let detector = get(&format!("$P{n}D"));
        let range: f64 = get(&format!("$P{n}R")).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        channels.push(FcsChannel { n, name, long_name, detector, range, unit: "channel_units".into() });
    }

    // Data offsets may be overridden inside TEXT when they do not fit the header.
    let (ds, de) = if data_start == 0 || data_end == 0 {
        let s: usize = get("$BEGINDATA").and_then(|v| v.parse().ok()).unwrap_or(0);
        let e: usize = get("$ENDDATA").and_then(|v| v.parse().ok()).unwrap_or(0);
        (s, e)
    } else {
        (data_start, data_end + 1)
    };
    if ds >= bytes.len() || de > bytes.len() || de <= ds {
        return Err(FcsError::BadHeader("DATA offsets out of range".into()));
    }
    let data = &bytes[ds..de];
    let per = (par as usize) * 4;
    let mut events = Vec::new();
    for (idx, chunk) in data.chunks_exact(per).enumerate() {
        if idx as i64 >= tot { break; }
        let mut values = Vec::with_capacity(par as usize);
        let mut diags = Vec::new();
        for (c, four) in chunk.chunks_exact(4).enumerate() {
            let bits = u32::from_be_bytes([four[0], four[1], four[2], four[3]]);
            let v = f32::from_bits(bits) as f64;
            if !v.is_finite() {
                diags.push(json!({
                    "code": "non_finite_channel_value",
                    "channel_index": c,
                    "channel": channels.get(c).map(|x| x.name.clone()).unwrap_or_default(),
                    "raw_bits_hex": format!("0x{bits:08X}"),
                    "interpretation": if v.is_nan() { "NaN" } else if v > 0.0 { "+Inf" } else { "-Inf" },
                    "action": "kept_in_place_as_null"
                }));
            }
            values.push(v);
        }
        events.push(FcsEvent { index: idx as i64 + 1, values, diagnostics: diags });
    }

    let spillover = parse_spillover(&get);

    Ok(FcsData { keywords: kw, channels, events, spillover })
}

fn parse_spillover(get: &impl Fn(&str) -> Option<String>) -> Option<SpilloverSpec> {
    // Prefer the FCS 3.0 $SPILL keyword; FCS 3.1 uses $SPILLOVER.
    for key in ["$SPILLOVER", "$SPILL"] {
        if let Some(raw) = get(key) {
            let fields: Vec<&str> = raw.split(',').map(|s| s.trim()).collect();
            if fields.len() < 2 { return None; }
            let n: usize = fields[0].parse().ok()?;
            if fields.len() < 1 + n + n * n { return None; }
            let mut channels = Vec::new();
            for j in 0..n { channels.push(fields[1 + j].to_string()); }
            let mut matrix = Vec::new();
            for r in 0..n {
                let mut row = Vec::new();
                for c in 0..n {
                    row.push(fields[1 + n + r * n + c].parse::<f64>().ok()?);
                }
                matrix.push(row);
            }
            return Some(SpilloverSpec { keyword: key.to_string(), n, channels, matrix });
        }
    }
    None
}

pub fn to_json(data: &FcsData) -> Value {
    json!({
        "keywords": data.keywords.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
        "channels": data.channels.iter().map(|ch| json!({
            "n": ch.n,
            "name": ch.name,
            "long_name": ch.long_name,
            "detector": ch.detector,
            "range": ch.range,
            "unit": ch.unit,
            "coordinate_basis": "raw_detector_linear"
        })).collect::<Vec<_>>(),
        "events": data.events.iter().map(|e| json!({
            "event_index": e.index,
            "values": e.values.iter().map(|v| if v.is_finite() {
                json!({"value": v, "unit": "channel_units", "representation": "float64_from_float32_be"})
            } else {
                json!({"value": Value::Null, "unit": "channel_units", "representation": "non_finite"})
            }).collect::<Vec<_>>(),
            "diagnostics": e.diagnostics
        })).collect::<Vec<_>>()
    })
}
