//! 稳定身份（stable identity）原语：所有领域对象的 id 都是确定性的内容/谱系标识，
//! 不依赖数据库 rowid，因此导出 -> 删除本地库 -> 再导入后身份保持不变。

use sha2::{Digest, Sha256};

/// 形如 `ev_3f8a…` 的稳定 id。
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        h.update((i as u64).to_le_bytes());
        let b = p.as_bytes();
        h.update((b.len() as u64).to_le_bytes());
        h.update(b);
    }
    format!("{}_{}", prefix, hex::encode(h.finalize()))
}

pub fn hash_bytes(prefix: &str, bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{}_{}", prefix, hex::encode(h.finalize()))
}

/// 派生工件的“流身份”：同一逻辑对象的不同版本共享 stream_id，
/// 版本之间用 version 区分；artifact_id 是某一具体版本的稳定身份。
pub fn stream_id(kind: &str, logical: &str) -> String {
    stable_id("st", &[kind, logical])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_order_sensitive() {
        let a = stable_id("ev", &["x", "y"]);
        let b = stable_id("ev", &["x", "y"]);
        let c = stable_id("ev", &["y", "x"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("ev_"));
    }
}
