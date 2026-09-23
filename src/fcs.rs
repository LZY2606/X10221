// 最小 FCS 3.1 解析器：真实解析文件头与 TEXT 段、DATA 段（浮点/整数，大小端）。
// 原始 token 逐事件逐通道保留；无法转换（NaN/Inf/越界 token）的值留在原位置并附诊断，
// 绝不为了画图丢掉。

use crate::model::{ChannelValue, RawEvent, RawEvents};

#[derive(Debug, Clone)]
pub struct FcsHeader {
    pub version: String,
    pub text_begin: usize,
    pub text_end: usize,
    pub data_begin: usize,
    pub data_end: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedFcs {
    pub header: FcsHeader,
    pub keywords: Vec<(String, String)>,
    pub raw: RawEvents,
    /// 原始字节指纹（$BEGINSTIMULUS 等所有字节可继续读取的证据）。
    pub bytes_fingerprint: String,
}

#[derive(Debug)]
pub struct FcsError(pub String);
impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS parse error: {}", self.0)
    }
}
impl std::error::Error for FcsError {}

fn parse_usize(s: &str) -> Result<usize, FcsError> {
    s.trim().parse::<usize>().map_err(|_| FcsError(format!("bad integer: {s:?}")))
}

/// 解析 TEXT 段：分隔符为首字节，键值成对出现；支持重复分隔符转义。
fn parse_text(text: &[u8]) -> Result<Vec<(String, String)>, FcsError> {
    if text.is_empty() {
        return Err(FcsError("empty TEXT segment".into()));
    }
    let delim = text[0];
    let body = &text[1..];
    let mut parts: Vec<String> = Vec::new();
    let mut cur = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == delim {
            if i + 1 < body.len() && body[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            parts.push(String::from_utf8_lossy(&cur).to_string());
            cur.clear();
        } else {
            cur.push(body[i]);
        }
        i += 1;
    }
    if !cur.is_empty() {
        parts.push(String::from_utf8_lossy(&cur).to_string());
    }
    let mut kvs = Vec::new();
    let mut k = None;
    for p in parts {
        match k.take() {
            None => k = Some(p),
            Some(key) => kvs.push((key, p)),
        }
    }
    Ok(kvs)
}

fn kw<'a>(kvs: &'a [(String, String)], key: &str) -> Result<&'a str, FcsError> {
    kvs.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| FcsError(format!("missing keyword {key}")))
}

