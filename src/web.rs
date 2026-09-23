use crate::store::{AppState, OpError};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct BranchQuery {
    pub branch_id: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DraftRequest {
    pub branch_id: Option<String>,
    pub draft_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UndoRequest {
    pub branch_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DecisionRequest {
    pub branch_id: Option<String>,
    pub expected_head_event_id: String,
    pub candidate_id: String,
    pub action: String,
    pub rationale: String,
    pub draft_label: Option<String>,
}

fn branch_id(value: Option<String>) -> String {
    value.unwrap_or_else(|| "baseline".to_string())
}

fn error_response(error: OpError) -> Response {
    match error {
        OpError::Conflict(conflict) => (StatusCode::CONFLICT, Json(json!({"error": conflict}))).into_response(),
        OpError::Invalid(message) => (StatusCode::BAD_REQUEST, Json(json!({"error": {"code": "INVALID_REQUEST", "message": message}}))).into_response(),
        OpError::Sql(error) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"code": "DATABASE_ERROR", "message": error.to_string()}}))).into_response(),
    }
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "case": crate::case::CASE_ID, "title": crate::case::CASE_TITLE}))
}

async fn state(State(state): State<AppState>, Query(query): Query<BranchQuery>) -> Result<Json<Value>, AppError> {
    if let Some(event_id) = query.event_id {
        return Ok(Json(state.event_snapshot(&event_id)?));
    }
    Ok(Json(state.snapshot(&branch_id(query.branch_id))?))
}

async fn branches(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({"branches": state.branches()?})))
}

async fn load_case(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.load_or_import_case()?))
}

async fn matrix_singular(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, AppError> {
    let branch = body.get("branch_id").and_then(Value::as_str).map(str::to_string);
    Ok(Json(state.matrix_singular(&branch_id(branch))?))
}

async fn source_local(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, AppError> {
    let branch = body.get("branch_id").and_then(Value::as_str).map(str::to_string);
    Ok(Json(state.source_local_revision(&branch_id(branch))?))
}

async fn gate_geometry(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, AppError> {
    let branch = body.get("branch_id").and_then(Value::as_str).map(str::to_string);
    Ok(Json(state.gate_geometry_revision(&branch_id(branch))?))
}

async fn gate_rename(State(state): State<AppState>, Json(body): Json<Value>) -> Result<Json<Value>, AppError> {
    let branch = body.get("branch_id").and_then(Value::as_str).map(str::to_string);
    Ok(Json(state.gate_nested_rename(&branch_id(branch))?))
}

async fn create_draft(State(state): State<AppState>, Json(request): Json<DraftRequest>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.create_draft(&branch_id(request.branch_id), &request.draft_id)?))
}

async fn decide(State(state): State<AppState>, Json(request): Json<DecisionRequest>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.decide(
        &branch_id(request.branch_id),
        &request.expected_head_event_id,
        &request.candidate_id,
        &request.action,
        &request.rationale,
        &request.draft_label.unwrap_or_else(|| "review window".to_string()),
    )?))
}

async fn undo(State(state): State<AppState>, Json(request): Json<UndoRequest>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.undo_last_decision(&branch_id(request.branch_id))?))
}

async fn compare(State(state): State<AppState>, Query(query): Query<HashMap<String, String>>) -> Result<Json<Value>, AppError> {
    let left = query.get("left").map(String::as_str).unwrap_or("baseline").to_string();
    let right = query.get("right").map(String::as_str).unwrap_or("baseline").to_string();
    Ok(Json(json!({
        "left": state.snapshot(&left)?,
        "right": state.snapshot(&right)?,
        "stable_identity_policy": "display values carry candidate_id/stable_id; refs pin source and parent versions"
    })))
}

async fn export_archive(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.export_archive()?))
}

async fn import_archive(State(state): State<AppState>, Json(archive): Json<Value>) -> Result<Json<Value>, AppError> {
    Ok(Json(state.import_archive(&archive)?))
}

pub struct AppError(pub OpError);

impl From<OpError> for AppError {
    fn from(value: OpError) -> Self {
        AppError(value)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        error_response(self.0)
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/state", get(state))
        .route("/api/event", get(state))
        .route("/api/branches", get(branches))
        .route("/api/case/load", post(load_case))
        .route("/api/ops/matrix-singular", post(matrix_singular))
        .route("/api/ops/source-local", post(source_local))
        .route("/api/ops/gate-geometry", post(gate_geometry))
        .route("/api/ops/gate-rename", post(gate_rename))
        .route("/api/drafts", post(create_draft))
        .route("/api/decisions", post(decide))
        .route("/api/undo", post(undo))
        .route("/api/compare", get(compare))
        .route("/api/archive", get(export_archive).post(import_archive))
        .with_state(state)
}
