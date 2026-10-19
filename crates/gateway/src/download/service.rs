use bytes::Bytes;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

use crate::{grpc::MasterClient, pb::ReadPlan};

use super::{
    master::DownloadMaster,
    worker::{BlockReadError, BlockReader},
};
use crate::encript::CipherService;

/// How many verified blocks may sit in the response channel waiting to be
/// written to the client. Keeps memory bounded on slow clients.
const CHANNEL_CAPACITY: usize = 2;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("master rejected read: {0}")]
    Master(#[from] Status),
}

/// Metadata the controller needs to build the HTTP response before the body
/// starts streaming.
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    pub size: u64,
}

#[derive(Clone)]
pub struct DownloadService {
    master: DownloadMaster,
    reader: BlockReader,
    cipher: CipherService,
}

impl DownloadService {
    pub fn new(master: MasterClient, cipher: CipherService) -> Self {
        Self {
            master: DownloadMaster::new(master),
            reader: BlockReader,
            cipher,
        }
    }

    /// Resolves the read plan, then returns the file metadata and a byte
    /// stream. Blocks are fetched one at a time, each verified against its
    /// sha256 before any of its bytes reach the client; on a hash mismatch the
    /// bad replica is reported to the master and the next replica is tried.
    pub async fn open(
        &self,
        user_id: String,
        path: String,
    ) -> Result<(DownloadInfo, ReceiverStream<Result<Bytes, std::io::Error>>), DownloadError> {
        let plan = self.master.read_plan(user_id, path).await?;
        let info = DownloadInfo { size: plan.size };

        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let service = self.clone();
        tokio::spawn(async move {
            service.stream_blocks(plan, tx).await;
        });

        Ok((info, ReceiverStream::new(rx)))
    }

    /// Drives the block-by-block fetch. Any failure is turned into an
    /// io::Error on the channel, which axum surfaces as a broken body so the
    /// client sees a truncated (failed) download rather than silent garbage.
    async fn stream_blocks(&self, plan: ReadPlan, tx: mpsc::Sender<Result<Bytes, std::io::Error>>) {
        for block in &plan.blocks {
            match self.read_block_with_failover(block).await {
                Ok(bytes) => {
                    if tx.send(Ok(bytes)).await.is_err() {
                        // Client went away; stop fetching.
                        return;
                    }
                }
                Err(message) => {
                    let _ = tx.send(Err(std::io::Error::other(message))).await;
                    return;
                }
            }
        }
    }

    /// Tries each replica of one block in order. Reports corruption to the
    /// master and moves on; returns the verified bytes on the first good
    /// replica, or an error message if every replica failed.
    async fn read_block_with_failover(
        &self,
        block: &crate::pb::BlockLocation,
    ) -> Result<Bytes, String> {
        let mut last_error = String::from("no replicas available for block");
        for replica in &block.replicas {
            match self
                .reader
                .read(&replica.addr, &block.block_id, block.size, &block.sha256)
                .await
            {
                // The worker returns ciphertext, already verified against the
                // plan's sha256 (which is the hash of the ciphertext). Decrypt
                // it with the block id before handing the plaintext to the
                // client.
                Ok(ciphertext) => {
                    return Ok(self.cipher.decrypt(&block.block_id, ciphertext));
                }
                Err(error) => {
                    if error.is_corruption() {
                        self.master
                            .report_corruption(block.block_id.clone(), replica.worker_id.clone())
                            .await;
                    }
                    last_error = describe(&block.block_id, &error);
                }
            }
        }
        Err(last_error)
    }
}

fn describe(block_id: &str, error: &BlockReadError) -> String {
    format!("block {block_id}: {error}")
}
