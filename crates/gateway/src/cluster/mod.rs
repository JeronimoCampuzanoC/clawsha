//! Cluster status for the dashboard: who leads, which workers are alive and
//! how many blocks are missing a copy. Read-only, any logged-in user.

mod controller;

use axum::{Router, routing::get};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/cluster/status", get(controller::status))
        .route_layer(axum::middleware::from_fn_with_state(
            state.auth.jwt_service(),
            crate::auth::authenticate,
        ))
        .with_state(state)
}
