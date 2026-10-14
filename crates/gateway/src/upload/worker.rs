use bytes::Bytes;
use futures::stream;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tonic::Status;

use crate::{
	grpc::WorkerClient,
	pb::{BlockAssignment, BlockFrame, BlockHeader, CommittedBlock, DataFrame, block_frame::Kind},
};

const FRAME_SIZE: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum WorkerUploadError {
	#[error("could not connect to worker: {0}")]
	Connection(#[from] tonic::transport::Error),
	#[error("worker rejected block: {0}")]
	Rejected(#[source] Status),
	#[error("master returned an invalid block assignment")]
	InvalidAssignment,
	#[error("worker acknowledged an unexpected block")]
	UnexpectedResponse,
}

#[derive(Clone, Default)]
pub struct BlockSender;

impl BlockSender {
	pub async fn send(
		&self,
		assignment: &BlockAssignment,
		block: Bytes,
		sha256: Vec<u8>,
	) -> Result<CommittedBlock, WorkerUploadError> {
		let first_worker = assignment
			.workers
			.first()
			.ok_or(WorkerUploadError::InvalidAssignment)?;
		let next_worker = assignment
			.workers
			.get(1)
			.ok_or(WorkerUploadError::InvalidAssignment)?;
		let worker = WorkerClient::connect(first_worker.addr.clone()).await?;

		let mut frames = Vec::with_capacity(1 + block.len().div_ceil(FRAME_SIZE));
		frames.push(BlockFrame {
			kind: Some(Kind::Header(BlockHeader {
				block_id: assignment.block_id.clone(),
				sha256: sha256.clone(),
				size: assignment.size,
				next_worker: next_worker.addr.clone(),
				overwrite: false,
			})),
		});

		for chunk in block.chunks(FRAME_SIZE) {
			frames.push(BlockFrame {
				kind: Some(Kind::Data(DataFrame {
					data: chunk.to_vec(),
					crc32c: 0,
				})),
			});
		}

		let response = worker
			.put_block(stream::iter(frames))
			.await
			.map_err(WorkerUploadError::Rejected)?;
		if response.block_id != assignment.block_id || response.stored_bytes != assignment.size {
			return Err(WorkerUploadError::UnexpectedResponse);
		}

		Ok(CommittedBlock {
			idx: assignment.idx,
			block_id: assignment.block_id.clone(),
			size: assignment.size,
			sha256,
			worker_ids: assignment.workers.iter().map(|worker| worker.worker_id.clone()).collect(),
		})
	}

	pub fn hash(block: &Bytes) -> Vec<u8> {
		Sha256::digest(block).to_vec()
	}
}