//! FCS 3.1 读写：导入时保留原始字节与原始表示，解析只发生在派生层。
//! 无法转换的值（NaN/Inf、未知 DATATYPE 等）留在原位置并附诊断，绝不丢弃事件。

use crate::model::{Diagnostic, EventChannel, EventPoint, FcsData};

pub struct ParsedFcs {
    pub data: FcsData,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(bytes: &[u8]) -> anyhow::Result<ParsedFcs> {
    if bytes.len() < 58 {
        anyhow::bail!("FCS header too short");
    }
    let version = String::from_utf8_lossy(&bytes[0..6]).trim().to_string();
    if !version.starts_with("FCS") {
        anyhow::bail!("not an FCS file (version field: {version})");
    }
    let off = |s: usize, e: usize| bytes[s..e].iter().position(|b| *b != b' ').and_then(|p| {
        let end = bytes[s..e].iter().rposition(|b| *b != b' ')? + 1;
        String::from_utf8_lossy(&bytes[s + p..s + end]).trim().parse::<i64>().ok()
    });
    let text_s = off(10, 18);
    let data_s_hdr = off(26, 34);
    let data_e_hdr = off(34, 42);

    let (text_start, _) = (text_s.unwrap_or(0) as usize, ());
    let text_end = off(18, 26).unwrap_or(0) as usize;
    let text_bytes = &bytes[text_start..text_end];
    if text_bytes.is_empty() {
        anyhow::bail!("empty TEXT segment");
    }
    let delim = text_bytes[0];
    let mut keywords: Vec<(String, String)> = Vec::new();
    {
        let body = &text_bytes[1..];
        let parts: Vec<&[u8]> = split_delim(body, delim);
        let mut i = 0;
        while i + 1 < parts.len() {
            let k = String::from_utf8_lossy(parts[i]).trim().to_string();
            let v = String::from_utf8_lossy(parts[i + 1]).trim().to_string();
            keywords.push((k, v));
            i += 2;
        }
    }
    let kw = |name: &str| keywords.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());

