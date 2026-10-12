use tonic::Status;

use super::StorageError;

/// Translates storage errors into gRPC status codes. This is the only place
/// where that decision is made, so `?` converts automatically.
impl From<StorageError> for Status {
    fn from(err: StorageError) -> Self {
        let message = err.to_string();

        match err {
            StorageError::InvalidBlockId(_) => Status::invalid_argument(message),
            StorageError::SizeMismatch { .. } => Status::invalid_argument(message),
            StorageError::NotFound => Status::not_found(message),
            StorageError::AlreadyExists => Status::already_exists(message),
            StorageError::HashMismatch => Status::data_loss(message),
            StorageError::ChecksumUnavailable => Status::data_loss(message),
            StorageError::OffsetOutOfRange { .. } => Status::out_of_range(message),
            StorageError::QuotaExceeded => Status::resource_exhausted(message),
            StorageError::Io(_) => Status::internal(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    #[test]
    fn not_found_becomes_not_found() {
        let status = Status::from(StorageError::NotFound);
        assert_eq!(status.code(), Code::NotFound);
    }

    #[test]
    fn corrupted_block_becomes_data_loss() {
        let status = Status::from(StorageError::HashMismatch);
        assert_eq!(status.code(), Code::DataLoss);
    }

    #[test]
    fn full_quota_becomes_resource_exhausted() {
        let status = Status::from(StorageError::QuotaExceeded);
        assert_eq!(status.code(), Code::ResourceExhausted);
    }
}
