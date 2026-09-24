//! 稳定身份：内容寻址哈希 + 显式版本号。
//! 导出/导入后身份保持不变，因为 id 与 created_seq 都随数据一起序列化。
use sha2::{Digest, Sha256};

/// 对一段规范文本（serde_json 默认按键排序的 map 输出）取 sha256，截断为 16 个十六进制字符。
pub fn content_hash(canonical: &str) -> String {
    let digest = Sha256::digest(canonical.as_bytes());
    digest[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

/// 对 serde_json::Value 计算稳定内容哈希（Map 默认 BTreeMap，键序确定）。
pub fn hash_value(v: &serde_json::Value) -> String {
    content_hash(&serde_json::to_string(v).unwrap_or_default())
}

pub fn artifact_key(kind: &str, logical_id: &str, version: u32) -> String {
    format!("{}:{}@{}", kind, logical_id, version)
}
