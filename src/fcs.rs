//! Minimal FCS 3.1 reader/writer for the bundled offline case package.
//!
//! Only the subset needed by the flow-boundary-chamber case is supported:
//! 32-bit floating point little/big endian DATA segments ($DATATYPE F),
//! list-mode events ($MODE L), the TEXT keyword segment with escape doubling,
//! and $SPILLOVER metadata. Raw byte representations of keywords and values
//! are preserved (no normalization at import).

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct FcsFile {
    pub version: String,
    pub text_start: usize,
    pub text_end: usize,
    pub data_start: usize,
    pub data_end: usize,
    pub keywords: Vec<(String, String)>,
    pub events: Vec<Vec<f32>>,
}

impl FcsFile {
    pub fn keyword(&self, name: &str) -> Option<&str> {
        let up = name.to_ascii_uppercase();
        self.keywords
            .iter()
            .find(|(k, _)| k.to_ascii_uppercase() == up)
            .map(|(_, v)| v.as_str())
    }

    pub fn channel_names(&self) -> Vec<String> {
        let n: usize = self
            .keyword("$PAR")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        (1..=n)
            .map(|i| {
                self.keyword(&format!("$P{}N", i))
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    pub fn channel_units(&self) -> Vec<Option<String>> {
        let n = self.channel_names().len();
        (1..=n)
            .map(|i| {
                self.keyword(&format!("$P{}E", i)).map(|e| {
                    // $PnE "0,0" marks a linear/raw acquisition channel; carry it through.
                    format!("E={e}")
                })
            })
            .collect()
    }
}

fn parse_text_segment(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    if bytes.is_empty() {
        bail!("empty TEXT segment");
    }
    let delim = bytes[0];
    let body = &bytes[1..];
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        if b == delim {
            // Doubled delimiter is an escaped literal delimiter.
            if i + 1 < body.len() && body[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            parts.push(std::mem::take(&mut cur));
            i += 1;
        } else {
            cur.push(b);
            i += 1;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    if parts.len() % 2 != 0 {
        bail!("TEXT segment has an odd number of tokens");
    }
    let mut kws = Vec::new();
    for pair in parts.chunks_exact(2) {
        let key = String::from_utf8_lossy(&pair[0]).to_string();
        let val = String::from_utf8_lossy(&pair[1]).to_string();
        kws.push((key, val));
    }
    Ok(kws)
}

pub fn parse(data: &[u8]) -> Result<FcsFile> {
    if data.len() < 58 {
        bail!("file shorter than FCS header (58 bytes)");
    }
    let version = String::from_utf8_lossy(&data[0..6]).trim().to_string();
    if !version.starts_with("FCS3.") {
        bail!("unsupported FCS version: {version}");
    }
    let off = |start: usize, end: usize| -> Result<usize> {
        let s = String::from_utf8_lossy(&data[start..end]).trim().to_string();
        s.parse::<usize>()
            .with_context(|| format!("invalid header offset at {start}..{end}: {s:?}"))
    };
    let text_start = off(10, 18)?;
    let text_end = off(18, 26)?;
    let data_start = off(26, 34)?;
    let data_end = off(34, 42)?;
    if text_end < text_start || text_end > data.len() {
        bail!("TEXT offsets out of range");
    }
    let keywords = parse_text_segment(&data[text_start..=text_end.min(data.len() - 1)])?;
    let fcs = FcsFile {
        version,
        text_start,
        text_end,
        data_start,
        data_end,
        keywords,
        events: Vec::new(),
    };
    let par: usize = fcs
        .keyword("$PAR")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow!("missing/invalid $PAR"))?;
    let mode = fcs.keyword("$MODE").unwrap_or("L").trim().to_string();
    if mode != "L" {
        bail!("only list-mode ($MODE L) supported, got {mode}");
    }
    let datatype = fcs.keyword("$DATATYPE").unwrap_or("F").trim().to_string();
    if datatype != "F" {
        bail!("only $DATATYPE F supported, got {datatype}");
    }
    let byteord = fcs.keyword("$BYTEORD").unwrap_or("1,2,3,4").trim().to_string();
    let big_endian = byteord.starts_with("4");
    let total: usize = fcs
        .keyword("$TOT")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| anyhow!("missing/invalid $TOT"))?;
    let bytes_per: usize = fcs
        .keyword("$BEGINDATA")
        .map(|_| 4)
        .unwrap_or(4);
    let _ = fcs.keyword("$ENDDATA");
    let expected_len = total * par * 4;
    let seg_end = if data_end > data_start && data_end - data_start + 1 >= expected_len {
        data_start + expected_len
    } else {
        // Offsets beyond 99,999,999 use $BEGINDATA/$ENDDATA keywords.
        let begin = fcs
            .keyword("$BEGINDATA")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(data_start);
        begin + expected_len
    };
    let _ = bytes_per;
    if seg_end > data.len() {
        bail!("DATA segment exceeds file length");
    }
    let seg = &data[data_start..seg_end];
    let mut events = Vec::with_capacity(total);
    for ev in 0..total {
        let mut vals = Vec::with_capacity(par);
        for p in 0..par {
            let idx = (ev * par + p) * 4;
            let raw: [u8; 4] = seg[idx..idx + 4].try_into().unwrap();
            let v = if big_endian {
                f32::from_be_bytes(raw)
            } else {
                f32::from_le_bytes(raw)
            };
            vals.push(v);
        }
        events.push(vals);
    }
    Ok(FcsFile { events, ..fcs })
}

/// Serialize an FCS 3.1 file (little-endian float list mode). Used by the
/// offline case generator so the repository ships real domain artifacts.
pub fn write(
    channel_names: &[String],
    channel_e: &[String],
    events: &[Vec<f32>],
    extra_keywords: &[(String, String)],
) -> Vec<u8> {
    let par = channel_names.len();
    let tot = events.len();
    let mut kws: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTEXT".into(), "0".into()),
        ("$ENDSTEXT".into(), "0".into()),
        ("$MODE".into(), "L".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$PAR".into(), par.to_string()),
        ("$TOT".into(), tot.to_string()),
    ];
    for (i, name) in channel_names.iter().enumerate() {
        let n = i + 1;
        kws.push((format!("$P{n}N"), name.clone()));
        kws.push((format!("$P{n}B"), "32".into()));
        kws.push((format!("$P{n}E"), channel_e.get(i).cloned().unwrap_or("0,0".into())));
        kws.push((format!("$P{n}R"), "262144".into()));
    }
    kws.extend(extra_keywords.iter().cloned());
    let delim = b'\\';
    let mut text = vec![delim];
    for (k, v) in &kws {
        text.extend(k.replace('\\', "\\\\").as_bytes());
        text.push(delim);
        text.extend(v.replace('\\', "\\\\").as_bytes());
        text.push(delim);
    }
    // text is padded to start data on an 8-byte boundary (after 58-byte header).
    let header_len = 58usize;
    let data_start = (header_len + text.len() + 7) & !7;
    let data_end = data_start + tot * par * 4 - 1;
    // Rewrite BEGINDATA/ENDDATA inside the already built text is unnecessary
    // because we add them now; recompute text with offsets included.
    text.clear();
    kws.push(("$BEGINDATA".into(), data_start.to_string()));
    kws.push(("$ENDDATA".into(), data_end.to_string()));
    text.push(delim);
    for (k, v) in &kws {
        text.extend(k.replace('\\', "\\\\").as_bytes());
        text.push(delim);
        text.extend(v.replace('\\', "\\\\").as_bytes());
        text.push(delim);
    }
    let text_start = header_len;
    let padded_data_start = (header_len + text.len() + 7) & !7;
    // Offsets shifted because BEGINDATA keyword values are fixed width-ish;
    // rebuild once more with final positions.
    let data_start = padded_data_start;
    let data_end = data_start + tot * par * 4 - 1;
    text.clear();
    kws.pop();
    kws.pop();
    kws.push(("$BEGINDATA".into(), data_start.to_string()));
    kws.push(("$ENDDATA".into(), data_end.to_string()));
    text.push(delim);
    for (k, v) in &kws {
        text.extend(k.replace('\\', "\\\\").as_bytes());
        text.push(delim);
        text.extend(v.replace('\\', "\\\\").as_bytes());
        text.push(delim);
    }
    let text_end = header_len + text.len() - 1;
    let mut out = Vec::new();
    let header = format!(
        "FCS3.1  {text_start:>8}{text_end:>8}{data_start:>8}{data_end:>8}{:>8}{:>8}",
        0, 0
    );
    assert_eq!(header.len(), 58, "FCS header must be 58 bytes");
    out.extend(header.as_bytes());
    out.extend_from_slice(&text);
    while out.len() < data_start {
        out.push(b'0');
    }
    for ev in events {
        for v in ev {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Parse the FCS 3.1 `$SPILLOVER` keyword: `n,name,...,name,row..`.
/// Returns channel labels and the spillover matrix S (S[i][j] = fraction of
/// detector i signal originating from fluor j). Compensation solves S x = y.
pub fn parse_spillover(value: &str) -> Result<(Vec<String>, Vec<Vec<f64>>)> {
    let fields: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
    if fields.len() < 2 {
        bail!("$SPILLOVER too short");
    }
    let n: usize = fields[0]
        .parse()
        .with_context(|| format!("bad $SPILLOVER dimension: {}", fields[0]))?;
    if fields.len() < 1 + n + n * n {
        bail!("$SPILLOVER truncated");
    }
    let labels = fields[1..1 + n].iter().map(|s| s.to_string()).collect();
    let mut mat = Vec::with_capacity(n);
    for i in 0..n {
        let mut row = Vec::with_capacity(n);
        for j in 0..n {
            row.push(
                fields[1 + n + i * n + j]
                    .parse::<f64>()
                    .context("bad $SPILLOVER number")?,
            );
        }
        mat.push(row);
    }
    Ok((labels, mat))
}

pub fn format_spillover(labels: &[String], mat: &[Vec<f64>]) -> String {
    let n = labels.len();
    let mut s = format!("{n}");
    for l in labels {
        s.push(',');
        s.push_str(l);
    }
    for row in mat {
        for v in row {
            s.push(',');
            s.push_str(&format!("{v:.6}"));
        }
    }
    s
}
