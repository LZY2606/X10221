//! HTTP API。所有写操作要求 `base_head_hash` 做乐观并发；冲突返回 409 与双方差异位置。

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::app::commands::*;
use crate::app::Service;

#[derive(Clone)]
pub struct AppState {
    pub svc: Arc<Service>,
}

pub fn router(svc: Arc<Service>) -> Router {
    let st = AppState { svc };
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/api/case/load", post(load_case))
        .route("/api/branches", get(list_branches))
        .route("/api/state/{branch}", get(state))
        .route("/api/events/{branch}", get(events))
        .route("/api/tree/{branch}", get(tree))
        .route("/api/scatter/{branch}/{gate}", get(scatter))
        .route("/api/compdiff/{branch}", get(compdiff))
        .route("/api/overview/{branch}", get(overview))
        .route("/api/diff", get(diff))
        .route("/api/comp/revise", post(comp_revise))
        .route("/api/comp/adjudicate", post(comp_adjudicate))
        .route("/api/transform/revise", post(transform_revise))
        .route("/api/gate/revise", post(gate_revise))
        .route("/api/boundary/adjudicate", post(boundary_adjudicate))
        .route("/api/derive", post(derive_now))
        .route("/api/draft/fork", post(fork))
        .route("/api/undo", post(undo))
        .route("/api/export", get(export))
        .route("/api/import", post(import_bundle))
        .with_state(st)
}

async fn index() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(crate::web::assets::INDEX_HTML))
        .unwrap()
}
async fn app_js() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .body(axum::body::Body::from(crate::web::assets::APP_JS))
        .unwrap()
}

