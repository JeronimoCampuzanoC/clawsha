use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::Storage;
use super::fs_util::io_to_storage;
use crate::error::StorageError;

impl Storage {
    /// The SHA-256 recorded when the block was written.
    pub fn stored_checksum(&self, block_id: &str) -> Result<[u8; 32], StorageError> {
        let path = self.block_path(block_id)?;
        read_checksum(&meta_path_for(&path))
    }

    /// Recomputes the block's SHA-256 from disk and compares it with the stored one.
    pub fn verify_block(&self, block_id: &str) -> Result<(), StorageError> {
        let path = self.block_path(block_id)?;

        // Open first, so a missing block is NotFound and not ChecksumUnavailable.
        let mut file = File::open(&path).map_err(io_to_storage)?;
        let expected = read_checksum(&meta_path_for(&path))?;

        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }

        if hasher.finalize()[..] != expected[..] {
            return Err(StorageError::HashMismatch);
        }
        Ok(())
    }
}

/// Path of the checksum file that sits next to a block file.
pub(crate) fn meta_path_for(block_path: &Path) -> PathBuf {
    block_path.with_extension("meta")
}

/// Reads the 32-byte checksum stored in a .meta file.
fn read_checksum(meta: &Path) -> Result<[u8; 32], StorageError> {
    let bytes = match fs::read(meta) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(StorageError::ChecksumUnavailable);
        }
        Err(e) => return Err(e.into()),
    };
    bytes
        .try_into()
        .map_err(|_| StorageError::ChecksumUnavailable)
}
