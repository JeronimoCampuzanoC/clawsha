use axum::{
    extract::State,
    http::{Request, StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::JwtService;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub user_id: String,
}

pub async fn authenticate(
    State(jwt): State<JwtService>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(header) = request.headers().get(AUTHORIZATION) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Ok(header) = header.to_str() else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(token) = header.strip_prefix("Bearer ") else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Ok(claims) = jwt.verify_token(token) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    request.extensions_mut().insert(AuthenticatedUser {
        user_id: claims.sub,
    });

    next.run(request).await
}
