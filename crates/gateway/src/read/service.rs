use tonic::Status;

use crate::{grpc::MasterClient, pb::ListResponse};

#[derive(Clone)]
pub struct ReadService {
	master: MasterClient,
}

impl ReadService {
	pub fn new(master: MasterClient) -> Self {
		Self { master }
	}

	pub async fn list(
		&self,
		user_id: String,
		path: String,
	) -> Result<ListResponse, Status> {
		self.master.list(user_id, path).await
	}
}