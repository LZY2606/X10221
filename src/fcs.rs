//! Minimal FCS 3.1 reader/writer used by the bundled offline case package.
//!
//! Import rules enforced here:
//! - original byte blob is preserved untouched (no irreversible cleaning);
//! - all measurements carry a coordinate basis / unit from $PnN/$PnE/$PnG;
//! - values that cannot be converted stay in place with diagnostics instead
//!   of being silently dropped for plotting.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelMeta>,
    pub events_f32_le: Vec<u8>,
    pub n_events: usize,
    pub spillover_csv: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChannelMeta {
    pub name: String,
    pub short_name: Option<String>,
    /// raw channel range $PnR; unit/scale basis from $PnE/$PnG
    pub range: i64,
    pub amp: String,
    pub gain: String,
    pub basis: String,
}

pub fn parse(bytes: &[u8]) -> Result<ParsedFcs> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" && &bytes[0..6] != b"FCS3.0" {
        bail!("not an FCS 3.0/3.1 file");
    }
    let delim = bytes[58] as char;
    if delim == ' ' || delim == '\0' {
        bail!("invalid TEXT delimiter");
    }
    let hdr = |a: usize, b: usize| -> String {
        bytes[a..b].iter().copied().take_while(|c| *c != b' ').map(|c| c as char).collect()
    };
    let text_start: usize = hdr(10, 18).parse().unwrap_or(0);
    let text_end: usize = hdr(18, 26).parse().unwrap_or(0);
    let _data_start_hdr: usize = hdr(26, 34).parse().unwrap_or(0);
    if text_end <= text_start || text_end > bytes.len() {
        bail!("bad TEXT segment offsets");
    }
    let text = &bytes[text_start..text_end];
    let tokens = tokenize(text, delim)?;
    let mut kw = Vec::new();
    for pair in tokens.chunks(2) {
        if pair.len() == 2 {
            kw.push((unescape(&pair[0], delim), unescape(&pair[1], delim)));
        }
    }
    let get = |k: &str| kw.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
    let n_par: usize = get("$PAR").ok_or_else(|| anyhow!("missing $PAR"))?.parse()?;
    let n_events: usize = get("$TOT").ok_or_else(|| anyhow!("missing $TOT"))?.parse()?;
    let datatype = get("$DATATYPE").ok_or_else(|| anyhow!("missing $DATATYPE"))?;
    let byteord = get("$BYTEORD").unwrap_or_else(|| "1,2,3,4".to_string());
    if datatype != "F" {
        bail!("only DATATYPE=F (float32) supported by bundled case, got {datatype}");
    }
    if byteord != "1,2,3,4" {
        bail!("only little-endian float32 supported, got {byteord}");
    }
    let mut channels = Vec::new();
    for i in 1..=n_par {
        let name = get(&format!("$P{}N", i)).ok_or_else(|| anyhow!("missing $P{i}N"))?;
        let range: i64 = get(&format!("$P{}R", i)).unwrap_or_else(|| "4096".into()).parse().unwrap_or(4096);
        let amp = get(&format!("$P{}E", i)).unwrap_or_else(|| "0,0".into());
        let gain = get(&format!("$P{}G", i)).unwrap_or_else(|| "1".into());
        let short = get(&format!("$P{}S", i));
        let basis = if amp.starts_with("0,") {
            format!("linear ADC counts / {range}; gain {gain}")
        } else {
            format!("log decade {amp}; gain {gain}")
        };
        channels.push(ChannelMeta { name, short_name: short, range, amp, gain, basis });
    }
    let beg = get("$BEGINDATA").and_then(|s| s.parse().ok());
    let end = get("$ENDDATA").and_then(|s| s.parse().ok());
    let (ds, de) = match (beg, end) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            // header offsets fall back (valid when < 10^8)
            let a: usize = hdr(26, 34).parse().map_err(|_| anyhow!("cannot locate DATA"))?;
            let b: usize = hdr(34, 42).parse().map_err(|_| anyhow!("cannot locate DATA"))?;
            (a, b)
        }
    };
    if de <= ds || de > bytes.len() {
        bail!("bad DATA offsets");
    }
    let data = bytes[ds..de].to_vec();
    if data.len() != n_events * n_par * 4 {
        bail!("DATA length {} != TOT*PAR*4 ({})", data.len(), n_events * n_par * 4);
    }
    let spillover_csv = get("$SPILLOVER");
    Ok(ParsedFcs { keywords: kw, channels, events_f32_le: data, n_events, spillover_csv })
}

pub fn parse_spillover_csv(s: &str) -> Result<(Vec<String>, Vec<Vec<f64>>)> {
    let rows: Vec<Vec<&str>> = s.lines().filter(|l| !l.trim().is_empty()).map(|l| l.split(',').collect()).collect();
    if rows.len() < 2 {
        bail!("$SPILLOVER needs header + >=1 rows");
    }
    let n = rows[0].len();
    if rows.len() != n + 1 {
        bail!("$SPILLOVER must be square: header {} cols, {} data rows", n, rows.len() - 1);
    }
    let labels = rows[0].iter().map(|x| x.trim().to_string()).collect::<Vec<_>>();
    let mut m = Vec::new();
    for r in 1..rows.len() {
        if rows[r].len() != n + 1 {
            bail!("$SPILLOVER row {} has {} cols, expected {}", r, rows[r].len(), n + 1);
        }
        let mut row = Vec::new();
        for c in 1..=n {
            row.push(rows[r][c].trim().parse::<f64>().map_err(|e| anyhow!("bad float: {e}"))?);
        }
        m.push(row);
    }
    Ok((labels, m))
}

