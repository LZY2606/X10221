//! Minimal FCS 3.1 reader/writer (list mode, $DATATYPE F/D).
//!
//! The parser is intentionally narrow but faithful to FCS 3.1: it reads the
//! 72-byte HEADER, the delimited TEXT segment and the DATA segment, keeping the
//! raw bytes and raw keyword spelling. Float NaN/Inf bit patterns survive the
//! round trip instead of becoming zeros or being dropped.

use crate::domain::{Diagnostic, Signal};

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub raw_header: Vec<u8>,
    pub raw_text: Vec<u8>,
    pub keywords: Vec<(String, String)>,
    pub par: usize,
    pub tot: usize,
    pub data_type: String,
    /// f64 values for 'F' (f32) and 'D' (f64); flattened row major.
    pub values: Vec<f64>,
    /// Canonical tokens for each value (bit-accurate for floats).
    pub tokens: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl FcsFile {
    pub fn keyword(&self, name: &str) -> Option<&str> {
        self.keywords
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn parse_usize(s: &str) -> Option<usize> {
    s.trim().parse::<usize>().ok()
}

pub fn parse_fcs(bytes: &[u8]) -> Result<FcsFile, String> {
    if bytes.len() < 72 {
        return Err("FCS file shorter than 72 byte header".into());
    }
    let version = String::from_utf8_lossy(&bytes[0..6]).to_string();
    if !version.starts_with("FCS") {
        return Err(format!("not an FCS file (magic {:?})", &version));
    }
    let field = |start: usize| String::from_utf8_lossy(&bytes[start..start + 8]).trim().to_string();
    let (text_s, text_e) = (field(10), field(18));
    let (data_s, data_e) = (field(26), field(34));

    let text_start = parse_usize(&text_s).ok_or("bad TEXT start")?;
    let text_end = parse_usize(&text_e).ok_or("bad TEXT end")?;
    if text_end >= bytes.len() || text_start > text_end {
        return Err("DATA-less FCS files are not supported".into());
    }
    let raw_text = bytes[text_start..=text_end].to_vec();
    if raw_text.is_empty() {
        return Err("empty TEXT segment".into());
    }
    let delim = raw_text[0];
    let mut keywords = Vec::new();
    let parts: Vec<&[u8]> = raw_text.split(|b| *b == delim).collect();
    let mut i = 1;
    while i + 1 < parts.len() {
        let k = String::from_utf8_lossy(parts[i]).to_string();
        let v = String::from_utf8_lossy(parts[i + 1]).to_string();
        if !k.is_empty() {
            keywords.push((k, v));
        }
        i += 2;
    }

    let get_kw = |name: &str| keywords.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let mut diagnostics = Vec::new();

    let par = get_kw("$PAR")
        .and_then(|v| parse_usize(&v))
        .ok_or("missing $PAR")?;
    let tot = get_kw("$TOT")
        .and_then(|v| parse_usize(&v))
        .ok_or("missing $TOT")?;
    let data_type = get_kw("$DATATYPE").unwrap_or_else(|| "F".into());
    let byteord = get_kw("$BYTEORD").unwrap_or_else(|| "1,2,3,4".into());
    let little = match byteord.as_str() {
        "1,2,3,4" => true,
        "4,3,2,1" => false,
        other => {
            diagnostics.push(Diagnostic {
                code: "fcs.byteord.unsupported".into(),
                message: format!("unsupported $BYTEORD {:?}, assuming little endian", other),
                raw_repr: other,
            });
            true
        }
    };
    let mode = get_kw("$MODE").unwrap_or_else(|mode| mode.to_string());
    if mode != "L" {
        return Err(format!("only list mode ($MODE=L) supported, got {:?}", mode));
    }

    let mut ds = parse_usize(&data_s);
    let mut de = parse_usize(&data_e);
    if let Some(b) = get_kw("$BEGINDATA").and_then(|v| parse_usize(&v)) {
        ds = Some(b);
    }
    if let Some(e) = get_kw("$ENDDATA").and_then(|v| parse_usize(&v)) {
        de = Some(e);
    }
    let (data_start, data_end) = match (ds, de) {
        (Some(s), Some(e)) => (s, e),
        _ => return Err("DATA segment offsets unavailable".into()),
    };
    if data_end >= bytes.len() || data_start > data_end {
        return Err("DATA segment out of range".into());
    }
    let data = &bytes[data_start..=data_end];

    let mut values = Vec::with_capacity(tot * par);
    let mut tokens = Vec::with_capacity(tot * par);
    match data_type.as_str() {
        "F" => {
            let need = tot * par * 4;
            if data.len() < need {
                return Err(format!(
                    "DATA segment {} bytes shorter than required {} bytes",
                    data.len(),
                    need
                ));
            }
            for n in 0..tot * par {
                let bits = u32::from_le_bytes([
                    data[n * 4],
                    data[n * 4 + 1],
                    data[n * 4 + 2],
                    data[n * 4 + 3],
                ]);
                let bits = if little { bits } else { bits.swap_bytes() };
                let f32v = f32::from_bits(bits);
                values.push(f64::from(f32v));
                tokens.push(format!("f32:0x{:08x}", bits));
            }
        }
        "D" => {
            let need = tot * par * 8;
            if data.len() < need {
                return Err(format!(
                    "DATA segment {} bytes shorter than required {} bytes",
                    data.len(),
                    need
                ));
            }
            for n in 0..tot * par {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&data[n * 8..n * 8 + 8]);
                if !little {
                    arr.reverse();
                }
                let bits = u64::from_le_bytes(arr);
                let f = f64::from_bits(bits);
                values.push(f);
                tokens.push(format!("f64:0x{:016x}", bits));
            }
        }
        other => {
            return Err(format!("unsupported $DATATYPE {:?} (only F and D)", other));
        }
    }

    for &v in &values {
        let s = Signal::new(v);
        if let Signal::Finite(f) = s {
            if f.abs() > 1e30 {
                diagnostics.push(Diagnostic {
                    code: "fcs.signal.extreme".into(),
                    message: format!("extreme finite signal {} retained at raw position", f),
                    raw_repr: format!("{}", f),
                });
            }
        }
    }

    Ok(FcsFile {
        raw_header: bytes[0..72].to_vec(),
        raw_text,
        keywords,
        par,
        tot,
        data_type,
        values,
        tokens,
        diagnostics,
    })
}

/// Write an FCS 3.1 file with little-endian float32 list-mode data.
pub fn write_fcs31_float(
    channels: &[(String, String)],
    values_flat: &[f32],
    extra_keywords: &[(String, String)],
) -> Vec<u8> {
    let par = channels.len();
    let tot = if par == 0 { 0 } else { values_flat.len() / par };
    let delim = b'|';
    let mut text: Vec<u8> = Vec::new();
    let mut pair = |k: &str, v: &str, text: &mut Vec<u8>| {
        text.push(delim);
        text.extend_from_slice(k.as_bytes());
        text.push(delim);
        text.extend_from_slice(v.as_bytes());
    };
    pair("FCS3.1", " ", &mut text);
    pair("$BEGINANALYSIS", "0", &mut text);
    pair("$ENDANALYSIS", "0", &mut text);
    pair("$BEGINSTEXT", "0", &mut text);
    pair("$ENDSTEXT", "0", &mut text);
    pair("$BYTEORD", "1,2,3,4", &mut text);
    pair("$DATATYPE", "F", &mut text);
    pair("$MODE", "L", &mut text);
    pair("$NEXTDATA", "0", &mut text);
    pair("$PAR", &par.to_string(), &mut text);
    pair("$TOT", &tot.to_string(), &mut text);
    for (i, (short, long)) in channels.iter().enumerate() {
        let n = i + 1;
        pair(&format!("$P{}N", n), short, &mut text);
        pair(&format!("$P{}S", n), long, &mut text);
        pair(&format!("$P{}B", n), "32", &mut text);
        pair(&format!("$P{}E", n), "0,0", &mut text);
        pair(&format!("$P{}R", n), "262144", &mut text);
    }
    for (k, v) in extra_keywords {
        pair(k, v, &mut text);
    }
    // BEGINDATA/ENDDATA values are fixed-width, so the final TEXT length is
    // known before we compute the DATA offsets.
    pair("$BEGINDATA", "00000000", &mut text);
    pair("$ENDDATA", "00000000", &mut text);
    text.push(delim);

    let text_start = 72usize;
    let text_end = text_start + text.len() - 1;
    let data_start = text_end + 1;
    let data_end = if tot * par == 0 {
        data_start
    } else {
        data_start + values_flat.len() * 4 - 1
    };
    let replace_num = |text: &mut Vec<u8>, key: &str, val: usize| {
        let needle = format!("{}{}{}", delim as char, key, delim as char);
        let pos = text
            .windows(needle.len())
            .position(|w| w == needle.as_bytes())
            .unwrap()
            + needle.len();
        let end = pos + 8;
        let rendered = format!("{:0>8}", val);
        assert_eq!(rendered.len(), 8);
        text[pos..end].copy_from_slice(rendered.as_bytes());
    };
    replace_num(&mut text, "$BEGINDATA", data_start);
    replace_num(&mut text, "$ENDDATA", data_end);

    let mut out = Vec::new();
    let f = |n: usize| format!("{:>8}", n);
    let mut header = String::from("FCS3.1    ");
    header.push_str(&f(text_start));
    header.push_str(&f(text_end));
    header.push_str(&f(data_start));
    header.push_str(&f(data_end));
    header.push_str(&f(0));
    header.push_str(&f(0));
    assert_eq!(header.len(), 72, "header must be exactly 72 bytes");
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&text);
    for v in values_flat {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}
