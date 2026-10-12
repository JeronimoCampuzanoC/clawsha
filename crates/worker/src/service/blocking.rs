use tonic::Status;

use crate::error::StorageError;

/// Runs storage code (which blocks the thread) on tokio's blocking pool.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, Status>
where
    F: FnOnce() -> Result<T, StorageError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result.map_err(Status::from),
        Err(_) => Err(Status::internal("storage task failed")),
    }
}
