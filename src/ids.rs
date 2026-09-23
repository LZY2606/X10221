use serde_json::Value;
use sha2::{Digest, Sha256};

/// Canonicalize a serde_json value deterministically: object keys sorted,
/// whitespace removed. Used so identical content always hashes identically,
/// including across export/import of the archive.
pub fn canonical_json(v: &Value) -> String {
    let mut out = String::new();
    emit(v, &mut out);
    out
}

fn emit(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            // Preserve f64 bit pattern via round-trip; serde_json ryu is deterministic.
            out.push_str(&n.to_string());
        }
        Value::String(s) => emit_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                emit(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                emit_str(k, out);
                out.push(':');
                emit(&o[*k], out);
            }
            out.push('}');
        }
    }
}

fn emit_str(s: &str, out: &mut String) {
    // JSON escape via serde_json (handles all unicode/control chars).
    let q = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    out.push_str(&q);
}

pub fn hash_id(prefix: &str, v: &Value) -> String {
    let mut h = Sha256::new();
    h.update(canonical_json(v).as_bytes());
    let d = h.finalize();
    format!("{}_{}", prefix, &hex_lower(&d)[..24])
}

pub fn hash_bytes_id(prefix: &str, b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    let d = h.finalize();
    format!("{}_{}", prefix, &hex_lower(&d)[..24])
}

pub fn hex_lower(d: &[u8]) -> String {
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
