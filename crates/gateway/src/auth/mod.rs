mod controller;
mod jwt_service;
mod auth_service;
mod middleware;

pub use auth_service::{AuthError, AuthResponse, AuthService, RegistrationResponse};
pub use middleware::{AuthenticatedUser, authenticate};
pub use jwt_service::{JwtError, JwtService};

use axum::{Router, routing::post};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
	Router::new()
		.route("/api/v1/auth/login", post(controller::login))
		.route("/api/v1/auth/register", post(controller::register))
		.with_state(state)
}
