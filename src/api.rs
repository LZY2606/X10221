//! Axum HTTP API.

use crate::casepack::{self, default_case_dir};
use crate::export;
use crate::pipeline;
use crate::state;
use crate::store::{self, Conflict, Store};
use axum::{
    body::Bytes,
    extract::{Path as AxPath, Query, State as AxState},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub case_dir: Arc<PathBuf>,
}

pub fn router(store: Arc<Store>, case_dir: PathBuf) -> Router {
    let st = AppState { store, case_dir: Arc::new(case_dir) };
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/api/state", get(get_state))
        .route("/api/compare", get(get_compare))
        .route("/api/events", get(get_events))
        .route("/api/object/{id}/versions", get(get_versions))
        .route("/api/scatter", get(get_scatter))
        .route("/api/case/load", post(case_load))
        .route("/api/case/scenario/{name}", post(case_scenario))
        .route("/api/adjudicate", post(adjudicate))
        .route("/api/adjudicate/revert", post(adjudicate_revert))
        .route("/api/gates/rename", post(gate_rename))
        .route("/api/gates/revise", post(gate_revise))
        .route("/api/drafts", post(draft_create))
        .route("/api/drafts/save", post(draft_save))
        .route("/api/import/fcs", post(import_fcs))
        .route("/api/export", get(do_export))
        .route("/api/reset", post(do_reset))
        .route("/api/import/archive", post(import_archive))
        .with_state(st)
}

fn page(content_type: &str, body: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(body))
        .unwrap()
}

async fn index() -> Response {
    page("text/html; charset=utf-8", include_str!("../web/index.html"))
}
async fn app_js() -> Response {
    page("application/javascript; charset=utf-8", include_str!("../web/app.js"))
}
async fn style_css() -> Response {
    page("text/css; charset=utf-8", include_str!("../web/style.css"))
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn ok(v: Value) -> ApiResult {
    Ok(Json(v))
}
fn err(status: StatusCode, v: Value) -> ApiResult {
    Err((status, Json(v)))
}
fn err500(e: impl std::fmt::Display) -> ApiResult {
    err(StatusCode::INTERNAL_SERVER_ERROR, json!({"error": e.to_string()}))
}
fn conflict(c: Conflict) -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "version conflict",
            "branch": c.branch,
            "expected_head": c.expected_head,
            "current_head": c.current_head,
            "events_since_base": c.events_since_base.iter().map(|e|
                json!({"seq": e.seq, "id": e.id, "kind": e.kind, "payload": e.payload})).collect::<Vec<_>>(),
            "rejected_ops": c.your_ops,
        })),
    )
}

#[derive(serde::Deserialize)]
struct BranchQuery {
    branch: Option<String>,
    at_seq: Option<i64>,
}

async fn get_state(AxState(s): AxState<AppState>, Query(q): Query<BranchQuery>) -> ApiResult {
    let branch = q.branch.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    ok(state::branch_state(&c, &branch, q.at_seq))
}

async fn get_compare(AxState(s): AxState<AppState>, Query(q): Query<CompareQuery>) -> ApiResult {
    let c = s.store.conn.lock().unwrap();
    ok(state::compare(&c, &q.left, &q.right))
}
#[derive(serde::Deserialize)]
struct CompareQuery {
    left: String,
    right: String,
}

async fn get_events(AxState(s): AxState<AppState>, Query(q): Query<BranchQuery>) -> ApiResult {
    let c = s.store.conn.lock().unwrap();
    let b = q.branch.as_deref();
    ok(json!({"events": store::list_events(&c, b).iter()
        .map(|e| json!({"seq": e.seq, "id": e.id, "ts": e.ts, "branch": e.branch,
                        "kind": e.kind, "payload": e.payload}))
        .collect::<Vec<_>>()}))
}

async fn get_versions(AxState(s): AxState<AppState>, AxPath(id): AxPath<String>, Query(q): Query<BranchQuery>) -> ApiResult {
    let branch = q.branch.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    let rows = store::list_versions(&c, &id, &branch);
    ok(json!({"id": id, "branch": branch, "versions": rows.iter().map(|o| json!({
        "version": o.version, "kind": o.kind, "status": o.status, "note": o.note,
        "created_seq": o.created_seq, "payload": o.payload})).collect::<Vec<_>>()}))
}

async fn case_load(AxState(s): AxState<AppState>) -> ApiResult {
    let dir = (*s.case_dir).clone();
    let c = s.store.conn.lock().unwrap();
    export::reset(&c).map_err(err500)?;
    casepack::load_baseline(&c, &dir).map_err(err500)
}

async fn case_scenario(AxState(s): AxState<AppState>, AxPath(name): AxPath<String>) -> ApiResult {
    let dir = (*s.case_dir).clone();
    let c = s.store.conn.lock().unwrap();
    casepack::scenario(&c, &name, &dir).map_err(err500)
}

