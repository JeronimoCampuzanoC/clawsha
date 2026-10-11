//! Health checks over plain HTTP, on their own port.
//!
//! /health        200 while the process runs. The ASG uses it to replace a
//!                dead control node.
//! /health/leader 200 only on the leader, 503 on the standby. The internal
//!                NLB uses it to send gateways and workers to the leader
//!                without anybody having to know who that is.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;

use crate::app::App;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", get(|| async { (StatusCode::OK, "ok") }))
        .route("/health/leader", get(leader))
        .with_state(app)
}

async fn leader(State(app): State<Arc<App>>) -> (StatusCode, String) {
    if app.leadership.is_leader() {
        (
            StatusCode::OK,
            format!("leader fencing={}", app.leadership.fencing()),
        )
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "standby".to_string())
    }
}

pub async fn serve(app: Arc<App>, listener: tokio::net::TcpListener) {
    if let Err(e) = axum::serve(listener, router(app)).await {
        tracing::error!(error = %e, "health server stopped");
    }
}
