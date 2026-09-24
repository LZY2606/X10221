use crate::domain::types::{ChannelDef, EventPoint, FcsEvents, Scaled};

#[derive(Debug, thiserror::Error)]
pub enum FcsError {
    #[error("字节数不足，不是合法 FCS 文件：{0}")]
    TooShort(String),
    #[error("FCS 头解析失败：{0}")]
    Header(String),
    #[error("TEXT 段解析失败：{0}")]
    Text(String),
    #[error("DATA 段解析失败：{0}")]
    Data(String),
}

fn parse_offset(s: &str) -> Result<usize, FcsError> {
    s.trim().parse::<usize>().map_err(|_| FcsError::Header(s.to_string()))
}

/// 解析 FCS 3.1。仅按 TEXT 段声明的偏移读取，支持 F/D/I 与大小端。
pub fn parse_fcs(bytes: &[u8]) -> Result<FcsEvents, FcsError> {
    if bytes.len() < 256 {
        return Err(FcsError::TooShort(format!("{} < 256", bytes.len())));
    }
    let version = String::from_utf8_lossy(&bytes[0..6]).trim().to_string();
    let text_start = parse_offset(std::str::from_utf8(&bytes[10..18]).map_err(|e| FcsError::Header(e.to_string()))?)?;
    let text_end_byte = parse_offset(std::str::from_utf8(&bytes[18..26]).map_err(|e| FcsError::Header(e.to_string()))?)?;
    if text_end_byte < text_start || text_end_byte >= bytes.len() {
        return Err(FcsError::Text(format!("TEXT 偏移越界 {}-{}", text_start, text_end_byte)));
    }

    let text_bytes = &bytes[text_start..=text_end_byte];
    let delim = text_bytes[0] as char;
    let text = String::from_utf8_lossy(&text_bytes[1..]).to_string();
    // 分隔符双写表示字面量；先按单分隔符切分，再把奇数位边界合并。
    let raw_tokens: Vec<&str> = text.split(delim).collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut carry = String::new();
    for tok in &raw_tokens {
        if !carry.is_empty() {
            carry.push(delim);
            carry.push_str(tok);
        } else {
            carry = tok.to_string();
        }
        // 双写：若下一个 token 是空串占位，split 已自然处理，这里按偶数配对在后面完成
        tokens.push(std::mem::take(&mut carry));
    }
    // 合并由双写分隔符产生的相邻 token（token 后紧跟空 token）
    let mut merged: Vec<String> = Vec::new();
    let mut idx = 0;
    let toks = &tokens;
    while idx < toks.len() {
        let mut piece = toks[idx].clone();
        // 原始 split: "a||b" => ["a","","b"]，表示字面 "a|b"
        while idx + 1 < toks.len() && toks[idx + 1].is_empty() {
            piece.push(delim);
            piece.push_str(&toks[idx + 2]);
            idx += 2;
        }
        merged.push(piece);
        idx += 1;
    }
    let tokens: Vec<String> = merged.into_iter().iter().filter(|t| !t.is_empty()).cloned().collect();
    let mut kw = std::collections::BTreeMap::<String, String>::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        kw.insert(tokens[i].trim().to_uppercase(), tokens[i + 1].clone());
        i += 2;
    }

    let get = |k: &str| kw.get(k).cloned();
    let n_events: usize = get("$TOT").and_then(|v| v.parse().ok()).ok_or_else(|| FcsError::Data("缺少 $TOT".into()))?;
    let n_par: usize = get("$PAR").and_then(|v| v.parse().ok()).ok_or_else(|| FcsError::Data("缺少 $PAR".into()))?;
    let datatype = get("$DATATYPE").unwrap_or_else(|| "F".to_string());
    let byteord = get("$BYTEORD").unwrap_or_else(|| "1,2,3,4".to_string());
    let big_endian = byteord.starts_with('4');
    let mode = get("$MODE").unwrap_or_else(|| "L".to_string());
    if mode != "L" {
        return Err(FcsError::Data(format!("仅支持列表模式 $MODE=L，得到 {mode}")));
    }

    let mut channels = Vec::with_capacity(n_par);
    for p in 1..=n_par {
        let name = get(&format!("$P{}N", p)).unwrap_or_else(|| format!("P{}", p));
        let detector = get(&format!("$P{}S", p)).unwrap_or_else(|| name.clone());
        let gains = get(&format!("$P{}G", p));
        let amp = get(&format!("$P{}E", p));
        let unit = if amp.as_deref() == Some("0,0") { "counts".to_string() } else { amp.clone().unwrap_or_else(|| "counts".to_string()) };
        channels.push(ChannelDef {
            name,
            detector,
            label: detector.clone(),
            unit,
            raw_repr: Some(serde_json::json!({ "$PnE": amp, "$PnG": gains })),
        });
    }

    let data_start: usize = match (get("$BEGINDATA").and_then(|v| v.parse().ok())) {
        Some(v) => v,
        None => text_end_byte + 1,
    };

    let mut points = Vec::with_capacity(n_events);
    let mut unconvertible = Vec::new();
    let bytes_per_val = match datatype.as_str() {
        "F" => 4usize,
        "D" => 8,
        "I" => {
            let b: usize = get("$P1B").and_then(|v| v.parse().ok()).unwrap_or(16);
            b / 8
        }
        other => return Err(FcsError::Data(format!("不支持的 $DATATYPE={other}"))),
    };
    for e in 0..n_events {
        let mut vals = Vec::with_capacity(n_par);
        for p in 0..n_par {
            let pbv = if datatype == "I" {
                get(&format!("$P{}B", p + 1)).and_then(|v| v.parse::<usize>().ok()).unwrap_or(bytes_per_val * 8) / 8
            } else {
                bytes_per_val
            };
            let off = data_start + (e * n_par + p) * pbv;
            let raw_repr = format!("{}:{}", datatype, byteord);
            if off + pbv > bytes.len() {
                let s = Scaled::unconvertible(raw_repr, "DATA 段截断，无法读取该通道原始字节");
                unconvertible.push((e, p, "truncated".to_string()));
                vals.push(s);
                continue;
            }
            let chunk = &bytes[off..off + pbv];
            let parsed: Option<f64> = match datatype.as_str() {
                "F" => Some(if big_endian { f32::from_be_bytes(chunk.try_into().unwrap()) } else { f32::from_le_bytes(chunk.try_into().unwrap()) } as f64),
                "D" => Some(if big_endian { f64::from_be_bytes(chunk.try_into().unwrap()) } else { f64::from_le_bytes(chunk.try_into().unwrap()) }),
                "I" => {
                    let mut tmp = vec![0u8; pbv];
                    tmp.copy_from_slice(chunk);
                    if big_endian { tmp.reverse(); }
                    let mut acc: u64 = 0;
                    for b in &tmp {
                        acc = (acc << 8) | *b as u64;
                    }
                    Some(acc as f64)
                }
                _ => None,
            };
            let s = match parsed {
                Some(v) if v.is_finite() => {
                    let mut sc = Scaled::raw_counts(v);
                    sc.raw_repr = None;
                    let diag = if v < 0.0 { Some("负原始计数（可能越界）") }
                        else if v > 262143.0 { Some("超过 18-bit 上限 262143，可能为饱和/极端信号") }
                        else { None };
                    if let Some(d) = diag {
                        sc.diagnostics.push(d.to_string());
                    }
                    let _ = raw_repr;
                    sc
                }
                Some(v) => {
                    let sc = Scaled { raw: format!("{}", v), value: None, unit: "counts".to_string(), diagnostics: vec!["非有限原始值".to_string()] };
                    unconvertible.push((e, p, "non-finite".to_string()));
                    sc
                }
                None => Scaled::unconvertible(raw_repr, "无法解码"),
            };
            vals.push(s);
        }
        let point_diags: Vec<String> = vals.iter().flat_map(|s| s.diagnostics.clone()).collect();
        points.push(EventPoint { channels: vals, diagnostics: point_diags });
    }

    Ok(FcsEvents { fcs_version: version, channels, points, unconvertible })
}

