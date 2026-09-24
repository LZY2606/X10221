//! 最小但真实的 FCS 3.0 解析器。
//! 原则：导入时保留原始表示（原始字节 + 原始 TEXT 段），派生层才做规范化。

use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct ParamMeta {
    pub index: usize,
    pub name: String,
    /// $PnR：ADC 量程（坐标基准）。
    pub range: f64,
    /// 单位/坐标基准，未知时保留原值并附诊断。
    pub unit: String,
}

#[derive(Clone, Debug)]
pub struct FcsFile {
    pub version: String,
    /// 原始 TEXT 段键值（原样保留）。
    pub text: BTreeMap<String, String>,
    pub raw_text_segment: String,
    pub params: Vec<ParamMeta>,
    /// 原始通道值（未补偿、未变换）。
    pub events: Vec<Vec<f64>>,
    /// 超出量程的事件下标（极端信号：留在原位并附诊断，不裁剪）。
    pub extreme_events: Vec<usize>,
}

#[derive(Debug)]
pub struct FcsError(pub String);

impl std::fmt::Display for FcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS 解析失败: {}", self.0)
    }
}

fn parse_offset(header: &[u8], from: usize, to: usize) -> Result<usize, FcsError> {
    let s = std::str::from_utf8(&header[from..to])
        .map_err(|e| FcsError(format!("头部偏移非 ASCII: {e}")))?;
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    s.parse::<usize>()
        .map_err(|e| FcsError(format!("头部偏移无法解析 '{s}': {e}")))
}

pub fn parse(bytes: &[u8]) -> Result<FcsFile, FcsError> {
    if bytes.len() < 58 {
        return Err(FcsError("文件太短，不是 FCS".into()));
    }
    let version = std::str::from_utf8(&bytes[0..6])
        .map_err(|e| FcsError(e.to_string()))?
        .to_string();
    if !version.starts_with("FCS3") {
        return Err(FcsError(format!("仅支持 FCS3.x，得到 {version}")));
    }
    let text_start = parse_offset(bytes, 10, 18)?;
    let text_end = parse_offset(bytes, 18, 26)?;
    let mut data_start = parse_offset(bytes, 26, 34)?;
    let mut data_end = parse_offset(bytes, 34, 42)?;
    if text_start == 0 || text_end >= bytes.len() || text_end < text_start {
        return Err(FcsError("TEXT 段偏移非法".into()));
    }
    let raw_text = String::from_utf8_lossy(&bytes[text_start..=text_end]).to_string();
    let text = parse_text_segment(&raw_text)?;

    let get = |k: &str| -> Result<&str, FcsError> {
        text.get(k)
            .map(|s| s.as_str())
            .ok_or_else(|| FcsError(format!("缺少关键字 {k}")))
    };
    let n_params: usize = get("$PAR")?
        .parse()
        .map_err(|_| FcsError("$PAR 非整数".into()))?;
    let n_events: usize = get("$TOT")?
        .parse()
        .map_err(|_| FcsError("$TOT 非整数".into()))?;
    let datatype = get("$DATATYPE")?;
    if datatype != "F" {
        return Err(FcsError(format!("仅支持 $DATATYPE=F，得到 {datatype}")));
    }
    let byteord = get("$BYTEORD")?;
    if byteord != "1,2,3,4" {
        return Err(FcsError(format!("仅支持小端 $BYTEORD=1,2,3,4，得到 {byteord}")));
    }
    if data_start == 0 {
        data_start = get("$BEGINDATA")?
            .parse()
            .map_err(|_| FcsError("$BEGINDATA 非整数".into()))?;
    }
    if data_end == 0 {
        data_end = get("$ENDDATA")?
            .parse()
            .map_err(|_| FcsError("$ENDDATA 非整数".into()))?;
    }
    let need = data_start + n_events * n_params * 4;
    if data_end + 1 < need || bytes.len() < need {
        return Err(FcsError(format!(
            "DATA 段不足：需要 {need} 字节，文件 {} 字节",
            bytes.len()
        )));
    }

    let mut params = Vec::with_capacity(n_params);
    for i in 1..=n_params {
        let name = text
            .get(&format!("$P{i}N"))
            .cloned()
            .unwrap_or_else(|| format!("P{i}"));
        let range: f64 = text
            .get(&format!("$P{i}R"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(262144.0);
        let unit = text
            .get(&format!("$P{i}U"))
            .cloned()
            .unwrap_or_else(|| "ADC".to_string());
        params.push(ParamMeta { index: i - 1, name, range, unit });
    }

    let mut events = Vec::with_capacity(n_events);
    let mut extreme_events = Vec::new();
    for e in 0..n_events {
        let mut row = Vec::with_capacity(n_params);
        let mut extreme = false;
        for (p, param) in params.iter().enumerate() {
            let off = data_start + (e * n_params + p) * 4;
            let v = f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as f64;
            // 极端信号：达到/超过 ADC 量程的值留在原位，仅记录诊断。
            if v >= param.range || v <= 0.0 && param.range > 0.0 && v.is_sign_negative() {
                extreme = true;
            }
            row.push(v);
        }
        if extreme {
            extreme_events.push(e);
        }
        events.push(row);
    }

    Ok(FcsFile { version, text, raw_text_segment: raw_text, params, events, extreme_events })
}

/// TEXT 段：首字符为分隔符，键值对以分隔符隔开，分隔符在值内双写转义。
pub fn parse_text_segment(seg: &str) -> Result<BTreeMap<String, String>, FcsError> {
    let mut chars: Vec<char> = seg.chars().collect();
    if chars.is_empty() {
        return Err(FcsError("空 TEXT 段".into()));
    }
    let delim = chars.remove(0);
    let mut map = BTreeMap::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == delim {
            if i + 1 < chars.len() && chars[i + 1] == delim {
                cur.push(delim);
                i += 2;
                continue;
            }
            tokens.push(std::mem::take(&mut cur));
            i += 1;
        } else {
            cur.push(chars[i]);
            i += 1;
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut it = tokens.into_iter();
    while let Some(k) = it.next() {
        let v = it.next().unwrap_or_default();
        if !k.is_empty() {
            map.insert(k.to_uppercase(), v);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_segment_escaping() {
        let m = parse_text_segment("/$PAR/2/$P1N/FL1//A/$TOT/3/").unwrap();
        assert_eq!(m.get("$PAR").unwrap(), "2");
        assert_eq!(m.get("$P1N").unwrap(), "FL1/A");
        assert_eq!(m.get("$TOT").unwrap(), "3");
    }
}
