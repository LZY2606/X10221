//! Axum Web UI 与 JSON API。本地页面承担全部复核操作；
//! 视图选择/候选切换/接受否定都不改写原始数据，只追加事件。

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Router,
};
use rusqlite::Connection;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::archive::{export_all, import_all, Archive};
use crate::db;
use crate::domain::{EventPayload, RevisionBundle};
use crate::scenarios;
use crate::service::{self, ApiError, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/api/case", get(case_info))
        .route("/api/branches", get(list_branches).post(create_branch))
        .route("/api/events", get(events))
        .route("/api/view", get(view))
        .route("/api/diff", get(diff))
        .route("/api/append/generic", post(append_generic))
        .route("/api/action/singular-comp", post(action_singular))
        .route("/api/action/singular-decision", post(action_singular_decision))
        .route("/api/action/transform", post(action_transform))
        .route("/api/action/rename", post(action_rename))
        .route("/api/action/upstream-gate", post(action_upstream_gate))
        .route("/api/action/recompute-downstream", post(action_recompute_downstream))
        .route("/api/action/source-patch", post(action_source_patch))
        .route("/api/action/repin-dataset", post(action_repin_dataset))
        .route("/api/action/decide-boundary", post(action_decide_boundary))
        .route("/api/action/retract", post(action_retract))
        .route("/api/export", get(export_archive))
        .route("/api/reset-import", post(reset_import))
}

async fn index() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(include_str!("../static/index.html")))
        .unwrap()
}

async fn app_js() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .body(Body::from(include_str!("../static/app.js")))
        .unwrap()
}

#[derive(Deserialize)]
struct BranchQuery {
    branch: Option<String>,
    seq: Option<i64>,
}

async fn case_info(State(st): State<AppState>) -> impl IntoResponse {
    let conn = st.conn.lock().await;
    let seeded = db::has_events(&conn, "main").unwrap_or(false);
    let bytes = service::baseline_source_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    json_ok(serde_json::json!({
        "case_id": crate::case::CASE_ID,
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "seeded": seeded,
        "source_text": text,
        "source_bytes_b64": crate::base64_lite::encode(&bytes),
    }))
}

async fn list_branches(State(st): State<AppState>) -> impl IntoResponse {
    let conn = st.conn.lock().await;
    json_ok(serde_json::json!({ "branches": db::list_branches(&conn).unwrap_or_default() }))
}

#[derive(Deserialize)]
struct CreateBranchReq {
    branch: String,
    from: Option<String>,
    note: Option<String>,
}

async fn create_branch(
    State(st): State<AppState>,
    Form(req): Form<CreateBranchReq>,
) -> Response {
    let mut conn = st.conn.lock().await;
    if db::branch_exists(&conn, &req.branch) {
        return err(409, ApiError {
            error: format!("分支 {} 已存在", req.branch),
            code: Some("branch_exists".into()),
            conflict: None,
        });
    }
    let from = req.from.clone().unwrap_or_else(|| "main".into());
    match db::create_branch(&mut conn, &req.branch, &from, req.note.as_deref().unwrap_or("")) {
        Ok(forked_seq) => json_ok(serde_json::json!({
            "branch": req.branch, "forked_from": from, "forked_seq": fored_seq_unwrap(forked_seq)
        })),
        Err(e) => err(500, ApiError { error: e.to_string(), code: Some("db".into()), conflict: None }),
    }
}

fn fored_seq_unwrap(v: i64) -> i64 { v }

async fn events(
    State(st): State<AppState>,
    Query(q): Query<BranchQuery>,
) -> impl IntoResponse {
    let conn = st.conn.lock().await;
    let branch = q.branch.unwrap_or_else(|| "main".into());
    json_ok(serde_json::json!({
        "branch": branch,
        "head_seq": db::head_seq(&conn, &branch).unwrap_or(0),
        "events": service::events_listing(&conn, &branch),
    }))
}

async fn view(
    State(st): State<AppState>,
    Query(q): Query<BranchQuery>,
) -> impl IntoResponse {
    let conn = st.conn.lock().await;
    let branch = q.branch.clone().unwrap_or_else(|| "main".into());
    if !db::branch_exists(&conn, &branch) {
        return err(404, ApiError { error: format!("未知分支 {branch}"), code: Some("no_branch".into()), conflict: None });
    }
    json_ok(service::view(&conn, &branch, q.seq))
}

#[derive(Deserialize)]
struct DiffQuery {
    a_branch: String,
    b_branch: String,
    a_seq: Option<i64>,
    b_seq: Option<i64>,
}

