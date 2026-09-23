//! 写命令处理：所有状态变化只追加事件；派生批次不可变；失效按依赖范围传播。

use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::domain::{
    etype, kind, rule, Basis, ContentObject, EventRecord, GatePayload, LineagePayload,
    RawDataPayload, SpilloverPayload, TransformPayload,
};
use crate::engine::{self, GateResult};
use crate::repo::{RepoError, RepoResult};
use crate::storage::{now_ts, sha256_hex, Store};

fn err400(m: impl Into<String>) -> RepoError {
    RepoError::bad(m)
}
fn err409(m: impl Into<String>) -> RepoError {
    RepoError::conflict(m)
}

// ---------- 事件追加 ----------

struct Inserted {
    seq: u64,
    id: String,
    branch_seq: Option<u64>,
}

fn insert_event(
    tx: &Transaction,
    case_id: &str,
    branch: &str,
    event_type: &str,
    actor: &str,
    summary: &str,
    payload: Value,
    depends_on: Vec<String>,
    basis: Vec<Basis>,
    branch_event: bool,
) -> rusqlite::Result<Inserted> {
    tx.execute(
        "INSERT INTO events(seq, id, case_id, branch, branch_seq, event_type, actor, summary,
                            payload, depends_on, basis, created_at)
         VALUES ((SELECT COALESCE(MAX(seq),0)+1 FROM events),
                 ?1 || '#' || printf('%08d', (SELECT COALESCE(MAX(seq),0)+1 FROM events)),
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            case_id,
            branch,
            Option::<u64>::None, // 先占位，下面回填 branch_seq
            event_type,
            actor,
            summary,
            payload.to_string(),
            serde_json::to_string(&depends_on).unwrap(),
            serde_json::to_string(&basis).unwrap(),
            now_ts()
        ],
    )?;
    let seq: u64 = tx.last_insert_rowid() as u64;
    // AUTOINCREMENT 的 rowid 即 seq（显式子查询 + 单写者，保持一致）
    let seq = tx.query_row(
        "SELECT seq FROM events WHERE rowid=?1",
        params![tx.last_insert_rowid()],
        |r| r.get::<_, i64>(0),
    )? as u64;
    let id = format!("{case_id}#{seq:08}");
    let branch_seq = if branch_event {
        let bs: i64 = tx.query_row(
            "SELECT COUNT(*)+1 FROM events WHERE case_id=?1 AND branch=?2 AND branch_seq IS NOT NULL AND seq<?3",
            params![case_id, branch, seq],
            |r| r.get(0),
        )?;
        Some(bs as u64)
    } else {
        None
    };
    tx.execute(
        "UPDATE events SET id=?1, branch_seq=?2 WHERE seq=?3",
        params![id, branch_seq, seq],
    )?;
    if branch_event {
        tx.execute(
            "INSERT INTO branch_heads(case_id, branch, head_seq) VALUES(?1,?2,?3)
             ON CONFLICT(case_id, branch) DO UPDATE SET head_seq=?3",
            params![case_id, branch, seq],
        )?;
    }
    Ok(Inserted { seq, id, branch_seq })
}

fn insert_object(tx: &Transaction, case_id: &str, obj: &ContentObject) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO objects(id, case_id, kind, version, supersedes, payload)
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            obj.id,
            case_id,
            obj.kind,
            obj.version,
            obj.supersedes,
            obj.payload.to_string()
        ],
    )?;
    Ok(())
}

// ---------- 上下文构建 ----------

fn obj_payload<T: serde::de::DeserializeOwned>(obj: &ContentObject) -> Result<T, RepoError> {
    serde_json::from_value(obj.payload.clone()).map_err(|e| err400(format!("对象载荷无效: {e}")))
}

fn load_object(tx: &Connection, id: &str) -> Result<ContentObject, RepoError> {
    tx.query_row(
        "SELECT id, kind, version, COALESCE(supersedes,''), payload FROM objects WHERE id=?1",
        params![id],
        |r| {
            let sup: String = r.get(3)?;
            Ok(ContentObject {
                id: r.get(0)?,
                kind: r.get(1)?,
                version: r.get::<_, i64>(2)? as u32,
                supersedes: if sup.is_empty() { None } else { Some(sup) },
                payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap(),
            })
        },
    )
    .map_err(|_| err400(format!("对象 {id} 不存在")))
}

