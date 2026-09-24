//! Axum HTTP 层：复核状态读取、裁定、修订、导出/导入、场景重放。

use std::sync::{Arc, Mutex};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::{casepack, db, rules, ui};

pub type Shared = Arc<Mutex<Connection>>;

pub fn router(conn: Connection) -> Router {
    let shared: Shared = Arc::new(Mutex::new(conn));
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(state))
        .route("/api/case/load", post(case_load))
        .route("/api/comp/evaluate", post(comp_evaluate))
        .route("/api/populations/compute", post(populations_compute))
        .route("/api/transforms/revise", post(transforms_revise))
        .route("/api/gates/revise", post(gates_revise))
        .route("/api/gates/rename", post(gates_rename))
        .route("/api/rulings", post(rulings_apply))
        .route("/api/rulings/undo", post(rulings_undo))
        .route("/api/scenario/replay", post(scenario_replay))
        .route("/api/export", get(export_archive))
        .route("/api/import", post(import_archive))
        .route("/api/admin/wipe", post(wipe))
        .route("/api/drafts/compare", get(drafts_compare))
        .with_state(shared)
}

async fn index() -> Html<&'static str> {
    Html(ui::INDEX)
}

fn qall(conn: &Connection, sql: &str) -> Vec<Value> {
    let mut stmt = conn.prepare(sql).unwrap();
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    stmt.query_map([], |r| {
        let mut m = serde_json::Map::new();
        for (i, c) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = r.get(i)?;
            m.insert(
                c.clone(),
                match v {
                    rusqlite::types::Value::Null => Value::Null,
                    rusqlite::types::Value::Integer(x) => json!(x),
                    rusqlite::types::Value::Real(x) => json!(x),
                    rusqlite::types::Value::Text(x) => {
                        serde_json::from_str(&x).unwrap_or(Value::String(x))
                    }
                    rusqlite::types::Value::Blob(_) => Value::String("<blob>".into()),
                },
            );
        }
        Ok(Value::Object(m))
    })
    .unwrap()
    .filter_map(|x| x.ok())
    .collect()
}

