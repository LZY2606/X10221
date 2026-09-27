//! Stable hashing and canonical JSON helpers.
//!
//! Identities in this system are content-derived so that export -> delete ->
//! import preserves every identifier. FNV-1a (64 bit) is deterministic and
//! dependency free; it is an identity hash, not a security hash.

pub const RULE_VERSION: &str = "rule-v1";

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn content_id(prefix: &str, value: &serde_json::Value) -> String {
    let data = canonical(value);
    format!("{prefix}_{:016x}", fnv1a64(&data))
}

/// Recursively sort object keys so JSON serialization is canonical.
pub fn canonical(value: &serde_json::Value) -> Vec<u8> {
    let sorted = sort_value(value);
    let mut buf = Vec::new();
    sorted
        .to_writer(&mut buf)
        .expect("serializing canonical json into a Vec cannot fail");
    buf
}

fn sort_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut ordered: std::collections::BTreeMap<&String, serde_json::Value> =
                std::collections::BTreeMap::new();
            for (k, v) in map {
                ordered.insert(k, sort_value(v));
            }
            let obj: serde_json::Map<String, serde_json::Value> = ordered
                .into_iter()
                .map(|(k, v)| (k.clone(), v))
                .collect();
            serde_json::Value::Object(obj)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(sort_value).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalization_is_key_order_independent() {
        let a = json!({"b": 1, "a": [1, 2, {"z": 0, "y": 1}]});
        let b = json!({"a": [1, 2, {"y": 1, "z": 0}], "b": 1});
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(content_id("mat", &a), content_id("mat", &b));
    }
}