fn svc_err(e: crate::app::service::ServiceError) -> Response {
    let status = match e.code.as_str() {
        "VERSION_CONFLICT" => StatusCode::CONFLICT,
        "NOT_FOUND" => StatusCode::NOT_FOUND,
        "BAD_REQUEST" | "BAD_CANDIDATE" | "BAD_GATE" | "BAD_TRANSFORM" | "CYCLE"
        | "NOT_NEEDS_DECISION" | "NO_COMPENSATION" | "FCS_PARSE" | "FCS_EMPTY"
        | "BAD_BUNDLE" | "BUNDLE_CHAIN" | "BUNDLE_TAMPERED" | "NOT_EMPTY"
        | "IMPORT_CONFLICT" | "DRAFT_EXISTS" | "ALREADY_UNDONE" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": e.code, "message": e.message}))).into_response()
}

async fn load_case(State(st): State<AppState>) -> Response {
    match st.svc.load_builtin_case() {
        Ok(sum) => (StatusCode::CREATED, Json(json!(sum))).into_response(),
        Err(e) => svc_err(e),
    }
}

async fn list_branches(State(st): State<AppState>) -> Response {
    match st.svc.branches() {
        Ok(bs) => Json(json!({"branches": bs.iter().map(|(b, s, h)| json!({"branch": b, "head_seq": s, "head_hash": h})).collect::<Vec<_>>()})).into_response(),
        Err(e) => svc_err(e),
    }
}

async fn state(State(st): State<AppState>, Path(branch): Path<String>, Query(q): Query<AtQuery>) -> Response {
    let branch_view = |b: &str| -> Result<Value, _> {
        let state = if let Some(seq) = q.at { st.svc.state_at(b, seq)? } else { st.svc.state(b)? };
        Ok(json!({
            "branch": state.branch,
            "head_seq": state.head_seq,
            "head_hash": state.head_hash,
            "channels": state.channel_order,
            "event_order": state.event_order,
            "matrix": state.matrices.values().last(),
            "gates": state.gate_order,
            "stale": state.stale,
            "adjudications": state.adjudications,
            "undone": state.undone,
            "forked_from": state.forked_from,
        }))
    };
    match branch_view(&branch) {
        Ok(v) => Json(v).into_response(),
        Err(e) => svc_err(e),
    }
}

#[derive(Deserialize)]
struct AtQuery { #[serde(default)] at: Option<i64> }

async fn events(State(st): State<AppState>, Path(branch): Path<String>) -> Response {
    match st.svc.events(&branch) {
        Ok(evs) => Json(json!({"events": evs})).into_response(),
        Err(e) => svc_err(e),
    }
}

async fn tree(State(st): State<AppState>, Path(branch): Path<String>) -> Response {
    match st.svc.view_tree(&branch) { Ok(v) => Json(v).into_response(), Err(e) => svc_err(e) }
}
async fn scatter(State(st): State<AppState>, Path((branch, gate)): Path<(String, String)>) -> Response {
    match st.svc.view_scatter(&branch, &gate) { Ok(v) => Json(v).into_response(), Err(e) => svc_err(e) }
}
async fn compdiff(State(st): State<AppState>, Path(branch): Path<String>) -> Response {
    match st.svc.view_comp_diff(&branch) { Ok(v) => Json(v).into_response(), Err(e) => svc_err(e) }
}
async fn overview(State(st): State<AppState>, Path(branch): Path<String>) -> Response {
    match st.svc.view_overview(&branch) { Ok(v) => Json(v).into_response(), Err(e) => svc_err(e) }
}

#[derive(Deserialize)]
struct DiffQuery { a: String, b: String }
async fn diff(State(st): State<AppState>, Query(q): Query<DiffQuery>) -> Response {
    match st.svc.view_diff(&q.a, &q.b) { Ok(v) => Json(v).into_response(), Err(e) => svc_err(e) }
}

#[derive(Deserialize)]
struct CompRevise {
    branch: String,
    #[serde(default)] base_head_hash: Option<String>,
    matrix_id: String,
    variant: String, // near-singular | singular
    #[serde(default)] note: Option<String>,
    #[serde(default)] actor: Option<String>,
}
async fn comp_revise(State(st): State<AppState>, Json(cmd): Json<CompRevise>) -> Response {
    let m = match cmd.variant.as_str() {
        "near-singular" => crate::casepack::near_singular_spillover(),
        "singular" => crate::casepack::singular_spillover(),
        other => return svc_err(crate::app::service::ServiceError { code: "BAD_REQUEST".into(), message: format!("未知 variant {other}") }),
    };
    match st.svc.revise_compensation(&cmd.branch, cmd.base_head_hash.as_deref(), &cmd.matrix_id, m,
        cmd.note.as_deref().unwrap_or("参数修订：引入边界矩阵"), cmd.actor.as_deref().unwrap_or("reviewer")) {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => svc_err(e),
    }
}

async fn comp_adjudicate(State(st): State<AppState>, Json(cmd): Json<AdjudicateComp>) -> Response {
    match st.svc.adjudicate_compensation(&cmd) {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => svc_err(e),
    }
}
async fn transform_revise(State(st): State<AppState>, Json(cmd): Json<ReviseTransform>) -> Response {
    match st.svc.revise_transform(&cmd) { Ok(v) => (StatusCode::CREATED, Json(v)).into_response(), Err(e) => svc_err(e) }
}
async fn gate_revise(State(st): State<AppState>, Json(cmd): Json<ReviseGate>) -> Response {
    match st.svc.revise_gate(&cmd) { Ok(v) => (StatusCode::CREATED, Json(v)).into_response(), Err(e) => svc_err(e) }
}
async fn boundary_adjudicate(State(st): State<AppState>, Json(cmd): Json<BoundaryCall>) -> Response {
    match st.svc.adjudicate_boundary(&cmd) { Ok(v) => (StatusCode::CREATED, Json(v)).into_response(), Err(e) => svc_err(e) }
}

#[derive(Deserialize)]
struct DeriveCall { branch: String, #[serde(default)] base_head_hash: Option<String>, #[serde(default)] actor: Option<String> }
async fn derive_now(State(st): State<AppState>, Json(cmd): Json<DeriveCall>) -> Response {
    match st.svc.derive_now(&cmd.branch, cmd.base_head_hash.as_deref(), cmd.actor.as_deref().unwrap_or("reviewer")) {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => svc_err(e),
    }
}
async fn fork(State(st): State<AppState>, Json(cmd): Json<ForkDraft>) -> Response {
    match st.svc.fork_draft(&cmd) { Ok(v) => (StatusCode::CREATED, Json(v)).into_response(), Err(e) => svc_err(e) }
}
async fn undo(State(st): State<AppState>, Json(cmd): Json<UndoCall>) -> Response {
    match st.svc.undo_decision(&cmd) { Ok(v) => (StatusCode::CREATED, Json(v)).into_response(), Err(e) => svc_err(e) }
}

async fn export(State(st): State<AppState>) -> Response {
    match st.svc.export_bundle() {
        Ok(v) => Json(v).into_response(),
        Err(e) => svc_err(e),
    }
}
async fn import_bundle(State(st): State<AppState>, Json(bundle): Json<Value>) -> Response {
    match st.svc.import_bundle(&bundle) {
        Ok(n) => (StatusCode::CREATED, Json(json!({"imported_events": n}))).into_response(),
        Err(e) => svc_err(e),
    }
}
