//! The orders the leader sends to workers: copy a block (health) and delete
//! a block (GC). A trait so the tests can fake the workers instead of
//! starting real ones.

use std::future::Future;
use std::time::Duration;

use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use crate::pb;
use crate::pb::worker_service_client::WorkerServiceClient;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub trait WorkerClient: Send + Sync + 'static {
    /// Sends `request` to the worker at `worker_addr` (the source of the
    /// copy). Returns the bytes it sent.
    fn replicate(
        &self,
        worker_addr: &str,
        request: pb::ReplicateBlockRequest,
    ) -> impl Future<Output = Result<u64, Status>> + Send;

    /// Asks the worker at `worker_addr` to drop a block. Returns the bytes
    /// freed (0 if it was already gone).
    fn delete(
        &self,
        worker_addr: &str,
        request: pb::DeleteBlockRequest,
    ) -> impl Future<Output = Result<u64, Status>> + Send;
}

/// The real client: plain gRPC to the worker's advertised address.
pub struct GrpcWorkers;

impl WorkerClient for GrpcWorkers {
    async fn replicate(
        &self,
        worker_addr: &str,
        request: pb::ReplicateBlockRequest,
    ) -> Result<u64, Status> {
        let response = connect(worker_addr).await?.replicate_block(request).await?;
        Ok(response.into_inner().sent_bytes)
    }

    async fn delete(
        &self,
        worker_addr: &str,
        request: pb::DeleteBlockRequest,
    ) -> Result<u64, Status> {
        let response = connect(worker_addr).await?.delete_block(request).await?;
        Ok(response.into_inner().freed_bytes)
    }
}

async fn connect(worker_addr: &str) -> Result<WorkerServiceClient<Channel>, Status> {
    let channel = Endpoint::from_shared(format!("http://{worker_addr}"))
        .map_err(|e| Status::invalid_argument(format!("bad worker address {worker_addr}: {e}")))?
        .connect_timeout(CONNECT_TIMEOUT)
        .connect()
        .await
        .map_err(|e| Status::unavailable(format!("cannot reach {worker_addr}: {e}")))?;
    Ok(WorkerServiceClient::new(channel))
}
