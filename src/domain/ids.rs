use sha2::{Digest, Sha256};
use serde_json::Value;

/// 规范化 JSON：对象键排序后紧凑序列化，保证身份哈希在导出/导入间稳定。
pub fn canonical_json(value: &Value) -> String {
    let canonical = canonicalize(value);
    canonical.to_string()
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize(v));
            }
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn short_hash(input: &str) -> String {
    sha256_hex(input.as_bytes())[..16].to_string()
}

/// 事件稳定身份：只依赖分支、前驱事件与规范化载荷，与自增序号无关。
pub fn event_id(branch_id: &str, parent_event_id: Option<&str>, payload: &Value) -> String {
    let mut material = serde_json::json!({
        "branch": branch_id,
        "parent": parent_event_id,
        "payload": canonicalize(payload),
    });
    material = canonicalize(&material);
    format!("ev_{}", short_hash(&canonical_json(&material)))
}

pub fn blob_id(bytes: &[u8]) -> String {
    format!("blob_{}", &sha256_hex(bytes)[..32])
}
