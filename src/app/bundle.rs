//! 导出/导入归档：事件日志 + 原始字节，身份（event_id / 稳定 ID）与顺序（seq）保持。

use serde_json::{json, Value};

use crate::app::service::{err, Service, SrvResult};
use crate::casepack::CasePack;
use crate::db;
use crate::domain::events::{EventKind, EventRecord, Judgement};
use crate::domain::project::MAIN_BRANCH;

impl Service {
    /// 导出全部分支为一个 JSON 归档（blob 字节 base64）。
    pub fn export_bundle(&self) -> SrvResult<Value> {
        let conn = self.conn.lock().unwrap();
        let branches = db::list_branches(&conn).map_err(|e| err("db", e.0))?;
        let mut branch_events = Vec::new();
        for (b, _, _) in &branches {
            let events = db::list_events(&conn, b).map_err(|e| err("db", e.0))?;
            branch_events.push(json!({"branch": b, "events": events}));
        }
        let blobs = db::list_blob_meta(&conn).map_err(|e| err("db", e.0))?;
        let mut blob_arr = Vec::new();
        use base64_lite::b64_encode;
        for (id, kind, filename, len, sha) in blobs {
            let (_, bytes) = db::get_blob(&conn, &id).map_err(|e| err("db", e.0))?.unwrap();
            blob_arr.push(json!({
                "blob_id": id, "kind": kind, "filename": filename,
                "byte_len": len, "sha256": sha,
                "content_base64": b64_encode(&bytes),
            }));
        }
        let drafts = db::list_drafts(&conn).map_err(|e| err("db", e.0))?;
        Ok(json!({
            "format": "flowline-bundle-v1",
            "exported_at": "2026-09-25T12:00:00+09:00",
            "branches": branch_events,
            "blobs": blob_arr,
            "drafts": drafts.iter().map(|d| json!({"draft_id": d.0, "branch": d.1, "base_seq": d.2, "label": d.3})).collect::<Vec<_>>(),
        }))
    }

    /// 从归档导入空库：逐条事件按原 seq/event_id/hash 写入；任何不一致整体回滚。
    pub fn import_bundle(&self, bundle: &Value) -> SrvResult<usize> {
        if bundle.get("format").and_then(|v| v.as_str()) != Some("flowline-bundle-v1") {
            return Err(err("BAD_BUNDLE", "不是 flowline-bundle-v1 归档"));
        }
        let mut conn = self.conn.lock().unwrap();
        if !db::list_branches(&conn).map_err(|e| err("db", e.0))?.is_empty() {
            return Err(err("NOT_EMPTY", "只允许导入空库（避免半份记录）"));
        }
        let tx = conn.transaction().map_err(|e| err("db", e.to_string()))?;

        if let Some(blobs) = bundle.get("blobs").and_then(|v| v.as_array()) {
            for b in blobs {
                let id = b.get("blob_id").and_then(|v| v.as_str()).ok_or_else(|| err("BAD_BUNDLE", "blob 缺 id"))?;
                let bytes = base64_lite::b64_decode(
                    b.get("content_base64").and_then(|v| v.as_str()).unwrap_or(""),
                ).map_err(|e| err("BAD_BUNDLE", e))?;
                let sha_actual = crate::domain::ids::content_rev(&String::from_utf8_lossy(&bytes));
                let sha = b.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
                if sha != sha_actual {
                    return Err(err("BUNDLE_TAMPERED", format!("blob {id} 指纹不符，整体回滚")));
                }
                tx.execute(
                    "INSERT INTO blobs (blob_id, kind, filename, bytes, sha256, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![
                        id,
                        b.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                        b.get("filename").and_then(|v| v.as_str()).unwrap_or(""),
                        bytes, sha, "2026-09-25T12:00:00+09:00",
                    ],
                ).map_err(|e| err("db", e.to_string()))?;
            }
        }

