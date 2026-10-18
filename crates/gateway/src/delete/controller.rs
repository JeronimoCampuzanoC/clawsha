use axum::{
	extract::{Extension, Query, State},
	http::StatusCode,
	response::IntoResponse,
};
use serde::Deserialize;

use crate::{auth::AuthenticatedUser, state::AppState};

use super::service::DeleteService;

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
	pub path: String,
	/// Required to remove a non-empty directory. Defaults to false so a
	/// mistaken call on a folder with contents fails with 409 instead of
	/// wiping a subtree.
	#[serde(default)]
	pub recursive: bool,
}

pub async fn remove(
	State(state): State<AppState>,
	Extension(user): Extension<AuthenticatedUser>,
	Query(query): Query<DeleteQuery>,
) -> Result<impl IntoResponse, StatusCode> {
	let service = DeleteService::new(state.master);
	service
		.remove(user.user_id, query.path, query.recursive)
		.await
		.map_err(map_delete_error)?;

	Ok(StatusCode::NO_CONTENT)
}

fn map_delete_error(status: tonic::Status) -> StatusCode {
	match status.code() {
		tonic::Code::NotFound => StatusCode::NOT_FOUND,
		// Non-empty directory removed without recursive, or path locked by
		// another writer.
		tonic::Code::FailedPrecondition => StatusCode::CONFLICT,
		tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
		tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
		_ => StatusCode::BAD_GATEWAY,
	}
}
