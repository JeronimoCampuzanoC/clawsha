mod controller;
mod service;

use axum::{Router, routing::get};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
	Router::new()
		.route("/api/v1/fs", get(controller::list))
		.layer(axum::middleware::from_fn_with_state(
			state.auth.jwt_service(),
			crate::auth::authenticate,
		))
		.with_state(state)
}
