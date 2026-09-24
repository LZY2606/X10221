//! Axum 服务：复核界面 + JSON API。

use crate::store::{ConflictInfo, Store, StoreError};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub type SharedStore = Arc<Mutex<Store>>;

#[derive(Clone)]
pub struct AppState {
    pub store: SharedStore,
}

pub fn router(store: Store) -> Router {
    let state = AppState { store: Arc::new(Mutex::new(store)) };
    Router::new()
        .route("/", get(index))
        .route("/api/casepack", get(casepack_meta))
        .route("/api/casepack/load", post(casepack_load))
        .route("/api/state", get(state_handler))
        .route("/api/drafts/{id}/view", get(draft_view))
        .route("/api/compare", get(compare))
        .route("/api/derive", post(derive))
        .route("/api/adjudications", post(adjudicate))
        .route("/api/adjudications/{id}/undo", post(undo))
        .route("/api/events", get(events))
        .route("/api/events/{seq}", get(event_detail))
        .route("/api/artifacts/{id}/versions/{seq}/raw", get(raw_version))
        .route("/api/export", get(export))
        .route("/api/import", post(import))
        .with_state(state)
}

fn err(e: StoreError) -> (StatusCode, Json<Value>) {
    match e {
        StoreError::Conflict(c) => (StatusCode::CONFLICT, Json(conflict_json(c))),
        other => (StatusCode::BAD_REQUEST, Json(json!({"error": other.to_string()}))),
    }
}

fn conflict_json(c: ConflictInfo) -> Value {
    json!({
        "error": "version-conflict",
        "current_head": c.current_head,
        "your_base": c.your_base,
        "their_diff": c.their_diff,
        "your_diff": c.your_diff,
    })
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn casepack_meta() -> Json<Value> {
    Json(crate::casepack::casepack_meta())
}

async fn casepack_load(State(s): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut store = s.store.lock().unwrap();
    crate::casepack::replay(&mut store).map(Json).map_err(err)
}

async fn state_handler(State(s): State<AppState>) -> Json<Value> {
    let store = s.store.lock().unwrap();
    Json(store.state_summary())
}

#[derive(serde::Deserialize)]
struct ViewQuery {
    at: Option<i64>,
}

async fn draft_view(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ViewQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = s.store.lock().unwrap();
    crate::view::build_view(&store, &id, q.at).map(Json).map_err(err)
}

#[derive(serde::Deserialize)]
struct CompareQuery {
    a: String,
    b: String,
}

async fn compare(
    State(s): State<AppState>,
    Query(q): Query<CompareQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = s.store.lock().unwrap();
    let a = crate::view::build_view(&store, &q.a, None).map_err(err)?;
    let b = crate::view::build_view(&store, &q.b, None).map_err(err)?;
    Ok(Json(json!({"a": a, "b": b})))
}

#[derive(serde::Deserialize)]
struct DeriveReq {
    draft_id: String,
}

async fn derive(
    State(s): State<AppState>,
    Json(req): Json<DeriveReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut store = s.store.lock().unwrap();
    store.derive_draft(&req.draft_id).map(Json).map_err(err)
}

#[derive(serde::Deserialize)]
struct AdjReq {
    draft_id: String,
    candidate_id: String,
    action: String,
    base_head: i64,
    #[serde(default)]
    note: String,
}

async fn adjudicate(
    State(s): State<AppState>,
    Json(req): Json<AdjReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut store = s.store.lock().unwrap();
    store
        .adjudicate(&req.draft_id, &req.candidate_id, &req.action, req.base_head, &req.note)
        .map(Json)
        .map_err(err)
}

async fn undo(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut store = s.store.lock().unwrap();
    store.undo_adjudication(&id).map(Json).map_err(err)
}

async fn events(State(s): State<AppState>) -> Json<Value> {
    let store = s.store.lock().unwrap();
    Json(json!(store.events(None)))
}

async fn event_detail(
    State(s): State<AppState>,
    Path(seq): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = s.store.lock().unwrap();
    let all = store.events(None);
    all.into_iter()
        .find(|e| e["seq"].as_i64() == Some(seq))
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "event not found"}))))
}

async fn raw_version(
    State(s): State<AppState>,
    Path((id, seq)): Path<(String, i64)>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let store = s.store.lock().unwrap();
    store
        .raw_repr(&id, seq)
        .ok_or_else(|| (StatusCode::NOT_FOUND, Json(json!({"error": "version not found"}))))
}

async fn export(State(s): State<AppState>) -> Json<Value> {
    let store = s.store.lock().unwrap();
    Json(store.export_archive())
}

async fn import(State(s): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut store = s.store.lock().unwrap();
    store.import_archive(&body).map(Json).map_err(err)
}
