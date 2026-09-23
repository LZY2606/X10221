use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 确定性哈希（DefaultHasher::new 使用固定密钥，跨进程稳定），
/// 用于内容寻址身份：导出/删除数据库/再导入后身份不变。
pub fn stable_hash(parts: &[&str]) -> String {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// 版本身份 = 内容寻址（kind + 父版本 + 规范化负载）。
/// serde_json 默认 BTreeMap，键序稳定，序列化结果确定。
pub fn version_id(kind: &str, parent: Option<&str>, payload: &Value) -> String {
    let canonical = serde_json::to_string(payload).unwrap_or_default();
    let h = stable_hash(&[kind, parent.unwrap_or("-"), &canonical]);
    let prefix: String = kind.chars().take(4).collect();
    format!("{prefix}-{h}")
}

pub fn short(id: &str) -> String {
    id.chars().take(14).collect()
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hash_is_deterministic_and_content_addressed() {
        let p = json!({"b":1,"a":2});
        let q = json!({"a":2,"b":1});
        assert_eq!(version_id("gates", None, &p), version_id("gates", None, &q));
        assert_ne!(version_id("gates", None, &p), version_id("gates", Some("x"), &p));
        assert_ne!(version_id("gates", None, &p), version_id("transform", None, &p));
    }
}
