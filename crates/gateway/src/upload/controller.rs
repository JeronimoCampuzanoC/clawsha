use axum::{
    Json,
    extract::{Extension, Query, Request, State},
    http::{StatusCode, header::CONTENT_LENGTH},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::{auth::AuthenticatedUser, pb::FileInfo, state::AppState};

use super::{MkdirError, UploadError, UploadService, worker::WorkerUploadError};

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub path: String,
    pub inode_id: String,
    pub version_no: u64,
    pub size: u64,
    pub sha256: Vec<u8>,
    pub block_count: u32,
    pub modified_at: i64,
}

pub async fn upload(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    request: Request,
) -> Result<impl IntoResponse, StatusCode> {
    let query: UploadQuery = serde_urlencoded::from_str(request.uri().query().unwrap_or(""))
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let size = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(StatusCode::LENGTH_REQUIRED)?;

    let service = UploadService::new(state.master, state.cipher);
    let file = service
        .upload(user.user_id, query.path, size, request.into_body())
        .await
        .map_err(map_upload_error)?;

    Ok((StatusCode::CREATED, Json(UploadResponse::from(file))))
}

#[derive(Debug, Deserialize)]
pub struct MkdirQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct MkdirResponse {
    pub path: String,
}

pub async fn mkdir(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<MkdirQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let service = UploadService::new(state.master, state.cipher);
    service
        .mkdir(user.user_id, query.path.clone())
        .await
        .map_err(map_mkdir_error)?;

    Ok((
        StatusCode::CREATED,
        Json(MkdirResponse { path: query.path }),
    ))
}

impl From<FileInfo> for UploadResponse {
    fn from(file: FileInfo) -> Self {
        Self {
            path: file.path,
            inode_id: file.inode_id,
            version_no: file.version_no,
            size: file.size,
            sha256: file.sha256,
            block_count: file.block_count,
            modified_at: file.modified_at,
        }
    }
}

fn map_upload_error(error: UploadError) -> StatusCode {
    match error {
        UploadError::Master(status) => match status.code() {
            tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
            tonic::Code::NotFound => StatusCode::NOT_FOUND,
            tonic::Code::AlreadyExists | tonic::Code::FailedPrecondition => StatusCode::CONFLICT,
            tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        },
        UploadError::Worker(error) => match error {
            WorkerUploadError::Connection(_) => StatusCode::SERVICE_UNAVAILABLE,
            WorkerUploadError::Rejected(status) => match status.code() {
                tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_GATEWAY,
            },
            WorkerUploadError::InvalidAssignment | WorkerUploadError::UnexpectedResponse => {
                StatusCode::BAD_GATEWAY
            }
        },
        UploadError::Lease(_) => StatusCode::SERVICE_UNAVAILABLE,
        UploadError::Body(_) | UploadError::InvalidBodySize => StatusCode::BAD_REQUEST,
    }
}

fn map_mkdir_error(error: MkdirError) -> StatusCode {
    match error {
        MkdirError::Master(status) => match status.code() {
            tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
            tonic::Code::NotFound => StatusCode::NOT_FOUND,
            tonic::Code::AlreadyExists | tonic::Code::FailedPrecondition => StatusCode::CONFLICT,
            tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_GATEWAY,
        },
    }
}