fn object_id(case_id: &str, kind: &str, version: u32) -> String {
    format!("{case_id}:{kind}:v{version}")
}

/// 分支视图中每类来源的最新对象引用（回放 content_committed/case_imported 得到）。
pub fn head_refs(events: &[EventRecord]) -> HashMap<String, String> {
    let mut refs = HashMap::new();
    for ev in events {
        match ev.event_type {
            etype::CASE_IMPORTED => {
                if let Some(objs) = ev.payload.get("objects").and_then(|v| v.as_array()) {
                    for o in objs {
                        if let (Some(k), Some(id)) = (o.get("kind").and_then(|v| v.as_str()),
                                                      o.get("object_id").and_then(|v| v.as_str())) {
                            refs.insert(k.to_string(), id.to_string());
                        }
                    }
                }
            }
            etype::CONTENT_COMMITTED => {
                if let (Some(k), Some(id)) = (
                    ev.payload.get("kind").and_then(|v| v.as_str()),
                    ev.payload.get("object_id").and_then(|v| v.as_str()),
                ) {
                    refs.insert(k.to_string(), id.to_string());
                }
            }
            _ => {}
        }
    }
    refs
}

fn build_context(tx: &Connection, refs: &HashMap<String, String>) -> Result<engine::Context, RepoError> {
    let require = |k: &str| -> Result<ContentObject, RepoError> {
        let id = refs.get(k).ok_or_else(|| err400(format!("缺少来源 {k}")))?;
        load_object(tx, id)
    };
    let raw: RawDataPayload = obj_payload(&require(kind::RAW_DATA)?)?;
    let spillover: SpilloverPayload = obj_payload(&require(kind::SPILLOVER)?)?;
    let transform: TransformPayload = obj_payload(&require(kind::TRANSFORM)?)?;
    let gates_obj = require(kind::GATE)?;
    // GATE 对象用“门集合载荷”：{ "gates": [GatePayload...] }
    #[derive(serde::Deserialize)]
    struct GateSet {
        gates: Vec<GatePayload>,
    }
    let gate_set: GateSet = obj_payload(&gates_obj)?;
    let lineage: LineagePayload = obj_payload(&require(kind::LINEAGE)?)?;
    let gates = gate_set
        .gates
        .into_iter()
        .map(|g| (g.gate_id.clone(), g))
        .collect();
    Ok(engine::Context {
        raw,
        spillover,
        transform,
        gates,
        lineage,
    })
}

// ---------- 派生批次持久化 ----------

#[allow(clippy::too_many_arguments)]
fn insert_gate_run(
    tx: &Transaction,
    case_id: &str,
    branch: &str,
    seq: u64,
    res: &GateResult,
    run_counter: &mut u32,
) -> rusqlite::Result<String> {
    *run_counter += 1;
    let run_id = format!("{case_id}:{branch}:gate:{}:{run_counter}", res.gate_id);
    tx.execute(
        "INSERT INTO gate_runs(id, case_id, branch, seq, gate_id, gate_ref, transform_ref,
                               spillover_ref, comp_run_ref, parent_run_ref, status,
                               chosen_candidate_id)
         VALUES(?1,?2,?3,?4,?5,?6,'','',?7,?8,'immutable',NULL)",
        params![
            run_id,
            case_id,
            branch,
            seq,
            res.gate_id,
            res.gate_version_ref,
            Option::<String>::None,
            res.parent_run_ref,
        ],
    )?;
    for c in &res.candidates {
        let cid = format!("{run_id}#{}", c.candidate_id);
        tx.execute(
            "INSERT INTO gate_run_candidates(id, run_id, case_id, candidate_id, member_indices,
                 numerator, denominator, percent, denominator_source, boundary_indices,
                 unconvertible_indices)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                cid,
                run_id,
                case_id,
                c.candidate_id,
                serde_json::to_string(&c.member_indices).unwrap(),
                c.numerator,
                c.denominator,
                c.percent,
                c.denominator_source,
                serde_json::to_string(&c.boundary_indices).unwrap(),
                serde_json::to_string(&c.unconvertible_indices).unwrap(),
            ],
        )?;
    }
    Ok(run_id)
}
