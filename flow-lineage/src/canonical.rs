//! Canonical JSON so content-addressed IDs stay stable across export/import.
//! Field order is taken from serde struct order; object keys are sorted.

use serde::Serialize;

pub fn canonical<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serialize");
    let normalized = normalize(v);
    let mut buf = Vec::new();
    write(&normalized, &mut buf);
    String::from_utf8(buf).expect("utf8")
}

fn normalize(v: serde_json::Value) -> serde_json::Value {
    use serde_json::{Map, Value};
    match v {
        Value::Object(map) => {
            let mut sorted: Map<String, Value> = Map::new();
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                sorted.insert(k, normalize(map.get(&k).cloned().unwrap()));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}

fn write(v: &serde_json::Value, buf: &mut Vec<u8>) {
    use serde_json::Value;
    match v {
        Value::Null => buf.extend_from_slice(b"null"),
        Value::Bool(b) => buf.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => buf.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => write_str(s, buf),
        Value::Array(items) => {
            buf.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                write(item, buf);
            }
            buf.push(b']');
        }
        Value::Object(map) => {
            buf.push(b'{');
            for (i, (k, val)) in map.iter().enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                write_str(k, buf);
                buf.push(b':');
                write(val, buf);
            }
            buf.push(b'}');
        }
    }
}

fn write_str(s: &str, buf: &mut Vec<u8>) {
    // JSON string escaping.
    buf.push(b'"');
    for c in s.chars() {
        match c {
            '"' => buf.extend_from_slice(b"\\\""),
            '\\' => buf.extend_from_slice(b"\\\\"),
            '\n' => buf.extend_from_slice(b"\\n"),
            '\r' => buf.extend_from_slice(b"\\r"),
            '\t' => buf.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                buf.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes())
            }
            c => {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
        }
    }
    buf.push(b'"');
}
