use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

use super::{AuthError, AuthResponse, RegistrationResponse};

#[derive(Debug, Deserialize)]
pub struct CredentialsRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user_id: String,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub user_id: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<CredentialsRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let response = state
        .auth
        .login(request.username, request.password)
        .await
        .map_err(map_auth_error)?;

    Ok((StatusCode::OK, Json(LoginResponse::from(response))))
}

pub async fn register(
    State(state): State<AppState>,
    Json(request): Json<CredentialsRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let response = state
        .auth
        .register(request.username, request.password)
        .await
        .map_err(map_auth_error)?;

    Ok((StatusCode::CREATED, Json(RegisterResponse::from(response))))
}

impl From<AuthResponse> for LoginResponse {
    fn from(response: AuthResponse) -> Self {
        Self {
            user_id: response.user_id,
            token: response.token,
        }
    }
}

impl From<RegistrationResponse> for RegisterResponse {
    fn from(response: RegistrationResponse) -> Self {
        Self {
            user_id: response.user_id,
        }
    }
}

fn map_auth_error(error: AuthError) -> StatusCode {
    match error {
        AuthError::Master(status) => match status.code() {
            tonic::Code::Unauthenticated => StatusCode::UNAUTHORIZED,
            tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
            tonic::Code::AlreadyExists => StatusCode::CONFLICT,
            tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
        AuthError::Jwt(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_username_is_a_conflict_not_a_server_error() {
        let status = tonic::Status::already_exists("username taken");
        assert_eq!(
            map_auth_error(AuthError::Master(status)),
            StatusCode::CONFLICT
        );
    }
}
