//! Axum HTTP 层。审阅页为静态 SPA；所有写动作走 JSON API 并带乐观并发。
use crate::case::{discover_case_dir, load_case, replay};
use crate::domain::engine::{build_scatter, diff_views};
use crate::domain::math::{Point, TransformSpec};
use crate::domain::model::DecisionOutcome;
use crate::service::{ConflictDetail, GateDraft, Service};
use crate::store::{Bundle, Store};
use axum::{
    extract::{Multipart, Path as AxumPath, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json, Redirect},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone)]
pub struct AppState {
    pub svc: Arc<Service>,
}

pub fn router(svc: Arc<Service>) -> Router {
    let state = AppState { svc };
    Router::new()
        .route("/", get(|| async { Redirect::to("/app") }))
        .route("/app", get(index))
        .route("/static/app.js", get(app_js))
        .route("/static/app.css", get(app_css))
        .route("/api/health", get(health))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{pid}", get(project_detail))
        .route("/api/projects/{pid}/load-case", post(load_bundled_case))
        .route("/api/projects/{pid}/events", get(list_events))
        .route("/api/projects/{pid}/events/{seq}", get(event_detail))
        .route("/api/projects/{pid}/view", get(view))
        .route("/api/projects/{pid}/scatter", get(scatter))
        .route("/api/projects/{pid}/diff", get(diff))
        .route("/api/projects/{pid}/fcs/{source_id}", get(fcs_detail))
        .route("/api/projects/{pid}/import-fcs", post(import_fcs))
        .route("/api/projects/{pid}/spillovers", post(record_spillover))
        .route("/api/projects/{pid}/transforms", post(set_transform))
        .route("/api/projects/{pid}/gates", post(save_gate))
        .route("/api/projects/{pid}/gates/rename", post(rename_gate))
        .route("/api/projects/{pid}/trees", post(save_tree))
        .route("/api/projects/{pid}/decisions", post(decide))
        .route("/api/projects/{pid}/revert", post(revert))
        .route("/api/projects/{pid}/forks", post(fork))
        .route("/api/projects/{pid}/conflicts", post(record_conflict))
        .route("/api/export", get(export_bundle))
        .route("/api/import-bundle", post(import_bundle))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}
async fn app_js() -> (StatusCode, [(&'static str, &'static str); 1], &'static str) {
    (
        StatusCode::OK,
        [("content-type", "application/javascript")],
        include_str!("../web/app.js"),
    )
}
async fn app_css() -> (StatusCode, [(&'static str, &'static str); 1], &'static str) {
    (
        StatusCode::OK,
        [("content-type", "text/css")],
        include_str!("../web/app.css"),
    )
}

async fn health(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "algorithm": crate::domain::model::ALGORITHM_VERSION,
        "schema_version": st.svc.store.schema_version().unwrap_or(0),
    }))
}

fn err(status: StatusCode, msg: String, conflict: Option<ConflictDetail>) -> impl IntoResponse {
    (
        status,
        Json(serde_json::json!({ "error": msg, "conflict": conflict })),
    )
}

