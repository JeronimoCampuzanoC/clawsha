use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "worker", about = "DFSha worker (DataNode)")]
pub struct Config {
    /// Unique id of this worker.
    #[arg(long, env = "WORKER_ID")]
    pub id: String,

    /// Availability zone this worker runs in.
    #[arg(long, env = "WORKER_AZ")]
    pub az: String,

    /// gRPC listen port.
    #[arg(long, env = "WORKER_PORT", default_value_t = 50052)]
    pub port: u16,

    /// Directory that holds blocks/ and tmp/.
    #[arg(long, env = "WORKER_DATA_DIR", default_value = "/data")]
    pub data_dir: PathBuf,

    /// Storage quota in bytes (default 2 GiB).
    #[arg(long, env = "WORKER_QUOTA_BYTES", default_value_t = 2_147_483_648)]
    pub quota_bytes: u64,
}