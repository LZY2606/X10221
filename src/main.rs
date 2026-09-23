mod case;
mod domain;
mod store;
mod web;

use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Extension,
};
use std::net::SocketAddr;
use store::AppState;

async fn index() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(include_str!("../static/index.html")),
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr_arg = std::env::args()
        .position(|arg| arg == "--addr")
        .and_then(|index| std::env::args().nth(index + 1))
        .unwrap_or_else(|| "127.0.0.1:5341".to_string());
    let addr: SocketAddr = addr_arg.parse()?;
    let db_path = std::env::var("FLOW_DB").unwrap_or_else(|_| "flow_boundary_chamber.sqlite".to_string());
    let conn = store::open(&db_path)?;
    let state = AppState::with_connection(conn);
    let app = web::router(state)
        .route("/", get(index))
        .layer(Extension(addr));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Flow Boundary Chamber listening on http://{addr} using {db_path}");
    axum::serve(listener, app).await?;
    Ok(())
}
