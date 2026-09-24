//! 最小 FCS 3.1 读取器：只依赖 TEXT 段关键字与 $DATATYPE F（big-endian float32）。
//! 导入时保留原始字节（bytes_sha）与原始文本表示，无法转换的值留在原位置并附诊断。

use anyhow::{anyhow, bail, Result};
use sha2_lite::Sha256;
use std::collections::BTreeMap;

pub struct FcsFile {
    pub keywords: BTreeMap<String, String>,
    /// 行=事件，列=参数顺序；None 表示该值无法转换（例如溢出标记），原始表示保留在 raw 中。
    pub events: Vec<Vec<Option<f64>>>,
    pub raw: Vec<Vec<String>>,
    pub param_names: Vec<String>,
    pub bytes_sha: String,
    pub diagnostics: Vec<String>,
}

mod sha2_lite {
    pub struct Sha256 {
        h: [u32; 8],
        buf: Vec<u8>,
        len: u64,
    }
    impl Sha256 {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        pub fn new() -> Self {
            Sha256 {
                h: [
                    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                    0x1f83d9ab, 0x5be0cd19,
                ],
                buf: Vec::new(),
                len: 0,
            }
        }
        pub fn update(&mut self, data: &[u8]) {
            self.buf.extend_from_slice(data);
            self.len += data.len() as u64;
            while self.buf.len() >= 64 {
                let block: [u8; 64] = self.buf[..64].try_into().unwrap();
                self.buf.drain(..64);
                self.block(&block);
            }
        }
        fn block(&mut self, block: &[u8; 64]) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) = (
                self.h[0], self.h[1], self.h[2], self.h[3], self.h[4], self.h[5], self.h[6],
                self.h[7],
            );
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(Self::K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            for (i, v) in [a, b, c, d, e, f, g, h].iter().enumerate() {
                self.h[i] = self.h[i].wrapping_add(*v);
            }
        }
        pub fn finalize(mut self) -> String {
            let bit_len = self.len * 8;
            self.buf.push(0x80);
            while self.buf.len() % 64 != 56 {
                self.buf.push(0);
            }
            self.buf.extend_from_slice(&bit_len.to_be_bytes());
            while let Some(chunk) = self.buf.chunks_exact(64).next() {
                let block: [u8; 64] = chunk.try_into().unwrap();
                self.block(&block);
                self.buf.drain(..64);
            }
            self.h.iter().map(|v| format!("{:08x}", v)).collect()
        }
    }
}

fn parse_text_segment(data: &[u8], start: usize, end: usize) -> Result<BTreeMap<String, String>> {
    if start >= end || end > data.len() {
        bail!("TEXT 段偏移无效 {}-{}", start, end);
    }
    let delim = data[start] as char;
    let body = std::str::from_utf8(&data[start + 1..=end - 1])
        .map_err(|e| anyhow!("TEXT 段不是合法 UTF-8: {e}"))?;
    let mut map = BTreeMap::new();
    // 双分隔符为转义
    let parts: Vec<&str> = body.split(delim).collect();
    let mut i = 0;
    while i + 1 < parts.len() {
        let key = parts[i];
        let mut val = parts[i + 1].to_string();
        i += 2;
        if key.is_empty() {
            continue;
        }
        map.insert(key.to_string(), val);
        let _ = &mut val;
    }
    Ok(map)
}

pub fn parse_fcs(data: &[u8]) -> Result<FcsFile> {
    if data.len() < 58 || &data[0..6] != b"FCS3.1" {
        bail!("不是 FCS3.1 文件");
    }
    let off = |a: usize, b: usize| -> Result<usize> {
        std::str::from_utf8(&data[a..=b])?
            .trim()
            .parse::<usize>()
            .map_err(|e| anyhow!("头部偏移解析失败: {e}"))
    };
    // header 偏移（1-based 含端点）
    let text_s = off(10, 17)?;
    let text_e = off(18, 25)?;
    let data_s = off(26, 33)?;
    let data_e = off(34, 41)?;
    let keywords = parse_text_segment(data, text_s, text_e)?;
    let need = |k: &str| -> Result<String> {
        keywords
            .get(k)
            .cloned()
            .ok_or_else(|| anyhow!("缺少关键字 {k}"))
    };
    let datatype = need("$DATATYPE")?;
    if datatype != "F" {
        bail!("案例仅支持 $DATATYPE=F（32 位浮点），实际 {datatype}");
    }
    if need("$BYTEORD")? != "4,3,2,1" {
        bail!("案例仅支持 big-endian 字节序 $BYTEORD=4,3,2,1");
    }
    let n_params: usize = need("$PAR")?.parse()?;
    let n_events: usize = if let Some(v) = keywords.get("$TOT") {
        v.parse()?
    } else {
        0
    };
    let mut param_names = Vec::with_capacity(n_params);
    for i in 1..=n_params {
        let nm = keywords
            .get(&format!("$P{}N", i))
            .cloned()
            .unwrap_or_else(|| format!("P{}", i));
        param_names.push(nm);
    }
    let mut hasher = Sha256::new();
    hasher.update(data);
    let bytes_sha = hasher.finalize();

    // 溢出事件表（自定义关键字 OVREVENTS）："event_idx:param_idx,..."（0-based）
    let mut ovr: BTreeMap<(usize, usize), String> = BTreeMap::new();
    if let Some(tbl) = keywords.get("OVREVENTS") {
        for pair in tbl.split(',') {
            let mut it = pair.split(':');
            if let (Some(e), Some(p)) = (it.next(), it.next()) {
                if let (Ok(e), Ok(p)) = (e.parse::<usize>(), p.parse::<usize>()) {
                    ovr.insert((e, p), "OVR".into());
                }
            }
        }
    }

    let mut diagnostics = Vec::new();
    let mut events = Vec::new();
    let mut raw = Vec::new();
    if data_e >= data_s && n_params > 0 && n_events > 0 {
        let seg = &data[data_s..=data_e.min(data.len() - 1)];
        for e in 0..n_events {
            let mut row = Vec::with_capacity(n_params);
            let mut raw_row = Vec::with_capacity(n_params);
            for p in 0..n_params {
                let pos = (e * n_params + p) * 4;
                if pos + 4 > seg.len() {
                    bail!("DATA 段长度不足");
                }
                let bits = u32::from_be_bytes(seg[pos..pos + 4].try_into().unwrap());
                let f = f32::from_bits(bits) as f64;
                if ovr.contains_key(&(e, p)) {
                    diagnostics.push(format!("事件 {} 通道 {} 溢出 OVR，原始值保留", e, param_names[p]));
                    row.push(None);
                    raw_row.push("OVR".into());
                } else if !f.is_finite() {
                    diagnostics.push(format!(
                        "事件 {} 通道 {} 非有限值 {}，保留为不可转换",
                        e,
                        param_names[p],
                        f
                    ));
                    row.push(None);
                    raw_row.push(format!("{:e}", f));
                } else {
                    row.push(Some(f));
                    raw_row.push(format!("{}", f));
                }
            }
            events.push(row);
            raw.push(raw_row);
        }
    }
    Ok(FcsFile {
        keywords,
        events,
        raw,
        param_names,
        bytes_sha,
        diagnostics,
    })
}
