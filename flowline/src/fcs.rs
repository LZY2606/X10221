//! Minimal FCS 3.1 reader/writer used by the offline case pack.
//!
//! Design constraints from the case narrative:
//! * the *raw representation* of channel values and metadata is preserved at
//!   import time (normalisation happens only in derived layers);
//! * values that cannot be converted (NaN / outside $PnR) stay in place and
//!   carry diagnostics instead of being dropped to make plots render.

use std::str;

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub index: usize,
    pub name: String,
    pub bits: u32,
    pub range: f64,
    pub amplification: String,
    pub gain: f64,
    pub unit: Option<String>,
    pub long_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spillover {
    /// Header of the SPILL/$SPILL keyword: fluorescent detector order.
    pub detectors: Vec<String>,
    /// Square spillover matrix, row-major, in detector order.
    pub matrix: Vec<Vec<f64>>,
    pub keyword: String,
}

#[derive(Debug, Clone)]
pub struct FcsFile {
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<Channel>,
    /// `events[e][c]` is the *raw* value exactly as decoded from DATA.
    pub events: Vec<Vec<f32>>,
    pub spillover: Option<Spillover>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diag {
    pub event: usize,
    pub channel: usize,
    pub channel_name: String,
    pub code: String,
    pub detail: String,
}

impl FcsFile {
    /// Diagnostics that describe non-convertible values without removing them.
    pub fn diagnostics(&self) -> Vec<Diag> {
        let mut out = Vec::new();
        for (ei, ev) in self.events.iter().enumerate() {
            for (ci, v) in ev.iter().enumerate() {
                let name = self
                    .channels
                    .get(ci)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| format!("P{}", ci + 1));
                if v.is_nan() {
                    out.push(Diag {
                        event: ei,
                        channel: ci,
                        channel_name: name,
                        code: "NAN_RAW".into(),
                        detail: "raw float is NaN; retained at original position".into(),
                    });
                } else if let Some(ch) = self.channels.get(ci) {
                    if *v < 0.0 {
                        out.push(Diag {
                            event: ei,
                            channel: ci,
                            channel_name: name,
                            code: "BELOW_ZERO_RAW".into(),
                            detail: format!("{} < 0 in raw scale (unit: {})", v, ch.unit.clone().unwrap_or_else(|| "channel".into())),
                        });
                    }
                    if ch.range > 0.0 && (*v > ch.range) {
                        out.push(Diag {
                            event: ei,
                            channel: ci,
                            channel_name: name,
                            code: "ABOVE_PNR_RAW".into(),
                            detail: format!("{} exceeds $P{}R {}", v, ci + 1, ch.range),
                        });
                    }
                }
            }
        }
        out
    }

    pub fn keyword(&self, key: &str) -> Option<&str> {
        self.keywords
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn parse_spill(value: &str, keyword: &str) -> Option<Spillover> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() < 2 {
        return None;
    }
    let n: usize = parts[0].trim().parse().ok()?;
    let mut rest = parts.iter().skip(1);
    let mut detectors = Vec::new();
    for _ in 0..n {
        detectors.push(rest.next()?.trim().to_string());
    }
    let mut matrix = Vec::new();
    for _ in 0..n {
        let mut row = Vec::new();
        for _ in 0..n {
            row.push(rest.next()?.trim().parse::<f64>().ok()?);
        }
        matrix.push(row);
    }
    Some(Spillover { detectors, matrix, keyword: keyword.to_string() })
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile, String> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        return Err("not an FCS3.1 file".into());
    }
    let single = |a: usize, b: usize| -> i64 {
        str::from_utf8(&bytes[a..b]).unwrap_or("").trim().parse().unwrap_or(0)
    };
    let mut text_b = single(10, 18);
    let mut text_e = single(18, 26);
    let mut data_b = single(26, 34);
    let mut data_e = single(34, 42);

    // TEXT segment: leading delimiter, pairs, trailing delimiter.
    if text_b <= 0 || text_e <= text_b {
        return Err("missing TEXT offsets".into());
    }
    let delim = bytes[text_b as usize];
    let text = &bytes[(text_b as usize) + 1..text_e as usize];
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == delim {
            if i + 1 < text.len() && text[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            tokens.push(std::mem::take(&mut cur));
        } else {
            cur.push(text[i]);
        }
        i += 1;
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut keywords: Vec<(String, String)> = Vec::new();
    for pair in tokens.chunks_exact(2) {
        keywords.push((
            String::from_utf8_lossy(&pair[0]).to_string(),
            String::from_utf8_lossy(&pair[1]).to_string(),
        ));
    }
    let kw = |k: &str| keywords.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());

    // Offsets that overflow fixed fields live in TEXT ("start,end").
    if let Some(v) = kw("$BEGINDATA") {
        data_b = v.parse().unwrap_or(data_b);
    }
    if let Some(v) = kw("$ENDDATA") {
        data_e = v.parse().unwrap_or(data_e);
    }
    if let Some(v) = kw("$BEGINSTEXT") {
        text_b = v.parse().unwrap_or(text_b);
    }
    if let Some(v) = kw("$ENDSTEXT") {
        text_e = v.parse().unwrap_or(text_e);
    }

    let par: usize = kw("$PAR").and_then(|v| v.parse().ok()).ok_or("missing $PAR")?;
    let tot: usize = kw("$TOT").and_then(|v| v.parse().ok()).ok_or("missing $TOT")?;
    let dtype = kw("$DATATYPE").unwrap_or_else(|| "F".into());
    if dtype != "F" {
        return Err(format!("unsupported $DATATYPE {dtype} (case pack uses F)"));
    }
    let byteord = kw("$BYTEORD").unwrap_or_else(|| "1,2,3,4".into());
    let little = byteord.starts_with('1');

