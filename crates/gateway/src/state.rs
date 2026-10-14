use crate::{auth::AuthService, grpc::MasterClient};

#[derive(Clone)]
pub struct AppState {
	pub auth: AuthService,
	pub master: MasterClient,
}

impl AppState {
	pub fn new(auth: AuthService) -> Self {
		let master = auth.master_client();
		Self { auth, master }
	}
}