/// 复核页完整状态：存储版本、群体（含分母来源）、事件流、候选、诊断、散点与补偿前后差异。
fn full_state(conn: &Connection) -> Value {
    let stores = qall(
        conn,
        "SELECT store, version, id, supersedes, branch, created_seq FROM store_versions
         ORDER BY store, version",
    );
    let populations = qall(
        conn,
        "SELECT id, version, gate_id, gate_version, parent_pop_id, parent_pop_version,
                event_count, denominator_count, percent, status, basis
         FROM populations ORDER BY id, version",
    );
    let events = qall(conn, "SELECT seq, id, kind, branch, payload FROM events ORDER BY seq");
    let candidates = qall(
        conn,
        "SELECT id, subject, options, status, ruling, branch, created_seq, decided_seq
         FROM candidates ORDER BY created_seq",
    );
    let diagnostics = qall(
        conn,
        "SELECT id, subject, raw_repr, message, created_seq FROM diagnostics ORDER BY created_seq",
    );

    // 散点 + 补偿前后差异（派生层；原始表示仍在 raw_channels 存储中）
    let mut scatter: Vec<Value> = Vec::new();
    if let Ok(Some((_, _, raw))) = db::latest(conn, "raw_channels") {
        if raw["id"].as_str().map(|s| s.contains("boundary")).unwrap_or(false) {
            let comp = db::latest(conn, "comp_matrix")
                .ok()
                .flatten()
                .map(|(_, _, c)| rules::matrix_from_json(&c["matrix"]));
            for ev in raw["events"].as_array().unwrap() {
                let mut row = serde_json::Map::new();
                row.insert("id".into(), ev["id"].clone());
                let get = |ch: &str| -> Option<f64> {
                    ev["values"][ch]["raw"].as_str().and_then(|s| s.parse().ok())
                };
                for ch in ["FSC-A", "SSC-A", "FSC-H", "FL1-A", "FL2-A"] {
                    row.insert(
                        ch.into(),
                        get(ch).map(|v| json!(v)).unwrap_or(Value::Null),
                    );
                }
                if let (Some(m), Some(fl1), Some(fl2)) = (&comp, get("FL1-A"), get("FL2-A")) {
                    row.insert("FL1-A_comp".into(), json!(m[0][0] * fl1 + m[0][1] * fl2));
                    row.insert("FL2-A_comp".into(), json!(m[1][0] * fl1 + m[1][1] * fl2));
                }
                let flags: Vec<Value> = ev["values"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter_map(|(ch, cell)| {
                        cell.get("flag").map(|f| json!(format!("{}:{}", ch, f.as_str().unwrap())))
                    })
                    .collect();
                row.insert("flags".into(), json!(flags));
                scatter.push(Value::Object(row));
            }
        }
    }

    // 门树（最新门多边形 + 关系）
    let mut gate_tree: Vec<Value> = Vec::new();
    if let Ok(Some((gv, gid, gates))) = db::latest(conn, "gate_polygons") {
        let rels = db::latest(conn, "gate_relations")
            .ok()
            .flatten()
            .map(|(_, _, r)| r["relations"].clone())
            .unwrap_or(json!([]));
        gate_tree.push(json!({
            "version": gv, "id": gid,
            "gates": gates["gates"], "relations": rels
        }));
    }

    json!({
        "stores": stores,
        "populations": populations,
        "events": events,
        "candidates": candidates,
        "diagnostics": diagnostics,
        "scatter": scatter,
        "gate_tree": gate_tree,
    })
}

async fn state(State(s): State<Shared>) -> Json<Value> {
    let conn = s.lock().unwrap();
    Json(full_state(&conn))
}

async fn case_load(State(s): State<Shared>) -> (StatusCode, Json<Value>) {
    let mut conn = s.lock().unwrap();
    match casepack::import_baseline(&mut conn, "main") {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
    }
}

async fn comp_evaluate(State(s): State<Shared>) -> Json<Value> {
    let conn = s.lock().unwrap();
    Json(rules::evaluate_compensation(&conn, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

async fn populations_compute(State(s): State<Shared>) -> Json<Value> {
    let conn = s.lock().unwrap();
    Json(rules::compute_populations(&conn, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

async fn transforms_revise(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let conn = s.lock().unwrap();
    let ch = body["channel"].as_str().unwrap_or("FL1-A");
    let w = body["w"].as_f64().unwrap_or(0.8);
    Json(rules::revise_transform(&conn, ch, w, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

async fn gates_revise(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let conn = s.lock().unwrap();
    let gid = body["gate_id"].as_str().unwrap_or("gate:fitc-pos");
    let t = body["threshold"].as_f64();
    Json(rules::revise_gate(&conn, gid, t, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

async fn gates_rename(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let conn = s.lock().unwrap();
    let gid = body["gate_id"].as_str().unwrap_or("gate:fitc-pos");
    let name = body["name"].as_str().unwrap_or("FITC_high");
    Json(rules::rename_gate(&conn, gid, name, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

async fn rulings_apply(State(s): State<Shared>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let conn = s.lock().unwrap();
    let cid = body["candidate_id"].as_str().unwrap_or("");
    let decision = &body["decision"];
    let expected = body["expected_seq"].as_i64().unwrap_or(0);
    match rules::apply_ruling(&conn, cid, decision, expected, "main") {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)),
        Ok(Err(conflict)) => (StatusCode::CONFLICT, Json(conflict)),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))),
    }
}

async fn rulings_undo(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    let conn = s.lock().unwrap();
    let cid = body["candidate_id"].as_str().unwrap_or("");
    Json(rules::undo_ruling(&conn, cid, "main").unwrap_or_else(|e| json!({"error": e.to_string()})))
}

const TABLES: [&str; 7] = [
    "store_versions",
    "events",
    "populations",
    "deps",
    "candidates",
    "diagnostics",
    "seq",
];

fn dump_archive(conn: &Connection) -> Value {
    let mut tables = serde_json::Map::new();
    for t in TABLES {
        let order = match t {
            "store_versions" => "ORDER BY store, version",
            "events" => "ORDER BY seq",
            "populations" => "ORDER BY id, version",
            "candidates" => "ORDER BY created_seq",
            "diagnostics" => "ORDER BY created_seq",
            _ => "",
        };
        tables.insert(t.into(), json!(qall(conn, &format!("SELECT * FROM {} {}", t, order))));
    }
    json!({"format": "flow-lineage-archive/1", "tables": tables})
}

async fn export_archive(State(s): State<Shared>) -> Json<Value> {
    let conn = s.lock().unwrap();
    Json(dump_archive(&conn))
}

fn wipe_tables(conn: &Connection) -> rusqlite::Result<()> {
    for t in TABLES {
        conn.execute(&format!("DELETE FROM {}", t), [])?;
    }
    conn.execute("DELETE FROM sqlite_sequence WHERE name='events'", [])?;
    Ok(())
}

async fn wipe(State(s): State<Shared>) -> Json<Value> {
    let conn = s.lock().unwrap();
    match wipe_tables(&conn) {
        Ok(_) => Json(json!({"status": "wiped"})),
        Err(e) => Json(json!({"error": e.to_string()})),
    }
}

/// 导入档案：单事务；任何一行失败都回滚本次变更，不留半份记录。
fn restore_archive(conn: &mut Connection, archive: &Value) -> Result<Value, String> {
    if archive["format"].as_str() != Some("flow-lineage-archive/1") {
        return Err("未知档案格式".into());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let r = (|| -> rusqlite::Result<()> {
        let tables = archive["tables"].as_object().unwrap();
        for (table, rows) in tables {
            if !TABLES.contains(&table.as_str()) {
                continue;
            }
            for row in rows.as_array().unwrap() {
                let obj = row.as_object().unwrap();
                let cols: Vec<&String> = obj.keys().collect();
                let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{}", i)).collect();
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    table,
                    cols.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","),
                    placeholders.join(",")
                );
                let vals: Vec<rusqlite::types::Value> = cols
                    .iter()
                    .map(|c| match &obj[*c] {
                        Value::Null => rusqlite::types::Value::Null,
                        Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                rusqlite::types::Value::Integer(i)
                            } else {
                                rusqlite::types::Value::Real(n.as_f64().unwrap())
                            }
                        }
                        v @ (Value::Object(_) | Value::Array(_)) => {
                            rusqlite::types::Value::Text(v.to_string())
                        }
                        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                        Value::Bool(b) => rusqlite::types::Value::Integer(*b as i64),
                    })
                    .collect();
                tx.execute(&sql, rusqlite::params_from_iter(vals))?;
            }
        }
        Ok(())
    })();
    match r {
        Ok(_) => {
            tx.commit().map_err(|e| e.to_string())?;
            Ok(json!({"status": "imported"}))
        }
        Err(e) => {
            let _ = tx.rollback();
            Err(format!("导入失败已回滚本次变更: {}", e))
        }
    }
}

async fn import_archive(State(s): State<Shared>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut conn = s.lock().unwrap();
    match restore_archive(&mut conn, &body) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))),
    }
}

async fn drafts_compare(
    State(s): State<Shared>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let conn = s.lock().unwrap();
    let store = q.get("store").cloned().unwrap_or_else(|| "gate_polygons".into());
    let a: i64 = q.get("a").and_then(|x| x.parse().ok()).unwrap_or(1);
    let b: i64 = q.get("b").and_then(|x| x.parse().ok()).unwrap_or(1);
    let get = |v: i64| -> Option<Value> {
        conn.query_row(
            "SELECT payload FROM store_versions WHERE store=?1 AND version=?2",
            params![store, v],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .map(|p| serde_json::from_str(&p).unwrap())
    };
    Json(json!({
        "store": store,
        "a": {"version": a, "payload": get(a)},
        "b": {"version": b, "payload": get(b)},
    }))
}

/// 场景重放：依次经过矩阵奇异、极端信号、边界点、门嵌套改名、
/// 上游过期传播、同版本并发修改，最后导出→清空→重导入验证身份保持。
pub fn replay(conn: &mut Connection) -> Result<Value, String> {
    let mut log: Vec<Value> = Vec::new();
    let run = |log: &mut Vec<Value>, step: &str, r: Value| {
        log.push(json!({"step": step, "result": r}));
    };

    // 1 基线导入
    let r = casepack::import_baseline(conn, "main")?;
    run(&mut log, "import_baseline", r);

    // 2 近奇异补偿：拒绝静默求逆 + 候选
    let r = rules::evaluate_compensation(conn, "main").map_err(|e| e.to_string())?;
    let cand_id = r["candidate_id"].as_str().unwrap_or("").to_string();
    run(&mut log, "evaluate_compensation", r);

    // 3 群体计算（极端信号、边界点、不可转换值诊断在此留痕）
    let r = rules::compute_populations(conn, "main").map_err(|e| e.to_string())?;
    run(&mut log, "compute_populations", r);

    // 4 参数修订：logicle W 0.5→0.8（局部失效）
    let r = rules::revise_transform(conn, "FL1-A", 0.8, "main").map_err(|e| e.to_string())?;
    run(&mut log, "revise_transform", r);

    // 5 局部源改动：FITC+ 阈值 4000→4500（下游过期不覆盖）
    let r = rules::revise_gate(conn, "gate:fitc-pos", Some(4500.0), "main").map_err(|e| e.to_string())?;
    run(&mut log, "revise_gate", r);

    // 6 门嵌套改名：FITC+ → FITC_high（身份保持）
    let r = rules::rename_gate(conn, "gate:fitc-pos", "FITC_high", "main").map_err(|e| e.to_string())?;
    run(&mut log, "rename_gate", r);

    // 重算：产生新版本群体，旧版本保留为 stale
    let r = rules::compute_populations(conn, "main").map_err(|e| e.to_string())?;
    run(&mut log, "recompute_populations", r);

    // 7 同版本并发修改：两个窗口基于同一 expected_seq 提交裁定
    let created_seq: i64 = conn
        .query_row(
            "SELECT created_seq FROM candidates WHERE id=?1",
            params![cand_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let win_a = rules::apply_ruling(
        conn,
        &cand_id,
        &json!({"action": "accept", "option": "damped_pseudoinverse", "window": "A"}),
        created_seq,
        "branch-a",
    )
    .map_err(|e| e.to_string())?;
    run(&mut log, "ruling_window_a", json!(win_a));
    let win_b = rules::apply_ruling(
        conn,
        &cand_id,
        &json!({"action": "reject", "window": "B"}),
        created_seq,
        "branch-b",
    )
    .map_err(|e| e.to_string())?;
    let conflict = matches!(&win_b, Err(_));
    run(&mut log, "ruling_window_b", json!(win_b));

    // 8 导出 → 清空 → 重导入：身份、顺序、来源关系保持
    let before = dump_archive(conn);
    wipe_tables(conn).map_err(|e| e.to_string())?;
    restore_archive(conn, &before)?;
    let after = dump_archive(conn);
    let identical = before == after;
    db::emit_event(conn, "archive.reimported", "main", json!({
        "identity_preserved": identical,
        "note": "导出→删除本地库→重导入后，身份、顺序、来源关系保持"
    }))
    .map_err(|e| e.to_string())?;
    run(&mut log, "export_wipe_reimport", json!({"identity_preserved": identical, "conflict_detected": conflict}));

    Ok(json!({"status": "replayed", "identity_preserved": identical, "log": log}))
}

async fn scenario_replay(State(s): State<Shared>) -> (StatusCode, Json<Value>) {
    let mut conn = s.lock().unwrap();
    match replay(&mut conn) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))),
    }
}