fn tokenize(text: &[u8], delim: char) -> Result<Vec<String>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let first = text[0] as char;
    if first != delim {
        bail!("TEXT must begin with delimiter");
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.iter().enumerate().peekable();
    chars.next(); // opening delimiter
    while let Some((_, &c)) = chars.next() {
        if c as char == delim {
            if let Some((_, &c2)) = chars.peek() {
                if *c2 as char == delim {
                    // escaped delimiter (literal)
                    cur.push(delim);
                    chars.next();
                    continue;
                }
            }
            // token boundary
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c as char);
        }
    }
    Ok(out)
}

fn unescape(s: &str, delim: char) -> String {
    s.replace(&format!("{delim}{delim}", delim = delim), &delim.to_string())
}

/// Read float events; non-finite values become diagnostic entries but the
/// events are never removed (all `n*par` positions are returned).
pub fn read_events(p: &ParsedFcs) -> (Vec<Vec<f64>>, Vec<Value>) {
    let n_par = p.channels.len();
    let mut ev = Vec::with_capacity(p.n_events);
    let mut diags = Vec::new();
    for i in 0..p.n_events {
        let mut row = Vec::with_capacity(n_par);
        for j in 0..n_par {
            let k = (i * n_par + j) * 4;
            let f = f32::from_le_bytes([p.events_f32_le[k], p.events_f32_le[k + 1], p.events_f32_le[k + 2], p.events_f32_le[k + 3]]);
            if !f.is_finite() {
                diags.push(json!({
                    "code": "non_finite_raw_value",
                    "event_index": i,
                    "channel": p.channels[j].name,
                    "raw_b32_hex": format!("{:08x}", f.to_bits()),
                    "message": "value cannot be converted for plotting; kept at original position",
                }));
                row.push(f as f64);
            } else {
                row.push(f as f64);
            }
        }
        ev.push(row);
    }
    (ev, diags)
}

#[derive(Debug, Clone)]
pub struct WriteSpec<'a> {
    pub channels: &'a [ChannelMeta],
    pub events_f32_le: &'a [u8],
    pub extra_keywords: &'a [(String, String)],
}

/// Build an FCS 3.1 file. DATA/TEXT offsets fit in the header window.
pub fn write(spec: &WriteSpec) -> Vec<u8> {
    let par = spec.channels.len();
    let tot = spec.events_f32_le.len() / (par * 4);
    let mut pairs: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTEXT".into(), "0".into()),
        ("$ENDSTEXT".into(), "0".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$PAR".into(), par.to_string()),
        ("$TOT".into(), tot.to_string()),
        ("$MODE".into(), "L".into()),
    ];
    for (i, ch) in spec.channels.iter().enumerate() {
        let n = i + 1;
        pairs.push((format!("$P{n}N"), ch.name.clone()));
        pairs.push((format!("$P{n}R"), ch.range.to_string()));
        pairs.push((format!("$P{n}E"), ch.amp.clone()));
        pairs.push((format!("$P{n}G"), ch.gain.clone()));
        if let Some(s) = &ch.short_name {
            pairs.push((format!("$P{n}S"), s.clone()));
        }
    }
    for (k, v) in spec.extra_keywords {
        pairs.push((k.clone(), v.clone()));
    }
    let delim = '/';
    let text = encode_text(&pairs, delim);
    let text_start = 256usize;
    let data_start = text_start + text.len();
    let data_end = data_start + spec.events_f32_le.len();
    // stamp actual offsets into keywords
    for (k, v) in pairs.iter_mut() {
        match k.as_str() {
            "$BEGINDATA" => *v = data_start.to_string(),
            "$ENDDATA" => *v = data_end.to_string(),
            "$BEGINSTEXT" => *v = "0".into(),
            "$ENDSTEXT" => *v = "0".into(),
            "$BEGINANALYSIS" => *v = "0".into(),
            "$ENDANALYSIS" => *v = "0".into(),
            _ => {}
        }
    }
    let text = encode_text(&pairs, delim);
    let data_start = text_start + text.len();
    let data_end = data_start + spec.events_f32_le.len();
    let mut out = Vec::new();
    out.extend_from_slice(b"FCS3.1");
    out.push(b' ');
    let mut h = |v: usize| {
        let s = v.to_string();
        out.extend_from_slice(s.as_bytes());
        while out.len() % 8 != 0 {
            out.push(b' ');
        }
    };
    out.extend_from_slice(b"    "); // 10
    h(text_start); // 10..18
    h(text_start + text.len()); // 18..26
    h(data_start); // 26..34
    h(data_end); // 34..42
    h(0);
    h(0);
    assert_eq!(out.len(), 58);
    while out.len() < text_start {
        out.push(b' ');
    }
    out.extend_from_slice(&text);
    out.extend_from_slice(spec.events_f32_le);
    out
}

fn encode_text(pairs: &[(String, String)], delim: char) -> Vec<u8> {
    let mut t = String::new();
    t.push(delim);
    for (k, v) in pairs {
        t.push_str(&k.replace(delim, &format!("{delim}{delim}")));
        t.push(delim);
        t.push_str(&v.replace(delim, &format!("{delim}{delim}")));
        t.push(delim);
    }
    t.into_bytes()
}
