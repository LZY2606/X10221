//! 极简 FCS 3.1 读取器：只负责本案例包需要的部分。
//! 导入时保留原始表示（浮点字节原值），派生层才规范化；无法转换留原位置附诊断。

use crate::models::{ChannelDef, FcsData, RawValue, UnitBasis};

#[derive(Debug)]
pub struct FcsParseError(pub String);
impl std::fmt::Display for FcsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FCS 解析失败：{}", self.0)
    }
}
impl std::error::Error for FcsParseError {}

fn keyword<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let delim = text.as_bytes()[0] as char;
    let pat = format!("{delim}{key}{delim}");
    let start = text.find(&pat)? + pat.len();
    let rest = &text[start..];
    let end = rest.find(delim).unwrap_or(rest.len());
    Some(&rest[..end])
}

pub fn parse_fcs(bytes: &[u8], source_blob_id: &str, filename: &str) -> Result<FcsData, FcsParseError> {
    if bytes.len() < 58 || &bytes[0..3] != b"FCS" {
        return Err(FcsParseError("不是 FCS 文件（缺少 FCS 魔数）".into()));
    }
    let version = std::str::from_utf8(&bytes[3..6]).unwrap_or("").trim();
    let header = std::str::from_utf8(&bytes[10..58])
        .map_err(|_| FcsParseError("HEADER 非 ASCII".into()))?;
    let mut fields = header.split_whitespace();
    let mut take = |name: &str| -> Result<usize, FcsParseError> {
        let s = fields.next().ok_or_else(|| FcsParseError(format!("HEADER 缺少 {name}")))?;
        s.parse::<i64>()
            .map(|v| v as usize)
            .map_err(|_| FcsParseError(format!("HEADER {name} 不是整数: {s}")))
    };
    let text_start = take("TEXT/BEG")?;
    let text_end = take("TEXT/END")?;
    let data_start_h = take("DATA/BEG")?;
    let data_end_h = take("DATA/END")?;

    let text_bytes = &bytes[text_start..=text_end];
    let text = std::str::from_utf8(text_bytes)
        .map_err(|_| FcsParseError("TEXT 段非 ASCII".into()))?;
    let delim = text.as_bytes()[0] as char;
    if delim != '|' && delim != '\\' && delim != '/' {
        return Err(FcsParseError(format!("不支持的 TEXT 分隔符 {delim:?}")));
    }

    let data_type = keyword(text, "$DATATYPE").unwrap_or("F");
    let mode = keyword(text, "$MODE").unwrap_or("L");
    let byteord = keyword(text, "$BYTEORD").unwrap_or("1,2,3,4");
    let n_params: usize = keyword(text, "$PAR")
        .ok_or_else(|| FcsParseError("缺少 $PAR".into()))?
        .parse()
        .map_err(|_| FcsParseError("$PAR 非整数".into()))?;
    let n_events: usize = keyword(text, "$TOT")
        .ok_or_else(|| FcsParseError("缺少 $TOT".into()))?
        .parse()
        .map_err(|_| FcsParseError("$TOT 非整数".into()))?;
    let bytes_per: usize = keyword(text, "$P1B")
        .ok_or_else(|| FcsParseError("缺少 $P1B".into()))?
        .parse()
        .map_err(|_| FcsParseError("$P1B 非整数".into()))?;
    let (ds, de) = match keyword(text, "$BEGINDATA").zip(keyword(text, "$ENDDATA")) {
        Some((s, e)) => (
            s.parse::<usize>().map_err(|_| FcsParseError("$BEGINDATA".into()))?,
            e.parse::<usize>().map_err(|_| FcsParseError("$ENDDATA".into()))?,
        ),
        None => (data_start_h, data_end_h),
    };

    if data_type != "F" || mode != "L" || bytes_per != 4 {
        return Err(FcsParseError(format!(
            "本读取器仅支持 DATATYPE=F/MODE=L/32 位（实际 {data_type}/{mode}/{bytes_per}）"
        )));
    }
    let big_endian = byteord.starts_with("4");

    let mut channels = Vec::with_capacity(n_params);
    for i in 1..=n_params {
        let name = keyword(text, &format!("$P{i}N")).unwrap_or("UNKNOWN").to_string();
        let detector = keyword(text, &format!("$P{i}S")).unwrap_or(&name).to_string();
        let gain = keyword(text, &format!("$P{i}G")).map(|s| s.to_string());
        let basis = if name.starts_with("FSC") || name.starts_with("SSC") {
            UnitBasis::Scatter
        } else {
            UnitBasis::Fluorescence { channel: name.clone() }
        };
        let unit = if matches!(basis, UnitBasis::Scatter) { "a.u.(scatter)" } else { "a.u.(fluorescence)" };
        channels.push(ChannelDef {
            channel_id: format!("ch{i}"),
            short_name: name,
            detector: detector.clone(),
            fluorophore: gain.map(|_| detector),
            unit: unit.to_string(),
            basis,
        });
    }

    let data = &bytes[ds..=de];
    let need = n_events * n_params * 4;
    if data.len() < need {
        return Err(FcsParseError(format!(
            "DATA 段不足：需要 {need} 字节，实际 {}",
            data.len()
        )));
    }

    let mut rows = Vec::with_capacity(n_events);
    for ev in 0..n_events {
        let mut row = Vec::with_capacity(n_params);
        for ch in 0..n_params {
            let off = (ev * n_params + ch) * 4;
            let word: [u8; 4] = data[off..off + 4].try_into().unwrap();
            let bits = if big_endian {
                u32::from_be_bytes(word)
            } else {
                u32::from_le_bytes(word)
            };
            let value = f32::from_bits(bits) as f64;
            let representation = format!("0x{bits:08X}");
            let mut diagnostics = Vec::new();
            let parsed: Option<f64> = if value.is_nan() {
                diagnostics.push("IEEE754 NaN：无法转换，保留原始位模式".into());
                None
            } else if value.is_infinite() {
                diagnostics.push("IEEE754 无穷：探测器溢出，保留原始位模式".into());
                None
            } else {
                Some(value)
            };
            row.push(RawValue {
                value: parsed,
                representation,
                unit: channels[ch].unit.clone(),
                basis: channels[ch].basis.clone(),
                diagnostics,
            });
        }
        rows.push(row);
    }

    Ok(FcsData {
        source_blob_id: source_blob_id.to_string(),
        filename: filename.to_string(),
        channels,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fcs(values: &[f32], n_par: usize) -> Vec<u8> {
        let mut text = String::from("|$BEGINANALYSIS|0|$ENDANALYSIS|0|$DATATYPE|F|$MODE|L|$BYTEORD|1,2,3,4|");
        text.push_str(&format!("$PAR|{n_par}|$TOT|{}|", values.len() / n_par));
        for i in 1..=n_par {
            text.push_str(&format!("$P{i}B|32|$P{i}R|262144|$P{i}N|CH{i}|$P{i}S|Detector{i}|"));
        }
        text.push_str("$BEGINDATA|{ds}|$ENDDATA|{de}|");
        let mut out = Vec::new();
        out.extend_from_slice(b"FCS3.1    ");
        let tb = 58usize;
        let te = 57 + text.len();
        let db = te + 1;
        let de = db + values.len() * 4 - 1;
        let hdr = format!("{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}", tb, te, db, de, 0, 0);
        out.extend_from_slice(hdr.as_bytes());
        let text = text.replace("{ds}", &db.to_string()).replace("{de}", &de.to_string());
        out.extend_from_slice(text.as_bytes());
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn roundtrip_and_diagnostics() {
        let bytes = make_fcs(&[1.0, 2.0, f32::NAN, 4.0, 100.0, 200.0], 3);
        let fcs = parse_fcs(&bytes, "blob_x", "t.fcs").unwrap();
        assert_eq!(fcs.rows.len(), 2);
        assert_eq!(fcs.rows[0][0].value, Some(1.0));
        assert!(fcs.rows[0][2].value.is_none());
        assert!(fcs.rows[0][2].diagnostics[0].contains("NaN"));
        assert_eq!(fcs.rows[1][1].representation.len(), 10);
    }

    #[test]
    fn rejects_garbage_and_rolls_back_at_call_site() {
        assert!(parse_fcs(b"not an fcs file at all", "b", "x").is_err());
    }
}
