//! 稳定身份与内容指纹。
//!
//! 身份（identity）回答“这是什么对象”，版本（revision）回答“这是该对象的哪一版内容”。
//! 导出归档 → 删除本地库 → 重新导入后，二者都必须保持不变。

use sha2::{Digest, Sha256};

/// 计算任意可序列化内容的 SHA-256 指纹（十六进制），作为内容版本号 `rev`。
pub fn content_rev(canonical: &str) -> String {
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    hex::encode(h.finalize())
}

/// 对多段输入做组合指纹（顺序敏感）。
pub fn content_rev_parts(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    hex::encode(h.finalize())
}

/// 由前缀 + 人类可读键构造稳定身份，例如 `gate#cells`、`matrix#spill-v1`。
pub fn stable_id(kind: &str, key: &str) -> String {
    format!("{kind}#{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev_is_deterministic_and_order_sensitive() {
        assert_eq!(content_rev("abc"), content_rev("abc"));
        assert_ne!(content_rev("abc"), content_rev("abd"));
        assert_ne!(content_rev_parts(&["a", "b"]), content_rev_parts(&["b", "a"]));
        // 分段编码避免 ["ab"] 与 ["a","b"] 碰撞
        assert_ne!(content_rev_parts(&["ab"]), content_rev_parts(&["a", "b"]));
    }

    #[test]
    fn stable_id_roundtrips() {
        assert_eq!(stable_id("gate", "cells"), "gate#cells");
    }
}