async fn diff(State(st): State<AppState>, Query(q): Query<DiffQuery>) -> Response {
    let conn = st.conn.lock().await;
    let a = service::view(&conn, &q.a_branch, q.a_seq);
    let b = service::view(&conn, &q.b_branch, q.b_seq);
    let changes = json_diff(&a, &b, "");
    json_ok(serde_json::json!({
        "a_branch": q.a_branch, "a_seq": a["requested_seq"],
        "b_branch": q.b_branch, "b_seq": b["requested_seq"],
        "a": a, "b": b, "changes": changes,
    }))
}

/// 叶子路径级别的差异（每个显示值都有稳定路径身份）。
fn json_diff(a: &serde_json::Value, b: &serde_json::Value, path: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    match (a, b) {
        (serde_json::Value::Object(am), serde_json::Value::Object(bm)) => {
            let mut keys: Vec<&String> = am.keys().chain(bm.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                match (am.get(k), bm.get(k)) {
                    (Some(x), Some(y)) => out.extend(json_diff(x, y, &p)),
                    (Some(x), None) => out.push(serde_json::json!({"path": p, "a": x, "b": null})),
                    (None, Some(y)) => out.push(serde_json::json!({"path": p, "a": null, "b": y})),
                    _ => {}
                }
            }
        }
        _ => {
            if a != b {
                out.push(serde_json::json!({"path": path, "a": a, "b": b}));
            }
        }
    }
    out
}

#[derive(Deserialize)]
struct ActionReq {
    branch: Option<String>,
    #[serde(default)]
    expected_seq: Option<i64>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    actor: Option<String>,
}

fn resolve_seq(conn: &Connection, branch: &str, expected: Option<i64>) -> i64 {
    expected.or_else(|| db::head_seq(conn, branch).ok()).unwrap_or(0)
}

async fn do_append(
    st: &AppState,
    branch: &str,
    expected_seq: Option<i64>,
    actor: Option<&str>,
    tag: &str,
    payload: EventPayload,
) -> Response {
    let mut conn = st.conn.lock().await;
    if !db::branch_exists(&conn, branch) {
        return err(404, ApiError { error: format!("未知分支 {branch}"), code: Some("no_branch".into()), conflict: None });
    }
    let seq = resolve_seq(&conn, branch, expected_seq);
    let event_id = service::new_event_id(branch, tag, seq + 1);
    match service::append(&mut conn, branch, seq, &event_id, actor, payload) {
        Ok(new_seq) => json_ok(serde_json::json!({
            "branch": branch, "seq": new_seq, "event_id": event_id,
            "view": service::view(&conn, branch, Some(new_seq)),
        })),
        Err(e) => {
            let (code, body) = service::from_db_error(e);
            err(code, body)
        }
    }
}

async fn action_singular(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::ParameterRevised {
        bundle: scenarios::singular_comp_bundle(),
        reason: "重新拟合补偿，提交近奇异矩阵（禁止默默求逆）".into(),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "singular", payload).await
}

async fn action_transform(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::ParameterRevised {
        bundle: scenarios::transform_revision_bundle(),
        reason: r.rationale.clone().unwrap_or_else(|| "asinh cofactor 150→100".into()),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "transform", payload).await
}

async fn action_rename(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::GateRenamed { bundle: scenarios::rename_bundle() };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "rename", payload).await
}

async fn action_upstream_gate(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::GateRevised {
        bundle: scenarios::upstream_gate_bundle(),
        reason: "细胞门扩大，触发上游过期传播".into(),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "upstream", payload).await
}

async fn action_recompute_downstream(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::Recomputed {
        bundle: scenarios::recompute_downstream_after_cells(),
        reason: "显式重算下游到新上游门；旧结果保留".into(),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "recompute", payload).await
}

async fn action_source_patch(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::SourcePatched {
        bundle: scenarios::local_source_patch_bundle(),
        reason: "局部源数据改动（E05 FITC 重对齐）".into(),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "source-patch", payload).await
}

async fn action_repin_dataset(State(st): State<AppState>, Form(r): Form<ActionReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = EventPayload::Recomputed {
        bundle: scenarios::repin_dataset_bundle(),
        reason: "显式重算门到 DS-v2".into(),
    };
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "repin-ds", payload).await
}

#[derive(Deserialize)]
struct BoundaryReq {
    branch: Option<String>,
    expected_seq: Option<i64>,
    gate_id: String,
    accept: Option<String>, // include | exclude | reject-all
    rationale: Option<String>,
    actor: Option<String>,
}

