use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::CorsLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(crate::auth::router(state.clone()))
        .merge(crate::read::router(state.clone()))
        .merge(crate::upload::router(state.clone()))
        .merge(crate::download::router(state.clone()))
        .merge(crate::delete::router(state.clone()))
        .merge(crate::r#move::router(state))
        .layer(cors_layer())
}

fn cors_layer() -> CorsLayer {
    let origins = std::env::var("CORS_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:5173,http://127.0.0.1:5173".to_string());

    let origins: Vec<HeaderValue> = origins
        .split(',')
        .filter_map(|origin| origin.trim().parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}
