use axum::{
    Json,
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;

use crate::{auth::AuthenticatedUser, state::AppState};

use super::service::MoveService;

#[derive(Debug, Deserialize)]
pub struct MoveBody {
    /// Absolute source path, e.g. "/docs/a.pdf".
    pub from: String,
    /// Absolute destination path including the new name, e.g. "/archive/a.pdf".
    pub to: String,
}

pub async fn move_entry(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<MoveBody>,
) -> Result<impl IntoResponse, StatusCode> {
    let service = MoveService::new(state.master);
    service
        .move_entry(user.user_id, body.from, body.to)
        .await
        .map_err(map_move_error)?;

    Ok(StatusCode::NO_CONTENT)
}

fn map_move_error(status: tonic::Status) -> StatusCode {
    match status.code() {
        // Source does not exist (or belongs to another user).
        tonic::Code::NotFound => StatusCode::NOT_FOUND,
        // Destination already taken, or either path locked by another writer.
        tonic::Code::AlreadyExists | tonic::Code::FailedPrecondition => StatusCode::CONFLICT,
        // Relative path, "..", or moving a directory into itself.
        tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
        tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_GATEWAY,
    }
}
