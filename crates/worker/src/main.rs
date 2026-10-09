mod block_writer;
mod config;
mod error;
mod storage;

use clap::Parser;

use config::Config;
use storage::Storage;

fn main() -> Result<(), error::StorageError> {
    // Load .env if it exists; real environment variables and CLI args still win.
    dotenvy::dotenv().ok();

    let config = Config::parse();

    let storage = Storage::open(config.data_dir.clone(), config.quota_bytes)?;

    println!(
        "worker {} (az {}) ready on port {}: {} of {} bytes used",
        config.id,
        config.az,
        config.port,
        storage.used_bytes(),
        storage.quota_bytes()
    );
    Ok(())
}