async fn list_projects(State(st): State<AppState>) -> impl IntoResponse {
    match st.svc.store.list_projects() {
        Ok(rows) => Json(serde_json::to_value(rows).unwrap()).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateProjectReq {
    title: String,
    #[serde(default)]
    project_id: Option<String>,
}

async fn create_project(
    State(st): State<AppState>,
    Json(req): Json<CreateProjectReq>,
) -> impl IntoResponse {
    let pid = req
        .project_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("proj-{}", unix_ms()));
    match st.svc.create_project(&pid, &req.title) {
        Ok(seq) => Json(serde_json::json!({ "project_id": pid, "head_seq": seq })).into_response(),
        Err(e) => err(StatusCode::from_u16(e.status).unwrap(), e.message, e.conflict).into_response(),
    }
}

async fn project_detail(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
) -> impl IntoResponse {
    let events = match st.svc.store.load_events(&pid) {
        Ok(e) => e,
        Err(e) => return err(StatusCode::NOT_FOUND, e.to_string(), None).into_response(),
    };
    let branches: std::collections::BTreeSet<String> =
        events.iter().map(|e| e.branch.clone()).collect();
    let blobs = st.svc.store.list_blobs(&pid).unwrap_or_default();
    Json(serde_json::json!({
        "project_id": pid,
        "head_seq": st.svc.store.head_seq(&pid).unwrap_or(0),
        "branches": branches,
        "blobs": blobs,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ViewQuery {
    branch: Option<String>,
    at_seq: Option<u64>,
}

async fn view(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Query(q): Query<ViewQuery>,
) -> impl IntoResponse {
    let branch = q.branch.unwrap_or_else(|| "main".into());
    match st.svc.view(&pid, &branch, q.at_seq) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::from_u16(e.status).unwrap(), e.message, e.conflict)
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ScatterQuery {
    branch: Option<String>,
    source_id: String,
    x: String,
    y: String,
    at_seq: Option<u64>,
}

async fn scatter(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Query(q): Query<ScatterQuery>,
) -> impl IntoResponse {
    let branch = q.branch.unwrap_or_else(|| "main".into());
    let view = match st.svc.view(&pid, &branch, q.at_seq) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::from_u16(e.status).unwrap(), e.message, None).into_response(),
    };
    let fcs = match st.svc.fcs_of(&pid, &q.source_id) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::NOT_FOUND, e.message, None).into_response(),
    };
    Json(build_scatter(&view, &fcs, &q.x, &q.y)).into_response()
}

#[derive(Deserialize)]
struct DiffQuery {
    left_branch: String,
    right_branch: String,
    left_seq: Option<u64>,
    right_seq: Option<u64>,
}

async fn diff(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Query(q): Query<DiffQuery>,
) -> impl IntoResponse {
    let l = match st.svc.view(&pid, &q.left_branch, q.left_seq) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::from_u16(e.status).unwrap(), e.message, None).into_response(),
    };
    let r = match st.svc.view(&pid, &q.right_branch, q.right_seq) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::from_u16(e.status).unwrap(), e.message, None).into_response(),
    };
    Json(serde_json::json!({
        "left_branch": q.left_branch,
        "right_branch": q.right_branch,
        "rows": diff_views(&l, &r),
    }))
    .into_response()
}

async fn list_events(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
) -> impl IntoResponse {
    match st.svc.store.load_events(&pid) {
        Ok(events) => {
            let light: Vec<_> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "seq": e.seq,
                        "event_id": e.event_id,
                        "branch": e.branch,
                        "parent_branch": e.parent_branch,
                        "kind": e.payload.kind(),
                        "direct_source": e.payload.direct_source(),
                    })
                })
                .collect();
            Json(light).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None).into_response(),
    }
}

async fn event_detail(
    State(st): State<AppState>,
    AxumPath((pid, seq)): AxumPath<(String, u64)>,
) -> impl IntoResponse {
    match st.svc.store.load_events(&pid) {
        Ok(events) => match events.into_iter().find(|e| e.seq == seq) {
            Some(e) => Json(e).into_response(),
            None => err(StatusCode::NOT_FOUND, format!("事件 #{seq} 不存在"), None).into_response(),
        },
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None).into_response(),
    }
}

