//! 最小 base64 标准字母表实现（仅用于归档字节往返，避免外部依赖）。

const TABLE: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 63) as usize] as char);
        out.push(TABLE[((triple >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn decode(input: &str) -> Result<Vec<u8>, String> {
    let mut buf: Vec<u8> = Vec::new();
    let chars: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let value = |b: u8| -> Result<u32, String> {
        TABLE
            .iter()
            .position(|t| *t == b)
            .map(|p| p as u32)
            .ok_or_else(|| format!("invalid base64 byte: {b}"))
    };
    for chunk in chars.chunks(4) {
        if chunk.len() != 4 {
            return Err("base64 input length must be multiple of 4".into());
        }
        let mut vals = [0u32; 4];
        let mut pad = 0;
        for i in 0..4 {
            vals[i] = if chunk[i] == b'=' {
                pad += 1;
                0
            } else {
                value(chunk[i])?
            };
        }
        let triple = (vals[0] << 18) | (vals[1] << 12) | (vals[2] << 6) | vals[3];
        buf.push((triple >> 16) as u8);
        if pad < 2 {
            buf.push((triple >> 8) as u8);
        }
        if pad == 0 {
            buf.push(triple as u8);
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for case in [&b""[..], b"f", b"fo", b"foo", b"some FCS bytes \x00\x01\xff", &vec![7u8; 100]] {
            let e = encode(case);
            let d = decode(&e).unwrap();
            assert_eq!(&d, case);
        }
    }
}
