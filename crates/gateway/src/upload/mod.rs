mod controller;
mod master;
mod service;
mod worker;

pub use service::{UploadError, UploadService};

use axum::{Router, routing::put};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
	Router::new()
		.route("/api/v1/files", put(controller::upload))
		.layer(axum::middleware::from_fn_with_state(
			state.auth.jwt_service(),
			crate::auth::authenticate,
		))
		.with_state(state)
}
