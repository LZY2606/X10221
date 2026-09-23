//! FCS 3.1 写入器，供 examples/gen_case.rs 生成仓库内离线案例包。

use std::collections::HashMap;

pub fn write_fcs_31(
    names: &[String],
    ranges: &[f64],
    events: &[Vec<f32>],
    extra: &HashMap<String, String>,
) -> Vec<u8> {
    let par = names.len();
    let tot = events.len();

    // TEXT 段（先占位算偏移；数据段紧跟 TEXT，所有偏移都 < 99）
    let build_text = |data_begin: usize, data_end: usize| -> Vec<u8> {
        let delim = b'|';
        let mut seg: Vec<u8> = vec![delim];
        let mut push = |k: &str, v: &str, seg: &mut Vec<u8>| {
            seg.extend_from_slice(k.as_bytes());
            seg.push(delim);
            seg.extend_from_slice(v.as_bytes());
            seg.push(delim);
        };
        push("BEGINANALYSIS", "0", &mut seg);
        push("ENDANALYSIS", "0", &mut seg);
        push("BEGINDATA", &data_begin.to_string(), &mut seg);
        push("ENDDATA", &data_end.to_string(), &mut seg);
        push("$BEGINSTEXT", "0", &mut seg);
        push("$ENDSTEXT", "0", &mut seg);
        push("$BYTEORD", "4,3,2,1", &mut seg);
        push("$DATATYPE", "F", &mut seg);
        push("$MODE", "L", &mut seg);
        push("$NEXTDATA", "0", &mut seg);
        push("$PAR", &par.to_string(), &mut seg);
        push("$TOT", &tot.to_string(), &mut seg);
        for (i, name) in names.iter().enumerate() {
            let n = i + 1;
            push(&format!("$P{n}B"), "32", &mut seg);
            push(&format!("$P{n}N"), name, &mut seg);
            push(&format!("$P{n}R"), &ranges[i].to_string(), &mut seg);
            push(&format!("$P{n}E"), "0,0", &mut seg);
        }
        let mut keys: Vec<&String> = extra.keys().collect();
        keys.sort();
        for k in keys {
            push(k, &extra[k], &mut seg);
        }
        seg
    };

    let data_len = tot * par * 4;
    // 头部 58 字节 + TEXT 段；先算一次 TEXT 长度
    let text_begin = 256usize;
    let probe = build_text(0, 0);
    let data_begin = text_begin + probe.len();
    let data_end = data_begin + data_len - 1;
    let text = build_text(data_begin, data_end);
    assert_eq!(text.len(), probe.len());

    let mut out = Vec::new();
    out.extend_from_slice(b"FCS3.1    "); // 6 + 4 空格 => 10
    let mut put32 = |v: usize, out: &mut Vec<u8>| {
        let s = format!("{:>8}", v);
        out.extend_from_slice(s.as_bytes());
    };
    put32(text_begin, &mut out); // 10..18 TEXT begin/end 共 8
    put32(text_begin + text.len() - 1, &mut out);
    put32(data_begin, &mut out); // 18..26
    put32(data_end, &mut out);
    put32(0, &mut out); // 26..34 analysis
    put32(0, &mut out);
    while out.len() < text_begin {
        out.push(b' ');
    }
    out.extend_from_slice(&text);
    for row in events {
        for v in row {
            out.extend_from_slice(&v.to_bits().to_be_bytes());
        }
    }
    out
}