        let mut total = 0usize;
        if let Some(branches) = bundle.get("branches").and_then(|v| v.as_array()) {
            for br in branches {
                let name = br.get("branch").and_then(|v| v.as_str()).ok_or_else(|| err("BAD_BUNDLE", "缺 branch"))?;
                let events = br.get("events").and_then(|v| v.as_array()).ok_or_else(|| err("BAD_BUNDLE", "缺 events"))?;
                let mut prev = String::new();
                for (i, evv) in events.iter().enumerate() {
                    let ev: EventRecord = serde_json::from_value(evv.clone())
                        .map_err(|e| err("BAD_BUNDLE", format!("事件反序列化失败：{e}")))?;
                    if ev.seq as usize != i + 1 || ev.prev_hash != prev {
                        return Err(err("BUNDLE_CHAIN", "事件顺序或哈希链断裂，整体回滚"));
                    }
                    let expect = content_hash_verify(&ev);
                    if expect != ev.hash {
                        return Err(err("BUNDLE_TAMPERED", format!("事件 {} 指纹不符", ev.event_id)));
                    }
                    let jv = serde_json::to_value(&ev.judgements).unwrap_or(json!([]));
                    tx.execute(
                        "INSERT INTO events (seq, branch, event_id, kind, prev_hash, hash, at, payload, judgements, actor)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'reviewer')",
                        rusqlite::params![ev.seq, name, ev.event_id, ev.kind, ev.prev_hash, ev.hash,
                                          ev.at, ev.payload.to_string(), jv.to_string()],
                    ).map_err(|e| err("db", e.to_string()))?;
                    tx.execute(
                        "INSERT INTO branch_heads (branch, head_seq, head_event_id, head_hash)
                         VALUES (?1,?2,?3,?4) ON CONFLICT(branch) DO UPDATE SET
                            head_seq=excluded.head_seq, head_event_id=excluded.head_event_id, head_hash=excluded.head_hash",
                        rusqlite::params![name, ev.seq, ev.event_id, ev.hash],
                    ).map_err(|e| err("db", e.to_string()))?;
                    prev = ev.hash.clone();
                    total += 1;
                }
            }
        }
        if let Some(drafts) = bundle.get("drafts").and_then(|v| v.as_array()) {
            for d in drafts {
                tx.execute(
                    "INSERT OR REPLACE INTO drafts (draft_id, branch, base_seq, label, created_at)
                     VALUES (?1,?2,?3,?4,?5)",
                    rusqlite::params![
                        d.get("draft_id").and_then(|v| v.as_str()).unwrap_or(""),
                        d.get("branch").and_then(|v| v.as_str()).unwrap_or(""),
                        d.get("base_seq").and_then(|v| v.as_i64()).unwrap_or(0),
                        d.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                        "2026-09-25T12:00:00+09:00",
                    ],
                ).map_err(|e| err("db", e.to_string()))?;
            }
        }
        tx.commit().map_err(|e| err("db", e.to_string()))?;
        Ok(total)
    }
}

fn content_hash_verify(ev: &EventRecord) -> String {
    crate::domain::ids::content_rev_parts(&[
        "flowline-event-v1",
        &ev.seq.to_string(),
        &ev.event_id,
        &ev.prev_hash,
        &ev.at,
        &ev.payload.to_string(),
    ])
}

/// 无依赖 base64（避免引入更多 crate）。
pub(crate) mod base64_lite {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn b64_encode(input: &[u8]) -> String {
        let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
        for chunk in input.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let n = ((b0) << 16) | (b1 << 8) | b2;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 { out.push(T[((n >> 6) & 63) as usize] as char); } else { out.push('='); }
            if chunk.len() > 2 { out.push(T[(n & 63) as usize] as char); } else { out.push('='); }
        }
        out
    }
    pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
        let val = |c: u8| -> Result<u32, String> {
            match c {
                b'A'..=b'Z' => Ok((c - b'A') as u32),
                b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
                b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err(format!("非法 base64 字符 {c}")),
            }
        };
        let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
        let mut out = Vec::new();
        for chunk in bytes.chunks(4) {
            if chunk.len() < 4 { return Err("base64 长度不对齐".into()); }
            let mut buf = [0u32; 4];
            let mut pad = 0;
            for i in 0..4 {
                buf[i] = if chunk[i] == b'=' { pad += 1; 0 } else { val(chunk[i])? };
            }
            let n = (buf[0] << 18) | (buf[1] << 12) | (buf[2] << 6) | buf[3];
            out.push((n >> 16) as u8);
            if pad < 2 { out.push((n >> 8) as u8); }
            if pad < 1 { out.push(n as u8); }
        }
        Ok(out)
    }
}
