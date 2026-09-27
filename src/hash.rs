//! Deterministic, content-addressed stable identities.
//!
//! Every versioned source object, derived object and event gets an id derived
//! from the canonical JSON of its payload. Canonical JSON recursively sorts
//! object keys so ids do not depend on field insertion order.

use serde_json::Value;
use sha2::{Digest, Sha256};

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize(v));
            }
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(k, v)| (k, v))
                    .collect::<serde_json::Map<String, Value>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

pub fn canonical_json<T: serde::Serialize>(value: &T) -> String {
    let raw = serde_json::to_value(value).expect("payload is serializable");
    let canon = canonicalize(&raw);
    serde_json::to_string(&canon).expect("canonical value serializes")
}

pub fn digest_hex(canonical: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// `"<prefix>_<12 hex>"` stable content id.
pub fn content_id<T: serde::Serialize>(prefix: &str, value: &T) -> String {
    format!("{}_{}", prefix, &digest_hex(&canonical_json(value))[..12])
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
