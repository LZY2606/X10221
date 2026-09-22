//! HTTP 层：Axum JSON API + 内嵌单页复核界面。

use crate::db::Store;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
}

pub fn router(store: Arc<Store>) -> Router {
    let state = AppState { store };
    Router::new()
        .route("/", get(index))
        .route("/api/case/manifest", get(case_manifest))
        .route("/api/case/load", post(load_case))
        .route("/api/case/validate-rollback", post(validate_rollback))
        .route("/api/branches", get(list_branches).post(create_branch))
        .route("/api/branches/{branch}/view", get(branch_view))
        .route("/api/branches/{branch}/fork", post(fork_branch))
        .route("/api/branches/{branch}/revert", post(revert))
        .route("/api/branches/{branch}/actions/near-singular", post(act_singular))
        .route("/api/branches/{branch}/actions/decide", post(act_decide))
        .route("/api/branches/{branch}/actions/transform-revision", post(act_transform))
        .route("/api/branches/{branch}/actions/source-patch", post(act_patch))
        .route("/api/branches/{branch}/actions/gate-rename", post(act_rename))
        .route("/api/branches/{branch}/actions/upstream-revision", post(act_upstream))
        .route("/api/conflicts", get(conflicts))
        .route("/api/export", get(export))
        .route("/api/import", post(import_archive))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

async fn case_manifest() -> Json<Value> {
    let manifest: Value = serde_json::from_str(crate::service::MANIFEST).unwrap();
    let case: Value = serde_json::from_str(crate::service::CASE_JSON).unwrap();
    Json(json!({"manifest": manifest, "case_id": case["case_id"], "title": case["title"],
                "scenarios": case["scenarios"],
                "fcs_text_sha256": sha(crate::service::FCS_TEXT)}))
}

fn sha(t: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(t.as_bytes()))
}

type AppResult<T> = Result<T, (StatusCode, Json<Value>)>;

fn err<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (StatusCode::CONFLICT, Json(json!({"error": e.to_string()})))
}

fn notfound<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()})))
}

#[derive(Deserialize)]
struct ViewQuery { at_event: Option<String> }

async fn load_case(State(s): State<AppState>) -> AppResult<Json<Value>> {
    s.store.load_bundled_case().map(Json).map_err(err)
}

async fn validate_rollback(State(s): State<AppState>, Json(body): Json<Value>) -> AppResult<Json<Value>> {
    let before = s.store.list_branches().len();
    let bad = body.get("case").cloned().unwrap_or(body);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::ingest::validate_case(&bad)
    }));
    let ok = matches!(result, Ok(Ok(())));
    if ok {
        return Err((StatusCode::BAD_REQUEST, Json(json!({"error": "package unexpectedly valid; rollback test expects a malformed package"}))));
    }
    let after = s.store.list_branches().len();
    Ok(Json(json!({"rejected": true, "rolled_back": true,
                   "branches_before": before, "branches_after": after,
                   "reason": result.ok().and_then(|r| r.err())})))
}

async fn list_branches(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.store.list_branches()))
}

#[derive(Deserialize)]
struct ForkReq { branch_id: String, label: String, parent: String, at_event: Option<String> }
async fn create_branch(State(st): State<AppState>, Json(req): Json<ForkReq>) -> AppResult<Json<Value>> {
    st.store.fork_at(&req.branch_id, &req.label, &req.parent, req.at_event.as_deref())
        .map(|_| Json(json!({"status": "created", "branch_id": req.branch_id}))).map_err(err)
}

async fn branch_view(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                     axum::extract::Query(q): axum::extract::Query<ViewQuery>) -> AppResult<Json<Value>> {
    if !s.store.list_branches().iter().any(|x| x.branch_id == b) {
        return Err(notfound("unknown branch"));
    }
    Ok(Json(json!(s.store.branch_view(&b, q.at_event.as_deref()))))
}

#[derive(Deserialize)]
struct RevertReq { target_event: String, expected_parent: Option<String> }
async fn revert(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                Json(req): Json<RevertReq>) -> AppResult<Json<Value>> {
    s.store.revert_to(&b, &req.target_event, req.expected_parent.as_deref())
        .map(|id| Json(json!({"event_id": id}))).map_err(err)
}

async fn fork_branch(State(_s): State<AppState>) -> impl IntoResponse {
    (StatusCode::MOVED_PERMANENTLY, Json(json!({"use": "POST /api/branches"})))
}

#[derive(Deserialize)]
struct Expect { expected_parent: Option<String> }

async fn act_singular(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                      Json(req): Json<Expect>) -> AppResult<Json<Value>> {
    s.store.scenario_near_singular(&b, req.expected_parent.as_deref())
        .map(Json).map_err(err)
}

#[derive(Deserialize)]
struct DecideReq { candidate_id: String, adopt: bool, expected_parent: Option<String> }
async fn act_decide(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                    Json(req): Json<DecideReq>) -> AppResult<Json<Value>> {
    s.store.decide_candidate(&b, &req.candidate_id, req.adopt, req.expected_parent.as_deref())
        .map(Json).map_err(err)
}

async fn act_transform(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                       Json(req): Json<Expect>) -> AppResult<Json<Value>> {
    s.store.scenario_transform_revision(&b, req.expected_parent.as_deref()).map(Json).map_err(err)
}
async fn act_patch(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                   Json(req): Json<Expect>) -> AppResult<Json<Value>> {
    s.store.scenario_source_patch(&b, req.expected_parent.as_deref()).map(Json).map_err(err)
}
async fn act_rename(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                    Json(req): Json<Expect>) -> AppResult<Json<Value>> {
    s.store.scenario_gate_rename(&b, req.expected_parent.as_deref()).map(Json).map_err(err)
}
async fn act_upstream(State(s): State<AppState>, axum::extract::Path(b): axum::extract::Path<String>,
                      Json(req): Json<Expect>) -> AppResult<Json<Value>> {
    s.store.scenario_upstream_revision(&b, req.expected_parent.as_deref()).map(Json).map_err(err)
}

async fn conflicts(State(s): State<AppState>) -> Json<Value> {
    Json(json!(s.store.conflicts()))
}

async fn export(State(s): State<AppState>) -> Json<Value> {
    Json(s.store.export_all())
}

#[derive(Deserialize)]
struct ImportReq { #[serde(default)] wipe: bool, archive: Value }
async fn import_archive(State(s): State<AppState>, Json(req): Json<ImportReq>) -> AppResult<Json<Value>> {
    s.store.import_all(&req.archive, req.wipe).map(|n| Json(json!({"imported_rows": n}))).map_err(err)
}