async fn fcs_detail(
    State(st): State<AppState>,
    AxumPath((pid, sid)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    match st.svc.fcs_of(&pid, &sid) {
        Ok(f) => Json(f).into_response(),
        Err(e) => err(StatusCode::from_u16(e.status).unwrap(), e.message, None).into_response(),
    }
}

#[derive(Deserialize)]
struct LoadCaseReq {
    #[serde(default)]
    project_id: Option<String>,
}

async fn load_bundled_case(
    State(st): State<AppState>,
    AxumPath(pid_hint): AxumPath<String>,
    Json(req): Json<LoadCaseReq>,
) -> impl IntoResponse {
    let pid = req.project_id.unwrap_or(pid_hint);
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(dir) = discover_case_dir(&cwd) else {
        return err(StatusCode::NOT_FOUND, "仓库内未找到 cases/flow-boundary-chamber".into(), None)
            .into_response();
    };
    let (def, root) = match load_case(&dir) {
        Ok(v) => v,
        Err(e) => return err(StatusCode::BAD_REQUEST, e, None).into_response(),
    };
    match replay(&st.svc, &pid, &def, &root) {
        Ok(reports) => Json(serde_json::json!({
            "project_id": pid,
            "case_id": def.case_id,
            "steps": reports,
        }))
        .into_response(),
        Err(f) => err(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("重放失败 step {} ({}): {}", f.step, f.action, f.message),
            None,
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
struct ImportQuery {
    branch: Option<String>,
    expected_head: Option<u64>,
    source_id: String,
    role: String,
    revision: Option<u32>,
    #[serde(default)]
    replaces: Option<String>,
}

async fn import_fcs(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Query(q): Query<ImportQuery>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut filename = String::from("upload.fcs");
    let mut bytes = Vec::new();
    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        filename = field.file_name().unwrap_or("upload.fcs").to_string();
        let data = field.bytes().await;
        if let Ok(d) = data {
            bytes = d.to_vec();
        }
    }
    if bytes.is_empty() {
        return err(StatusCode::BAD_REQUEST, "未收到 FCS 文件".into(), None).into_response();
    }
    let role = if q.role == "single_stain" {
        crate::domain::model::SourceRole::SingleStain
    } else {
        crate::domain::model::SourceRole::Sample
    };
    let affected = if q.revision.unwrap_or(1) > 1 {
        vec!["compensation:sp-main".into(), "populations:all-compensated".into()]
    } else {
        vec![]
    };
    match st.svc.import_fcs(
        &pid,
        q.branch.as_deref().unwrap_or("main"),
        q.expected_head,
        &q.source_id,
        role,
        q.revision.unwrap_or(1),
        q.replaces,
        affected,
        &filename,
        &bytes,
    ) {
        Ok(seq) => Json(serde_json::json!({ "head_seq": seq })).into_response(),
        Err(e) => {
            let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST);
            err(status, e.message, e.conflict).into_response()
        }
    }
}

#[derive(Deserialize)]
struct WriteReq {
    branch: Option<String>,
    expected_head: Option<u64>,
    spillover_id: Option<String>,
    revision: Option<u32>,
    channel_order: Option<Vec<String>>,
    matrix: Option<Vec<Vec<f64>>>,
    unit: Option<String>,
    basis: Option<String>,
    derived_from_sources: Option<Vec<String>>,
    channel: Option<String>,
    spec: Option<TransformSpec>,
    gate: Option<GateDraft>,
    note: Option<String>,
    gate_id: Option<String>,
    new_name: Option<String>,
    edges: Option<Vec<(String, Option<String>)>>,
    outcome: Option<String>,
    candidate_id: Option<String>,
    rationale: Option<String>,
    reverted_event_id: Option<String>,
    from_branch: Option<String>,
    new_branch: Option<String>,
    display_name: Option<String>,
    fork_at_head: Option<u64>,
}

fn handle_cmd<T: Serialize>(r: Result<T, crate::service::CommandError>) -> axum::response::Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST), e.message, e.conflict)
            .into_response(),
    }
}

async fn record_spillover(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.record_spillover(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.spillover_id.as_deref().unwrap_or("sp-main"),
        r.revision.unwrap_or(1),
        r.channel_order.unwrap_or_default(),
        r.matrix.unwrap_or_default(),
        r.derived_from_sources.unwrap_or_default(),
        r.unit.as_deref().unwrap_or("fraction"),
        r.basis.as_deref().unwrap_or("unspecified"),
    ))
}

