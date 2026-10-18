use tonic::Status;

use crate::{
	grpc::MasterClient,
	pb::{CorruptionReport, ReadPlan, ReadPlanRequest},
};

/// Thin wrapper over MasterClient for the read path, mirroring UploadMaster.
#[derive(Clone)]
pub struct DownloadMaster {
	client: MasterClient,
}

impl DownloadMaster {
	pub fn new(client: MasterClient) -> Self {
		Self { client }
	}

	/// Asks the master where a file's blocks live. version_no 0 means the
	/// current version.
	pub async fn read_plan(
		&self,
		user_id: String,
		path: String,
	) -> Result<ReadPlan, Status> {
		self.client
			.get_read_plan(ReadPlanRequest {
				user_id,
				path,
				version_no: 0,
			})
			.await
	}

	/// Tells the master that a replica served a block whose hash did not
	/// match, so it stops handing out that copy. Best effort: the caller does
	/// not block the download on the outcome.
	pub async fn report_corruption(&self, block_id: String, worker_id: String) {
		let _ = self
			.client
			.report_corruption(CorruptionReport {
				block_id,
				worker_id,
			})
			.await;
	}
}
