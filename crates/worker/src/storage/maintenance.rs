use std::fs;
use std::sync::atomic::Ordering;

use super::Storage;
use super::changes::lock;
use super::checksum::meta_path_for;
use super::fs_util::{io_to_storage, read_dir_if_exists};
use crate::error::StorageError;

impl Storage {
    pub fn delete_block(&self, block_id: &str) -> Result<u64, StorageError> {
        let path = self.block_path(block_id)?;

        let size = fs::metadata(&path).map_err(io_to_storage)?.len();
        fs::remove_file(&path).map_err(io_to_storage)?;
        self.used_bytes.fetch_sub(size, Ordering::SeqCst);
        let _ = fs::remove_file(meta_path_for(&path));
        lock(&self.changes).removed_blocks.push(block_id.to_string());

        Ok(size)
    }

    // Removes empty shard directories under blocks/ and returns how many were removed.
    // Safe to call while writes are in progress: remove_dir refuses non-empty
    // directories, and BlockWriter::finish retries if its directory disappears.
    pub fn prune_empty_dirs(&self) -> Result<usize, StorageError> {
        let blocks_dir = self.data_dir.join("blocks");

        let Some(outer_entries) = read_dir_if_exists(&blocks_dir)? else {
            return Ok(0);
        };

        let mut removed = 0;
        for outer in outer_entries {
            let outer_path = outer?.path();
            if !outer_path.is_dir() {
                continue;
            }

            for inner in fs::read_dir(&outer_path)? {
                let inner_path = inner?.path();
                if inner_path.is_dir() && fs::remove_dir(&inner_path).is_ok() {
                    removed += 1;
                }
            }

            if fs::remove_dir(&outer_path).is_ok() {
                removed += 1;
            }
        }

        Ok(removed)
    }

    /// Deletes leftover temp files. Only call at startup, before accepting writes.
    pub fn cleanup_tmp(&self) -> Result<usize, StorageError> {
        let Some(entries) = read_dir_if_exists(&self.data_dir.join("tmp"))? else {
            return Ok(0);
        };

        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            if path.is_file() && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}
