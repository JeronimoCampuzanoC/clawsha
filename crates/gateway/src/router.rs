use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::CorsLayer;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(crate::auth::router(state.clone()))
        .merge(crate::cluster::router(state.clone()))
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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::router;
    use crate::auth::{AuthService, JwtService};
    use crate::encript::CipherService;
    use crate::grpc::MasterClient;
    use crate::state::AppState;

    /// The master address is never dialed: these requests stop before any RPC.
    fn app() -> axum::Router {
        let master = MasterClient::connect("http://127.0.0.1:1").unwrap();
        let auth = AuthService::new(master, JwtService::new("test-secret").unwrap());
        let cipher = CipherService::new(&"00".repeat(32)).unwrap();
        router(AppState::new(auth, cipher))
    }

    async fn status(method: &str, uri: &str) -> StatusCode {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        app().oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn an_unknown_route_is_not_found_not_unauthorized() {
        assert_eq!(status("GET", "/").await, StatusCode::NOT_FOUND);
        assert_eq!(status("GET", "/nonexistent").await, StatusCode::NOT_FOUND);
        assert_eq!(status("POST", "/api/v1/nope").await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn protected_routes_still_need_a_token() {
        assert_eq!(status("GET", "/api/v1/fs").await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            status("PUT", "/api/v1/files?path=/x").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status("GET", "/api/v1/files?path=/x").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status("DELETE", "/api/v1/files?path=/x").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status("POST", "/api/v1/move").await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status("GET", "/api/v1/cluster/status").await,
            StatusCode::UNAUTHORIZED
        );
    }
}
