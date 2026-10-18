mod cluster;
mod config;
mod error;
mod fencing;
mod service;
mod storage;
#[cfg(test)]
mod tests;

pub mod pb {
    tonic::include_proto!("dfs.v1");
}

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

use config::Config;
use pb::worker_service_server::WorkerServiceServer;
use service::WorkerSvc;
use storage::Storage;

/// Resolves when the process receives Ctrl+C or SIGTERM (what Docker sends).
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = signal(SignalKind::terminate()).expect("cannot listen for SIGTERM");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }

    info!("shutting down, waiting for in-flight requests...");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::parse();
    let storage = Storage::open(config.data_dir.clone(), config.quota_bytes)?;

    info!(
        worker = %config.id,
        az = %config.az,
        used_bytes = storage.used_bytes(),
        quota_bytes = storage.quota_bytes(),
        frame_size = config.frame_size,
        "worker starting"
    );

    let storage = Arc::new(storage);
    let service = WorkerSvc::new(Arc::clone(&storage), config.frame_size as usize);

    tokio::spawn(cluster::run(config.clone(), storage, service.fencing()));

    let address = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(%address, "listening");

    Server::builder()
        .add_service(WorkerServiceServer::new(service))
        .serve_with_shutdown(address, shutdown_signal())
        .await?;

    info!("worker stopped");
    Ok(())
}
