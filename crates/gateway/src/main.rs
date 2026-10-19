mod auth;
mod delete;
mod download;
mod encript;
mod grpc;
mod r#move;
mod read;
mod router;
mod state;
mod upload;

pub mod pb {
    tonic::include_proto!("dfs.v1");
}

use tokio::net::TcpListener;

use auth::{AuthService, JwtService};
use encript::CipherService;
use grpc::MasterClient;
use state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gateway=info".into()),
        )
        .init();

    let master_address =
        std::env::var("MASTER_ADDRESS").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());
    let master = MasterClient::connect(master_address)?;

    let jwt_secret = std::env::var("JWT_SECRET").map_err(|_| "JWT_SECRET must be configured")?;
    let jwt = JwtService::new(jwt_secret)?;
    let auth_service = AuthService::new(master, jwt);

    let encryption_key = std::env::var("GATEWAY_ENCRYPTION_KEY")
        .map_err(|_| "GATEWAY_ENCRYPTION_KEY must be configured (32 bytes, hex or base64)")?;
    let cipher = CipherService::new(&encryption_key)?;

    let state = AppState::new(auth_service, cipher);

    let address = std::env::var("GATEWAY_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let listener = TcpListener::bind(&address).await?;
    tracing::info!(address = ?listener.local_addr()?, "gateway listening");

    axum::serve(listener, router::router(state)).await?;
    Ok(())
}
