use crate::actions::{self, Adjudication, NewArtifact};
use crate::engine::fold;
use crate::model::{ArtifactKind, EventKind};
use crate::store::Store;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const INDEX_HTML: &str = include_str!("../ui/index.html");

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub db_path: String,
}

pub fn app(store: Store, db_path: String) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(state_view))
        .route("/api/events", get(events_list))
        .route("/api/case/load", post(case_load))
        .route("/api/case/replay", post(case_replay))
        .route("/api/derive/compensation", post(derive_comp))
        .route("/api/derive/populations", post(derive_pops))
        .route("/api/adjudicate", post(adjudicate))
        .route("/api/adjudicate/undo", post(undo))
        .route("/api/commit", post(commit))
        .route("/api/branches", post(create_branch))
        .route("/api/export", get(export_archive))
        .route("/api/import", post(import_archive))
        .route("/api/reset", post(reset))
        .with_state(AppState {
            store: Arc::new(Mutex::new(store)),
            db_path,
        })
}

async fn index() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], INDEX_HTML)
}

#[derive(Debug, Deserialize)]
struct StateQuery {
    #[serde(default)]
    seq: Option<u64>,
    #[serde(default)]
    branch: Option<String>,
}

async fn state_view(
    State(app): State<AppState>,
    Query(q): Query<StateQuery>,
) -> Response {
    let events = app.store.lock().unwrap().load_events();
    match events {
        Ok(all) => {
            let sliced: Vec<crate::model::Event> = match q.seq {
                Some(n) => all.into_iter().take(n as usize).collect(),
                None => all,
            };
            let st = fold(&sliced);
            let branch = q.branch.unwrap_or_else(|| "main".to_string());
            Json(json!({"state": crate::engine::state_json(&st), "branch": branch,
                        "event_count": sliced.len(),
                        "head_seq": sliced.last().map(|e| e.seq)})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.0).into_response(),
    }
}

async fn events_list(State(app): State<AppState>) -> Response {
    match app.store.lock().unwrap().load_events() {
        Ok(events) => Json(json!({"events": events})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.0).into_response(),
    }
}

async fn case_load(State(app): State<AppState>) -> Response {
    let mut store = app.store.lock().unwrap();
    match crate::casepack::load_case(&mut store, "reviewer") {
        Ok(ids) => Json(json!({"ok": true, "imported_events": ids})).into_response(),
        Err(e) => (StatusCode::CONFLICT, json!({"ok": false, "error": e})).into_response(),
    }
}

async fn case_replay(State(app): State<AppState>) -> Response {
    let mut store = app.store.lock().unwrap();
    match crate::casepack::replay(&mut store, "reviewer") {
        Ok(log) => Json(json!({"ok": true, "steps": log})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e})).into_response(),
    }
}

async fn derive_comp(State(app): State<AppState>) -> Response {
    let mut store = app.store.lock().unwrap();
    match actions::derive_compensation(&mut store, "main", "reviewer") {
        Ok(ev) => Json(json!({"ok": true, "event": ev})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response(),
    }
}

async fn derive_pops(State(app): State<AppState>) -> Response {
    let mut store = app.store.lock().unwrap();
    match actions::derive_populations(&mut store, "main", "reviewer") {
        Ok(evs) => Json(json!({"ok": true, "events": evs})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AdjReq {
    diagnostic_id: String,
    action: String,
    #[serde(default)]
    candidate_id: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

async fn adjudicate(State(app): State<AppState>, Json(req): Json<AdjReq>) -> Response {
    let mut store = app.store.lock().unwrap();
    let branch = req.branch.clone().unwrap_or_else(|| "main".into());
    match actions::adjudicate(
        &mut store,
        &branch,
        "reviewer",
        Adjudication {
            diagnostic_id: req.diagnostic_id,
            action: req.action,
            candidate_id: req.candidate_id,
            params: req.params,
            rationale: req.rationale.unwrap_or_default(),
        },
    ) {
        Ok(evs) => Json(json!({"ok": true, "events": evs})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct UndoReq {
    adjudication_event_id: String,
    #[serde(default)]
    branch: Option<String>,
}

async fn undo(State(app): State<AppState>, Json(req): Json<UndoReq>) -> Response {
    let mut store = app.store.lock().unwrap();
    let branch = req.branch.unwrap_or_else(|| "main".into());
    match actions::undo_adjudication(&mut store, &branch, "reviewer", &req.adjudication_event_id) {
        Ok(ev) => Json(json!({"ok": true, "event": ev})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CommitReq {
    id: String,
    kind: ArtifactKind,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    base_version: Option<u64>,
    payload: Value,
    #[serde(default)]
    record_conflict: Option<bool>,
    #[serde(default)]
    actor: Option<String>,
}

async fn commit(State(app): State<AppState>, Json(req): Json<CommitReq>) -> Response {
    let mut store = app.store.lock().unwrap();
    let branch = req.branch.clone().unwrap_or_else(|| "main".into());
    let actor = req.actor.clone().unwrap_or_else(|| "reviewer".into());
    let new_art = NewArtifact {
        id: req.id.clone(),
        kind: req.kind,
        branch: branch.clone(),
        payload: req.payload.clone(),
        raw_b64: None,
        base_version: req.base_version,
    };
    match actions::commit_artifact(&mut store, new_art, &actor) {
        Ok(ev) => Json(json!({"ok": true, "event": ev})).into_response(),
        Err(cf) => {
            if req.record_conflict.unwrap_or(true) {
                let _ = actions::record_conflict(&mut store, &branch, &actor, &cf);
            }
            (
                StatusCode::CONFLICT,
                Json(json!({"ok": false, "conflict": cf})),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct BranchReq {
    branch: String,
    #[serde(default)]
    parent: Option<String>,
}

async fn create_branch(State(app): State<AppState>, Json(req): Json<BranchReq>) -> Response {
    let mut store = app.store.lock().unwrap();
    let parent = req.parent.unwrap_or_else(|| "main".into());
    match actions::create_branch(&mut store, &req.branch, &parent, "reviewer") {
        Ok(ev) => Json(json!({"ok": true, "event": ev})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response(),
    }
}

async fn export_archive(State(app): State<AppState>) -> Response {
    match app.store.lock().unwrap().export_json() {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.0).into_response(),
    }
}

async fn import_archive(State(app): State<AppState>, Json(body): Json<Value>) -> Response {
    let mut store = app.store.lock().unwrap();
    match store.import_json(&body) {
        Ok(n) => Json(json!({"ok": true, "events_imported": n})).into_response(),
        Err(e) => {
            // 导入失败已整体回滚，不留半份记录
            (StatusCode::BAD_REQUEST, json!({"ok": false, "error": e.0})).into_response()
        }
    }
}

async fn reset(State(app): State<AppState>) -> Response {
    match app.store.lock().unwrap().wipe() {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.0).into_response(),
    }
}

/// 端到端测试可直接拿到监听套接字的服务。
pub async fn serve(addr: &str, db_path: &str) -> std::io::Result<()> {
    let store = Store::open(db_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.0))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(store, db_path.to_string())).await
}

// 兼容上面少量未使用 import（EventKind 等在扩展时使用）
#[allow(dead_code)]
fn _unused(_k: EventKind) {}
