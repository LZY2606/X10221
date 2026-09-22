//! 最小可用的 FCS 3.0/3.1 读取器。
//! 只做“读取并保留原始表示”：不解补偿、不做变换、不丢弃坏值。

use crate::domain::{ChannelSpec, FcsDataSet, RawEvent};

#[derive(Debug)]
pub struct FcsError(pub String);
impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS 解析失败: {}", self.0)
    }
}
impl std::error::Error for FcsError {}

fn err<T>(msg: impl Into<String>) -> Result<T, FcsError> {
    Err(FcsError(msg.into()))
}

fn header_offsets(bytes: &[u8], segment: &str) -> Option<(usize, usize)> {
    let s = std::str::from_utf8(bytes.get(..58)?).ok()?;
    if !s.starts_with("FCS") {
        return None;
    }
    let (a, b) = match segment {
        "text" => (10, 18),
        "data" => (18, 26),
        _ => return None,
    };
    let begin: usize = s.get(a..a + 8)?.trim().parse().ok()?;
    let end: usize = s.get(b..b + 8)?.trim().parse().ok()?;
    if begin == 0 && end == 0 {
        return None;
    }
    Some((begin, end + 1))
}

fn parse_text_segment(seg: &[u8]) -> Vec<(String, String)> {
    let delim = seg[0];
    let body = &seg[1..];
    let mut kvs = Vec::new();
    let parts: Vec<&[u8]> = split_respecting_escape(body, delim);
    let mut i = 0;
    while i + 1 < parts.len() {
        let key = String::from_utf8_lossy(parts[i]).to_string();
        let val = String::from_utf8_lossy(parts[i + 1]).to_string();
        kvs.push((key, val));
        i += 2;
    }
    kvs
}

