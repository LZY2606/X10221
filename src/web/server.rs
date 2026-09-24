use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::app::Service;

pub async fn serve(addr: SocketAddr, svc: Arc<Service>) -> std::io::Result<()> {
    let app = crate::web::api::router(svc);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}

pub fn router_test(svc: Arc<Service>) -> Router {
    crate::web::api::router(svc)
}
