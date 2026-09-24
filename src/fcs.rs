//! 最小 FCS 3.0 解析器：读取 HEADER/TEXT/DATA 段。
//! 只做结构化读取，不做任何不可逆清洗；原始字节与 TEXT 段原样保留。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FcsParam {
    pub index: usize,
    pub name: String,
    pub range: Option<f64>,
    pub amplification: String,
    pub bits: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FcsFile {
    pub text: Vec<(String, String)>,
    pub params: Vec<FcsParam>,
    pub n_events: usize,
    /// 原始事件，列序与 $PAR 一致；f32 原值，不做规范化。
    pub events: Vec<Vec<f32>>,
    pub raw_text: String,
}

#[derive(Debug)]
pub enum FcsError {
    TooShort,
    BadMagic(String),
    BadOffset(&'static str),
    BadTextSegment,
    Unsupported(&'static str),
    Truncated,
}

impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "数据短于 FCS HEADER(58 字节)"),
            Self::BadMagic(m) => write!(f, "不是 FCS3.0 文件，magic={m:?}"),
            Self::BadOffset(s) => write!(f, "HEADER 偏移非法: {s}"),
            Self::BadTextSegment => write!(f, "TEXT 段定界/配对非法"),
            Self::Unsupported(s) => write!(f, "不支持的 FCS 特性: {s}"),
            Self::Truncated => write!(f, "DATA 段被截断"),
        }
    }
}
impl std::error::Error for FcsError {}

fn num_field(bytes: &[u8], name: &'static str) -> Result<u64, FcsError> {
    let s = std::str::from_utf8(bytes).map_err(|_| FcsError::BadOffset(name))?;
    s.trim()
        .parse::<u64>()
        .map_err(|_| FcsError::BadOffset(name))
}

pub fn parse(data: &[u8]) -> Result<FcsFile, FcsError> {
    if data.len() < 58 {
        return Err(FcsError::TooShort);
    }
    let magic = std::str::from_utf8(&data[0..6]).unwrap_or("");
    if magic != "FCS3.0" && magic != "FCS2.0" {
        return Err(FcsError::BadMagic(magic.to_string()));
    }
    let text_start = num_field(&data[10..18], "text_start")? as usize;
    let text_end = num_field(&data[18..26], "text_end")? as usize;
    let data_start = num_field(&data[26..34], "data_start")? as usize;
    let data_end = num_field(&data[34..42], "data_end")? as usize;
    if text_start == 0 || text_end < text_start || text_end > data.len() {
        return Err(FcsError::BadOffset("text"));
    }

    let raw = &data[text_start..=text_end];
    if raw.is_empty() {
        return Err(FcsError::BadTextSegment);
    }
    let delim = raw[0] as char;
    let body = std::str::from_utf8(&raw[1..]).map_err(|_| FcsError::BadTextSegment)?;
    let parts: Vec<&str> = body.split(delim).collect();
    let mut kv: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i + 1 < parts.len() {
        if parts[i].is_empty() {
            i += 1;
            continue;
        }
        kv.push((parts[i].to_string(), parts[i + 1].to_string()));
        i += 2;
    }
    if i >= parts.len() || !parts[i].is_empty() {
        // 允许尾部恰好为空；否则配对不完整
        if i < parts.len() && !parts[i].is_empty() {
            return Err(FcsError::BadTextSegment);
        }
    }

    let get = |k: &str| -> Option<&str> {
        kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    };
    let npar: usize = get("$PAR")
        .ok_or(FcsError::BadTextSegment)?
        .parse()
        .map_err(|_| FcsError::BadTextSegment)?;
    let ntot: usize = get("$TOT")
        .ok_or(FcsError::BadTextSegment)?
        .parse()
        .map_err(|_| FcsError::BadTextSegment)?;
    let dtype = get("$DATATYPE").unwrap_or("F");
    if dtype != "F" {
        return Err(FcsError::Unsupported("仅实现 $DATATYPE=F (float32)"));
    }
    let mode = get("$MODE").unwrap_or("L");
    if mode != "L" {
        return Err(FcsError::Unsupported("仅实现 $MODE=L (list mode)"));
    }
    let byteord = get("$BYTEORD").unwrap_or("1,2,3,4");
    if byteord != "1,2,3,4" {
        return Err(FcsError::Unsupported("仅实现 $BYTEORD=1,2,3,4 (小端)"));
    }

    let mut params = Vec::new();
    for p in 1..=npar {
        let name = get(&format!("$P{p}N")).unwrap_or("?").to_string();
        let range = get(&format!("$P{p}R")).and_then(|s| s.parse().ok());
        let amp = get(&format!("$P{p}E")).unwrap_or("0,0").to_string();
        let bits = get(&format!("$P{p}B")).and_then(|s| s.parse()).unwrap_or(32u32);
        params.push(FcsParam {
            index: p - 1,
            name,
            range,
            amplification: amp,
            bits,
        });
    }

    if data_start == 0 || data_end < data_start || data_end >= data.len() {
        return Err(FcsError::BadOffset("data"));
    }
    let dseg = &data[data_start..=data_end];
    if dseg.len() != ntot * npar * 4 {
        return Err(FcsError::Truncated);
    }
    let mut events = Vec::with_capacity(ntot);
    for e in 0..ntot {
        let mut row = Vec::with_capacity(npar);
        for c in 0..npar {
            let off = (e * npar + c) * 4;
            let b: [u8; 4] = dseg[off..off + 4].try_into().unwrap();
            row.push(f32::from_le_bytes(b));
        }
        events.push(row);
    }

    Ok(FcsFile {
        text: kv,
        params,
        n_events: ntot,
        events,
        raw_text: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_fcs(events: &[Vec<f32>], names: &[&str]) -> Vec<u8> {
        let mut pairs = vec![
            ("$BYTEORD".to_string(), "1,2,3,4".to_string()),
            ("$DATATYPE".to_string(), "F".to_string()),
            ("$MODE".to_string(), "L".to_string()),
            ("$PAR".to_string(), names.len().to_string()),
            ("$TOT".to_string(), events.len().to_string()),
        ];
        for (i, n) in names.iter().enumerate() {
            pairs.push((format!("$P{}N", i + 1), n.to_string()));
            pairs.push((format!("$P{}R", i + 1), "262144".to_string()));
        }
        let body: String = pairs
            .iter()
            .flat_map(|(k, v)| ["|".to_string(), k.clone(), v.clone()])
            .collect::<Vec<_>>()
            .join("")
            + "|";
        let header_len = 58usize;
        let ts = header_len;
        let te = ts + body.len() - 1;
        let ds = te + 1;
        let de = ds + events.len() * names.len() * 4 - 1;
        let mut hdr = b"FCS3.0    ".to_vec();
        hdr.extend_from_slice(format!("{ts:8d}{te:8d}{ds:8d}{de:8d}{0:8d}{0:8d}").as_bytes());
        let mut out = hdr;
        out.extend_from_slice(body.as_bytes());
        for row in events {
            for v in row {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn parses_roundtrip() {
        let bytes = synth_fcs(&[vec![1.0, 2.0], vec![3.0, -4.5]], &["FSC-A", "FL1-A"]);
        let f = parse(&bytes).unwrap();
        assert_eq!(f.n_events, 2);
        assert_eq!(f.params[0].name, "FSC-A");
        assert_eq!(f.events[1][1], -4.5);
        assert!(f.raw_text.contains("FSC-A"));
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(matches!(parse(b"not-an-fcs-file-at-all!!"), Err(FcsError::BadMagic(_))));
    }
}
