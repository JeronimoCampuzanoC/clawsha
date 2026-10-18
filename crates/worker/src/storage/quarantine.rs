use std::fs;
use std::sync::atomic::Ordering;

use super::Storage;
use super::changes::lock;
use super::checksum::meta_path_for;
use super::fs_util::io_to_storage;
use crate::error::StorageError;

impl Storage {
    /// Takes a damaged block out of service without deleting it: the block and
    /// its checksum file move to `quarantine/`, so the next heartbeat and block
    /// report no longer list it and the master sees a missing replica. The
    /// bytes stay on disk in case the other copy is gone too.
    pub fn quarantine_block(&self, block_id: &str) -> Result<(), StorageError> {
        let path = self.block_path(block_id)?;
        let size = fs::metadata(&path).map_err(io_to_storage)?.len();

        let dir = self.data_dir.join("quarantine");
        fs::create_dir_all(&dir)?;
        let name = path
            .file_name()
            .expect("block_path always ends in the block name");
        let target = dir.join(name);

        fs::rename(&path, &target).map_err(io_to_storage)?;
        self.used_bytes.fetch_sub(size, Ordering::SeqCst);
        // The checksum file may be the damaged part, or already gone.
        let _ = fs::rename(meta_path_for(&path), meta_path_for(&target));
        lock(&self.changes)
            .removed_blocks
            .push(block_id.to_string());

        Ok(())
    }
}
