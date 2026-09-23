//! Minimal FCS 3.1 reader/writer for the bundled offline case pack.
//!
//! Supports `$DATATYPE` `F`/`D` (float32/float64), big- or little-endian,
//! and the `$SPILLOVER` keyword. Non-finite raw samples (NaN/+-Inf produced
//! by a saturating or failing digitiser) are preserved at their event/channel
//! position with a diagnostic instead of being dropped or clipped.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const DELIM: u8 = b'\\';

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub index: usize, // 1-based FCS parameter number
    pub short_name: String,
    pub long_name: String,
    pub unit: String,
    pub range: f64,
    pub bits: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpilloverKeyword {
    pub labels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FcsFile {
    pub keywords: Vec<(String, String)>,
    pub channels: Vec<ChannelSpec>,
    pub spillover: Option<SpilloverKeyword>,
    /// Raw event rows; NaN / +Inf / -Inf are preserved as f64 values.
    pub events: Vec<Vec<f64>>,
    pub diagnostics: Vec<RawDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawDiagnostic {
    pub event_index: u32,
    pub channel: String,
    pub code: String,
    pub detail: String,
}

fn keyword(kws: &[(String, String)], key: &str) -> Option<String> {
    kws.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile> {
    if bytes.len() < 58 || &bytes[0..6] != b"FCS3.1" {
        bail!("not an FCS 3.1 file");
    }
    let off = |s: &str| -> Result<(usize, usize)> {
        let b: usize = s[0..8].trim().parse().context("bad segment offset")?;
        let e: usize = s[8..16].trim().parse().context("bad segment offset")?;
        Ok((b, e))
    };
    let (tb, te) = off(std::str::from_utf8(&bytes[10..26]).unwrap_or("0       0       "))?;
    let (db, de) = off(std::str::from_utf8(&bytes[26..42]).unwrap_or("0       0       "))?;
    if tb == 0 || te <= tb || db == 0 || de <= db || de > bytes.len() {
        bail!("invalid FCS segment offsets");
    }

    let text = &bytes[tb..=te];
    let pairs = parse_text(text)?;

    let par: usize = keyword(&pairs, "$PAR")
        .context("missing $PAR")?
        .parse()
        .context("bad $PAR")?;
    let total: usize = keyword(&pairs, "$TOT")
        .context("missing $TOT")?
        .parse()
        .context("bad $TOT")?;
    let dtype = keyword(&pairs, "$DATATYPE").context("missing $DATATYPE")?;
    let byteord = keyword(&pairs, "$BYTEORD").context("missing $BYTEORD")?;
    let big_endian = byteord.starts_with("4,3,2,1") || byteord.starts_with("8,7");

    let mut channels = Vec::with_capacity(par);
    for i in 1..=par {
        let short = keyword(&pairs, &format!("$P{}N", i)).unwrap_or_default();
        let long = keyword(&pairs, &format!("$P{}S", i)).unwrap_or_else(|| short.clone());
        let unit = keyword(&pairs, &format!("$P{}U", i)).unwrap_or_else(|| "channel-unit".to_string());
        let range = keyword(&pairs, &format!("$P{}R", i))
            .and_then(|v| v.parse().ok())
            .unwrap_or(1024.0);
        let bits = keyword(&pairs, &format!("$P{}B", i))
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);
        channels.push(ChannelSpec {
            index: i,
            short_name: short,
            long_name: long,
            unit,
            range,
            bits,
        });
    }

    let spillover = keyword(&pairs, "$SPILLOVER").and_then(|v| parse_spillover(&v));

    let data = &bytes[db..=de];
    let need = match dtype.as_str() {
        "F" => total * par * 4,
        "D" => total * par * 8,
        other => bail!("unsupported $DATATYPE `{other}`"),
    };
    if data.len() < need {
        bail!("DATA segment truncated: have {} need {}", data.len(), need);
    }
    let mut diagnostics = Vec::new();
    let mut events = Vec::with_capacity(total);
    for e in 0..total {
        let mut row = Vec::with_capacity(par);
        for c in 0..par {
            let v: f64 = match dtype.as_str() {
                "F" => {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&data[(e * par + c) * 4..(e * par + c + 1) * 4]);
                    if big_endian {
                        f32::from_be_bytes(b)
                    } else {
                        f32::from_le_bytes(b)
                    } as f64
                }
                "D" => {
                    let mut b = [0u8; 8];
                    b.copy_from_slice(&data[(e * par + c) * 8..(e * par + c + 1) * 8]);
                    if big_endian {
                        f64::from_be_bytes(b)
                    } else {
                        f64::from_le_bytes(b)
                    }
                }
                _ => unreachable!(),
            };
            if v.is_nan() || v.is_infinite() {
                let name = channels.get(c).map(|x| x.short_name.clone()).unwrap_or_default();
                let code = if v.is_nan() {
                    "raw-nonfinite-nan"
                } else if v > 0.0 {
                    "raw-nonfinite-posinf"
                } else {
                    "raw-nonfinite-neginf"
                };
                diagnostics.push(RawDiagnostic {
                    event_index: e as u32,
                    channel: name,
                    code: code.to_string(),
                    detail: "kept at source position; excluded from derived layers"
                        .to_string(),
                });
            }
            row.push(v);
        }
        events.push(row);
    }

    Ok(FcsFile {
        keywords: pairs,
        channels,
        spillover,
        events,
        diagnostics,
    })
}

fn parse_text(seg: &[u8]) -> Result<Vec<(String, String)>> {
    let delim = seg[0];
    let body = &seg[1..seg.len() - if seg[seg.len() - 1] == delim { 1 } else { 0 }];
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == delim {
            if i + 1 < body.len() && body[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(body[i]);
        }
        i += 1;
    }
    parts.push(cur);
    if parts.len() % 2 == 1 {
        parts.push(Vec::new());
    }
    Ok(parts
        .chunks_exact(2)
        .map(|kv| {
            (
                String::from_utf8_lossy(&kv[0]).into_owned(),
                String::from_utf8_lossy(&kv[1]).into_owned(),
            )
        })
        .collect())
}

fn parse_spillover(v: &str) -> Option<SpilloverKeyword> {
    let fields: Vec<&str> = v.split(',').map(|s| s.trim()).collect();
    let n: usize = fields.first()?.parse().ok()?;
    if fields.len() < 1 + n + n * n {
        return None;
    }
    let labels = fields[1..1 + n].iter().map(|s| s.to_string()).collect();
    let mut matrix = Vec::with_capacity(n);
    for r in 0..n {
        let mut row = Vec::with_capacity(n);
        for c in 0..n {
            row.push(fields[1 + n + r * n + c].parse().ok()?);
        }
        matrix.push(row);
    }
    Some(SpilloverKeyword { labels, matrix })
}

fn encode_text(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = vec![DELIM];
    for (k, v) in pairs {
        out.extend_from_slice(k.as_bytes());
        out.push(DELIM);
        let escaped = v.replace(DELIM as char, &String::from_utf8(vec![DELIM, DELIM]).unwrap());
        out.extend_from_slice(escaped.as_bytes());
        out.push(DELIM);
    }
    out
}

/// Serialise to FCS 3.1, float32 big-endian DATA segment.
pub fn write(file: &FcsFile) -> Result<Vec<u8>> {
    let par = file.channels.len();
    let total = file.events.len();
    let mut pairs: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTEXT".into(), "0".into()),
        ("$ENDSTEXT".into(), "0".into()),
        ("$NEXTDATA".into(), "0".into()),
        ("$MODE".into(), "L".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "4,3,2,1".into()),
        ("$TOT".into(), total.to_string()),
        ("$PAR".into(), par.to_string()),
        ("$SYS".into(), "flowline-fcsgen".into()),
        ("$DATE".into(), "2026-09-24".into()),
        ("$BTIM".into(), "09:00:00".into()),
    ];
    for ch in &file.channels {
        let p = ch.index;
        pairs.push((format!("$P{}N", p), ch.short_name.clone()));
        pairs.push((format!("$P{}S", p), ch.long_name.clone()));
        pairs.push((format!("$P{}U", p), ch.unit.clone()));
        pairs.push((format!("$P{}R", p), format_number(ch.range)));
        pairs.push((format!("$P{}B", p), ch.bits.to_string()));
        pairs.push((format!("$P{}E", p), "0,0".into()));
    }
    if let Some(sp) = &file.spillover {
        let mut s = format!("{}", sp.labels.len());
        for l in &sp.labels {
            s.push(',');
            s.push_str(l);
        }
        for row in &sp.matrix {
            for v in row {
                s.push(',');
                s.push_str(&format_number(*v));
            }
        }
        pairs.push(("$SPILLOVER".into(), s));
    }
    let mut text = encode_text(&pairs);
    let header_len = 58usize;
    let mut data_begin = header_len + text.len();
    // Pad TEXT so the DATA segment is 4-byte aligned.
    while data_begin % 4 != 0 {
        // insert padding spaces before the trailing delimiter
        let last = text.pop().unwrap_or(DELIM);
        text.push(b' ');
        text.push(last);
        data_begin = header_len + text.len();
    }
    let mut data = Vec::with_capacity(total * par * 4);
    for row in &file.events {
        assert_eq!(row.len(), par);
        for v in row {
            data.extend_from_slice(&(*v as f32).to_be_bytes());
        }
    }
    let data_end = data_begin + data.len() - 1;
    let text_end = header_len + text.len() - 1;
    let mut header = Vec::with_capacity(header_len);
    header.extend_from_slice(b"FCS3.1    ");
    header.extend_from_slice(format!("{:>8}", header_len).as_bytes());
    header.extend_from_slice(format!("{:>8}", text_end).as_bytes());
    header.extend_from_slice(format!("{:>8}", data_begin).as_bytes());
    header.extend_from_slice(format!("{:>8}", data_end).as_bytes());
    header.extend_from_slice(b"       0");
    header.extend_from_slice(b"       0");
    assert_eq!(header.len(), header_len);

    let mut out = Vec::new();
    out.extend(header);
    out.extend(text);
    out.extend(data);
    Ok(out)
}

fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{:.6}", v)
    }
}
