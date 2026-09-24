use crate::derive;
use crate::seed;
use crate::store::{ApiErr, Store};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
}

fn err_response(e: ApiErr) -> Response {
    match e {
        ApiErr::BadRequest(m) => (StatusCode::BAD_REQUEST, Json(json!({"error": m}))).into_response(),
        ApiErr::NotFound(m) => (StatusCode::NOT_FOUND, Json(json!({"error": m}))).into_response(),
        ApiErr::Conflict(c) => (StatusCode::CONFLICT, Json(c)).into_response(),
        ApiErr::Internal(m) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": m}))).into_response()
        }
    }
}

macro_rules! with_store {
    ($st:expr, $body:expr) => {
        match $st.store.lock() {
            Ok(store) => match ($body)(&store) {
                Ok(v) => Json(v).into_response(),
                Err(e) => err_response(e),
            },
            Err(_) => err_response(ApiErr::Internal("store lock poisoned".into())),
        }
    };
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_state(State(st): State<AppState>) -> Response {
    with_store!(st, |s: &Store| {
        Ok(json!({
            "materials": s.materials_summary()?,
            "drafts": s.list_drafts()?,
            "results": {
                "main": s.results_for("main").unwrap_or_default(),
                "alt": s.results_for("alt").unwrap_or_default(),
            },
            "diagnostics": s.diagnostics()?,
            "events": s.events(500)?,
            "casepack": {
                "manifest": serde_json::from_str::<Value>(seed::MANIFEST).unwrap_or(Value::Null),
                "steps": serde_json::from_str::<Value>(seed::STEPS).unwrap_or(Value::Null),
            },
        }))
    })
}

async fn post_seed(State(st): State<AppState>) -> Response {
    with_store!(st, |s: &Store| seed::seed(s))
}

async fn post_replay(State(st): State<AppState>) -> Response {
    with_store!(st, |s: &Store| seed::replay(s))
}

async fn post_material(
    State(st): State<AppState>,
    Path(kind): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    with_store!(st, |s: &Store| s.commit_material(&kind, body, None))
}

async fn post_derive(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    with_store!(st, |s: &Store| s.derive(&id))
}

#[derive(serde::Deserialize)]
struct RulingBody {
    base_seq: i64,
    target: String,
    decision: String,
}

async fn post_ruling(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RulingBody>,
) -> Response {
    with_store!(st, |s: &Store| s.save_ruling(&id, body.base_seq, &body.target, &body.decision))
}

async fn post_undo(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    with_store!(st, |s: &Store| s.undo(&id))
}

async fn get_compare(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let a = q.get("a").cloned().unwrap_or_else(|| "main".into());
    let b = q.get("b").cloned().unwrap_or_else(|| "alt".into());
    with_store!(st, |s: &Store| s.compare(&a, &b))
}

async fn get_event_view(State(st): State<AppState>, Path(seq): Path<i64>) -> Response {
    with_store!(st, |s: &Store| s.view_at(seq))
}

async fn get_export(State(st): State<AppState>) -> Response {
    with_store!(st, |s: &Store| s.export_archive())
}

async fn post_import(State(st): State<AppState>, Json(body): Json<Value>) -> Response {
    with_store!(st, |s: &Store| s.import_archive(&body))
}

async fn get_scatter(State(st): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let draft = q.get("draft").cloned().unwrap_or_else(|| "main".into());
    let x = q.get("x").cloned().unwrap_or_else(|| "FL1".into());
    let y = q.get("y").cloned().unwrap_or_else(|| "FL2".into());
    let comp = q.get("comp").cloned().unwrap_or_else(|| "post".into());
    with_store!(st, |s: &Store| scatter(s, &draft, &x, &y, &comp))
}

fn scatter(s: &Store, draft_id: &str, x: &str, y: &str, comp: &str) -> Result<Value, ApiErr> {
    let (state, _) = s.get_draft(draft_id)?;
    let fcs_v = s
        .latest_material("fcs_events")?
        .map(|(v, _)| v)
        .ok_or_else(|| ApiErr::bad("not seeded"))?;
    let fcs: derive::FcsData = serde_json::from_value(s.material_content("fcs_events", fcs_v)?)?;
    let comp_v = s.latest_material("compensation_matrix")?.map(|(v, _)| v).unwrap_or(1);
    let compm: derive::CompensationMatrix =
        serde_json::from_value(s.material_content("compensation_matrix", comp_v)?)?;
    let tp_v = s.latest_material("transform_params")?.map(|(v, _)| v).unwrap_or(1);
    let tp: derive::TransformParams =
        serde_json::from_value(s.material_content("transform_params", tp_v)?)?;
    let gates_v = s.latest_material("gate_polygons")?.map(|(v, _)| v).unwrap_or(1);
    let gates: derive::GateSet =
        serde_json::from_value(s.material_content("gate_polygons", gates_v)?)?;
    let rulings: HashMap<String, String> =
        serde_json::from_value(state["rulings"].clone()).unwrap_or_default();

    let ch_idx: HashMap<String, usize> = fcs
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.clone(), i))
        .collect();
    let xi = *ch_idx.get(x).ok_or_else(|| ApiErr::bad(format!("unknown channel {x}")))?;
    let yi = *ch_idx.get(y).ok_or_else(|| ApiErr::bad(format!("unknown channel {y}")))?;
    let comp_idx: Vec<usize> = compm
        .channels
        .iter()
        .filter_map(|c| ch_idx.get(c).copied())
        .collect();

    let inv = if comp == "post" {
        match derive::analyze_matrix(&compm.matrix) {
            derive::MatrixVerdict::Ok { inv, .. } => Some(inv),
            _ => match rulings.get("compensation").map(|s| s.as_str()) {
                Some("regularize") => derive::regularized_inverse(&compm.matrix, derive::REG_LAMBDA),
                _ => None,
            },
        }
    } else {
        None
    };

    let mut boundary_set: Vec<usize> = Vec::new();
    for r in s.results_for(draft_id)? {
        if let Some(arr) = r["content"]["boundary_events"].as_array() {
            for v in arr {
                if let Some(i) = v.as_u64() {
                    boundary_set.push(i as usize);
                }
            }
        }
    }

    let cof = |name: &str| tp.cofactor.get(name).copied().unwrap_or(1.0);
    let points: Vec<Value> = fcs
        .events
        .iter()
        .enumerate()
        .map(|(i, ev)| {
            let mut row = ev.clone();
            if let Some(inv) = &inv {
                let sub: Vec<f64> = comp_idx.iter().map(|&ci| ev[ci]).collect();
                let comped = derive::mat_vec(inv, &sub);
                for (k, &ci) in comp_idx.iter().enumerate() {
                    row[ci] = comped[k];
                }
            }
            let tx = derive::asinh_transform(row[xi], cof(x));
            let ty = derive::asinh_transform(row[yi], cof(y));
            let extreme = ev.iter().any(|v| v.abs() > derive::EXTREME_LIMIT);
            json!({
                "i": i, "x": tx, "y": ty,
                "extreme": extreme,
                "boundary": boundary_set.contains(&i),
            })
        })
        .collect();
    let gate_overlays: Vec<Value> = gates
        .gates
        .iter()
        .filter(|(_, g)| g.channel_x == x && g.channel_y == y)
        .map(|(id, g)| json!({"id": id, "polygon": g.polygon}))
        .collect();
    Ok(json!({
        "draft": draft_id, "x": x, "y": y, "comp": comp,
        "channels": fcs.channels,
        "points": points,
        "gates": gate_overlays,
        "raw_repr": fcs.raw_repr,
        "source": fcs.source,
    }))
}

pub fn app(store: Store) -> Router {
    let state = AppState {
        store: Arc::new(Mutex::new(store)),
    };
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/seed", post(post_seed))
        .route("/api/replay", post(post_replay))
        .route("/api/materials/{kind}", post(post_material))
        .route("/api/drafts/{id}/derive", post(post_derive))
        .route("/api/drafts/{id}/rulings", post(post_ruling))
        .route("/api/drafts/{id}/undo", post(post_undo))
        .route("/api/compare", get(get_compare))
        .route("/api/events/{seq}/view", get(get_event_view))
        .route("/api/export", get(get_export))
        .route("/api/import", post(post_import))
        .route("/api/scatter", get(get_scatter))
        .with_state(state)
}

pub async fn run(addr: &str, db_path: &str) -> std::io::Result<()> {
    let store = Store::open(std::path::Path::new(db_path)).expect("open db");
    store.migrate().expect("migrate");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("flowline listening on http://{addr} (db: {db_path})");
    axum::serve(listener, app(store)).await
}
