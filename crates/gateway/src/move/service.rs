use tonic::Status;

use crate::grpc::MasterClient;

/// Stateless wrapper over MasterClient for the move/rename path. Mirrors
/// DeleteService and ReadService: the master's Move takes an absolute source
/// and an absolute destination (including the new name), so it covers both
/// renaming in place and moving across directories.
#[derive(Clone)]
pub struct MoveService {
	master: MasterClient,
}

impl MoveService {
	pub fn new(master: MasterClient) -> Self {
		Self { master }
	}

	pub async fn move_entry(
		&self,
		user_id: String,
		from: String,
		to: String,
	) -> Result<(), Status> {
		self.master.move_entry(user_id, from, to).await?;
		Ok(())
	}
}