fn split_respecting_escape(body: &[u8], delim: u8) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < body.len() {
        if body[i] == delim {
            if i + 1 < body.len() && body[i + 1] == delim {
                i += 2;
                continue;
            }
            out.push(&body[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < body.len() {
        out.push(&body[start..]);
    }
    out
}

fn kw<'a>(kvs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kvs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}

pub fn parse(bytes: &[u8]) -> Result<FcsDataSet, FcsError> {
    let (tb, te) = header_offsets(bytes, "text")
        .ok_or_else(|| FcsError("缺少 TEXT 段偏移".into()))?;
    let text_seg = bytes.get(tb..te).ok_or_else(|| FcsError("TEXT 段越界".into()))?;
    let kvs = parse_text_segment(text_seg);

    let par: usize = kw(&kvs, "$PAR").and_then(|v| v.parse().ok())
        .ok_or_else(|| FcsError("$PAR 缺失".into()))?;
    let tot: usize = kw(&kvs, "$TOT").and_then(|v| v.parse().ok())
        .ok_or_else(|| FcsError("$TOT 缺失".into()))?;
    let mode = kw(&kvs, "$MODE").unwrap_or("L");
    if mode != "L" {
        return err(format!("仅支持 list mode ($MODE=L)，当前 {mode}"));
    }
    let datatype = kw(&kvs, "$DATATYPE").unwrap_or("F");
    let byteord = kw(&kvs, "$BYTEORD").unwrap_or("1,2,3,4");
    let big_endian = byteord.starts_with('4');

    let (db, de) = match header_offsets(bytes, "data") {
        Some(x) => x,
        None => {
            let b: usize = kw(&kvs, "$BEGINDATA").and_then(|v| v.parse().ok())
                .ok_or_else(|| FcsError("DATA 段偏移缺失".into()))?;
            let e: usize = kw(&kvs, "$ENDDATA").and_then(|v| v.parse().ok())
                .ok_or_else(|| FcsError("$ENDDATA 缺失".into()))?;
            (b, e + 1)
        }
    };
    let data = bytes.get(db..de).ok_or_else(|| FcsError("DATA 段越界".into()))?;

    let mut channels = Vec::with_capacity(par);
    for n in 1..=par {
        let (pf, pl) = kw(&kvs, &format!("$P{n}E")).map(|v| {
            let mut it = v.split(',');
            (it.next().unwrap_or("0").to_string(), it.next().unwrap_or("0").to_string())
        }).unwrap_or_default();
        channels.push(ChannelSpec {
            name: kw(&kvs, &format!("$P{n}N")).unwrap_or("?").to_string(),
            detector: kw(&kvs, &format!("$P{n}S")).unwrap_or("?").to_string(),
            fluorophore: None,
            range: kw(&kvs, &format!("$P{n}R")).and_then(|v| v.parse().ok()).unwrap_or(0.0),
            decade: format!("{pf},{pl}"),
        });
    }

    let mut events: Vec<RawEvent> = Vec::with_capacity(tot);
    match datatype {
        "A" => parse_ascii(data, par, tot, &mut events)?,
        "I" => parse_integer(data, par, tot, &kvs, big_endian, &mut events)?,
        "F" | "D" => parse_float(data, par, tot, datatype, big_endian, &mut events)?,
        other => return err(format!("不支持的 $DATATYPE={other}")),
    }

    Ok(FcsDataSet {
        version: String::from_utf8_lossy(&bytes[0..6]).trim().to_string(),
        text_delimiter: (text_seg[0] as char).to_string(),
        keywords: kvs,
        channels,
        events,
    })
}

fn push_values(events: &mut Vec<RawEvent>, ordinal: usize, tokens: Vec<String>, values: Vec<Option<f64>>) {
    events.push(RawEvent { ordinal, values, raw_tokens: tokens });
}

fn token_to_value(tok: &str) -> Option<f64> {
    let t = tok.trim();
    if t.is_empty() { return None; }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_ascii(data: &[u8], par: usize, tot: usize, events: &mut Vec<RawEvent>) -> Result<(), FcsError> {
    let text = String::from_utf8_lossy(data);
    let tokens: Vec<&str> = text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty()).collect();
    if tokens.len() < par * tot {
        return err(format!("ASCII DATA 数值不足: 需要 {}, 实际 {}", par * tot, tokens.len()));
    }
    for e in 0..tot {
        let slice = &tokens[e * par..(e + 1) * par];
        let raw: Vec<String> = slice.iter().map(|s| s.to_string()).collect();
        let vals = raw.iter().map(|t| token_to_value(t)).collect();
        push_values(events, e, raw, vals);
    }
    Ok(())
}

fn bit_widths(par: usize, kvs: &[(String, String)]) -> Result<Vec<usize>, FcsError> {
    let mut out = Vec::with_capacity(par);
    for n in 1..=par {
        let bits: usize = kw(kvs, &format!("$P{n}B")).and_then(|v| v.parse().ok())
            .ok_or_else(|| FcsError(format!("$P{n}B 缺失")))?;
        out.push(bits);
    }
    Ok(out)
}

fn parse_integer(
    data: &[u8], par: usize, tot: usize, kvs: &[(String, String)],
    big_endian: bool, events: &mut Vec<RawEvent>,
) -> Result<(), FcsError> {
    let widths = bit_widths(par, kvs)?;
    let mut off = 0usize;
    for e in 0..tot {
        let mut raw = Vec::with_capacity(par);
        let mut vals = Vec::with_capacity(par);
        for bits in &widths {
            let bytes = bits / 8;
            let chunk = data.get(off..off + bytes).ok_or_else(|| FcsError("I 型 DATA 截断".into()))?;
            off += bytes;
            let mut buf = [0u8; 8];
            if big_endian {
                buf[8 - bytes..].copy_from_slice(chunk);
                let n = i64::from_be_bytes(buf);
                raw.push(format!("I:{n}"));
                vals.push(Some(n as f64));
            } else {
                buf[..bytes].copy_from_slice(chunk);
                let n = i64::from_le_bytes(buf);
                raw.push(format!("I:{n}"));
                vals.push(Some(n as f64));
            }
        }
        push_values(events, e, raw, vals);
    }
    Ok(())
}

fn parse_float(
    data: &[u8], par: usize, tot: usize, dtype: &str,
    big_endian: bool, events: &mut Vec<RawEvent>,
) -> Result<(), FcsError> {
    let bytes_per = if dtype == "D" { 8 } else { 4 };
    let need = par * tot * bytes_per;
    if data.len() < need {
        return err(format!("DATA 字节不足: 需要 {need}, 实际 {}", data.len()));
    }
    for e in 0..tot {
        let mut raw = Vec::with_capacity(par);
        let mut vals = Vec::with_capacity(par);
        for p in 0..par {
            let o = (e * par + p) * bytes_per;
            let v = if dtype == "D" {
                let mut b: [u8; 8] = data[o..o + 8].try_into().unwrap();
                if !big_endian { b.reverse(); }
                f64::from_be_bytes(b)
            } else {
                let mut b: [u8; 4] = data[o..o + 4].try_into().unwrap();
                if !big_endian { b.reverse(); }
                f32::from_be_bytes(b) as f64
            };
            raw.push(format!("{dtype}:{}", fmt_f64(v)));
            vals.push(if v.is_finite() { Some(v) } else { None });
        }
        push_values(events, e, raw, vals);
    }
    Ok(())
}

fn fmt_f64(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ascii_listmode_preserving_bad_token() {
        // 手工构造最小 FCS：ASCII list mode，含一个不可解析标记 OVFL。
        let b = 256usize;
        let data = "1.0,2.0 OVFL,4.0";
        let text = format!(
            "/$BEGINANALYSIS/0/$ENDANALYSIS/0/$BEGINDATA/{b}/$ENDDATA/{e}/$PAR/2/$TOT/2/$MODE/L/$DATATYPE/A/$P1B/32/$P2B/32/$P1N/FL1/$P2N/FL2/$P1R/1024/$P2R/1024/$P1E/0,0/$P2E/0,0",
            e = b + 200 - 1
        );
        let e = b + text.len() - 1;
        let a = e + 1;
        let z = a + data.len() - 1;
        let mut file = vec![0u8; a];
        let hdr = format!("FCS3.1    {b:>8}{e:>8}{a:>8}{z:>8}0       0       ");
        file[..58].copy_from_slice(hdr.as_bytes());
        file[b..=e].copy_from_slice(text.as_bytes());
        file.extend_from_slice(data.as_bytes());

        let ds = parse(&file).unwrap();
        assert_eq!(ds.events.len(), 2);
        assert_eq!(ds.events[0].values, vec![Some(1.0), Some(2.0)]);
        assert_eq!(ds.events[1].values, vec![None, Some(4.0)]);
        assert_eq!(ds.events[1].raw_tokens[0], "OVFL");
        assert_eq!(ds.channels[0].name, "FL1");
    }
}