    let mut channels = Vec::new();
    for c in 0..par {
        let n = c + 1;
        let name = kw(&format!("$P{n}N")).unwrap_or_else(|| format!("P{n}"));
        channels.push(Channel {
            index: c,
            name,
            bits: kw(&format!("$P{n}B")).and_then(|v| v.parse().ok()).unwrap_or(32),
            range: kw(&format!("$P{n}R")).and_then(|v| v.parse().ok()).unwrap_or(0.0),
            amplification: kw(&format!("$P{n}E")).unwrap_or_else(|| "0,0".into()),
            gain: kw(&format!("$P{n}G")).and_then(|v| v.parse().ok()).unwrap_or(1.0),
            unit: kw(&format!("$P{n}U")),
            long_name: kw(&format!("$P{n}S")),
        });
    }

    let spillover = kw("$SPILL")
        .and_then(|v| parse_spill(&v, "$SPILL"))
        .or_else(|| kw("SPILL").and_then(|v| parse_spill(&v, "SPILL")));

    let mut events = Vec::with_capacity(tot);
    for e in 0..tot {
        let mut row = Vec::with_capacity(par);
        for c in 0..par {
            let pos = data_b as usize + (e * par + c) * 4;
            if pos + 4 > bytes.len() {
                return Err("DATA segment truncated".into());
            }
            let arr: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
            let bits = if little { u32::from_le_bytes(arr) } else { u32::from_be_bytes(arr) };
            row.push(f32::from_bits(bits));
        }
        events.push(row);
    }

    Ok(FcsFile { keywords, channels, events, spillover })
}

fn escape_delim(s: &str, delim: u8) -> Vec<u8> {
    let d = delim as char;
    s.replace(d, &format!("{d}{d}")).into_bytes()
}

#[derive(Debug, Clone)]
pub struct WriteOpts<'a> {
    pub channels: &'a [Channel],
    pub events: &'a [Vec<f32>],
    pub extra_keywords: &'a [(String, String)],
    pub spillover: Option<&'a Spillover>,
}

pub fn write(opts: WriteOpts<'_>) -> Vec<u8> {
    let delim = b'/';
    let par = opts.channels.len();
    let tot = opts.events.len();

    // DATA first length is known; place TEXT then DATA; offsets fit in 8 digits.
    let data_len = tot * par * 4;
    let mut text: Vec<u8> = vec![delim];
    let mut pair = |k: &str, v: &str| {
        text.extend_from_slice(&escape_delim(k, delim));
        text.push(delim);
        text.extend_from_slice(&escape_delim(v, delim));
        text.push(delim);
    };
    pair("$BEGINANALYSIS", "0");
    pair("$ENDANALYSIS", "0");
    pair("$BEGINSTEXT", "0");
    pair("$ENDSTEXT", "0");
    pair("$TOT", &tot.to_string());
    pair("$PAR", &par.to_string());
    pair("$DATATYPE", "F");
    pair("$BYTEORD", "1,2,3,4");
    pair("$MODE", "L");
    pair("$NEXTDATA", "0");
    pair("$DATATYPE", "F");
    pair("$BEGINDATA", "OFFB");
    pair("$ENDDATA", "OFFE");
    for (i, ch) in opts.channels.iter().enumerate() {
        let n = i + 1;
        pair(&format!("$P{n}B"), &ch.bits.to_string());
        pair(&format!("$P{n}R"), &format_float(ch.range));
        pair(&format!("$P{n}E"), &ch.amplification);
        pair(&format!("$P{n}G"), &format_float(ch.gain));
        pair(&format!("$P{n}N"), &ch.name);
        if let Some(u) = &ch.unit {
            pair(&format!("$P{n}U"), u);
        }
        if let Some(s) = &ch.long_name {
            pair(&format!("$P{n}S"), s);
        }
    }
    for (k, v) in opts.extra_keywords {
        pair(k, v);
    }
    if let Some(sp) = opts.spillover {
        let mut val = format!("{}", sp.detectors.len());
        for d in &sp.detectors {
            val.push(',');
            val.push_str(d);
        }
        for r in &sp.matrix {
            for x in r {
                val.push(',');
                val.push_str(&format_float(*x));
            }
        }
        pair("$SPILL", &val);
    }

    let text_b = 256usize;
    let text_e = text_b + text.len() - 1;
    let data_b = text_e + 1;
    let data_e = data_b + data_len - 1;

    let mut out = Vec::new();
    out.extend_from_slice(b"FCS3.1    ");
    out.extend_from_slice(format!("{:>8}", text_b).as_bytes());
    out.extend_from_slice(format!("{:>8}", text_e).as_bytes());
    out.extend_from_slice(format!("{:>8}", data_b).as_bytes());
    out.extend_from_slice(format!("{:>8}", data_e).as_bytes());
    out.extend_from_slice(b"       0       0");
    while out.len() < text_b {
        out.push(b' ');
    }
    // Patch real offsets into the $BEGINDATA/$ENDDATA pairs.
    let mut patched: Vec<u8> = Vec::with_capacity(text.len());
    let mut toks: Vec<&[u8]> = text.split(|b| *b == delim).collect();
    // split gives leading empty; easier to patch via string replace
    let text_str = String::from_utf8(text).unwrap();
    let text_str = text_str.replace("OFFB", &data_b.to_string()).replace("OFFE", &data_e.to_string());
    let _ = toks;
    patched.extend_from_slice(text_str.as_bytes());
    out.extend_from_slice(&patched);

    for ev in opts.events {
        for v in ev {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn format_float(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
