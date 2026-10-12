mod blocking;
mod get_block;
mod put_block;
mod replicate;

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Code, Request, Response, Status, Streaming};

use crate::fencing::Fencing;
use crate::pb::worker_service_server::WorkerService;
use crate::pb::{
    BlockFrame, DeleteBlockRequest, DeleteBlockResponse, GetBlockRequest, PutBlockResponse,
    ReplicateBlockRequest, ReplicateBlockResponse,
};
use crate::storage::Storage;

pub(crate) use blocking::blocking;
use get_block::send_frames;
pub(crate) use put_block::receive_block;
use replicate::{connect_to, prepare_replica, replica_header, send_replica_frames, target_error};

const CHANNEL_CAPACITY: usize = 4;

pub struct WorkerSvc {
    storage: Arc<Storage>,
    frame_size: usize,
    fencing: Arc<Fencing>,
}

impl WorkerSvc {
    pub fn new(storage: Arc<Storage>, frame_size: usize) -> Self {
        Self {
            storage,
            frame_size,
            fencing: Arc::new(Fencing::new()),
        }
    }

    /// Shared handle, so the heartbeat loop can feed the leader's token later.
    pub fn fencing(&self) -> Arc<Fencing> {
        Arc::clone(&self.fencing)
    }
}

#[tonic::async_trait]
impl WorkerService for WorkerSvc {
    type GetBlockStream = ReceiverStream<Result<BlockFrame, Status>>;

    async fn put_block(
        &self,
        request: Request<Streaming<BlockFrame>>,
    ) -> Result<Response<PutBlockResponse>, Status> {
        let storage = Arc::clone(&self.storage);
        let response = receive_block(storage, request.into_inner()).await?;
        Ok(Response::new(response))
    }

    async fn get_block(
        &self,
        request: Request<GetBlockRequest>,
    ) -> Result<Response<Self::GetBlockStream>, Status> {
        let GetBlockRequest { block_id, offset } = request.into_inner();

        // Open the block: checks the id, that it exists and that the offset fits.
        let storage = Arc::clone(&self.storage);
        let id = block_id.clone();
        let file = blocking(move || storage.open_block(&id, offset)).await?;

        //  Verify the block against its stored hash.
        let storage = Arc::clone(&self.storage);
        blocking(move || storage.verify_block(&block_id)).await?;

        //  Read the file on a blocking thread and feed the channel.
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let frame_size = self.frame_size;
        tokio::task::spawn_blocking(move || send_frames(file, frame_size, tx));

        //  Return the other end of the channel as the response stream.
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn delete_block(
        &self,
        request: Request<DeleteBlockRequest>,
    ) -> Result<Response<DeleteBlockResponse>, Status> {
        let DeleteBlockRequest { block_id, fencing } = request.into_inner();

        // 1. Only the current leader may give orders.
        self.fencing.check(fencing)?;

        // 2. Delete on the blocking pool. A block that is already gone is fine.
        let storage = Arc::clone(&self.storage);
        let freed_bytes = match blocking(move || storage.delete_block(&block_id)).await {
            Ok(bytes) => bytes,
            Err(status) if status.code() == Code::NotFound => 0,
            Err(status) => return Err(status),
        };

        Ok(Response::new(DeleteBlockResponse { freed_bytes }))
    }

    async fn replicate_block(
        &self,
        request: Request<ReplicateBlockRequest>,
    ) -> Result<Response<ReplicateBlockResponse>, Status> {
        let ReplicateBlockRequest {
            block_id,
            target,
            fencing,
            overwrite,
        } = request.into_inner();

        // 1. Only the current leader may give orders.
        self.fencing.check(fencing)?;
        if target.is_empty() {
            return Err(Status::invalid_argument("target must not be empty"));
        }

        // 2. Check the local copy and open it.
        let storage = Arc::clone(&self.storage);
        let id = block_id.clone();
        let source = blocking(move || prepare_replica(&storage, &id)).await?;
        let size = source.size;

        // 3. Connect to the target worker.
        let mut client = connect_to(&target).await?;

        // 4. Build the outgoing stream: header first, then data frames from a blocking thread.
        let (tx, rx) = mpsc::channel::<BlockFrame>(CHANNEL_CAPACITY);
        let header = replica_header(&block_id, &source, overwrite);
        tx.send(header)
            .await
            .map_err(|_| Status::internal("could not queue the header"))?;

        let frame_size = self.frame_size;
        let file = source.file;
        tokio::task::spawn_blocking(move || send_replica_frames(file, frame_size, tx));

        // 5. Send the stream and wait for the target to confirm.
        client
            .put_block(ReceiverStream::new(rx))
            .await
            .map_err(|status| target_error(&target, status))?;

        Ok(Response::new(ReplicateBlockResponse { sent_bytes: size }))
    }
}
