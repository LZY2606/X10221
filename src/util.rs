//! 稳定身份：FNV-1a 64 位内容哈希（跨平台确定性，不依赖随机种子）。

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 由确定性键生成带前缀的稳定身份。
pub fn stable_id(prefix: &str, key: &str) -> String {
    format!("{prefix}-{:016x}", fnv1a64(key.as_bytes()))
}
