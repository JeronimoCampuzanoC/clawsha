mod app;
mod auth;
mod cluster;
mod config;
mod db;
mod error;
mod grpc;
mod health;
mod http;
mod leader;
mod namespace;
mod path;
mod pb;
mod placement;
mod read_plan;
#[cfg(test)]
mod tests;
mod time;
mod upload;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

use app::App;
use config::Config;
use db::Db;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load a .env if there is one: first from the current directory, then
    // from this crate's own folder, so `cargo run -p master` works from the
    // repository root too. Real environment variables and CLI args still win.
    dotenvy::dotenv().ok();
    dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/.env")).ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();

    let db = Db::connect(config.dynamodb_endpoint.as_deref(), &config.table_prefix)
        .await
        .context("connecting to DynamoDB")?;
    if config.create_tables {
        db.create_tables().await.context("creating tables")?;
    }

    let app = Arc::new(App::new(
        db,
        &config.id,
        &config.advertise_addr,
        config.block_size,
        config.replication,
    ));

    // The lease loop decides whether this process answers or waits.
    tokio::spawn(app.leadership.clone().run());

    // Re-replication: only acts while this process leads.
    let healer = Arc::new(health::Healer::new(
        app.cluster.clone(),
        health::GrpcWorkers,
        app.replication,
        app.block_size,
    ));
    tokio::spawn(health::run(healer, app.leadership.clone()));

    let grpc = TcpListener::bind(("0.0.0.0", config.port))
        .await
        .with_context(|| format!("binding gRPC port {}", config.port))?;
    let health = TcpListener::bind(("0.0.0.0", config.health_port))
        .await
        .with_context(|| format!("binding health port {}", config.health_port))?;

    tracing::info!(
        master = %config.id,
        az = %config.az,
        grpc = config.port,
        health = config.health_port,
        block_size = config.block_size,
        "master up, waiting for the leader lease"
    );

    tokio::select! {
        _ = grpc::serve(app.clone(), grpc) => {},
        _ = http::serve(app.clone(), health) => {},
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
        }
    }
    Ok(())
}
