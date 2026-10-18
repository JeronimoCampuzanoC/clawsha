use tonic::Status;
use tonic::transport::{Channel, Endpoint};

use crate::pb::{
	LoginRequest,
	LoginResponse,
	ListResponse,
	PathRequest,
	RegisterRequest,
	RegisterResponse,
	RemoveRequest,
	AbortUploadRequest,
	BlockAssignment,
	CommitUploadRequest,
	CorruptionReport,
	Empty,
	FileInfo,
	Lease,
	LeaseRequest,
	MoveRequest,
	ReadPlan,
	ReadPlanRequest,
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
	/// Builds a client without opening a TCP connection yet. The channel
	/// connects on the first RPC and reconnects on its own if the master goes
	/// away, so the gateway can start before the master is up and survive a
	/// master restart or failover. The per-RPC timeout keeps a call from
	/// hanging forever while the master is unreachable; the caller sees it as
	/// a transport error and maps it to 503.
	pub fn connect(address: impl Into<String>) -> Result<Self, tonic::transport::Error> {
		let endpoint = Endpoint::from_shared(address.into())?
			.connect_timeout(std::time::Duration::from_secs(5))
			.timeout(std::time::Duration::from_secs(30))
			.tcp_keepalive(Some(std::time::Duration::from_secs(30)));
		let channel = endpoint.connect_lazy();

		Ok(Self {
			client: MasterServiceClient::new(channel),
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

	pub async fn get_read_plan(
		&self,
		request: ReadPlanRequest,
	) -> Result<ReadPlan, Status> {
		let mut client = self.client.clone();
		let response = client.get_read_plan(request).await?;

		Ok(response.into_inner())
	}

	pub async fn report_corruption(
		&self,
		request: CorruptionReport,
	) -> Result<Empty, Status> {
		let mut client = self.client.clone();
		let response = client.report_corruption(request).await?;

		Ok(response.into_inner())
	}

	pub async fn mkdir(
		&self,
		user_id: String,
		path: String,
	) -> Result<Empty, Status> {
		let mut client = self.client.clone();
		let response = client.mkdir(PathRequest { user_id, path }).await?;

		Ok(response.into_inner())
	}

	pub async fn remove(
		&self,
		user_id: String,
		path: String,
		recursive: bool,
	) -> Result<Empty, Status> {
		let mut client = self.client.clone();
		let response = client
			.remove(RemoveRequest {
				user_id,
				path,
				recursive,
			})
			.await?;

		Ok(response.into_inner())
	}

	pub async fn move_entry(
		&self,
		user_id: String,
		from: String,
		to: String,
	) -> Result<Empty, Status> {
		let mut client = self.client.clone();
		let response = client
			.r#move(MoveRequest { user_id, from, to })
			.await?;

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
