use std::fs::File;
use std::time::Duration;

use tokio::sync::mpsc;
use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use super::get_block::read_frames;
use crate::error::StorageError;
use crate::pb::block_frame::Kind;
use crate::pb::worker_service_client::WorkerServiceClient;
use crate::pb::{BlockFrame, BlockHeader};
use crate::storage::Storage;

/// How long to wait when connecting to another worker.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Everything needed to send one local block to another worker.
pub(super) struct ReplicaSource {
    pub(super) file: File,
    sha256: [u8; 32],
    pub(super) size: u64,
}

/// Checks the local copy and opens it. A corrupted copy is never propagated.
pub(super) fn prepare_replica(
    storage: &Storage,
    block_id: &str,
) -> Result<ReplicaSource, StorageError> {
    storage.verify_block(block_id)?;
    let sha256 = storage.stored_checksum(block_id)?;
    let file = storage.open_block(block_id, 0)?;
    let size = file.metadata()?.len();

    Ok(ReplicaSource { file, sha256, size })
}

/// The first frame of the PutBlock stream we send to the target.
pub(super) fn replica_header(
    block_id: &str,
    source: &ReplicaSource,
    overwrite: bool,
) -> BlockFrame {
    BlockFrame {
        kind: Some(Kind::Header(BlockHeader {
            block_id: block_id.to_string(),
            sha256: source.sha256.to_vec(),
            size: source.size,
            next_worker: String::new(),
            overwrite,
        })),
    }
}

/// Opens a gRPC connection to another worker.
pub(super) async fn connect_to(target: &str) -> Result<WorkerServiceClient<Channel>, Status> {
    let endpoint = Endpoint::from_shared(format!("http://{target}"))
        .map_err(|e| Status::invalid_argument(format!("bad target {target}: {e}")))?
        .connect_timeout(CONNECT_TIMEOUT);

    let channel = endpoint
        .connect()
        .await
        .map_err(|e| Status::unavailable(format!("cannot reach {target}: {e}")))?;

    Ok(WorkerServiceClient::new(channel))
}

/// Keeps the target's error code but says which worker it came from.
pub(super) fn target_error(target: &str, status: Status) -> Status {
    Status::new(
        status.code(),
        format!("target {target}: {}", status.message()),
    )
}

/// ReplicateBlock side: feeds the outgoing stream with the data frames.
/// If reading fails, the stream just ends early and the target rejects the
/// incomplete block.
pub(super) fn send_replica_frames(file: File, frame_size: usize, tx: mpsc::Sender<BlockFrame>) {
    let _ = read_frames(file, frame_size, |frame| tx.blocking_send(frame).is_ok());
}
