use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tonic::Status;

use crate::{
    grpc::WorkerClient,
    pb::{GetBlockRequest, block_frame::Kind},
};

#[derive(Debug, Error)]
pub enum BlockReadError {
    #[error("could not connect to worker: {0}")]
    Connection(#[from] tonic::transport::Error),
    #[error("worker rejected GetBlock: {0}")]
    Rejected(#[source] Status),
    #[error("stream from worker failed: {0}")]
    Stream(#[source] Status),
    #[error("worker sent a header frame on GetBlock")]
    UnexpectedHeader,
    #[error("block size did not match the plan: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("block hash did not match the plan")]
    HashMismatch,
}

impl BlockReadError {
    /// A corrupt block (bad hash) is the only case the caller reports to the
    /// master; everything else is a transport/availability problem.
    pub fn is_corruption(&self) -> bool {
        matches!(self, BlockReadError::HashMismatch)
    }
}

/// Stateless reader of a single block from one worker. Mirrors BlockSender on
/// the upload side.
#[derive(Clone, Default)]
pub struct BlockReader;

impl BlockReader {
    /// Reads the whole block from `addr`, checks its size and sha256 against
    /// the plan, and returns the verified bytes. The block is buffered in full
    /// before anything is returned, so a replica whose hash is wrong never
    /// leaks bytes to the client (16 MiB cap per block).
    pub async fn read(
        &self,
        addr: &str,
        block_id: &str,
        expected_size: u64,
        expected_sha256: &[u8],
    ) -> Result<Bytes, BlockReadError> {
        let worker = WorkerClient::connect(addr.to_string()).await?;
        let mut stream = worker
            .get_block(GetBlockRequest {
                block_id: block_id.to_string(),
                offset: 0,
            })
            .await
            .map_err(BlockReadError::Rejected)?;

        let mut buffer = BytesMut::with_capacity(expected_size as usize);
        let mut hasher = Sha256::new();

        while let Some(frame) = stream.next().await {
            let frame = frame.map_err(BlockReadError::Stream)?;
            match frame.kind {
                // GetBlock only ever streams data frames.
                Some(Kind::Data(data)) => {
                    hasher.update(&data.data);
                    buffer.extend_from_slice(&data.data);
                }
                Some(Kind::Header(_)) => return Err(BlockReadError::UnexpectedHeader),
                None => {}
            }
        }

        let actual_size = buffer.len() as u64;
        if actual_size != expected_size {
            return Err(BlockReadError::SizeMismatch {
                expected: expected_size,
                actual: actual_size,
            });
        }
        if hasher.finalize().as_slice() != expected_sha256 {
            return Err(BlockReadError::HashMismatch);
        }

        Ok(buffer.freeze())
    }
}
