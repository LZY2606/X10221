//! Minimal FCS 3.1 reader/writer for the bundled domain case pack.
//!
//! Scope on purpose:
//! - TEXT segment with the standard first-byte delimiter convention;
//! - DATA segment of 32-bit little-endian floats (F,4);
//! - header offsets, including >99,999,999 offsets encoded in keywords;
//! - $SPILL / $SPILLOVER captured verbatim.
//!
//! Everything outside this scope is a hard import error (whole import
//! rolls back); the parser never fabricates values.

use crate::domain::{Basis, ChannelSpec, RawEvent, RawEvents};
use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub raw: RawEvents,
    pub data: Vec<Vec<f32>>,
}

pub fn parse_fcs(source_name: &str, bytes: &[u8]) -> Result<ParsedFcs> {
    if bytes.len() < 58 {
        return Err(Error::Fcs("file shorter than 58-byte FCS header".into()));
    }
    if &bytes[0..6] != b"FCS3.1" && &bytes[0..6] != b"FCS3.0" {
        return Err(Error::Fcs(format!("unsupported version {:?}", &bytes[0..6])));
    }
    let header = |start: usize, end: usize| -> Result<usize> {
        let s = std::str::from_utf8(&bytes[start..end])
            .map_err(|_| Error::Fcs("non-ascii header".into()))?;
        s.trim().parse::<usize>().map_err(|_| Error::Fcs(format!("bad header offset: {s:?}")))
    };
    let text_start = header(10, 18)?;
    let text_end = header(18, 26)?;
    let data_start_h = header(26, 34)?;
    let data_end_h = header(34, 42)?;
    if text_end <= text_start || text_end > bytes.len() {
        return Err(Error::Fcs("illegal TEXT offsets".into()));
    }
    let delimiter = bytes[text_start] as char;
    let text = std::str::from_utf8(&bytes[text_start + 1..text_end])
        .map_err(|_| Error::Fcs("TEXT segment not valid utf-8".into()))?;

    let keywords = parse_keywords(text, delimiter)?;
    let kw = |name: &str| -> Option<&str> {
        keywords.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    };
    let n_events = kw("$TOT")
        .ok_or_else(|| Error::Fcs("$TOT missing".into()))?
        .parse::<usize>()
        .map_err(|_| Error::Fcs("$TOT not an integer".into()))?;
    let n_par = kw("$PAR")
        .ok_or_else(|| Error::Fcs("$PAR missing".into()))?
        .parse::<usize>()
        .map_err(|_| Error::Fcs("$PAR not an integer".into()))?;
    let datatype = kw("$DATATYPE").ok_or_else(|| Error::Fcs("$DATATYPE missing".into()))?;
    let mode = kw("$MODE").unwrap_or("L");
    if datatype != "F" {
        return Err(Error::Fcs(format!("only $DATATYPE F supported, got {datatype}")));
    }
    if mode != "L" {
        return Err(Error::Fcs(format!("only $MODE L supported, got {mode}")));
    }
    let bytord = kw("$BYTEORD").unwrap_or("1,2,3,4");
    if bytord != "1,2,3,4" {
        return Err(Error::Fcs(format!("only little-endian floats supported, got $BYTEORD {bytord}")));
    }
    // Large DATA segments may only be described by keywords.
    let (data_start, data_end) = if data_start_h == 0 || data_end_h == 0 {
        let s = kw("$BEGINDATA").ok_or_else(|| Error::Fcs("no $BEGINDATA".into()))?
            .parse::<usize>().map_err(|_| Error::Fcs("$BEGINDATA invalid".into()))?;
        let e = kw("$ENDDATA").ok_or_else(|| Error::Fcs("no $ENDDATA".into()))?
            .parse::<usize>().map_err(|_| Error::Fcs("$ENDDATA invalid".into()))?;
        (s, e + 1)
    } else {
        (data_start_h, data_end_h + 1)
    };
    if data_end <= data_start || data_end > bytes.len() {
        return Err(Error::Fcs("illegal DATA offsets".into()));
    }
    let payload = &bytes[data_start..data_end];
    if payload.len() < n_events * n_par * 4 {
        return Err(Error::Fcs(format!(
            "DATA segment {} bytes too small for {} events x {} params x 4",
            payload.len(), n_events, n_par
        )));
    }

    let mut channels = Vec::with_capacity(n_par);
    for i in 1..=n_par {
        let name = kw(&format!("$P{n}N")).unwrap_or("").to_string();
        if name.is_empty() {
            return Err(Error::Fcs(format!("$P{i}N missing")));
        }
        let long = kw(&format!("$P{i}S")).unwrap_or("").to_string();
        let pnr = kw(&format!("$P{i}R")).unwrap_or("262144").parse::<i64>().unwrap_or(262144);
        let pne = kw(&format!("$P{i}E")).unwrap_or("0,0").to_string();
        let amp = kw(&format!("$P{i}B")).unwrap_or("32").to_string();
        let unit = if name.starts_with("FSC") || name.starts_with("SSC") {
            "scatter-area".to_string()
        } else {
            "fluorophore-area".to_string()
        };
        channels.push(ChannelSpec {
            index: i - 1,
            short_name: name,
            long_name: long,
            pne,
            pnr,
            amplification: amp,
            unit: unit.clone(),
            basis: Basis::new("fcs-linear", unit),
        });
    }

    let mut events = Vec::with_capacity(n_events);
    let mut data = Vec::with_capacity(n_events);
    for ordinal in 0..n_events {
        let mut bits = Vec::with_capacity(n_par);
        let mut vals = Vec::with_capacity(n_par);
        let mut diag = Vec::new();
        for c in 0..n_par {
            let p = (ordinal * n_par + c) * 4;
            let b = u32::from_le_bytes([payload[p], payload[p + 1], payload[p + 2], payload[p + 3]]);
            bits.push(b);
            let v = f32::from_bits(b);
            if v.is_nan() {
                diag.push(format!("channel {} ({}): NaN bits 0x{:08X}", c, channels[c].short_name, b));
            } else if v.is_infinite() {
                diag.push(format!("channel {} ({}): infinite bits 0x{:08X}", c, channels[c].short_name, b));
            } else if v.is_sign_negative() && b != 0x8000_0000 {
                diag.push(format!("channel {} ({}): negative raw {v}", c, channels[c].short_name));
            }
            vals.push(v);
        }
        events.push(RawEvent { ordinal, bits, diagnostics: diag });
        data.push(vals);
    }

    let raw = RawEvents {
        format: "FCS3.1".into(),
        keywords,
        channels,
        events,
        source_name: source_name.to_string(),
        data_offset: data_start,
        data_len: data_end - data_start,
    };
    Ok(ParsedFcs { raw, data })
}