async fn set_transform(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    let Some(channel) = r.channel else {
        return err(StatusCode::BAD_REQUEST, "缺少 channel".into(), None).into_response();
    };
    let Some(spec) = r.spec else {
        return err(StatusCode::BAD_REQUEST, "缺少 spec".into(), None).into_response();
    };
    handle_cmd(st.svc.set_transform(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        &channel,
        r.revision.unwrap_or(1),
        spec,
    ))
}

async fn save_gate(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    let Some(gate) = r.gate else {
        return err(StatusCode::BAD_REQUEST, "缺少 gate".into(), None).into_response();
    };
    handle_cmd(st.svc.save_gate(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        gate,
        r.note.unwrap_or_else(|| "审阅页保存门".into()),
    ))
}

async fn rename_gate(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.rename_gate(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.gate_id.as_deref().unwrap_or(""),
        r.new_name.as_deref().unwrap_or(""),
    ))
}

async fn save_tree(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.save_tree(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.revision.unwrap_or(1),
        r.edges.unwrap_or_default(),
    ))
}

async fn decide(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    let outcome = match r.outcome.as_deref() {
        Some("accepted_auto") => DecisionOutcome::AcceptedAuto,
        Some("accepted_candidate") => DecisionOutcome::AcceptedCandidate,
        Some("rejected") => DecisionOutcome::Rejected,
        _ => return err(StatusCode::BAD_REQUEST, "outcome 非法".into(), None).into_response(),
    };
    handle_cmd(st.svc.decide_compensation(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.spillover_id.as_deref().unwrap_or("sp-main"),
        outcome,
        r.candidate_id,
        r.rationale.unwrap_or_default(),
    ))
}

async fn revert(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.revert_decision(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.reverted_event_id.as_deref().unwrap_or(""),
        r.note.unwrap_or_else(|| "审阅页撤销".into()),
    ))
}

async fn fork(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<WriteReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.fork(
        &pid,
        r.from_branch.as_deref().unwrap_or("main"),
        r.new_branch.as_deref().unwrap_or("draft"),
        r.display_name.as_deref().unwrap_or("草稿分支"),
        r.fork_at_head.or(r.expected_head),
    ))
}

#[derive(Deserialize)]
struct ConflictReq {
    branch: Option<String>,
    expected_head: u64,
    attempted_action: String,
    left_draft: String,
    right_draft: String,
    diff: Vec<String>,
}

async fn record_conflict(
    State(st): State<AppState>,
    AxumPath(pid): AxumPath<String>,
    Json(r): Json<ConflictReq>,
) -> axum::response::Response {
    handle_cmd(st.svc.record_conflict(
        &pid,
        r.branch.as_deref().unwrap_or("main"),
        r.expected_head,
        r.attempted_action,
        r.left_draft,
        r.right_draft,
        r.diff,
    ))
}

async fn export_bundle(State(st): State<AppState>) -> axum::response::Response {
    match st.svc.store.export_bundle("http-api") {
        Ok(b) => Json(b).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string(), None).into_response(),
    }
}

async fn import_bundle(Json(bundle): Json<Bundle>) -> impl IntoResponse {
    // 导入到临时库，成功后再替换进程内数据库文件由运维完成；
    // 这里用同目录新文件做完整校验（失败不留半成品）。
    let tmp = format!("flowlineage-import-{}.db", unix_ms());
    match Store::import_bundle(&tmp, &bundle) {
        Ok(()) => {
            let _ = std::fs::remove_file(&tmp);
            Json(serde_json::json!({"imported_projects": bundle.projects.len()}))
                .into_response()
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            err(StatusCode::BAD_REQUEST, e.to_string(), None).into_response()
        }
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// Point/Spec 的新类型仅为序列化再导出
#[allow(unused)]
fn _unused(_p: Point) {}
