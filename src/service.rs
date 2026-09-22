//! 应用服务：案例播种、分支事件追加（含乐观并发）、审阅投影、场景操作。

use std::sync::Arc;

use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::archive::simple_sha256_hex;
use crate::case;
use crate::db::{self, AppendError};
use crate::domain::*;
use crate::state::{project_view, replay};
use crate::util::stable_id;

#[derive(Clone)]
pub struct AppState {
    pub db_path: String,
    pub conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<serde_json::Value>,
}

pub fn from_db_error(e: AppendError) -> (u16, ApiError) {
    match e {
        AppendError::Conflict { branch, expected_seq, actual_head, newer_event_ids } => (
            409,
            ApiError {
                error: format!("分支 {branch} 在 seq={expected_seq} 处存在并发修改（当前 head={actual_head}）"),
                code: Some("version_conflict".into()),
                conflict: Some(serde_json::json!({
                    "branch": branch,
                    "expected_seq": expected_seq,
                    "actual_head": actual_head,
                    "newer_event_ids": newer_event_ids,
                })),
            },
        ),
        AppendError::Db(e) => (500, ApiError { error: e.to_string(), code: Some("db".into()), conflict: None }),
    }
}

/// 构造基线 FCS 文本字节（导入来源；与 RevisionBundle 分开保存）。
pub fn baseline_source_bytes() -> Vec<u8> {
    let d = case::dataset_v1();
    let mut s = String::new();
    s.push_str("# FCS-like offline fixture (raw representations preserved)\n");
    s.push_str(&format!("case: {}\n", case::CASE_ID));
    for c in &d.channels {
        s.push_str(&format!(
            "channel\t{}\t{}\tunit={}\tbasis={}\tlinear_max={}\n",
            c.code, c.label, c.unit, c.basis, c.linear_max
        ));
    }
    for e in &d.events {
        let vals: Vec<String> = e.values.iter().map(|v| format!("{}={}", v.channel, v.raw)).collect();
        s.push_str(&format!("event\t{}\t{}\n", e.event_id, vals.join("\t")));
    }
    s.into_bytes()
}

/// 首次启动：在空 main 分支播种 v1 基线。整体事务，失败回滚。
pub fn seed_if_empty(conn: &mut Connection) -> rusqlite::Result<bool> {
    db::ensure_main_branch(conn)?;
    if db::has_events(conn, "main")? {
        return Ok(false);
    }
    let bytes = baseline_source_bytes();
    let b64 = crate::base64_lite::encode(&bytes);
    let hash = simple_sha256_hex(&bytes);

    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO blobs(blob_id, sha256, bytes_b64, media_type, created_at)
         VALUES ('blob:baseline-fcs', ?1, ?2, 'text/x-fcs-fixture', 'unix:0')",
        rusqlite::params![hash, b64],
    )?;
    tx.execute(
        "INSERT INTO case_state(key,value) VALUES ('case_id',?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![case::CASE_ID],
    )?;
    let payload = EventPayload::Imported {
        bundle: case::baseline_bundle(),
        source_blob: "blob:baseline-fcs".into(),
        source_sha256: hash.clone(),
    };
    let json = serde_json::to_string(&payload).expect("payload");
    tx.execute(
        "INSERT INTO events(branch, seq, event_id, at, actor, payload_json)
         VALUES ('main', 1, 'EV-import-v1', 'unix:0', 'seeder', ?1)",
        rusqlite::params![json],
    )?;
    tx.execute(
        "UPDATE branches SET head_seq=1 WHERE branch='main'",
        [],
    )?;
    tx.commit()?;
    Ok(true)
}

/// 重放并返回审阅投影（可指定 seq 回看历史）。
pub fn view(conn: &Connection, branch: &str, seq: Option<i64>) -> serde_json::Value {
    let events = db::load_events(conn, branch).unwrap_or_default();
    let replay = replay(&events, seq);
    let mut v = project_view(&replay);
    v["branch"] = serde_json::Value::String(branch.to_string());
    v["requested_seq"] = serde_json::Value::Number(seq.unwrap_or(replay.last_seq).into());
    v
}

pub fn events_listing(conn: &Connection, branch: &str) -> Vec<serde_json::Value> {
    let events = db::load_events(conn, branch).unwrap_or_default();
    events
        .iter()
        .map(|e| {
            serde_json::json!({
                "event_id": e.event_id,
                "seq": e.seq,
                "at": e.at,
                "actor": e.actor,
                "kind": payload_kind(&e.payload),
                "retracted": is_retraction_target(&events, &e.event_id),
            })
        })
        .collect()
}

fn is_retraction_target(events: &[BranchEvent], target: &str) -> bool {
    events.iter().any(|e| matches!(&e.payload,
        EventPayload::Retracted { target_event_id, .. } if target_event_id == target))
}

fn payload_kind(p: &EventPayload) -> &'static str {
    match p {
        EventPayload::Imported { .. } => "imported",
        EventPayload::ParameterRevised { .. } => "parameter_revised",
        EventPayload::SourcePatched { .. } => "source_patched",
        EventPayload::GateRevised { .. } => "gate_revised",
        EventPayload::GateRenamed { .. } => "gate_renamed",
        EventPayload::Recomputed { .. } => "recomputed",
        EventPayload::Decision { .. } => "decision",
        EventPayload::Retracted { .. } => "retracted",
    }
}

/// 追加事件，返回新 seq。
pub fn append(
    conn: &mut Connection,
    branch: &str,
    expected_seq: i64,
    event_id: &str,
    actor: Option<&str>,
    payload: EventPayload,
) -> Result<i64, AppendError> {
    db::append_event(conn, branch, expected_seq, event_id, actor, &payload)
}

pub fn new_event_id(branch: &str, tag: &str, seq: i64) -> String {
    stable_id("EV", &format!("{branch}|{tag}|{seq}"))
}
