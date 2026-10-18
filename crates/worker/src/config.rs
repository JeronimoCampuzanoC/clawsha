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

    /// Address other nodes use to reach this worker (host:port).
    #[arg(long, env = "WORKER_ADVERTISE_ADDR", default_value = "127.0.0.1:50052")]
    pub advertise_addr: String,

    /// Address of the master's gRPC port (the NLB in production).
    #[arg(long, env = "MASTER_ADDR", default_value = "http://127.0.0.1:50051")]
    pub master_addr: String,

    /// Directory that holds blocks/ and tmp/.
    #[arg(long, env = "WORKER_DATA_DIR", default_value = "/data")]
    pub data_dir: PathBuf,

    /// Storage quota in bytes (default 2 GiB).
    #[arg(long, env = "WORKER_QUOTA_BYTES", default_value_t = 2_147_483_648)]
    pub quota_bytes: u64,

    /// Maximum size of a single frame in bytes (default 256 KiB, max 1 MiB).
    #[arg(long,env = "WORKER_FRAME_SIZE",default_value_t = 262_144,value_parser = clap::value_parser!(u32).range(1..=1_048_576))]
    pub frame_size: u32,

    /// Seconds between two full scrubs of the blocks on disk; 0 turns the
    /// scrubber off. Default 600 (10 minutes), meant for demos: in a real
    /// deployment this should be much longer.
    #[arg(long, env = "WORKER_SCRUB_INTERVAL_SECS", default_value_t = 600)]
    pub scrub_interval_secs: u64,
}
