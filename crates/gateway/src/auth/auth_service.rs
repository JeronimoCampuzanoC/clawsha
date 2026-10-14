use tonic::Status;

use crate::grpc::MasterClient;
use super::{JwtError, JwtService};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
	#[error("master rejected login: {0}")]
	Master(#[from] Status),
	#[error("could not create JWT: {0}")]
	Jwt(#[from] JwtError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResponse {
	pub user_id: String,
	pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationResponse {
	pub user_id: String,
}

#[derive(Clone)]
pub struct AuthService {
	master: MasterClient,
	jwt: JwtService,
}

impl AuthService {
	pub fn new(master: MasterClient, jwt: JwtService) -> Self {
		Self { master, jwt }
	}

	pub fn master_client(&self) -> MasterClient {
		self.master.clone()
	}

	pub fn jwt_service(&self) -> JwtService {
		self.jwt.clone()
	}

	pub async fn login(
		&self,
		username: impl Into<String>,
		password: impl Into<String>,
	) -> Result<AuthResponse, AuthError> {
		let response = self.master.login(username.into(), password.into()).await?;

		let token = self.jwt.create_token(&response.user_id)?;

		Ok(AuthResponse {
			user_id: response.user_id,
			token,
		})
	}

	pub async fn register(
		&self,
		username: impl Into<String>,
		password: impl Into<String>,
	) -> Result<RegistrationResponse, AuthError> {
		let response = self
			.master
			.register(username.into(), password.into())
			.await?;

		Ok(RegistrationResponse {
			user_id: response.user_id,
		})
	}
}