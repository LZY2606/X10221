//! 极简 base64（导出档案中内嵌原始字节用，避免额外依赖）。

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let mut table = [255u8; 256];
    for (i, c) in ALPHABET.iter().enumerate() {
        table[*c as usize] = i as u8;
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b" \r\n\t".contains(b)).collect();
    if bytes.len() % 4 != 0 {
        return Err("base64 长度不是 4 的倍数".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|&&b| b == b'=').count();
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            let v = if b == b'=' { 0 } else { table[b as usize] };
            if b != b'=' && v == 255 {
                return Err(format!("非法 base64 字符: {}", b as char));
            }
            n |= (v as u32) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"hello world", &[0u8, 255, 1, 254, 128][..]] {
            assert_eq!(decode(&encode(data)).unwrap(), data);
        }
    }
}
