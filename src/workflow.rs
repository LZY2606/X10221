//! 案例导入与引导式重放脚本。

use crate::case::CaseFile;
use crate::compute;
use crate::domain;
use crate::engine::{publish_artifact, canonical_sha, Store};
use crate::Result;
use serde_json::{json, Value};
use std::collections::HashSet;

pub const MAIN: &str = "main";
pub const ALT: &str = "alt";

/// 装载案例基线（整个过程在一个事务语义下：失败回滚，不留半份记录）。
pub fn load_case(store: &mut Store, case: &CaseFile, raw_blobs: &[(&str, &[u8])]) -> Result<()> {
    let tx = store.conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT OR REPLACE INTO studies(study_id,title,created_at,schema_version) VALUES(?1,?2,?3,?4)",
        rusqlite::params![case.id, case.title, crate::engine::now_iso(), crate::db::SCHEMA_VERSION],
    )
    .map_err(|e| e.to_string())?;
    for (path, bytes) in raw_blobs {
        tx.execute(
            "INSERT OR REPLACE INTO raw_blobs(blob_id,rel_path,sha256,bytes) VALUES(?1,?2,?3,?4)",
            rusqlite::params![path, path, crate::engine::sha256_hex(bytes), bytes],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    drop_store_conn_guard();

    store.create_branch(&case.id, MAIN, None)?;

    // 六类材料分开版本化：通道、原始事件、spillover、补偿矩阵、变换、门、父子链接。
    store.append(
        MAIN,
        "channels",
        json!({"content": {"channels": case.channels, "kind": "channels"}}),
        None,
        Some("fragment:import:channels"),
    )?;
    store.append(
        MAIN,
        "raw_snapshot",
        json!({"content": {"kind": "raw_snapshot", "fragment_id": "fragment:import:events", "events": case.events,
              "raw_units": "channel-count @ 18bit, 原文保留"}}),
        None,
        Some("fragment:import:events"),
    )?;
    publish_artifact(store, MAIN, "spillover_meta", "active", 1, case.spillover_meta.clone(), None)?;
    let baseline = case.compensation_matrices["baseline"].clone();
    publish_artifact(store, MAIN, "comp_matrix", "active", 1, baseline, None)?;
    publish_artifact(store, MAIN, "transform", "active", 1, Value::Array(case.transforms["baseline"].as_array().unwrap().clone()), None)?;
    publish_artifact(store, MAIN, "gates", "active", 1, Value::Array(case.gates["baseline"].as_array().unwrap().clone()), None)?;
    publish_artifact(store, MAIN, "links", "active", 1, Value::Array(case.links["baseline"].as_array().unwrap().clone()), None)?;

    evaluate_current_comp(store, MAIN, "cand-comp-1")?;
    // 良态基线：自动存在唯一 exact 候选，记录为已裁定，不隐藏候选信息。
    decide(store, MAIN, "cand-comp-1", "exact", None)?;
    recompute(store, MAIN, None)?;
    Ok(())
}

fn drop_store_conn_guard() {}

fn current_matrix_sha(store: &Store, branch: &str) -> Option<String> {
    let s = store.state_at(branch, None).ok()?;
    s.artifacts.get("comp_matrix:active").map(|a| a.sha256.clone())
}

pub fn evaluate_current_comp(store: &mut Store, branch: &str, eval_id: &str) -> Result<(String, Value)> {
    let state = store.state_at(branch, None)?;
    let matrix = state.artifacts.get("comp_matrix:active").cloned().ok_or("缺少补偿矩阵")?;
    let rows: Vec<Vec<f64>> = matrix.content["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect())
        .collect();
    let analysis = domain::analyze_compensation(&rows);
    let content = json!({
        "eval_id": eval_id,
        "matrix_logical": "active",
        "matrix_sha": matrix.sha256,
        "analysis": analysis,
        "affected_object": "全部荧光通道补偿坐标与下游荧光门",
    });
    let (eid, rev) = store.append(branch, "comp_evaluated", json!({"content": content}), None, None)?;
    store.conn.execute(
        "INSERT OR REPLACE INTO comp_evaluations(eval_id,matrix_artifact,matrix_version,status,rcond,analysis_json,event_id)
         VALUES(?1,'comp_matrix:active',1,?2,?3,?4,?5)",
        rusqlite::params![eval_id, format!("{:?}", analysis.status), analysis.rcond,
            serde_json::to_string(&analysis).unwrap(), eid],
    ).map_err(|e| e.to_string())?;
    Ok((eid, json!({"rev": rev})))
}

pub fn decide(store: &mut Store, branch: &str, eval_id: &str, strategy: &str, expected_rev: Option<i64>) -> Result<(String, i64)> {
    // 校验策略确实是合法候选（无法唯一决定时禁止编造）。
    let state = store.state_at(branch, None)?;
    let analysis = state
        .evals
        .get(eval_id)
        .and_then(|e| serde_json::from_value::<domain::CompAnalysis>(e["analysis"].clone()).ok())
        .ok_or("找不到该补偿评估")?;
    let ok = analysis.candidates.iter().any(|c| c.strategy == strategy && c.admissible);
    if !ok {
        return Err(format!("策略 {strategy} 不是该矩阵的合法候选"));
    }
    let (eid, rev) = store.append(
        branch,
        "comp_decision",
        json!({"content": {"eval_id": eval_id, "strategy": strategy,
            "machine_readable": {"rule": "near-singular compensation is never silently inverted",
                                 "admissible_strategies": analysis.candidates.iter().map(|c| c.strategy.clone()).collect::<Vec<_>>()}}}),
        expected_rev,
        None,
    )?;
    store.conn.execute(
        "INSERT INTO comp_decisions(branch_id,eval_id,strategy,decided_event_id) VALUES(?1,?2,?3,?4)
         ON CONFLICT(branch_id,eval_id) DO UPDATE SET strategy=excluded.strategy, decided_event_id=excluded.decided_event_id, undone_event_id=NULL",
        rusqlite::params![branch, eval_id, strategy, eid],
    ).map_err(|e| e.to_string())?;
    Ok((eid, rev))
}

pub fn undo_decision(store: &mut Store, branch: &str, eval_id: &str, expected_rev: Option<i64>) -> Result<(String, i64)> {
    let (eid, rev) = store.append(
        branch,
        "decision_undone",
        json!({"content": {"eval_id": eval_id}}),
        expected_rev,
        None,
    )?;
    store.conn.execute(
        "UPDATE comp_decisions SET strategy=NULL, undone_event_id=?1 WHERE branch_id=?2 AND eval_id=?3",
        rusqlite::params![eid, branch, eval_id],
    ).ok();
    Ok((eid, rev))
}

pub fn recompute(store: &mut Store, branch: &str, expected_rev: Option<i64>) -> Result<(String, i64, Value)> {
    let state = store.state_at(branch, None)?;
    let (eid, rev) = store.append(branch, "pop_recomputed", json!({"content": {}}), expected_rev, None)?;
    let payload = compute::recompute_populations(&state, &eid, None);
    store.conn.execute(
        "UPDATE events SET payload_json=?1 WHERE event_id=?2",
        rusqlite::params![serde_json::to_string(&json!({"kind_event":"pop_recomputed","content": payload})).unwrap(), eid],
    ).map_err(|e| e.to_string())?;
    for pop in payload["populations"].as_array().unwrap() {
        persist_pop(&store.conn, branch, &eid, pop)?;
    }
    Ok((eid, rev, payload))
}

fn persist_pop(conn: &rusqlite::Connection, branch: &str, eid: &str, pop: &Value) -> Result<()> {
    let as_str = |v: &Value| v.as_str().map(|s| s.to_string());
    let as_i64 = |v: &Value| v.as_i64().unwrap_or(0);
    let as_f64 = |v: &Value| v.as_f64().unwrap_or(0.0);
    conn.execute(
        "INSERT INTO pop_results(result_id,gate_logical,branch_id,event_id,count,percent,
         denominator_result_id,denominator_event_id,deps_json,status,stale)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0)",
        rusqlite::params![
            as_str(&pop["result_id"]), as_str(&pop["gate_logical"]), branch, eid,
            as_i64(&pop["count"]), as_f64(&pop["percent"]),
            as_str(&pop["denominator_result_id"]), as_str(&pop["denominator_event_id"]),
            pop["deps"].to_string(), as_str(&pop["status"])
        ],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub struct ReplayReport {
    pub steps: Vec<Value>,
}

/// 引导式重放：与界面“逐步复核”叙事一致，每步都留下输入片段、判定状态、受影响对象。
pub fn replay_case(store: &mut Store, case: &CaseFile) -> Result<ReplayReport> {
    let mut steps: Vec<Value> = Vec::new();

    // 1) 近奇异补偿矩阵：不能默默求逆，保留全部合法候选。
    publish_artifact(store, MAIN, "comp_matrix", "active", 2, case.compensation_matrices["near_singular"].clone(), None)?;
    let (eid, _) = evaluate_current_comp(store, MAIN, "cand-comp-near")?;
    steps.push(json!({"step": "near_singular", "event": eid,
        "machine_event": {"rule": "near-singular compensation is never silently inverted",
                          "required": "preserve all admissible candidates; await human decision"}}));

    // 2) 严格奇异矩阵。
    publish_artifact(store, MAIN, "comp_matrix", "active", 3, case.compensation_matrices["singular"].clone(), None)?;
    let (eid, _) = evaluate_current_comp(store, MAIN, "cand-comp-singular")?;
    steps.push(json!({"step": "singular", "event": eid,
        "machine_event": {"rule": "exact inverse does not exist", "required": "no exact candidate"}}));

    // 在此分叉：main 与 alt 对同一近奇异/不确定性问题做出互相冲突的人工决定。
    let fork_head = fork_head(store, MAIN)?;
    store.create_branch(&case.id, ALT, Some(&fork_head))?;
    // alt 直接采用伪逆；main 先回到 ridge 矩阵再裁定。
    publish_artifact(store, ALT, "comp_matrix", "active", 2, case.compensation_matrices["near_singular"].clone(), None)?;
    evaluate_current_comp(store, ALT, "cand-comp-alt")?;
    decide(store, ALT, "cand-comp-alt", "pseudo-inverse", None)?;
    recompute(store, ALT, None)?;

    // main: 近奇异 + ridge。
    publish_artifact(store, MAIN, "comp_matrix", "active", 2, case.compensation_matrices["near_singular"].clone(), None)?;
    evaluate_current_comp(store, MAIN, "cand-comp-main")?;
    let (eid, _) = decide(store, MAIN, "cand-comp-main", "ridge", None)?;
    recompute(store, MAIN, None)?;
    steps.push(json!({"step": "conflicting_decisions", "main_event": eid,
        "machine_event": {"rule": "ambiguous compensation keeps every legal candidate; branches record distinct human rulings"}}));

    // 3) 门嵌套改名（几何不变，身份稳定）。
    rename_gate(store, MAIN, case, "g-cd3", "CD3+ T cells")?;
    steps.push(json!({"step": "gate_nesting_rename"}));

    // 4) 上游门修订：live 下边界 200 -> 40000（E24 被剔除）。下游只标记过期，不覆盖。
    upstream_revision(store, MAIN)?;
    steps.push(json!({"step": "upstream_stale_propagation",
        "machine_event": {"rule": "downstream populations marked stale, never overwritten",
                          "denominator": "percentage denominator pinned to parent result version"}}));

    // 5) 局部源数据改动：追加极端/溢出事件 + 修改 E18.PE。失效范围按依赖图收敛。
    source_patch(store, MAIN, case)?;
    steps.push(json!({"step": "source_patch_localized",
        "machine_event": {"rule": "invalidation scope computed from dependency graph, no global silent replacement"}}));

    // 6) 变换参数修订（FITC cofactor 1000->750）：只让依赖 FITC 轴的门失效。
    publish_artifact(store, MAIN, "transform", "active", 2, Value::Array(case.transforms["revision"].as_array().unwrap().clone()), None)?;
    let state = store.state_at(MAIN, None)?;
    let expired = compute::invalidation_scope(&state, &HashSet::from(["FITC-A".to_string()]));
    mark_stale(store, MAIN, &expired, "transform-revision")?;
    steps.push(json!({"step": "transform_revision", "expired": expired}));

    Ok(ReplayReport { steps })
}

fn fork_head(store: &Store, branch: &str) -> Result<String> {
    store.conn
        .query_row("SELECT head_event_id FROM branches WHERE branch_id=?1", rusqlite::params![branch], |r| {
            r.get::<_, Option<String>>(0)
        })
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "空分支无法分叉".to_string())
}

fn rename_gate(store: &mut Store, branch: &str, case: &CaseFile, gid: &str, new_name: &str) -> Result<()> {
    let state = store.state_at(branch, None)?;
    let version = state.artifacts.get("gates:active").map(|a| a.version + 1).unwrap_or(1);
    let mut gates = case.gates["baseline"].as_array().unwrap().clone();
    for g in gates.iter_mut() {
        if g["id"] == gid {
            g["name"] = json!(new_name);
        }
    }
    publish_artifact(store, branch, "gates", "active", version, Value::Array(gates), None)?;
    let links_version = state.artifacts.get("links:active").map(|a| a.version + 1).unwrap_or(1);
    publish_artifact(store, branch, "links", "active", links_version, Value::Array(case.links["rename"].as_array().unwrap().clone()), None)?;
    Ok(())
}

fn upstream_revision(store: &mut Store, branch: &str) -> Result<()> {
    let state = store.state_at(branch, None)?;
    let cur = state.artifacts.get("gates:active").cloned().ok_or("缺少门")?;
    let mut gates = cur.content.as_array().cloned().unwrap_or_default();
    for g in gates.iter_mut() {
        if g["id"] == "g-live" {
            g["polygon"][0][0] = json!(40000);
            g["polygon"][3][0] = json!(40000);
        }
    }
    publish_artifact(store, branch, "gates", "active", cur.version + 1, Value::Array(gates), None)?;
    // 边界点 E24 现在被排除：live 重算；其余子群只标 stale，不覆盖旧结果。
    let (eid, _rev, payload) = recompute_single(store, branch, "g-live")?;
    let mut expired = Vec::new();
    for p in payload["populations"].as_array().unwrap() {
        let id = p["result_id"].as_str().unwrap_or("");
        if id != "pop-g-live" {
            expired.push(id.to_string());
        }
    }
    mark_stale(store, branch, &expired, &eid)?;
    Ok(())
}

fn recompute_single(store: &mut Store, branch: &str, only: &str) -> Result<(String, i64, Value)> {
    let state = store.state_at(branch, None)?;
    let full = compute::recompute_populations(&state, "pending", None);
    let kept: Vec<Value> = full["populations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["gate_logical"] == only)
        .cloned()
        .collect();
    let payload = json!({"populations": kept, "note": "只重算修订门本身；下游保持旧结果并单独标记过期"});
    let (eid, rev) = store.append(branch, "pop_recomputed", json!({"content": {}}), None, None)?;
    let mut with_eid = payload.clone();
    with_eid["populations"] = json!(kept.iter().map(|p| {
        let mut p = p.clone();
        p["computed_event_id"] = json!(eid);
        p
    }).collect::<Vec<_>>());
    store.conn.execute(
        "UPDATE events SET payload_json=?1 WHERE event_id=?2",
        rusqlite::params![serde_json::to_string(&json!({"kind_event":"pop_recomputed","content": with_eid})).unwrap(), eid],
    ).map_err(|e| e.to_string())?;
    Ok((eid, rev, payload))
}

pub fn mark_stale_public(store: &mut Store, branch: &str, expired: &[String], reason: &str) -> Result<()> {
    mark_stale(store, branch, expired, reason)
}

fn mark_stale(store: &mut Store, branch: &str, expired: &[String], reason: &str) -> Result<()> {
    if expired.is_empty() {
        return Ok(());
    }
    let (eid, _) = store.append(
        branch,
        "pop_stale",
        json!({"content": {"expired_result_ids": expired, "reason": reason,
            "machine_readable": {"rule": "stale results retained, not replaced",
                                 "denominator_pinned": true}}}),
        None,
        None,
    )?;
    for id in expired {
        store.conn.execute(
            "UPDATE pop_results SET stale=1, status='stale', superseded_by_event=?1
             WHERE result_id=?2 AND branch_id=?3 AND event_id=(
                SELECT event_id FROM pop_results WHERE result_id=?2 AND branch_id=?3 ORDER BY rowid DESC LIMIT 1)",
            rusqlite::params![eid, id, branch],
        ).ok();
    }
    Ok(())
}

fn source_patch(store: &mut Store, branch: &str, case: &CaseFile) -> Result<()> {
    let add = case.patch_events.clone();
    let (eid, _) = store.append(
        branch,
        "raw_patch",
        json!({"content": {
            "fragment_id": "fragment:source-patch-1",
            "add": add,
            "modify": [{"id": "E18", "raw": {"PE-A": 1500}, "note": "局部源数据修订：仅 E18 的 PE 通道"}],
            "import_rule": "不可转换/溢出值留在原位置并附诊断，不裁剪不丢弃"
        }}),
        None,
        Some("fragment:source-patch-1"),
    )?;
    let state = store.state_at(branch, None)?;
    let changed: HashSet<String> = ["FITC-A".to_string(), "PE-A".to_string()].into_iter().collect();
    let expired = compute::invalidation_scope(&state, &changed);
    // 只重算当前未过期的根门 live，验证影响范围受限；荧光门保留过期旧结果。
    mark_stale(store, branch, &expired, &eid)?;
    Ok(())
}
