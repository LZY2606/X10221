//! Axum HTTP 层：本地复核页面 + JSON API。

use crate::case::CaseFile;
use crate::compute;
use crate::engine::Store;
use crate::transfer;
use crate::workflow::{self, ALT, MAIN};
use crate::Result;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

static CASE_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/cases/flow-boundary-chamber");
static STATIC_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/static");

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub case_json: Arc<String>,
    pub db_path: Arc<String>,
}

pub fn router(store: Store, db_path: String, case_json: String) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        case_json: Arc::new(case_json),
        db_path: Arc::new(db_path),
    };
    Router::new()
        .route("/", get(index))
        .route("/static/app.js", get(app_js))
        .route("/api/case", get(get_case))
        .route("/api/load", post(load_case))
        .route("/api/replay", post(replay))
        .route("/api/view", get(view))
        .route("/api/scatter", get(scatter))
        .route("/api/decide", post(decide))
        .route("/api/undo", post(undo))
        .route("/api/fork", post(fork))
        .route("/api/gate-revise", post(gate_revise))
        .route("/api/recompute", post(recompute))
        .route("/api/export", get(export))
        .route("/api/import", post(import_archive))
        .with_state(state)
}

async fn index() -> Response {
    serve_file("index.html", "text/html; charset=utf-8")
}
async fn app_js() -> Response {
    serve_file("app.js", "application/javascript; charset=utf-8")
}

