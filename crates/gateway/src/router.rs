use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
	Router::new()
		.merge(crate::auth::router(state.clone()))
		.merge(crate::read::router(state.clone()))
		.merge(crate::upload::router(state))
}