async fn action_decide_boundary(
    State(st): State<AppState>,
    Form(r): Form<BoundaryReq>,
) -> Response {
    let mut conn = st.conn.lock().await;
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    if !db::branch_exists(&conn, &branch) {
        return err(404, ApiError { error: format!("未知分支 {branch}"), code: Some("no_branch".into()), conflict: None });
    }
    // 找到该门当前边界候选集（取最新门版本）。
    let view = service::view(&conn, &branch, None);
    let sets = view["candidate_sets"].as_array().cloned().unwrap_or_default();
    let set = sets.iter().find(|s| {
        s["kind"] == "boundary-membership"
            && s["subject"].as_str().unwrap_or("").starts_with(&format!("{}|", r.gate_id))
            && s["accepted"].is_null()
    });
    let set = match set {
        Some(s) => s.clone(),
        None => {
            return err(404, ApiError {
                error: format!("门 {} 没有待裁定的边界候选集", r.gate_id),
                code: Some("no_candidates".into()), conflict: None,
            });
        }
    };
    let set_id = set["candidate_set_id"].as_str().unwrap().to_string();
    let (accepted, rejected) = match r.accept.as_deref() {
        Some("include") => {
            let id = set["candidates"][0]["candidate_id"].as_str().unwrap().to_string();
            (Some(id), vec![set["candidates"][1]["candidate_id"].as_str().unwrap_or("").to_string()])
        }
        Some("exclude") => {
            let id = set["candidates"][1]["candidate_id"].as_str().unwrap().to_string();
            (Some(id), vec![set["candidates"][0]["candidate_id"].as_str().unwrap_or("").to_string()])
        }
        _ => (None, set["candidates"].as_array().map(|a| a.iter().filter_map(|c| c["candidate_id"].as_str().map(String::from)).collect()).unwrap_or_default()),
    };
    let payload = scenarios::decision(&set_id, accepted.as_deref(), rejected, r.rationale.as_deref().unwrap_or("边界点人工裁定"));
    let seq = resolve_seq(&conn, &branch, r.expected_seq);
    let event_id = service::new_event_id(&branch, "decide-boundary", seq + 1);
    match service::append(&mut conn, &branch, seq, &event_id, r.actor.as_deref(), payload) {
        Ok(new_seq) => json_ok(serde_json::json!({"branch": branch, "seq": new_seq, "event_id": event_id})),
        Err(e) => { let (c, b) = service::from_db_error(e); return err(c, b); }
    }
}

#[derive(Deserialize)]
struct SingularReq {
    branch: Option<String>,
    expected_seq: Option<i64>,
    /// `reject` = 拒绝近奇异矩阵，门回钉 COMP-v1；`regularize` = 显式正则化（contested）。
    choice: String,
    rationale: Option<String>,
    actor: Option<String>,
}

/// 近奇异补偿的互相冲突人工决定：reject 与 regularize 是两个互斥分支。
/// 同一分支上先做一次后再做相反决定，系统保留两条决定事件与候选历史。
async fn action_singular_decision(
    State(st): State<AppState>,
    Form(r): Form<SingularReq>,
) -> Response {
    let mut conn = st.conn.lock().await;
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    if !db::branch_exists(&conn, &branch) {
        return err(404, ApiError { error: format!("未知分支 {branch}"), code: Some("no_branch".into()), conflict: None });
    }
    let set_id = crate::util::stable_id("CS-COMP", "COMP-v2");
    let seq = resolve_seq(&conn, &branch, r.expected_seq);
    let rationale = r.rationale.clone().unwrap_or_else(|| match r.choice.as_str() {
        "reject" => "拒绝近奇异补偿矩阵，不做求逆".into(),
        "regularize" => "审阅者显式接受加脊正则化（有争议人工修正）".into(),
        other => format!("决定 {other}"),
    });

    let (accepted_cand, rejected_cands, repin_bundle) = match r.choice.as_str() {
        "reject" => (
            "COMP-v2:reject".to_string(),
            vec!["COMP-v2:regularize".to_string()],
            scenarios::reject_singular_bundle(),
        ),
        "regularize" => (
            "COMP-v2:regularize".to_string(),
            vec!["COMP-v2:reject".to_string(), "COMP-v2:reject".to_string()].into_iter().take(1).collect::<Vec<_>>(),
            RevisionBundle { gates: crate::case::gates_repinned("COMP-v2", "v2reg", "显式正则化后继续钉 COMP-v2（contested）"), ..Default::default() },
        ),
        other => return err(400, ApiError { error: format!("未知选择 {other}"), code: Some("bad_choice".into()), conflict: None }),
    };

    let tx_payload = scenarios::decision(&set_id, Some(&accepted_cand), rejected_cands, &rationale);
    let ev1 = service::new_event_id(&branch, "decide-singular", seq + 1);
    if let Err(e) = service::append(&mut conn, &branch, seq, &ev1, r.actor.as_deref(), tx_payload) {
        let (c, b) = service::from_db_error(e);
        return err(c, b);
    }
    let payload2 = EventPayload::Recomputed {
        bundle: repin_bundle,
        reason: format!("近奇异补偿裁定：{rationale}"),
    };
    let ev2 = service::new_event_id(&branch, "recompute-after-singular", seq + 2);
    match service::append(&mut conn, &branch, seq + 1, &ev2, r.actor.as_deref(), payload2) {
        Ok(new_seq) => json_ok(serde_json::json!({
            "branch": branch, "seq": new_seq,
            "decision_event_id": ev1, "recompute_event_id": ev2,
            "view": service::view(&conn, &branch, Some(new_seq)),
        })),
        Err(e) => { let (c, b) = service::from_db_error(e); return err(c, b); }
    }
}

