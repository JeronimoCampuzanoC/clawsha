use crate::{auth::AuthService, encript::CipherService, grpc::MasterClient};

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthService,
    pub master: MasterClient,
    pub cipher: CipherService,
}

impl AppState {
    pub fn new(auth: AuthService, cipher: CipherService) -> Self {
        let master = auth.master_client();
        Self {
            auth,
            master,
            cipher,
        }
    }
}
