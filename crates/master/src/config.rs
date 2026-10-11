use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "master", about = "DFSha master (NameNode)")]
pub struct Config {
    /// Unique id of this master, e.g. "master-a". Shows up in the leader lease.
    #[arg(long, env = "MASTER_ID")]
    pub id: String,

    /// Availability zone this master runs in.
    #[arg(long, env = "MASTER_AZ", default_value = "local")]
    pub az: String,

    /// Address gateways and workers use to reach this master's gRPC port.
    /// Stored in the leader lease so the dashboard can show who is leading.
    #[arg(long, env = "MASTER_ADVERTISE_ADDR", default_value = "127.0.0.1:50051")]
    pub advertise_addr: String,

    /// gRPC listen port (MasterService + ClusterService).
    #[arg(long, env = "MASTER_PORT", default_value_t = 50051)]
    pub port: u16,

    /// HTTP port for /health and /health/leader (the NLB and the ASG poll these).
    #[arg(long, env = "MASTER_HEALTH_PORT", default_value_t = 8080)]
    pub health_port: u16,

    /// Block size in bytes (default 16 MiB). Every upload plan uses this value.
    #[arg(long, env = "MASTER_BLOCK_SIZE", default_value_t = 16 * 1024 * 1024)]
    pub block_size: u64,

    /// Prefix for every DynamoDB table name, e.g. "dfsha_" -> dfsha_users.
    #[arg(long, env = "MASTER_TABLE_PREFIX", default_value = "dfsha_")]
    pub table_prefix: String,

    /// DynamoDB endpoint override. Set it to http://localhost:8000 for
    /// dynamodb-local; leave it empty on AWS to use the real service.
    #[arg(long, env = "MASTER_DYNAMODB_ENDPOINT")]
    pub dynamodb_endpoint: Option<String>,

    /// Create the tables at startup if they do not exist. Handy with
    /// dynamodb-local; on AWS Terraform owns the tables, so leave it off.
    #[arg(long, env = "MASTER_CREATE_TABLES", default_value_t = false)]
    pub create_tables: bool,
}