    let data_start = kw("$BEGINDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .or(data_s_hdr.map(|v| v as usize))
        .ok_or_else(|| anyhow::anyhow!("missing $BEGINDATA"))?;
    let data_end = kw("$ENDDATA")
        .and_then(|v| v.parse::<usize>().ok())
        .or(data_e_hdr.map(|v| v as usize))
        .ok_or_else(|| anyhow::anyhow!("missing $ENDDATA"))?;

    let par: usize = kw("$PAR")
        .ok_or_else(|| anyhow::anyhow!("missing $PAR"))?
        .parse()?;
    let tot: usize = kw("$TOT")
        .ok_or_else(|| anyhow::anyhow!("missing $TOT"))?
        .parse()?;
    let datatype = kw("$DATATYPE").unwrap_or_default();
    let byteord = kw("$BYTEORD").unwrap_or_else(|| "1,2,3,4".into());
    let little_endian = byteord.starts_with('1');

    let mut channels = Vec::with_capacity(par);
    for n in 1..=par {
        let name = kw(&format!("$P{n}N")).unwrap_or_else(|| format!("CH{n}"));
        let basis = kw(&format!("FLOWLINEAGE_P{n}_BASIS"))
            .or_else(|| kw(&format!("$P{n}E")))
            .unwrap_or_else(|| "FCS raw".into());
        channels.push(EventChannel { name, basis });
    }

    let mut diagnostics = Vec::new();
    let mut events = Vec::with_capacity(tot);
    let seg = &bytes[data_start..data_end];

    match datatype.as_str() {
        "F" | "D" => {
            let width = if datatype == "F" { 4 } else { 8 };
            let expect = tot * par * width;
            if seg.len() < expect {
                anyhow::bail!("DATA segment truncated: {} < {}", seg.len(), expect);
            }
            for (idx, chunk) in seg.chunks_exact(par * width).enumerate().take(tot) {
                let mut values = Vec::with_capacity(par);
                let mut evdiag = Vec::new();
                for (c, raw) in chunk.chunks_exact(width).enumerate() {
                    let v = if width == 4 {
                        let mut b = [0u8; 4];
                        b.copy_from_slice(raw);
                        if !little_endian {
                            b.reverse();
                        }
                        f32::from_le_bytes(b) as f64
                    } else {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(raw);
                        if !little_endian {
                            b.reverse();
                        }
                        f64::from_le_bytes(b)
                    };
                    if v.is_nan() {
                        evdiag.push(Diagnostic {
                            code: "non_finite_value".into(),
                            message: format!("通道 {} 的 NaN 保留在原位置，未参与作图/门控", channels[c].name),
                            location: format!("event[{idx}].{}", channels[c].name),
                            severity: "warning".into(),
                        });
                        values.push(None);
                    } else if v.is_infinite() {
                        evdiag.push(Diagnostic {
                            code: "extreme_signal".into(),
                            message: format!("通道 {} 的无穷大极端信号保留在原位置", channels[c].name),
                            location: format!("event[{idx}].{}", channels[c].name),
                            severity: "warning".into(),
                        });
                        values.push(None);
                    } else {
                        values.push(Some(v));
                    }
                }
                events.push(EventPoint {
                    index: idx,
                    values,
                    diagnostics: evdiag,
                });
            }
        }
        other => {
            anyhow::bail!("unsupported $DATATYPE '{other}'（仅 F/D，原始字节已保留）");
        }
    }

    let role = kw("FLOWLINEAGE_ROLE").unwrap_or_else(|| "sample".into());
    let stain_channel = kw("FLOWLINEAGE_STAIN");
    if !version.starts_with("FCS3.") {
        diagnostics.push(Diagnostic {
            code: "unexpected_fcs_version".into(),
            message: format!("按 FCS3.1 兼容方式解析 {version}"),
            location: String::new(),
            severity: "info".into(),
        });
    }

    Ok(ParsedFcs {
        data: FcsData {
            channels,
            events,
            role,
            stain_channel,
        },
        diagnostics,
    })
}

fn split_delim(body: &[u8], delim: u8) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < body.len() {
        if body[i] == delim {
            // 双分隔符为转义
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

/// 生成 FCS3.1（$DATATYPE=F，小端，float32）。
/// TEXT 内偏移用固定 8 位零填充，使 TEXT 长度不依赖偏移本身。
pub fn write_fcs(data: &FcsData) -> Vec<u8> {
    let par = data.channels.len();
    let tot = data.events.len();
    let delim = b'|';

    let mut kvs: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "00000000".into()),
        ("$ENDANALYSIS".into(), "00000000".into()),
        ("$BEGINDATA".into(), "00000000".into()),
        ("$ENDDATA".into(), "00000000".into()),
        ("$MODE".into(), "L".into()),
        ("$DATATYPE".into(), "F".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$PAR".into(), par.to_string()),
        ("$TOT".into(), tot.to_string()),
        ("$NEXTDATA".into(), "0".into()),
        ("FLOWLINEAGE_ROLE".into(), data.role.clone()),
    ];
    if let Some(st) = &data.stain_channel {
        kvs.push(("FLOWLINEAGE_STAIN".into(), st.clone()));
    }
    for (n, ch) in data.channels.iter().enumerate() {
        let n = n + 1;
        kvs.push((format!("$P{n}N"), ch.name.clone()));
        kvs.push((format!("$P{n}B"), "32".into()));
        kvs.push((format!("$P{n}E"), "0,0".into()));
        kvs.push((format!("$P{n}R"), "262144".into()));
        kvs.push((format!("FLOWLINEAGE_P{n}_BASIS"), ch.basis.clone()));
    }

    let mut text = vec![delim];
    for (k, v) in &kvs {
        text.extend_from_slice(k.as_bytes());
        text.push(delim);
        text.extend_from_slice(v.as_bytes());
        text.push(delim);
    }
    let text_start = 58usize;
    let data_start = text_start + text.len();
    let mut raw = Vec::with_capacity(tot * par * 4);
    for ev in &data.events {
        for c in 0..par {
            let v: f32 = ev.values.get(c).copied().flatten().unwrap_or(f32::NAN) as f32;
            raw.extend_from_slice(&v.to_le_bytes());
        }
    }
    let data_end = data_start + raw.len();
    let fill8 = |x: usize| format!("{:08}", x);

    let inject = |text: &mut Vec<u8>, key: &str, val: &str| {
        let pat = format!("{}{}{}", delim as char, key, delim as char);
        let pos = find_bytes(text, pat.as_bytes())
            .unwrap_or_else(|| panic!("keyword {key}"));
        let val_start = pos + pat.len();
        let end = text[val_start..].iter().position(|b| *b == delim).unwrap();
        assert_eq!(end, 8, "offset placeholder must be 8 chars");
        text.splice(val_start..val_start + end, val.bytes());
    };
    inject(&mut text, "$BEGINDATA", &fill8(data_start));
    inject(&mut text, "$ENDDATA", &fill8(data_end));

    let mut out = Vec::with_capacity(data_end);
    let field = |s: &str| format!("{s:>8}");
    out.extend_from_slice(b"FCS3.1  ");
    out.extend_from_slice(field(&text_start.to_string()).as_bytes());
    out.extend_from_slice(field(&(text_start + text.len()).to_string()).as_bytes());
    out.extend_from_slice(field(&data_start.to_string()).as_bytes());
    out.extend_from_slice(field(&data_end.to_string()).as_bytes());
    out.extend_from_slice(field("0").as_bytes());
    out.extend_from_slice(field("0").as_bytes());
    assert_eq!(out.len(), 58);
    out.extend_from_slice(&text);
    out.extend_from_slice(&raw);
    out
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FcsData {
        FcsData {
            channels: vec![
                EventChannel { name: "FSC-A".into(), basis: "FCS linear ADC".into() },
                EventChannel { name: "FITC-A".into(), basis: "FCS linear ADC".into() },
            ],
            events: vec![
                EventPoint { index: 0, values: vec![Some(1.0), Some(2.0)], diagnostics: vec![] },
                EventPoint { index: 1, values: vec![Some(3.0), None], diagnostics: vec![] },
            ],
            role: "sample".into(),
            stain_channel: None,
        }
    }

    #[test]
    fn roundtrip_preserves_events_and_nan() {
        let bytes = write_fcs(&sample());
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.data.channels[0].name, "FSC-A");
        assert_eq!(parsed.data.events.len(), 2);
        assert_eq!(parsed.data.events[0].values, vec![Some(1.0), Some(2.0)]);
        assert_eq!(parsed.data.events[1].values, vec![Some(3.0), None]);
        assert!(parsed.data.events[1].diagnostics.iter().any(|d| d.code == "non_finite_value"));
        assert_eq!(parsed.data.role, "sample");
    }
}
