//! 确定性规范化：事件身份（sha256）基于规范化 JSON，保证导出/删除/导入后身份一致。

use serde_json::Value;
use sha2::{Digest, Sha256};

fn write_canonical(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => {
            // serde_json 保证字符串转义确定；复用其转义。
            out.push(b'"');
            let escaped = serde_json::to_string(s).expect("string");
            out.extend_from_slice(escaped[1..escaped.len() - 1].as_bytes());
            out.push(b'"');
        }
        Value::Array(a) => {
            out.push(b'[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(&Value::String((*key).clone()), out);
                out.push(b':');
                write_canonical(&o[*key], out);
            }
            out.push(b'}');
        }
    }
}

pub fn to_canonical_bytes<T: serde::Serialize>(t: &T) -> Vec<u8> {
    let v = serde_json::to_value(t).expect("serialize to value");
    let mut out = Vec::new();
    write_canonical(&v, &mut out);
    out
}

pub fn digest_hex<T: serde::Serialize>(t: &T) -> String {
    let mut h = Sha256::new();
    h.update(&to_canonical_bytes(t));
    hex_encode(&h.finalize())
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
