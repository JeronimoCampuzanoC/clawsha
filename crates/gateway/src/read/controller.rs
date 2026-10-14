use axum::{
	extract::{Extension, Query, State},
	http::StatusCode,
	response::IntoResponse,
	Json,
};
use serde::{Deserialize, Serialize};

use crate::{auth::AuthenticatedUser, pb::EntryType, state::AppState};

use super::service::ReadService;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
	pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DirectoryListing {
	pub entries: Vec<ListedEntry>,
}

#[derive(Debug, Serialize)]
pub struct ListedEntry {
	pub name: String,
	pub kind: String,
	pub size: u64,
	pub modified_at: i64,
}

pub async fn list(
	State(state): State<AppState>,
	Extension(user): Extension<AuthenticatedUser>,
	Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, StatusCode> {
	let service = ReadService::new(state.master);
	let response = service
		.list(user.user_id, query.path.unwrap_or_else(|| "/".to_string()))
		.await
		.map_err(map_list_error)?;

	let entries = response
		.entries
		.into_iter()
		.map(ListedEntry::from)
		.collect();

	Ok((StatusCode::OK, Json(DirectoryListing { entries })))
}

impl From<crate::pb::Entry> for ListedEntry {
	fn from(entry: crate::pb::Entry) -> Self {
		let kind = match EntryType::try_from(entry.r#type).unwrap_or(EntryType::Unspecified) {
			EntryType::Dir => "directory",
			EntryType::File => "file",
			EntryType::Unspecified => "unknown",
		};

		Self {
			name: entry.name,
			kind: kind.to_string(),
			size: entry.size,
			modified_at: entry.modified_at,
		}
	}
}

fn map_list_error(status: tonic::Status) -> StatusCode {
	match status.code() {
		tonic::Code::NotFound => StatusCode::NOT_FOUND,
		tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
		tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
		_ => StatusCode::INTERNAL_SERVER_ERROR,
	}
}