pub fn parse_fcs(bytes: &[u8], source_file: &str) -> Result<ParsedFcs, FcsError> {
    if bytes.len() < 58 || &bytes[0..3] != b"FCS" {
        return Err(FcsError("not an FCS file".into()));
    }
    let version = String::from_utf8_lossy(&bytes[3..6]).trim().to_string();
    let off = |start: usize| -> Result<usize, FcsError> { parse_usize(std::str::from_utf8(&bytes[start..start + 8]).map_err(|_| FcsError("header utf8".into()))?) };
    let header = FcsHeader {
        version,
        text_begin: off(10)?,
        text_end: off(18)?,
        data_begin: off(26)?,
        data_end: off(34)?,
    };
    let text = &bytes[header.text_begin..=header.text_end];
    let kvs = parse_text(text)?;

    let mode = kw(&kvs, "$MODE")?;
    if mode != "L" {
        return Err(FcsError(format!("unsupported $MODE {mode}")));
    }
    let par: usize = kw(&kvs, "$PAR")?.trim().parse().map_err(|_| FcsError("bad $PAR".into()))?;
    let datatype = kw(&kvs, "$DATATYPE")?;
    let byteord = kw(&kvs, "$BYTEORD")?;
    let big_endian = byteord.starts_with("2,1");
    let n_bytes_param: Vec<usize> = (1..=par)
        .map(|i| {
            kw(&kvs, &format!("$P{i}B"))?.trim().parse::<usize>().map_err(|_| FcsError(format!("bad $P{i}B")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let amplification: Vec<(f64, f64)> = (1..=par)
        .map(|i| {
            let e = kw(&kvs, &format!("$P{i}E")).unwrap_or("0,0");
            let mut it = e.split(',');
            let f = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0.0);
            let g = it.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0.0);
            (f, g)
        })
        .collect();
    let resolution: Vec<f64> = (1..=par)
        .map(|i| kw(&kvs, &format!("$P{i}R")).ok().and_then(|x| x.trim().parse().ok()).unwrap_or(0.0))
        .collect();
    let names: Vec<String> = (1..=par)
        .map(|i| kw(&kvs, &format!("$P{i}N")).map(|s| s.to_string()).unwrap_or_else(|_| format!("P{i}")))
        .collect();
    let labels: Vec<String> = (1..=par)
        .map(|i| kw(&kvs, &format!("$P{i}S")).map(|s| s.to_string()).unwrap_or_default())
        .collect();
    let total: usize = kw(&kvs, "$TOT")?.trim().parse().map_err(|_| FcsError("bad $TOT".into()))?;

    let data = &bytes[header.data_begin..=header.data_end];
    let mut events: Vec<RawEvent> = Vec::with_capacity(total);
    let mut cursor = 0usize;
    let read_num = |buf: &[u8], bytes_per: usize, signed: bool| -> f64 {
        match bytes_per {
            4 if datatype == "F" => {
                let mut a = [0u8; 4];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                f32::from_le_bytes(a) as f64
            }
            8 if datatype == "D" => {
                let mut a = [0u8; 8];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                f64::from_le_bytes(a)
            }
            1 if !signed => buf[0] as f64,
            1 => buf[0] as i8 as f64,
            2 if !signed => {
                let mut a = [0u8; 2];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                u16::from_le_bytes(a) as f64
            }
            2 => {
                let mut a = [0u8; 2];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                i16::from_le_bytes(a) as f64
            }
            4 if !signed => {
                let mut a = [0u8; 4];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                u32::from_le_bytes(a) as f64
            }
            4 => {
                let mut a = [0u8; 4];
                a.copy_from_slice(buf);
                if big_endian { a.reverse(); }
                i32::from_le_bytes(a) as f64
            }
            _ => f64::NAN,
        }
    };
    let signed = matches!(datatype, "I") && amplification.iter().any(|(f, _)| *f < 0.0);
    for idx in 0..total {
        let mut channels = Vec::with_capacity(par);
        for p in 0..par {
            let nb = n_bytes_param[p];
            if cursor + nb > data.len() {
                return Err(FcsError(format!("DATA truncated at event {idx} param {p}")));
            }
            let buf = &data[cursor..cursor + nb];
            cursor += nb;
            let num = read_num(buf, nb, signed);
            let token = if matches!(datatype, "F" | "D") {
                format_num_token(num)
            } else {
                format!("{}", num as i64)
            };
            let mut diags = Vec::new();
            if !num.is_finite() {
                diags.push("non-finite value preserved at origin; cannot normalize".into());
            }
            if resolution[p] > 0.0 && num.is_finite() && (num < 0.0 || num >= resolution[p]) {
                diags.push(format!("value outside $P{}R range [0,{})", p + 1, resolution[p] as i64));
            }
            let unit = if matches!(datatype, "F" | "D") { "fcs_float" } else { "adc_count" }.to_string();
            let basis = format!(
                "FCS{} ${}B={} ${}E={},{} ${}R={} $BYTEORD={}",
                header.version,
                p + 1, nb, p + 1, amplification[p].0 as i64, amplification[p].1 as i64,
                p + 1, resolution[p] as i64, byteord
            );
            channels.push(ChannelValue {
                channel: names[p].clone(),
                raw_token: token,
                value: if num.is_finite() { Some(num) } else { None },
                unit,
                basis,
                diagnostics: diags,
            });
        }
        events.push(RawEvent { index: idx as u32, channels });
    }

    let _ = labels;
    Ok(ParsedFcs {
        header,
        keywords: kvs,
        raw: RawEvents { source_file: source_file.to_string(), events },
        bytes_fingerprint: crate::hash::fingerprint_bytes(bytes),
    })
}

fn format_num_token(n: f64) -> String {
    if n.is_nan() { "NaN".into() } else if n.is_infinite() { if n > 0.0 { "Inf".into() } else { "-Inf".into() } } else { format!("{n:.6E}") }
}

/// 从 FCS 关键词中解析 $SPILL（n,ch1..chn,n*n 个逗号分隔数）。
pub fn parse_spill_keyword(value: &str) -> Option<crate::model::SpilloverMeta> {
    let fields: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
    if fields.is_empty() {
        return None;
    }
    let n: usize = fields[0].parse().ok()?;
    if fields.len() < 1 + n + n * n {
        return None;
    }
    let channels = fields[1..=n].iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut matrix = Vec::with_capacity(n);
    let nums: Vec<f64> = fields[1 + n..1 + n + n * n].iter().filter_map(|s| s.parse().ok()).collect();
    if nums.len() != n * n {
        return None;
    }
    for r in 0..n {
        matrix.push(nums[r * n..(r + 1) * n].to_vec());
    }
    Some(crate::model::SpilloverMeta {
        keyword: "$SPILL".into(),
        channels,
        matrix,
        unit: "spillover_fraction".into(),
        basis: "FCS3.1 $SPILL row=emission col=fluorophore".into(),
    })
}