/// 生成最小 FCS3.1（F 型、小端），供离线案例包使用；返回原始字节。
pub fn build_fcs31(channels: &[(&str, &str)], values: &[Vec<f32>]) -> Vec<u8> {
    let n_par = channels.len();
    let n_tot = values.len();
    let mut kvs: Vec<String> = vec![
        "$BEGINANALYSIS".to_string(), "0".to_string(),
        "$ENDANALYSIS".to_string(), "0".to_string(),
        "$BEGINDATA".to_string(), "__DATASTART__".to_string(),
        "$ENDDATA".to_string(), "__DATAEND__".to_string(),
        "$DATATYPE".to_string(), "F".to_string(),
        "$BYTEORD".to_string(), "1,2,3,4".to_string(),
        "$MODE".to_string(), "L".to_string(),
        "$NEXTDATA".to_string(), "0".to_string(),
        "$PAR".to_string(), n_par.to_string(),
        "$TOT".to_string(), n_tot.to_string(),
    ];
    for (i, (name, label)) in channels.iter().enumerate() {
        kvs.push(format!("$P{}B", i + 1)); kvs.push("32".to_string());
        kvs.push(format!("$P{}R", i + 1)); kvs.push("262144".to_string());
        kvs.push(format!("$P{}E", i + 1)); kvs.push("0,0".to_string());
        kvs.push(format!("$P{}N", i + 1)); kvs.push(name.to_string());
        kvs.push(format!("$P{}S", i + 1)); kvs.push(label.to_string());
    }
    let mut text = String::from("|");
    for kv in kvs {
        text.push('|');
        text.push_str(&kv.replace('|', "||"));
    }
    text.push('|');
    let mut text_bytes = text.as_bytes().to_vec();
    // TEXT: 10.. (header offsets fixed width)
    let text_start = 256usize;
    let data_start = text_start + text_bytes.len();
    let mut data = Vec::new();
    for row in values {
        for v in row {
            data.extend_from_slice(&v.to_le_bytes());
        }
    }
    let data_end = data_start + data.len() - 1;
    let text_str = String::from_utf8(text_bytes.clone()).unwrap();
    let text_str = text_str.replace("__DATASTART__", &data_start.to_string()).replace("__DATAEND__", &data_end.to_string());
    text_bytes = text_str.as_bytes().to_vec();

    let mut out = vec![0x20u8; 256];
    out[0..6].copy_from_slice(b"FCS3.1");
    out[6..10].copy_from_slice(b"    ");
    let write_off = |buf: &mut [u8], at: usize, val: usize| {
        let s = format!("{:>8}", val);
        buf[at..at + 8].copy_from_slice(s.as_bytes());
    };
    write_off(&mut out, 10, text_start);
    write_off(&mut out, 18, data_start - 1);
    write_off(&mut out, 26, data_end);
    out.extend_from_slice(&text_bytes);
    out.extend_from_slice(&data);
    out
}
