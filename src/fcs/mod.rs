pub mod dataset;
pub mod spill;
pub mod types;

use std::collections::BTreeMap;

/// FCS 3.1 TEXT 段解析错误。无法转换/解析的值保留在原始位置并附诊断，
/// 因此解析器在事件级别尽量不使用硬错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseDiag {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct FcsFile {
    /// 原始字节：来源材料必须可继续读取，派生层不得覆盖。
    pub raw: Vec<u8>,
    pub version: String,
    pub text_offset: u64,
    pub text_end: u64,
    pub data_offset: u64,
    pub data_end: u64,
    pub keywords: BTreeMap<String, String>,
    pub channels: Vec<ChannelSpec>,
    pub events: Vec<Vec<f64>>,
    pub diagnostics: Vec<ParseDiag>,
}

#[derive(Debug, Clone)]
pub struct ChannelSpec {
    pub index: usize,
    pub name: String,
    /// 通道单位（如 channels, volts），缺失时为无量纲 channels。
    pub unit: String,
    /// 量程基准：例如 18-bit ADC 的 262143，缺失则为 None。
    pub range: Option<f64>,
    pub gain: Option<f64>,
    pub voltage: Option<f64>,
}

fn decode_text(raw: &[u8]) -> (char, BTreeMap<String, String>, Vec<ParseDiag>) {
    let mut diags = Vec::new();
    if raw.len() < 2 {
        return (
            '/',
            BTreeMap::new(),
            vec![ParseDiag {
                code: "TEXT_TOO_SHORT".into(),
                detail: "TEXT 段不足 2 字节，缺少分隔符".into(),
            }],
        );
    }
    let delim = raw[0] as char;
    // 正文位于首末分隔符之间；连续两个分隔符表示字面分隔符。
    let body: &[u8] = if raw.len() >= 2 && raw[raw.len() - 1] as char == delim {
        &raw[1..raw.len() - 1]
    } else {
        &raw[1..]
    };
    let text = String::from_utf8_lossy(body);
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == delim {
            if chars.peek() == Some(&delim) {
                cur.push(delim);
                chars.next();
            } else {
                fields.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    let mut kv = BTreeMap::new();
    let mut i = 0;
    while i + 1 < fields.len() {
        let key = fields[i].trim().to_string();
        let val = fields[i + 1].clone();
        if !key.is_empty() {
            kv.insert(key, val);
        }
        i += 2;
    }
    if fields.len() % 2 != 0 {
        diags.push(ParseDiag {
            code: "ODD_KEYWORD_TOKENS".into(),
            detail: "TEXT 段关键字/值数量不成对，末尾原始片段保留".into(),
        });
    }
    (delim, kv, diags)
}

fn f64_keyword(kv: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    kv.get(key).and_then(|v| v.trim().parse::<f64>().ok())
}

impl FcsFile {
    /// 解析 FCS 3.1。支持 F（float32 little endian）事件数据。
    pub fn parse(raw: Vec<u8>) -> Self {
        let mut diags = Vec::new();
        let mut f = FcsFile {
            raw: raw.clone(),
            version: String::new(),
            text_offset: 0,
            text_end: 0,
            data_offset: 0,
            data_end: 0,
            keywords: BTreeMap::new(),
            channels: Vec::new(),
            events: Vec::new(),
            diagnostics: Vec::new(),
        };
        if raw.len() < 58 {
            f.diagnostics.push(ParseDiag {
                code: "HEADER_TRUNCATED".into(),
                detail: format!("仅 {} 字节，短于 FCS3.1 头部 58 字节", raw.len()),
            });
            return f;
        }
        f.version = String::from_utf8_lossy(&raw[0..6]).trim().to_string();
        let at = |start: usize, end: usize| String::from_utf8_lossy(&raw[start..end]).trim().to_string();
        // FCS3.1 头部固定偏移：TEXT 10..18/18..26, DATA 26..34/34..42。
        f.text_offset = at(10, 18).parse().unwrap_or(0);
        f.text_end = at(18, 26).parse().unwrap_or(0);
        f.data_offset = at(26, 34).parse().unwrap_or(0);
        f.data_end = at(34, 42).parse().unwrap_or(0);

        if f.text_end as usize > raw.len() || f.text_end < f.text_offset {
            f.diagnostics.push(ParseDiag {
                code: "TEXT_OFFSET_INVALID".into(),
                detail: "TEXT 段偏移越界，原始字节保留".into(),
            });
            return f;
        }
        let (_delim, kv, mut d) = decode_text(&raw[f.text_offset as usize..=f.text_end as usize]);
        diags.append(&mut d);
        // 大数据段偏移存放在 $BEGINDATA/$ENDDATA 中。
        if let Some(b) = f64_keyword(&kv, "$BEGINDATA") {
            f.data_offset = b as u64;
        }
        if let Some(e) = f64_keyword(&kv, "$ENDDATA") {
            f.data_end = e as u64;
        }
        f.keywords = kv.clone();

        let par = f64_keyword(&kv, "$PAR").map(|v| v as usize).unwrap_or(0);
        let byteord = kv.get("$BYTEORD").cloned().unwrap_or_default();
        let datatype = kv.get("$DATATYPE").cloned().unwrap_or_default();
        let mode = kv.get("$MODE").cloned().unwrap_or_default();
        if mode != "L" {
            diags.push(ParseDiag {
                code: "UNSUPPORTED_MODE".into(),
                detail: format!("$MODE={mode} 非列表模式 L，事件层不解析，原始字节保留"),
            });
        }
        if datatype != "F" {
            diags.push(ParseDiag {
                code: "UNSUPPORTED_DATATYPE".into(),
                detail: format!("$DATATYPE={datatype}，本工具仅解析 F(float32)，原始字节保留"),
            });
        }
        if byteord != "1,2,3,4" {
            diags.push(ParseDiag {
                code: "BYTE_ORDER".into(),
                detail: format!("$BYTEORD={byteord}，仅支持小端 1,2,3,4"),
            });
        }

        for idx in 1..=par {
            let name = kv
                .get(&format!("$P{idx}N"))
                .cloned()
                .unwrap_or_else(|| format!("P{idx}"));
            let unit = kv
                .get(&format!("$P{idx}U"))
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "channels".to_string());
            let range = f64_keyword(&kv, &format!("$P{idx}R"));
            let gain = f64_keyword(&kv, &format!("$P{idx}G"));
            let voltage = f64_keyword(&kv, &format!("$P{idx}V"));
            f.channels.push(ChannelSpec {
                index: idx,
                name,
                unit,
                range,
                gain,
                voltage,
            });
        }

        if datatype == "F" && byteord == "1,2,3,4" && mode == "L" && par > 0 {
            if f.data_end as usize > raw.len() || f.data_end < f.data_offset {
                diags.push(ParseDiag {
                    code: "DATA_OFFSET_INVALID".into(),
                    detail: "DATA 段偏移越界，事件层不解析".into(),
                });
            } else {
                let seg = &raw[f.data_offset as usize..=f.data_end as usize];
                let n = seg.len() / 4;
                let mut events = Vec::new();
                for (ei, chunk) in seg.chunks_exact(4).enumerate() {
                    let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    let val = f32::from_bits(bits) as f64;
                    let pos = ei % par;
                    if pos == 0 {
                        events.push(Vec::with_capacity(par));
                    }
                    events.last_mut().unwrap().push(val);
                }
                if n % par != 0 {
                    diags.push(ParseDiag {
                        code: "DATA_PARTIAL_EVENT".into(),
                        detail: format!("DATA 段 {n} 个数值不能被 $PAR={par} 整除，尾部值保留在原始字节"),
                    });
                }
                if let Some(tot) = f64_keyword(&kv, "$TOT").map(|v| v as usize) {
                    if tot != events.len() {
                        diags.push(ParseDiag {
                            code: "TOT_MISMATCH".into(),
                            detail: format!("$TOT={tot} 与实际解析事件数 {} 不一致", events.len()),
                        });
                    }
                }
                f.events = events;
            }
        }
        f.diagnostics = diags;
        f
    }
}

/// 构造 FCS 3.1 字节。供离线案例包构建使用（build.rs）。
pub fn encode_fcs(
    channels: &[ChannelSpec],
    events: &[Vec<f64>],
    extra: &BTreeMap<String, String>,
) -> Vec<u8> {
    let mut kv: BTreeMap<String, String> = BTreeMap::new();
    kv.insert("FCS3.1".to_string(), " ".to_string());
    kv.insert("$BEGINANALYSIS".to_string(), "0".to_string());
    kv.insert("$ENDANALYSIS".to_string(), "0".to_string());
    kv.insert("$BEGINSTEXT".to_string(), "0".to_string());
    kv.insert("$ENDSTEXT".to_string(), "0".to_string());
    kv.insert("$BYTEORD".to_string(), "1,2,3,4".to_string());
    kv.insert("$DATATYPE".to_string(), "F".to_string());
    kv.insert("$MODE".to_string(), "L".to_string());
    kv.insert("$NEXTDATA".to_string(), "0".to_string());
    kv.insert("$PAR".to_string(), channels.len().to_string());
    kv.insert("$TOT".to_string(), events.len().to_string());
    for (i, ch) in channels.iter().enumerate() {
        let n = i + 1;
        kv.insert(format!("$P{n}N"), ch.name.clone());
        kv.insert(format!("$P{n}U"), ch.unit.clone());
        if let Some(r) = ch.range {
            kv.insert(format!("$P{n}R"), (r as u64).to_string());
        }
        if let Some(g) = ch.gain {
            kv.insert(format!("$P{n}G"), format!("{g}"));
        }
        if let Some(v) = ch.voltage {
            kv.insert(format!("$P{n}V"), format!("{v}"));
        }
        kv.insert(format!("$P{n}B"), "32".to_string());
        kv.insert(format!("$P{n}E"), "0,0".to_string());
    }
    for (k, v) in extra {
        if k != "FCS3.1" {
            kv.insert(k.clone(), v.clone());
        }
    }
    let mut data = Vec::with_capacity(events.len() * channels.len() * 4);
    for ev in events {
        for v in ev {
            data.extend_from_slice(&(*v as f32).to_le_bytes());
        }
    }
    // 偏移写入 TEXT，而 TEXT 长度依赖偏移：先用固定 12 位数字占位，一次成型。
    let text_start = 58u64;
    kv.insert("$BEGINDATA".to_string(), "000000000000".to_string());
    kv.insert("$ENDDATA".to_string(), "000000000000".to_string());
    let mut render = |kv: &BTreeMap<String, String>| {
        let mut text = String::from('/');
        for (k, v) in kv {
            text.push_str(k);
            text.push('/');
            text.push_str(&v.replace('/', "//"));
            text.push('/');
        }
        text
    };
    let probe = render(&kv);
    let data_start = text_start + probe.len() as u64;
    let data_stop = data_start + data.len() as u64 - 1;
    kv.insert("$BEGINDATA".to_string(), format!("{data_start:012}"));
    kv.insert("$ENDDATA".to_string(), format!("{data_stop:012}"));
    let text = render(&kv);
    let text_stop = data_start - 1;
    debug_assert_eq!(text_start + text.len() as u64 - 1, text_stop);
    let header = format!(
        "FCS3.1     {text_start:>8}{text_stop:>8}{data_start:>8}{data_stop:>8}0       0       "
    );
    debug_assert_eq!(header.len(), 58, "header len {}", header.len());
    let mut out = header.into_bytes();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(&data);
    out
}
