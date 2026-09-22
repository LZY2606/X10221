//! 案例摄入：六类材料分开版本化写入；失败在单个事务内回滚。
//! 原始表示（Measured.raw、FCS 文本字节哈希）保留，派生层只做规范化。

use crate::db::Store;
use crate::domain::*;
use crate::repository::sha256_hex;
use rusqlite::params;
use serde_json::Value;

pub const TYPES: [(&str, &str); 6] = [
    ("raw_events", "raw_events"),
    ("spillover_meta", "spillover_meta"),
    ("compensation", "compensation"),
    ("transform", "transform"),
    ("gates", "gates"),
    ("hierarchy", "hierarchy"),
];

/// 完整案例包校验：缺任一必需材料即拒绝（回滚调用方事务）。
pub fn validate_case(case: &Value) -> Result<(), String> {
    let mats = case.get("materials").ok_or("missing materials")?;
    for (key, _) in TYPES {
        let v = mats.get(key).ok_or_else(|| format!("missing material section: {key}"))?;
        if key == "gates" {
            let arr = v.as_array().filter(|a| a.len() >= 3).ok_or("gates must be a non-empty array")?;
            for g in arr {
                let _ = serde_json::from_value::<Gate>(g.clone())
                    .map_err(|e| format!("invalid gate: {e}"))?;
            }
        } else if v.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            return Err(format!("material section empty: {key}"));
        }
    }
    // 原始通道值必须保留 raw 表示；缺 raw 的包拒绝
    for ev in mats["raw_events"][0]["events"].as_array().unwrap() {
        for ch in ev["channels"].as_array().unwrap() {
            let m = &ch[1];
            if m.get("raw").is_none() || m.get("unit").is_none() {
                return Err("raw representation or unit missing; irreversible cleaning forbidden".into());
            }
        }
    }
    Ok(())
}

pub fn ingest_case_locked(
        conn: &rusqlite::Connection,
        case: &Value,
        mut seq_from: i64,
    ) -> std::result::Result<i64, String> {
        validate_case(case)?;
        let mats = &case["materials"];
        for (key, mtype) in TYPES {
            let items = mats[key].as_array().unwrap();
            for item in items {
                let id = item["material_id"].as_str().unwrap_or_else(|| {
                    item["gate_id"].as_str().expect("gate requires gate_id")
                });
                let version = item["version"].as_i64().ok_or("missing version")?;
                let hash = sha256_hex(item);
                let exists: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM materials WHERE material_type=?1 AND material_id=?2 AND version=?3",
                        params![mtype, id, version],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if exists == 0 {
                    conn.execute(
                        "INSERT INTO materials(material_type,material_id,version,payload_json,sha256,ingest_seq)
                         VALUES(?1,?2,?3,?4,?5,?6)",
                        params![mtype, id, version, item.to_string(), hash, seq_from],
                    )
                    .map_err(|e| e.to_string())?;
                    seq_from += 1;
                }
            }
        }
        Ok(seq_from)
}

impl Store {
    pub fn load_materials(&self) -> Loaded {
        let c = self.conn.lock().unwrap();
        let mut out = Loaded::default();
        let mut stmt = c
            .prepare("SELECT material_type,payload_json FROM materials ORDER BY material_type,material_id,version")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap();
        for row in rows {
            let (mtype, payload) = row.unwrap();
            let v: Value = serde_json::from_str(&payload).unwrap();
            match mtype.as_str() {
                "raw_events" => {
                    let m: RawEvents = serde_json::from_value(v).unwrap();
                    out.raw.insert((m.material_id.clone(), m.version), m);
                }
                "spillover_meta" => {
                    let m: SpilloverMeta = serde_json::from_value(v).unwrap();
                    out.meta.insert((m.material_id.clone(), m.version), m);
                }
                "compensation" => {
                    let m: CompensationMatrix = serde_json::from_value(v).unwrap();
                    out.comp.insert((m.material_id.clone(), m.version), m);
                }
                "transform" => {
                    let m: TransformParams = serde_json::from_value(v).unwrap();
                    out.transform.insert((m.material_id.clone(), m.version), m);
                }
                "gates" => {
                    let m: Gate = serde_json::from_value(v).unwrap();
                    out.gates.insert((m.gate_id.clone(), m.version), m);
                }
                "hierarchy" => {
                    let m: GateHierarchy = serde_json::from_value(v).unwrap();
                    out.hierarchy.insert((m.material_id.clone(), m.version), m);
                }
                _ => {}
            }
        }
        out
    }
}

use crate::engine::LoadedMaterials as Loaded;
