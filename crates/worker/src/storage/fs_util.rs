use std::fs;
use std::path::Path;

use crate::error::StorageError;

/// read_dir that returns None when the directory does not exist.
pub(super) fn read_dir_if_exists(path: &Path) -> Result<Option<fs::ReadDir>, StorageError> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Like `From<io::Error>`, but a missing file becomes `NotFound`.
pub(super) fn io_to_storage(e: std::io::Error) -> StorageError {
    if e.kind() == std::io::ErrorKind::NotFound {
        StorageError::NotFound
    } else {
        StorageError::Io(e)
    }
}
