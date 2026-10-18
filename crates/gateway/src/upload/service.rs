use bytes::{Bytes, BytesMut};
use futures::{StreamExt, stream::FuturesUnordered};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{oneshot, watch};
use tonic::Status;

use crate::{
	grpc::MasterClient,
	pb::{
		AbortUploadRequest, BlockAssignment, CommitUploadRequest, FileInfo, LeaseRequest,
		ReassignBlockRequest, CommittedBlock, UploadPlan,
	},
};

use super::{
	master::UploadMaster,
	worker::{BlockSender, WorkerUploadError},
};

const MAX_IN_FLIGHT_BLOCKS: usize = 4;
const LEASE_TTL_SECONDS: u32 = 60;
const LEASE_RENEW_INTERVAL_SECONDS: u64 = 20;

#[derive(Debug, Error)]
pub enum UploadError {
	#[error("master rejected upload: {0}")]
	Master(#[from] Status),
	#[error("worker upload failed: {0}")]
	Worker(#[from] WorkerUploadError),
	#[error("request body failed: {0}")]
	Body(String),
	#[error("request body size did not match the upload plan")]
	InvalidBodySize,
	#[error("upload lease renewal failed: {0}")]
	Lease(String),
}

#[derive(Debug, Error)]
pub enum MkdirError {
	#[error("master rejected mkdir: {0}")]
	Master(#[from] Status),
}

#[derive(Clone)]
pub struct UploadService {
	master: UploadMaster,
	workers: BlockSender,
}

impl UploadService {
	pub fn new(master: MasterClient) -> Self {
		Self {
			master: UploadMaster::new(master),
			workers: BlockSender,
		}
	}

	/// Creates a single directory. The master's Mkdir is not recursive, so the
	/// immediate parent must already exist; it reports AlreadyExists if the
	/// directory is already there and NotFound if the parent is missing.
	pub async fn mkdir(&self, user_id: String, path: String) -> Result<(), MkdirError> {
		self.master.mkdir(user_id, path).await?;
		Ok(())
	}

	pub async fn upload(
		&self,
		user_id: String,
		path: String,
		size: u64,
		body: axum::body::Body,
	) -> Result<FileInfo, UploadError> {
		let plan = self.master.start(user_id.clone(), path.clone(), size).await?;
		let (cancel_sender, cancel_rx) = oneshot::channel();
		let mut cancel_tx = Some(cancel_sender);
		let (lease_failure_tx, lease_failure_rx) = watch::channel(None);
		self.spawn_lease_renewer(&user_id, &path, &plan, cancel_rx, lease_failure_tx);

		let mut body_stream = body.into_data_stream();
		let mut pending = BytesMut::new();
		let mut file_hash = Sha256::new();
		let mut block_tasks = FuturesUnordered::new();
		let mut committed_blocks = Vec::with_capacity(plan.blocks.len());

		for assignment in &plan.blocks {
			let lease_message = lease_failure_rx.borrow().clone();
			if let Some(message) = lease_message {
				self.abort(&user_id, &path, &plan).await;
				if let Some(sender) = cancel_tx.take() {
					let _ = sender.send(());
				}
				return Err(UploadError::Lease(message));
			}
			let block_size = assignment.size as usize;
			while pending.len() < block_size {
				let Some(chunk) = body_stream.next().await else {
					self.abort(&user_id, &path, &plan).await;
					if let Some(sender) = cancel_tx.take() {
						let _ = sender.send(());
					}
					return Err(UploadError::InvalidBodySize);
				};
				let chunk = match chunk {
					Ok(chunk) => chunk,
					Err(error) => {
						self.abort(&user_id, &path, &plan).await;
						if let Some(sender) = cancel_tx.take() {
							let _ = sender.send(());
						}
						return Err(UploadError::Body(error.to_string()));
					}
				};
				pending.extend_from_slice(&chunk);
			}

			let block = pending.split_to(block_size).freeze();
			file_hash.update(&block);
			let block_hash = BlockSender::hash(&block);
			let service = self.clone();
			let user_id_for_task = user_id.clone();
			let plan_for_task = plan.clone();
			block_tasks.push(async move {
				service
					.send_block_with_retry(
						&user_id_for_task,
						&plan_for_task,
						assignment.clone(),
						block,
						block_hash,
					)
					.await
			});

			if block_tasks.len() >= MAX_IN_FLIGHT_BLOCKS {
				match block_tasks.next().await {
					Some(Ok(committed)) => committed_blocks.push(committed),
					Some(Err(error)) => {
						self.abort(&user_id, &path, &plan).await;
						if let Some(sender) = cancel_tx.take() {
							let _ = sender.send(());
						}
						return Err(error);
					}
					None => {
						self.abort(&user_id, &path, &plan).await;
						if let Some(sender) = cancel_tx.take() {
							let _ = sender.send(());
						}
						return Err(UploadError::Worker(WorkerUploadError::InvalidAssignment));
					}
				}
			}
		}

		while let Some(result) = block_tasks.next().await {
			match result {
				Ok(committed) => committed_blocks.push(committed),
				Err(error) => {
					self.abort(&user_id, &path, &plan).await;
					if let Some(sender) = cancel_tx.take() {
						let _ = sender.send(());
					}
					return Err(error);
				}
			}
		}

		if !pending.is_empty() || body_stream.next().await.is_some() {
			self.abort(&user_id, &path, &plan).await;
			if let Some(sender) = cancel_tx.take() {
				let _ = sender.send(());
			}
			return Err(UploadError::InvalidBodySize);
		}

			let lease_message = lease_failure_rx.borrow().clone();
		if let Some(message) = lease_message {
			self.abort(&user_id, &path, &plan).await;
			if let Some(sender) = cancel_tx.take() {
				let _ = sender.send(());
			}
			return Err(UploadError::Lease(message));
		}

		committed_blocks.sort_by_key(|block| block.idx);
		let response = self
			.master
			.commit(CommitUploadRequest {
				user_id,
				path,
				inode_id: plan.inode_id.clone(),
				version_no: plan.version_no,
				lease_id: plan.lease_id.clone(),
				size,
				sha256: file_hash.finalize().to_vec(),
				blocks: committed_blocks,
			})
			.await;
		if let Some(sender) = cancel_tx.take() {
			let _ = sender.send(());
		}
		Ok(response?)
	}

	fn spawn_lease_renewer(
		&self,
		user_id: &str,
		path: &str,
		plan: &UploadPlan,
		mut cancel_rx: oneshot::Receiver<()>,
		failure_tx: watch::Sender<Option<String>>,
	) {
		let master = self.master.clone();
		let user_id = user_id.to_string();
		let path = path.to_string();
		let lease_id = plan.lease_id.clone();
		tokio::spawn(async move {
			let mut interval = tokio::time::interval(std::time::Duration::from_secs(
				LEASE_RENEW_INTERVAL_SECONDS,
			));
			loop {
				tokio::select! {
					_ = &mut cancel_rx => break,
					_ = interval.tick() => {
						let result = master.renew(LeaseRequest {
							user_id: user_id.clone(),
							path: path.clone(),
							ttl_seconds: LEASE_TTL_SECONDS,
							lease_id: lease_id.clone(),
						}).await;
						if let Err(error) = result {
							let _ = failure_tx.send(Some(error.message().to_string()));
							break;
						}
					}
				}
			}
		});
	}

	async fn send_block_with_retry(
		&self,
		user_id: &str,
		plan: &UploadPlan,
		mut assignment: BlockAssignment,
		block: Bytes,
		sha256: Vec<u8>,
	) -> Result<CommittedBlock, UploadError> {
		for attempt in 0..=1 {
			match self.workers.send(&assignment, block.clone(), sha256.clone()).await {
				Ok(committed) => return Ok(committed),
				Err(_error) if attempt == 0 => {
					let excluded_worker_ids = assignment
						.workers
						.iter()
						.map(|worker| worker.worker_id.clone())
						.collect();
					assignment = self
						.master
						.reassign(ReassignBlockRequest {
							user_id: user_id.to_string(),
							inode_id: plan.inode_id.clone(),
							version_no: plan.version_no,
							lease_id: plan.lease_id.clone(),
							idx: assignment.idx,
							exclude_worker_ids: excluded_worker_ids,
						})
						.await?;
				}
				Err(error) => return Err(UploadError::Worker(error)),
			}
		}
		Err(UploadError::Worker(WorkerUploadError::InvalidAssignment))
	}

	async fn abort(&self, user_id: &str, path: &str, plan: &UploadPlan) {
		let _ = self
			.master
			.abort(AbortUploadRequest {
				user_id: user_id.to_string(),
				path: path.to_string(),
				inode_id: plan.inode_id.clone(),
				version_no: plan.version_no,
				lease_id: plan.lease_id.clone(),
			})
			.await;
	}
}