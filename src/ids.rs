//! 稳定身份：所有材料版本、派生结果与事件使用内容寻址/确定性身份。
//! 要求：导出、删除本地数据库再导入后，身份、顺序与来源关系仍保持。

use serde::Serialize;
use sha2::{Digest, Sha256};

/// 规范化 JSON（键排序、无空白），保证不同进程/语言下哈希一致。
pub fn canonical_json<T: Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serialize");
    canonical_value(&v)
}

fn canonical_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap(),
        serde_json::Value::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_value).collect();
            format!("[{}]", inner.join(","))
        }
        serde_json::Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_value(&o[*k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// 材料版本身份：`mat_<kind>_v<seq>_<hash12>`，由种类、序号与正文哈希钉住。
pub fn material_id(kind: &str, seq: u32, body: &impl Serialize) -> String {
    let h = sha256_hex(canonical_json(body).as_bytes());
    format!("mat_{}_v{:02}_{}", kind, seq, &h[..12])
}

/// 原始字节 blob 身份。
pub fn blob_id(bytes: &[u8]) -> String {
    format!("blob_{}", &sha256_hex(bytes)[..16])
}

/// 派生结果身份：`der_<kind>_<hash12>`。
pub fn derived_id(kind: &str, body: &impl Serialize) -> String {
    let h = sha256_hex(canonical_json(body).as_bytes());
    format!("der_{}_{}", kind, &h[..12])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn canonical_is_key_order_independent() {
        let mut a = BTreeMap::new();
        a.insert("x", 1);
        a.insert("y", 2);
        let s1 = canonical_json(&a);
        let s2 = canonical_json(&serde_json::json!({"y": 2, "x": 1}));
        assert_eq!(s1, s2);
        assert_eq!(s1, "{\"x\":1,\"y\":2}");
    }

    #[test]
    fn material_id_is_deterministic() {
        let id1 = material_id("comp", 1, &serde_json::json!([[1.0, 0.2]]));
        let id2 = material_id("comp", 1, &serde_json::json!([[1.0, 0.2]]));
        assert_eq!(id1, id2);
        assert!(id1.starts_with("mat_comp_v01_"));
    }
}
