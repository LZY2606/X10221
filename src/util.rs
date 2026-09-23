use sha2::{Digest, Sha256};

/// 稳定身份：对稳定的规范化输入取 SHA-256 的前 12 个十六进制字符。
/// 同一逻辑对象在导出/删除数据库/重新导入后必须得到相同身份。
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update(b"\x1f");
        }
        hasher.update(part.as_bytes());
    }
    format!("{}_{}", prefix, hex::encode(hasher.finalize())[..12].to_string())
}

/// 对原始字节取内容指纹。
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// 规范化 JSON（保留字段顺序由 serde_json::preserve_order 保证），用于哈希与去重。
pub fn canonical_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("canonical json serialization is infallible")
}

/// 以稳定方式把 f64 渲染成身份字符串（避免 -0/NaN 影响）。
pub fn num_id(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_string()
    } else if x.is_infinite() {
        if x > 0.0 { "Inf".to_string() } else { "-Inf".to_string() }
    } else if x == 0.0 {
        "0".to_string()
    } else {
        // 固定精度，保证跨进程一致
        let s = format!("{:.10e}", x);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
