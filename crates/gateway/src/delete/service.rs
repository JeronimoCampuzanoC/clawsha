use tonic::Status;

use crate::grpc::MasterClient;

/// Stateless wrapper over MasterClient for the delete path. Mirrors
/// ReadService: the master's Remove deletes a file or a directory; a
/// non-empty directory requires `recursive = true`, otherwise the master
/// answers FAILED_PRECONDITION (409).
#[derive(Clone)]
pub struct DeleteService {
    master: MasterClient,
}

impl DeleteService {
    pub fn new(master: MasterClient) -> Self {
        Self { master }
    }

    pub async fn remove(
        &self,
        user_id: String,
        path: String,
        recursive: bool,
    ) -> Result<(), Status> {
        self.master.remove(user_id, path, recursive).await?;
        Ok(())
    }
}
