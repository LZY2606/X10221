//! 导出/导入档案：身份、顺序与来源关系在往返后保持。

use serde_json::json;

use crate::b64;
use crate::model::{Event, Version};
use crate::store::{Store, StoreError};

pub const ARCHIVE_FORMAT: &str = "flow-lineage-archive/1";

pub fn export_archive(store: &Store) -> Result<serde_json::Value, StoreError> {
    let versions = store.all_versions()?;
    let events = store.events()?;
    let mut packed_versions = Vec::with_capacity(versions.len());
    for v in &versions {
        let raw = store.raw_of(&v.id)?;
        packed_versions.push(json!({
            "id": v.id,
            "kind": v.kind,
            "line_id": v.line_id,
            "branch": v.branch,
            "seq": v.seq,
            "base_id": v.base_id,
            "payload": v.payload,
            "created_ms": v.created_ms,
            "status": v.status,
            "raw_b64": raw.map(|r| b64::encode(&r)),
        }));
    }
    let packed_events: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "seq": e.seq,
                "kind": e.kind,
                "payload": e.payload,
                "created_ms": e.created_ms,
            })
        })
        .collect();
    Ok(json!({
        "format": ARCHIVE_FORMAT,
        "versions": packed_versions,
        "events": packed_events,
    }))
}

pub fn import_archive(store: &Store, archive: &serde_json::Value) -> Result<serde_json::Value, StoreError> {
    let format = archive.get("format").and_then(|f| f.as_str()).unwrap_or("");
    if format != ARCHIVE_FORMAT {
        return Err(StoreError::BadInput(format!("未知档案格式: {format}")));
    }
    let mut imported_versions = 0usize;
    let mut imported_events = 0usize;
    if let Some(vs) = archive.get("versions").and_then(|v| v.as_array()) {
        for item in vs {
            let v = Version {
                id: item.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                kind: item.get("kind").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                line_id: item.get("line_id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                branch: item.get("branch").and_then(|x| x.as_str()).unwrap_or("main").to_string(),
                seq: item.get("seq").and_then(|x| x.as_i64()).unwrap_or(0),
                base_id: item.get("base_id").and_then(|x| x.as_str()).map(String::from),
                payload: item.get("payload").cloned().unwrap_or(serde_json::Value::Null),
                has_raw: item.get("raw_b64").map(|r| !r.is_null()).unwrap_or(false),
                created_ms: item.get("created_ms").and_then(|x| x.as_i64()).unwrap_or(0),
                status: item.get("status").and_then(|x| x.as_str()).unwrap_or("fresh").to_string(),
            };
            let raw = match item.get("raw_b64").and_then(|r| r.as_str()) {
                Some(s) => Some(b64::decode(s).map_err(StoreError::BadInput)?),
                None => None,
            };
            if store.insert_version_raw(&v, raw)? {
                imported_versions += 1;
            }
        }
    }
    if let Some(es) = archive.get("events").and_then(|v| v.as_array()) {
        for item in es {
            let e = Event {
                id: item.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                seq: item.get("seq").and_then(|x| x.as_i64()).unwrap_or(0),
                kind: item.get("kind").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                payload: item.get("payload").cloned().unwrap_or(serde_json::Value::Null),
                created_ms: item.get("created_ms").and_then(|x| x.as_i64()).unwrap_or(0),
            };
            if store.insert_event_raw(&e)? {
                imported_events += 1;
            }
        }
    }
    store.record_event(
        "archive.imported",
        json!({"imported_versions": imported_versions, "imported_events": imported_events}),
    )?;
    Ok(json!({"imported_versions": imported_versions, "imported_events": imported_events}))
}
