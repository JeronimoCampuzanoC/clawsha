use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("invalid block id: {0}")]
    InvalidBlockId(#[from] uuid::Error),

    #[error("block already exists")]
    AlreadyExists,

    #[error("block not found")]
    NotFound,

    #[error("sha256 mismatch")]
    HashMismatch,

    #[error("size mismatch: expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("offset {offset} is past the end of the block ({size} bytes)")]
    OffsetOutOfRange { offset: u64, size: u64 },

    #[error("storage quota exceeded")]
    QuotaExceeded,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("checksum file missing or malformed")]
    ChecksumUnavailable,
}