#[derive(serde::Deserialize)]
struct AdjBody {
    branch: Option<String>,
    boundary_event_seq: i64,
    choice: String,
}
async fn adjudicate(AxState(s): AxState<AppState>, Json(b): Json<AdjBody>) -> ApiResult {
    let branch = b.branch.unwrap_or_else(|| "main".into());
    if b.choice != "inside" && b.choice != "outside" {
        return err(StatusCode::BAD_REQUEST, json!({"error": "choice must be inside|outside"}));
    }
    let c = s.store.conn.lock().unwrap();
    pipeline::adjudicate(&c, &branch, b.boundary_event_seq, &b.choice).map_err(err500)
}

#[derive(serde::Deserialize)]
struct RevertBody {
    branch: Option<String>,
    adjudication_seq: i64,
}
async fn adjudicate_revert(AxState(s): AxState<AppState>, Json(b): Json<RevertBody>) -> ApiResult {
    let branch = b.branch.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    pipeline::revert_adjudication(&c, &branch, b.adjudication_seq).map_err(err500)
}

#[derive(serde::Deserialize)]
struct RenameBody {
    branch: Option<String>,
    gate_id: String,
    new_name: String,
}
async fn gate_rename(AxState(s): AxState<AppState>, Json(b): Json<RenameBody>) -> ApiResult {
    let branch = b.branch.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    let g = pipeline::rename_gate(&c, &branch, &b.gate_id, &b.new_name).map_err(err500)?;
    ok(json!({"gate_id": b.gate_id, "new_version": g.version}))
}

#[derive(serde::Deserialize)]
struct ReviseBody {
    branch: Option<String>,
    gate_id: String,
    polygon: Value,
    expected_head: Option<i64>,
}
async fn gate_revise(AxState(s): AxState<AppState>, Json(b): Json<ReviseBody>) -> ApiResult {
    let branch = b.branch.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    let ops = json!({"gate_id": b.gate_id, "polygon": b.polygon});
    let head = b.expected_head.unwrap_or_else(|| store::branch_head(&c, &branch));
    let res = store::with_optimistic(&c, &branch, head, ops, |tx| {
        let g = store::latest_version(tx, &b.gate_id).ok_or_else(|| "gate not found".to_string())?;
        let mut payload = g.payload.clone();
        payload["polygon"] = b.polygon.clone();
        let ng = store::put_object(tx, &branch, &b.gate_id, "gate_polygon", payload, "polygon revised")?;
        pipeline::mark_stale(tx, &branch, &b.gate_id, ng.version, "polygon_revised")?;
        Ok(())
    });
    match res {
        Ok(new_head) => ok(json!({"head": new_head, "status": "saved"})),
        Err(cf) => Err(conflict(cf)),
    }
}

#[derive(serde::Deserialize)]
struct DraftBody {
    name: String,
    from: Option<String>,
}
async fn draft_create(AxState(s): AxState<AppState>, Json(b): Json<DraftBody>) -> ApiResult {
    let from = b.from.unwrap_or_else(|| "main".into());
    let c = s.store.conn.lock().unwrap();
    casepack::fork_branch(&c, &b.name, &from).map_err(err500)
}

#[derive(serde::Deserialize)]
struct DraftSave {
    branch: String,
    expected_head: i64,
    ops: Vec<Value>,
}
async fn draft_save(AxState(s): AxState<AppState>, Json(b): Json<DraftSave>) -> ApiResult {
    let c = s.store.conn.lock().unwrap();
    let ops_desc = json!(b.ops);
    let res = store::with_optimistic(&c, &b.branch, b.expected_head, ops_desc, |tx| {
        for op in &b.ops {
            apply_op(tx, &b.branch, op)?;
        }
        Ok(())
    });
    match res {
        Ok(new_head) => ok(json!({"head": new_head, "status": "saved"})),
        Err(cf) => Err(conflict(cf)),
    }
}

fn apply_op(conn: &Connection, branch: &str, op: &Value) -> Result<(), String> {
    match op["type"].as_str() {
        Some("rename_gate") => {
            pipeline::rename_gate(conn, branch,
                op["gate_id"].as_str().ok_or("gate_id")?,
                op["new_name"].as_str().ok_or("new_name")?
            ).map(|_| ())
        }
        Some("revise_gate") => {
            let gid = op["gate_id"].as_str().ok_or("gate_id")?;
            let g = store::latest_version(conn, branch, gid).ok_or("gate not found")?;
            let mut payload = g.payload.clone();
            payload["polygon"] = op["polygon"].clone();
            let ng = store::put_object(conn, branch, gid, "gate_polygon", payload, "draft revision")?;
            pipeline::mark_stale(conn, branch, gid, ng.version, "polygon_revised")?;
            Ok(())
        }
        Some("adjudicate") => {
            pipeline::adjudicate(conn, branch,
                op["boundary_event_seq"].as_i64().ok_or("boundary_event_seq")?,
                op["choice"].as_str().unwrap_or("inside")).map(|_| ())
        }
        other => Err(format!("unknown op {other:?}")),
    }
}