fn serve_file(name: &str, mime: &str) -> Response {
    match STATIC_DIR.get_file(name) {
        Some(f) => ([(header::CONTENT_TYPE, mime)], f.contents()).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn get_case(State(s): State<AppState>) -> Json<Value> {
    Json(serde_json::from_str(&s.case_json).unwrap_or(json!({})))
}

fn api_err(e: String) -> Response {
    let status = if e.starts_with("CONFLICT:") { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST };
    (status, Json(json!({"error": e}))).into_response()
}

async fn load_case(State(s): State<AppState>) -> Response {
    let run = || -> Result<Value> {
        let case = CaseFile::from_str(&s.case_json)?;
        let blobs: Vec<(&str, &[u8])> = CASE_DIR
            .files()
            .map(|f| (f.path().to_str().unwrap_or("file"), f.contents()))
            .collect();
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        workflow::load_case(&mut store, &case, &blobs)?;
        Ok(json!({"loaded": case.id, "main_rev": store.branch_revision(MAIN)?}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

#[derive(Deserialize)]
struct DecideReq { branch: Option<String>, eval_id: String, strategy: String, expected_rev: Option<i64> }

async fn decide(State(s): State<AppState>, Json(req): Json<DecideReq>) -> Response {
    let run = || -> Result<Value> {
        let branch = req.branch.unwrap_or_else(|| MAIN.to_string());
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let (eid, rev) = workflow::decide(&mut store, &branch, &req.eval_id, &req.strategy, req.expected_rev)?;
        workflow::recompute(&mut store, &branch, Some(rev))?;
        Ok(json!({"event_id": eid, "revision": rev + 1}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

#[derive(Deserialize)]
struct UndoReq { branch: Option<String>, eval_id: String, expected_rev: Option<i64> }

async fn undo(State(s): State<AppState>, Json(req): Json<UndoReq>) -> Response {
    let run = || -> Result<Value> {
        let branch = req.branch.unwrap_or_else(|| MAIN.to_string());
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let (eid, rev) = workflow::undo_decision(&mut store, &branch, &req.eval_id, req.expected_rev)?;
        Ok(json!({"event_id": eid, "revision": rev}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

#[derive(Deserialize)]
struct ForkReq { name: String, fork_event_id: Option<String> }

async fn fork(State(s): State<AppState>, Json(req): Json<ForkReq>) -> Response {
    let run = || -> Result<Value> {
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let case = CaseFile::from_str(&s.case_json)?;
        let id = store.create_branch(&case.id, &req.name, req.fork_event_id.as_deref())?;
        Ok(json!({"branch_id": id}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

#[derive(Deserialize)]
struct GateReq {
    branch: Option<String>, gate_id: String, polygon: Vec<[f64; 2]>,
    expected_rev: Option<i64>,
}

async fn gate_revise(State(s): State<AppState>, Json(req): Json<GateReq>) -> Response {
    let run = || -> Result<Value> {
        let branch = req.branch.unwrap_or_else(|| MAIN.to_string());
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let state = store.state_at(&branch, None)?;
        let cur = state.artifacts.get("gates:active").cloned().ok_or("缺少门版本")?;
        let mut gates = cur.content.as_array().cloned().unwrap_or_default();
        let mut hit = false;
        for g in gates.iter_mut() {
            if g["id"] == req.gate_id {
                g["polygon"] = json!(req.polygon);
                hit = true;
            }
        }
        if !hit {
            return Err("门不存在".into());
        }
        crate::engine::publish_artifact(&mut store, &branch, "gates", "active", cur.version + 1, Value::Array(gates), req.expected_rev)?;
        let state = store.state_at(&branch, None)?;
        let cols: std::collections::HashSet<String> = state
            .artifacts
            .get("gates:active")
            .and_then(|a| a.content.as_array())
            .and_then(|a| a.iter().find(|x| x["id"] == req.gate_id))
            .map(|g| g["axes"].as_array().unwrap().iter().map(|ax| ax["channel"].as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();
        let expired = compute::invalidation_scope(&state, &cols);
        let only = expired.iter().take(1).cloned().collect::<Vec<_>>();
        workflow::mark_stale_public(&mut store, &branch, &expired, "interactive gate revision")?;
        Ok(json!({"expired": expired, "first": only}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

#[derive(Deserialize)]
struct RecomputeReq { branch: Option<String>, expected_rev: Option<i64> }

async fn recompute(State(s): State<AppState>, Json(req): Json<RecomputeReq>) -> Response {
    let run = || -> Result<Value> {
        let branch = req.branch.unwrap_or_else(|| MAIN.to_string());
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let (eid, rev, payload) = workflow::recompute(&mut store, &branch, req.expected_rev)?;
        Ok(json!({"event_id": eid, "revision": rev, "populations": payload["populations"]}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

async fn export(State(s): State<AppState>) -> Response {
    let store = s.store.lock().unwrap();
    match transfer::export_all(&store.conn) {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

async fn import_archive(State(s): State<AppState>, Json(dump): Json<Value>) -> Response {
    let run = || -> Result<Value> {
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        // 全新数据库：物理删除后重建，再导入，验证身份/顺序/来源关系保持。
        let path = s.db_path.as_str().to_string();
        drop(store);
        let fresh = Store::open(&path)?;
        let mut guard = s.store.lock().map_err(|e| e.to_string())?;
        *guard = fresh;
        transfer::import_all(&mut guard, &dump)?;
        Ok(json!({"imported": true, "main_rev": guard.branch_revision(MAIN).unwrap_or(0)}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

pub async fn serve_addr(addr: &str, db_path: String) -> Result<()> {
    let case_json = CASE_DIR
        .get_file("case.json")
        .map(|f| String::from_utf8_lossy(f.contents()).to_string())
        .ok_or("内置案例包缺失 case.json")?;
    let store = Store::open(&db_path)?;
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| e.to_string())?;
    axum::serve(listener, router(store, db_path, case_json)).await.map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct ViewQ { branch: Option<String>, at: Option<String>, strategy: Option<String> }

async fn view(State(s): State<AppState>, Query(q): Query<ViewQ>) -> Response {
    let run = || -> Result<Value> {
        let branch = q.branch.unwrap_or_else(|| MAIN.to_string());
        let store = s.store.lock().map_err(|e| e.to_string())?;
        let state = store.state_at(&branch, q.at.as_deref())?;
        let events = store.branch_events(&branch)?;
        let coords = build_view(&state, q.strategy.as_deref(), &events, &store, &branch)?;
        Ok(coords)
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

fn build_view(state: &crate::engine::State, strategy: Option<&str>, events: &[Value], store: &Store, branch: &str) -> Result<Value> {
    let pops = compute::recompute_populations(state, "view", strategy);
    let matrix = state.artifacts.get("comp_matrix:active");
    let analysis = matrix.and_then(|m| {
        state.evals.values().find(|e| e["matrix_sha"] == m.sha256).map(|e| e["analysis"].clone())
    });
    let decision = state.decisions.values().next().cloned();
    let mut gate_tree = Vec::new();
    if let Some(links) = state.artifacts.get("links:active") {
        for l in links.content.as_array().cloned().unwrap_or_default() {
            let gid = l["gate"].as_str().unwrap_or("");
            let name = state
                .artifacts
                .get("gates:active")
                .and_then(|g| g.content.as_array())
                .and_then(|a| a.iter().find(|x| x["id"] == gid))
                .and_then(|g| g["name"].as_str())
                .unwrap_or(gid)
                .to_string();
            let pop = pops["populations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["gate_logical"] == gid)
                .cloned()
                .unwrap_or(json!({}));
            gate_tree.push(json!({
                "identity": format!("pop-{gid}"),
                "gate_id": gid, "name": name,
                "parent": l["parent"], "relation": l["relation"],
                "count": pop["count"], "percent": pop["percent"],
                "denominator_result_id": pop["denominator_result_id"],
                "denominator_event_id": pop["denominator_event_id"],
                "denominator_count": pop["denominator_count"],
                "status": state.pops.get(&format!("pop-{gid}")).and_then(|p| p["status"].as_str()).unwrap_or("current"),
            }));
        }
    }
    let rev = store.branch_revision(branch).unwrap_or(0);
    Ok(json!({
        "branch": branch, "revision": rev, "at": events.last().and_then(|e| e["event_id"].as_str()),
        "events": events.iter().map(|e| json!({
            "event_id": e["event_id"], "kind": e["kind"], "seq": e["seq"],
            "parent_event_id": e["parent_event_id"],
            "source_fragment_id": e["source_fragment_id"],
        })).collect::<Vec<_>>(),
        "artifact_versions": state.artifacts.values().map(|a| json!({
            "identity": a.artifact_id, "version": a.version, "kind": a.kind,
            "logical_id": a.logical_id, "sha256": a.sha256, "introduced_event_id": a.introduced_event_id,
        })).collect::<Vec<_>>(),
        "compensation": {
            "matrix": matrix.map(|m| json!({"id": m.logical_id, "version": m.version, "rows": m.content["rows"], "sha256": m.sha256})),
            "analysis": analysis,
            "decided_strategy": decision,
            "view_strategy": strategy,
        },
        "gate_tree": gate_tree,
        "overflows": compute::overflow_diagnostics(state),
        "stable_identity_note": "所有显示值携带 evt-/art-/pop-/cand-/br- 稳定身份",
    }))
}

#[derive(Deserialize)]
struct ScatterQ {
    branch: Option<String>, at: Option<String>, x: String, y: String, strategy: Option<String>,
}

async fn scatter(State(s): State<AppState>, Query(q): Query<ScatterQ>) -> Response {
    let run = || -> Result<Value> {
        let branch = q.branch.unwrap_or_else(|| MAIN.to_string());
        let store = s.store.lock().map_err(|e| e.to_string())?;
        let state = store.state_at(&branch, q.at.as_deref())?;
        let mut points = Vec::new();
        for id in &state.order {
            let coord = compute::coordinates(&state, id, q.strategy.as_deref());
            if let Some(c) = coord {
                points.push(json!({
                    "event_id": id, "x": c.get(&q.x).copied().unwrap_or(0.0),
                    "y": c.get(&q.y).copied().unwrap_or(0.0),
                    "raw_flags": state.raw_events.get(id).and_then(|e| e.get("flags").cloned()),
                }));
            }
        }
        Ok(json!({"x": q.x, "y": q.y, "points": points, "basis_note": "荧光轴为补偿后 asinh 索引；散色轴为原始 channel-count"}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}

async fn replay(State(s): State<AppState>) -> Response {
    let run = || -> Result<Value> {
        let case = CaseFile::from_str(&s.case_json)?;
        let mut store = s.store.lock().map_err(|e| e.to_string())?;
        let report = workflow::replay_case(&mut store, &case)?;
        Ok(json!({"steps": report.steps,
                  "main_rev": store.branch_revision(MAIN)?,
                  "alt_rev": store.branch_revision(ALT)?}))
    };
    match run() {
        Ok(v) => Json(v).into_response(),
        Err(e) => api_err(e),
    }
}