#[derive(Deserialize)]
struct RetractReq {
    branch: Option<String>,
    expected_seq: Option<i64>,
    target_event_id: String,
    reason: Option<String>,
    actor: Option<String>,
}

async fn action_retract(State(st): State<AppState>, Form(r): Form<RetractReq>) -> Response {
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    let payload = scenarios::retraction(&r.target_event_id, r.reason.as_deref().unwrap_or("审阅撤销"));
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "retract", payload).await
}

#[derive(Deserialize)]
struct GenericReq {
    branch: Option<String>,
    expected_seq: Option<i64>,
    payload_json: String,
    actor: Option<String>,
}

async fn append_generic(State(st): State<AppState>, Form(r): Form<GenericReq>) -> Response {
    let payload: EventPayload = match serde_json::from_str(&r.payload_json) {
        Ok(p) => p,
        Err(e) => return err(400, ApiError { error: format!("payload_json 解析失败: {e}"), code: Some("bad_payload".into()), conflict: None }),
    };
    let branch = r.branch.clone().unwrap_or_else(|| "main".into());
    do_append(&st, &branch, r.expected_seq, r.actor.as_deref(), "generic", payload).await
}

async fn export_archive(State(st): State<AppState>) -> Response {
    let conn = st.conn.lock().await;
    match export_all(&conn) {
        Ok(archive) => {
            let bytes = service::baseline_source_bytes();
            let mut value = serde_json::to_value(&archive).unwrap_or(serde_json::json!({}));
            value["raw_source_b64"] = serde_json::Value::String(crate::base64_lite::encode(&bytes));
            json_ok(value)
        }
        Err(e) => err(500, ApiError { error: e.to_string(), code: Some("db".into()), conflict: None }),
    }
}

#[derive(Deserialize)]
struct ResetReq {
    archive_json: String,
    raw_source_b64: String,
}

/// 删除本地数据库（事件/分支/字节）后重新导入归档：
/// 身份、顺序与来源关系必须保持。整体事务，失败回滚。
async fn reset_import(State(st): State<AppState>, Form(r): Form<ResetReq>) -> Response {
    let archive: Archive = match serde_json::from_str(&r.archive_json) {
        Ok(a) => a,
        Err(e) => return err(400, ApiError { error: format!("归档解析失败: {e}"), code: Some("bad_archive".into()), conflict: None }),
    };
    let raw_bytes = match crate::base64_lite::decode(&r.raw_source_b64) {
        Ok(b) => b,
        Err(e) => return err(400, ApiError { error: format!("raw 字节解码失败: {e}"), code: Some("bad_blob".into()), conflict: None }),
    };

    // 在同一连接上清空本地库，然后整体导入；导入失败由其内部事务回滚。
    let mut guard = st.conn.lock().await;
    if let Err(e) = guard.execute_batch(
        "DELETE FROM events; DELETE FROM branches; DELETE FROM blobs; DELETE FROM case_state;",
    ) {
        return err(500, ApiError { error: e.to_string(), code: Some("db".into()), conflict: None });
    }
    if let Err(e) = import_all(&mut guard, &archive, &raw_bytes) {
        return err(400, ApiError { error: format!("导入失败，已回滚，不留半份记录: {e}"), code: Some("import_failed".into()), conflict: None });
    }
    json_ok(serde_json::json!({"imported": true, "branches": db::list_branches(&*guard).unwrap_or_default()}))
}

fn json_ok(value: serde_json::Value) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(serde_json::to_string(&value).unwrap_or_default()))
        .unwrap()
}

fn err(code: u16, body: ApiError) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, json_ok(serde_json::to_value(body).unwrap_or_default())).into_response()
}

/// 绑定 TCP 并返回实际地址（测试可用端口 0）。
pub async fn serve(state: AppState, addr: &str) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let app = router().with_state(state);
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    Ok((local, handle))
}

/// 构造 AppState 并在空库时播种。
pub fn build_state(db_path: &str) -> rusqlite::Result<AppState> {
    let mut conn = db::open(db_path)?;
    service::seed_if_empty(&mut conn)?;
    Ok(AppState { db_path: db_path.to_string(), conn: Arc::new(Mutex::new(conn)) })
}
