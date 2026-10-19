use tonic::Status;

use crate::{
    grpc::MasterClient,
    pb::{
        AbortUploadRequest, BlockAssignment, CommitUploadRequest, Empty, FileInfo, Lease,
        LeaseRequest, StartUploadRequest, UploadPlan,
    },
};

#[derive(Clone)]
pub struct UploadMaster {
    client: MasterClient,
}

impl UploadMaster {
    pub fn new(client: MasterClient) -> Self {
        Self { client }
    }

    pub async fn mkdir(&self, user_id: String, path: String) -> Result<Empty, Status> {
        self.client.mkdir(user_id, path).await
    }

    pub async fn start(
        &self,
        user_id: String,
        path: String,
        size: u64,
    ) -> Result<UploadPlan, Status> {
        self.client
            .start_upload(StartUploadRequest {
                user_id,
                path,
                size,
                lease_id: String::new(),
            })
            .await
    }

    pub async fn reassign(&self, request: ReassignBlockRequest) -> Result<BlockAssignment, Status> {
        self.client.reassign_block(request).await
    }

    pub async fn commit(&self, request: CommitUploadRequest) -> Result<FileInfo, Status> {
        self.client.commit_upload(request).await
    }

    pub async fn abort(&self, request: AbortUploadRequest) -> Result<(), Status> {
        self.client.abort_upload(request).await.map(|_| ())
    }

    pub async fn renew(&self, request: LeaseRequest) -> Result<Lease, Status> {
        self.client.acquire_lease(request).await
    }
}

use crate::pb::ReassignBlockRequest;
