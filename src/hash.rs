// 内容指纹：FNV-1a 64bit（无外部依赖），用于工件版本与稳定身份。
// 导出 -> 删除库 -> 再导入后身份保持一致，因为身份由内容决定。

pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn fingerprint_str(s: &str) -> String {
    fingerprint_bytes(s.as_bytes())
}

pub fn fingerprint_json<T: serde::Serialize>(value: &T) -> String {
    let s = serde_json::to_vec(value).expect("serialize fingerprint");
    fingerprint_bytes(&s)
}