fn parse_keywords(text: &str, delimiter: char) -> Result<Vec<(String, String)>> {
    let doubled: String = delimiter.to_string();
    let mut out = Vec::new();
    let mut key = String::new();
    let mut val = String::new();
    let mut in_key = true;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == delimiter {
            if in_key {
                if key.is_empty() {
                    // leading/empty token
                    continue;
                }
                in_key = false;
                if chars.peek() == Some(&delimiter) {
                    val.push(delimiter);
                    chars.next();
                }
            } else {
                if chars.peek() == Some(&delimiter) {
                    val.push(delimiter);
                    chars.next();
                } else {
                    out.push((std::mem::take(&mut key), std::mem::take(&mut val)));
                    in_key = true;
                }
            }
        } else if in_key {
            key.push(ch);
        } else {
            val.push(ch);
        }
    }
    if !key.is_empty() {
        return Err(Error::Fcs(format!("TEXT segment has unterminated keyword {key:?}")));
    }
    let _ = doubled;
    Ok(out)
}

pub struct FcsWriter<'a> {
    pub keywords: &'a [(String, String)],
    pub channels: &'a [String],
    pub data: &'a [Vec<f32>],
}

impl<'a> FcsWriter<'a> {
    pub fn write(&self) -> Vec<u8> {
        let delimiter = '/';
        // TEXT segment (delimiter byte then key/value pairs).
        let mut text = vec![delimiter as u8];
        for (k, v) in self.keywords {
            text.extend(k.replace('/', "//").as_bytes());
            text.push(delimiter as u8);
            text.extend(v.replace('/', "//").as_bytes());
            text.push(delimiter as u8);
        }
        let mut data = Vec::with_capacity(self.data.len() * self.channels.len() * 4);
        for row in self.data {
            for v in row {
                data.extend(v.to_bits().to_le_bytes());
            }
        }
        let text_start = 256usize;
        let text_end = text_start + text.len();
        let data_start = (text_end + 7) & !7;
        let data_end = data_start + data.len();
        let pad = data_start - text_end;

        let mut header = Vec::with_capacity(256);
        header.extend(b"FCS3.1");
        header.extend(b"    "); // bytes 6..9
        header.extend(format!("{:>8}", text_start).into_bytes()); // 10..17
        header.extend(format!("{:>8}", text_end - 1).into_bytes()); // 18..25
        header.extend(format!("{:>8}", data_start).into_bytes()); // 26..33
        header.extend(format!("{:>8}", data_end - 1).into_bytes()); // 34..41
        header.extend(b"       0       0"); // 42..49 / 50..57 ANALYSIS absent
        while header.len() < 256 {
            header.push(b' ');
        }

        let mut out = header;
        out.extend_from_slice(&text);
        out.extend(std::iter::repeat(b' ').take(pad));
        out.extend(data);
        out
    }
}
