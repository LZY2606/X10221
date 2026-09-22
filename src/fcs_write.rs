//! 离线案例包 FCS 3.1 文件的确定性写出器（工具与测试用）。
//!
//! 布局：HEADER(58) + TEXT + DATA。TEXT 为 ASCII，以 '|' 分隔，
//! 大端 32 位浮点。

use std::collections::BTreeMap;

pub fn write_fcs_3_1(channels: &[&str], events: &[Vec<f32>]) -> Vec<u8> {
    write_fcs_3_1_named(channels.iter().map(|s| s.to_string()).collect(), events, &[], true)
}

/// extra: 额外 (key,value) 关键字；big_endian=false 时写 $BYTEORD=1,2,3,4。
pub fn write_fcs_3_1_named(
    channels: Vec<String>,
    events: &[Vec<f32>],
    extra: &[(&str, &str)],
    big_endian: bool,
) -> Vec<u8> {
    let par = channels.len();
    let tot = events.len();
    let mut kws: BTreeMap<String, String> = BTreeMap::new();
    kws.insert("$BEGINANALYSIS".into(), "0".into());
    kws.insert("$ENDANALYSIS".into(), "0".into());
    kws.insert("$BEGINDATA".into(), "__DS__".into());
    kws.insert("$ENDDATA".into(), "__DE__".into());
    kws.insert("$BEGINSTEXT".into(), "0".into());
    kws.insert("$ENDSTEXT".into(), "0".into());
    kws.insert("$BYTEORD".into(), if big_endian { "4,3,2,1".into() } else { "1,2,3,4".into() });
    kws.insert("$DATATYPE".into(), "F".into());
    kws.insert("$MODE".into(), "L".into());
    kws.insert("$NEXTDATA".into(), "0".into());
    kws.insert("$PAR".into(), par.to_string());
    kws.insert("$TOT".into(), tot.to_string());
    for (i, name) in channels.iter().enumerate() {
        kws.insert(format!("$P{}B", i + 1), "32".into());
        kws.insert(format!("$P{}E", i + 1), "0,0".into());
        kws.insert(format!("$P{}N", i + 1), name.clone());
        kws.insert(format!("$P{}R", i + 1), "262144".into());
    }
    for (k, v) in extra {
        kws.insert((*k).to_string(), (*v).to_string());
    }

    // 先构造 TEXT（偏移占位），再回填。
    let header_placeholder = vec![b' '; 58];
    let build_text = |ds: usize, de: usize| -> Vec<u8> {
        let mut s = String::from("|");
        let mut entries: Vec<(String, String)> = kws
            .iter()
            .map(|(k, v)| {
                let v = match (k.as_str(), v.as_str()) {
                    ("$BEGINDATA", "__DS__") => ds.to_string(),
                    ("$ENDDATA", "__DE__") => de.to_string(),
                    _ => v.clone(),
                };
                (k.clone(), v)
            })
            .collect();
        // $BEGINDATA/$ENDDATA 放最前，保证不依赖自身长度的鸡生蛋问题最小化。
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in entries {
            s.push_str(&k.replace('|', "||"));
            s.push('|');
            s.push_str(&v.replace('|', "||"));
            s.push('|');
        }
        s.into_bytes()
    };

    // 迭代求偏移。
    let mut text = build_text(0, 0);
    let mut text_start = 58usize;
    let mut text_end = text_start + text.len() - 1;
    let mut data_start = text_end + 1;
    let mut data_len = tot * par * 4;
    let mut data_end = data_start + data_len - 1;
    text = build_text(data_start, data_end);
    // TEXT 长度可能变化（偏移位数变化），最多迭代几次。
    for _ in 0..5 {
        let new_text_end = text_start + text.len() - 1;
        let new_data_start = new_text_end + 1;
        let new_data_end = new_data_start + data_len - 1;
        if new_data_start == data_start && new_data_end == data_end && new_text_end == text_end {
            break;
        }
        data_start = new_data_start;
        data_end = new_data_end;
        text_end = new_text_end;
        text = build_text(data_start, data_end);
    }

    let mut out = header_placeholder.clone();
    {
        let write_range = |buf: &mut [u8], off: usize, val: &str, len: usize| {
            let s = if val.len() > len {
                format!("{:>width$}", val, width = len)
            } else {
                format!("{:<width$}", val, width = len)
            };
            buf[off..off + len].copy_from_slice(s.as_bytes());
        };
        write_range(&mut out, 0, "FCS3.1", 6);
        write_range(&mut out, 6, " ", 4);
        write_range(&mut out, 10, &text_start.to_string(), 8);
        write_range(&mut out, 18, &text_end.to_string(), 8);
        write_range(&mut out, 26, &data_start.to_string(), 8);
        write_range(&mut out, 34, &data_end.to_string(), 8);
        write_range(&mut out, 42, "0", 8);
        write_range(&mut out, 50, "0", 8);
    }
    out.extend_from_slice(&text);
    for row in events {
        for v in row {
            let b = if big_endian { v.to_bits().to_be_bytes() } else { v.to_bits().to_le_bytes() };
            out.extend_from_slice(&b);
        }
    }
    out
}
