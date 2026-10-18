use axum::{
	body::Body,
	extract::{Extension, Query, State},
	http::{
		StatusCode,
		header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS},
	},
	response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{auth::AuthenticatedUser, state::AppState};

use super::{DownloadError, DownloadService};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
	Inline,
	#[default]
	Attachment,
}

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
	pub path: String,
	/// How the browser should treat the response. Defaults to attachment
	/// (download); inline asks the browser to render it in place.
	#[serde(default)]
	pub disposition: Disposition,
}

pub async fn download(
	State(state): State<AppState>,
	Extension(user): Extension<AuthenticatedUser>,
	Query(query): Query<DownloadQuery>,
) -> Result<impl IntoResponse, StatusCode> {
	let service = DownloadService::new(state.master, state.cipher);
	let (info, stream) = service
		.open(user.user_id, query.path.clone())
		.await
		.map_err(map_download_error)?;

	let filename = file_name(&query.path);
	let mime = content_type(&filename);
	let disposition = disposition_value(&query.disposition, mime, &filename);

	let response = Response::builder()
		.status(StatusCode::OK)
		.header(CONTENT_TYPE, mime)
		.header(CONTENT_LENGTH, info.size)
		.header(CONTENT_DISPOSITION, disposition)
		// Stop the browser from guessing a type we did not send, which would
		// defeat the inline safety check below.
		.header(X_CONTENT_TYPE_OPTIONS, "nosniff")
		.body(Body::from_stream(stream))
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

	Ok(response)
}

/// Builds the Content-Disposition value. inline is only honoured for types the
/// browser can render safely; html/svg (script execution) and octet-stream
/// (unknown) are forced to attachment even if inline was asked, so an uploaded
/// file can never run as a document on the gateway's origin.
fn disposition_value(requested: &Disposition, mime: &str, filename: &str) -> String {
	let name = sanitize(filename);
	let safe_inline = matches!(requested, Disposition::Inline) && inline_is_safe(mime);
	if safe_inline {
		format!("inline; filename=\"{name}\"")
	} else {
		format!("attachment; filename=\"{name}\"")
	}
}

/// Types we are willing to serve inline: images, pdf, plain text and media.
/// Explicitly excludes text/html and image/svg+xml (XSS) and octet-stream.
fn inline_is_safe(mime: &str) -> bool {
	mime.starts_with("image/") && mime != "image/svg+xml"
		|| mime.starts_with("video/")
		|| mime.starts_with("audio/")
		|| mime == "application/pdf"
		|| mime.starts_with("text/plain")
}

/// Last path segment, e.g. "/a/b/report.pdf" -> "report.pdf".
fn file_name(path: &str) -> String {
	path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("download").to_string()
}

/// Strips quotes and control characters so the value cannot break out of the
/// Content-Disposition header.
fn sanitize(name: &str) -> String {
	name.chars()
		.filter(|c| *c != '"' && *c != '\\' && !c.is_control())
		.collect()
}

/// Minimal extension -> MIME map for the formats a browser can preview; the
/// rest fall back to octet-stream, which browsers download.
fn content_type(name: &str) -> &'static str {
	let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
	match ext.as_str() {
		"txt" | "log" | "md" => "text/plain; charset=utf-8",
		"json" => "application/json",
		"csv" => "text/csv",
		"html" | "htm" => "text/html; charset=utf-8",
		"pdf" => "application/pdf",
		"png" => "image/png",
		"jpg" | "jpeg" => "image/jpeg",
		"gif" => "image/gif",
		"webp" => "image/webp",
		"svg" => "image/svg+xml",
		"mp4" => "video/mp4",
		"webm" => "video/webm",
		"mp3" => "audio/mpeg",
		"wav" => "audio/wav",
		_ => "application/octet-stream",
	}
}

fn map_download_error(error: DownloadError) -> StatusCode {
	match error {
		DownloadError::Master(status) => match status.code() {
			tonic::Code::NotFound => StatusCode::NOT_FOUND,
			tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
			tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
			_ => StatusCode::BAD_GATEWAY,
		},
	}
}
