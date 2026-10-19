mod controller;
mod service;

use axum::{Router, routing::post};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/move", post(controller::move_entry))
        .route_layer(axum::middleware::from_fn_with_state(
            state.auth.jwt_service(),
            crate::auth::authenticate,
        ))
        .with_state(state)
}
