//! 稳定身份：内容寻址 ID 与确定性规范化。
//!
//! 所有“来源/解释”材料以 *规范化字节* 的 SHA-256 得到稳定身份，
//! 因此导出 -> 删除数据库 -> 再导入后身份不变。浮点数使用最短
//! 可往返表示，对象键按字典序排序，数组保持顺序。

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 把任意可序列化值转换为“规范化 JSON 字节”。
/// - 对象按键排序；
/// - 数组保序；
/// - 浮点数使用最短 round-trip 表示，整数保持整数。
pub fn canonical_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    let mut v = serde_json::to_value(value).expect("canonical input must serialize");
    normalize(&mut v);
    let mut out = Vec::new();
    write_canonical(&v, &mut out);
    out
}

/// 内容寻址 ID：`<kind>_<sha256(canonical)[:16]>`。
pub fn content_id(kind: &str, value: &Value) -> String {
    let mut v = value.clone();
    normalize(&mut v);
    let mut out = Vec::new();
    write_canonical(&v, &mut out);
    let digest = Sha256::digest(&out);
    format!("{kind}_{}", hex16(&digest))
}

fn hex16(digest: &[u8]) -> String {
    digest
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn normalize(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (_, child) in map.iter_mut() {
                normalize(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalize(item);
            }
        }
        _ => {}
    }
}

fn write_canonical(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => {
            // 3.0 与 3 规范化为同一表示：优先以整数写出无小数的值。
            if n.is_f64() {
                if let Some(f) = n.as_f64() {
                    if f.fract() == 0.0 && f.is_finite() {
                        out.extend_from_slice((f as i64).to_string().as_bytes());
                    } else {
                        out.extend_from_slice(n.to_string().as_bytes());
                    }
                } else {
                    out.extend_from_slice(n.to_string().as_bytes());
                }
            } else if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else {
                out.extend_from_slice(n.to_string().as_bytes());
            }
        }
        Value::String(s) => {
            let escaped = serde_json::to_string(s).expect("string escapes");
            out.extend_from_slice(escaped.as_bytes());
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                let escaped = serde_json::to_string(key).unwrap();
                out.extend_from_slice(escaped.as_bytes());
                out.push(b':');
                write_canonical(&map[*key], out);
            }
            out.push(b'}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_change_id() {
        let a = json!({"b": 1, "a": [1, 2.5, 0.0], "c": {"z": true, "y": null}});
        let b = json!({"c": {"y": null, "z": true}, "a": [1, 2.5, 0.0], "b": 1});
        assert_eq!(content_id("x", &a), content_id("x", &b));
    }

    #[test]
    fn array_order_changes_id() {
        let a = json!({"v": [1, 2]});
        let b = json!({"v": [2, 1]});
        assert_ne!(content_id("x", &a), content_id("x", &b));
    }

    #[test]
    fn floats_roundtrip_stably() {
        let a = json!({"x": 0.1_f64 + 0.2_f64});
        let b = json!({"x": 0.30000000000000004_f64});
        assert_eq!(content_id("x", &a), content_id("x", &b));
        let c = json!({"x": 3.0_f64});
        let d = json!({"x": 3});
        assert_eq!(content_id("x", &c), content_id("x", &d));
    }
}
