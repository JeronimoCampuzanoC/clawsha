use std::fs::{self, File};
use std::sync::Arc;

use super::Storage;
use crate::block_writer::BlockWriter;
use crate::error::StorageError;

impl Storage {
    /// Starts writing a new block. Fails if the block already exists.
    pub fn begin_write(
        &self,
        block_id: &str,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<BlockWriter, StorageError> {
        self.begin(block_id, expected_size, expected_sha256, false)
    }

    /// Like begin_write, but allowed to replace an existing block.
    /// Used to repair a corrupted replica.
    pub fn begin_overwrite(
        &self,
        block_id: &str,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<BlockWriter, StorageError> {
        self.begin(block_id, expected_size, expected_sha256, true)
    }

    // Validates the id, checks duplicates, reserves quota and opens a temp file.
    fn begin(
        &self,
        block_id: &str,
        expected_size: u64,
        expected_sha256: [u8; 32],
        overwrite: bool,
    ) -> Result<BlockWriter, StorageError> {
        let final_path = self.block_path(block_id)?;
        if !overwrite && final_path.exists() {
            return Err(StorageError::AlreadyExists);
        }

        let tmp_dir = self.data_dir.join("tmp");
        fs::create_dir_all(&tmp_dir)?;

        let name = final_path
            .file_name()
            .expect("block path always ends with a file name");
        let tmp_path = tmp_dir.join(name).with_extension("tmp");

        let file = File::create_new(&tmp_path)?;

        // Reserve the space; if it does not fit, undo the temp file.
        if let Err(e) = self.reserve(expected_size) {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        Ok(BlockWriter::new(
            file,
            tmp_path,
            final_path,
            expected_size,
            expected_sha256,
            Arc::clone(&self.used_bytes),
            overwrite,
        ))
    }
}
