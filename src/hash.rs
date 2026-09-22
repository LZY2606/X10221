use serde_json::Value;
use sha2::{Digest, Sha256};

/// Canonical JSON serialization: object keys sorted, no extra whitespace.
/// This keeps content-addressed identities stable across export/import.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            let v = Value::String(s.clone());
            out.push_str(&v.to_string());
        }
        Value::Array(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 { out.push(','); }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

pub fn blob_id(data: &[u8]) -> String {
    format!("blob_{}", &sha256_hex(data)[..16])
}

/// Content id for an artifact payload (kind-independent).
pub fn artifact_id(kind: &str, logical_id: &str, version: i64, payload: &Value) -> String {
    let mut h = Sha256::new();
    h.update(canonical(payload).as_bytes());
    h.update(kind.as_bytes());
    h.update(logical_id.as_bytes());
    h.update(version.to_string().as_bytes());
    format!("art_{}", &hex::encode(h.finalize())[..12])
}

/// Content id for a derived result group definition.
pub fn group_id(scope: &str, refs: &Value) -> String {
    let mut h = Sha256::new();
    h.update(scope.as_bytes());
    h.update(canonical(refs).as_bytes());
    format!("grp_{}", &hex::encode(h.finalize())[..12])
}

/// Id for an individual derived result row (candidate-specific).
pub fn derived_id(group: &str, kind: &str, candidate: Option<i64>, payload: &Value) -> String {
    let mut h = Sha256::new();
    h.update(group.as_bytes());
    h.update(kind.as_bytes());
    if let Some(c) = candidate { h.update(c.to_string().as_bytes()); }
    h.update(canonical(payload).as_bytes());
    format!("der_{}", &hex::encode(h.finalize())[..12])
}

/// Event id binds (branch, seq, kind, parent, payload) so tampering is detectable.
pub fn event_id(branch: &str, seq: i64, kind: &str, parent: Option<&str>, payload: &Value) -> String {
    let mut h = Sha256::new();
    h.update(branch.as_bytes());
    h.update(seq.to_string().as_bytes());
    h.update(kind.as_bytes());
    if let Some(p) = parent { h.update(p.as_bytes()); }
    h.update(canonical(payload).as_bytes());
    format!("evt_{}", &hex::encode(h.finalize())[..12])
}
