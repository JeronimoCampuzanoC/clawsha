use thiserror::Error;
use tonic::Status;

/// Everything the master can refuse to do, and why. The gateway only sees the
/// gRPC status this maps to (see the table in proto/dfs.proto).
#[derive(Debug, Error)]
pub enum MasterError {
    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not found")]
    NotFound,

    #[error("already exists")]
    AlreadyExists,

    #[error("directory is not empty")]
    DirNotEmpty,

    #[error("path is locked by another writer")]
    Locked,

    #[error("wrong username or password")]
    Unauthenticated,

    #[error("this master is not the leader")]
    NotLeader,

    #[error("lost leadership: another master took the lease")]
    Fenced,

    #[error("not enough workers: {0}")]
    NoWorkers(String),

    #[error("block {0} has no live replica")]
    NoReplica(String),

    #[error("dynamodb: {0}")]
    Db(String),
}

impl From<MasterError> for Status {
    fn from(e: MasterError) -> Status {
        use MasterError::*;
        let msg = e.to_string();
        match e {
            InvalidPath(_) | InvalidArgument(_) => Status::invalid_argument(msg),
            NotFound => Status::not_found(msg),
            AlreadyExists => Status::already_exists(msg),
            DirNotEmpty | Locked => Status::failed_precondition(msg),
            Unauthenticated => Status::unauthenticated(msg),
            NotLeader | Fenced | NoWorkers(_) | NoReplica(_) => Status::unavailable(msg),
            Db(_) => Status::internal(msg),
        }
    }
}
