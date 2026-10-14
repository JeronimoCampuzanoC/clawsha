use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use crate::pb::{
	LoginRequest,
	LoginResponse,
	ListResponse,
	PathRequest,
	RegisterRequest,
	RegisterResponse,
	AbortUploadRequest,
	BlockAssignment,
	CommitUploadRequest,
	Empty,
	FileInfo,
	Lease,
	LeaseRequest,
	ReassignBlockRequest,
	StartUploadRequest,
	UploadPlan,
	master_service_client::MasterServiceClient,
};

#[derive(Clone)]
pub struct MasterClient {
	client: MasterServiceClient<Channel>,
}

impl MasterClient {
	pub async fn connect(address: impl Into<String>) -> Result<Self, tonic::transport::Error> {
		let endpoint = Endpoint::from_shared(address.into())?;
		let client = endpoint.connect().await?;

		Ok(Self {
			client: MasterServiceClient::new(client),
		})
	}

	pub async fn login(
		&self,
		username: String,
		password: String,
	) -> Result<LoginResponse, Status> {
		let mut client = self.client.clone();
		let response = client.login(LoginRequest { username, password }).await?;

		Ok(response.into_inner())
	}

	pub async fn register(
		&self,
		username: String,
		password: String,
	) -> Result<RegisterResponse, Status> {
		let mut client = self.client.clone();
		let response = client
			.register(RegisterRequest { username, password })
			.await?;

		Ok(response.into_inner())
	}

	pub async fn list(
		&self,
		user_id: String,
		path: String,
	) -> Result<ListResponse, Status> {
		let mut client = self.client.clone();
		let response = client.list(PathRequest { user_id, path }).await?;

		Ok(response.into_inner())
	}

	pub async fn start_upload(
		&self,
		request: StartUploadRequest,
	) -> Result<UploadPlan, Status> {
		let mut client = self.client.clone();
		let response = client.start_upload(request).await?;

		Ok(response.into_inner())
	}

	pub async fn reassign_block(
		&self,
		request: ReassignBlockRequest,
	) -> Result<BlockAssignment, Status> {
		let mut client = self.client.clone();
		let response = client.reassign_block(request).await?;

		Ok(response.into_inner())
	}

	pub async fn commit_upload(
		&self,
		request: CommitUploadRequest,
	) -> Result<FileInfo, Status> {
		let mut client = self.client.clone();
		let response = client.commit_upload(request).await?;

		Ok(response.into_inner())
	}

	pub async fn abort_upload(
		&self,
		request: AbortUploadRequest,
	) -> Result<Empty, Status> {
		let mut client = self.client.clone();
		let response = client.abort_upload(request).await?;

		Ok(response.into_inner())
	}

	pub async fn acquire_lease(&self, request: LeaseRequest) -> Result<Lease, Status> {
		let mut client = self.client.clone();
		let response = client.acquire_lease(request).await?;

		Ok(response.into_inner())
	}
}
