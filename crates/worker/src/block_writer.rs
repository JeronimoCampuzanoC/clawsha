use crate::error::StorageError;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct BlockWriter {
    file: File,
    tmp_path: PathBuf,
    final_path: PathBuf,
    hasher: Sha256,
    expected_size: u64,
    expected_sha256: [u8; 32],
    written: u64,
    finished: bool,
    used_bytes: Arc<AtomicU64>,
}

impl BlockWriter {
    pub(crate) fn new(
        file: File,
        tmp_path: PathBuf,
        final_path: PathBuf,
        expected_size: u64,
        expected_sha256: [u8; 32],
        used_bytes: Arc<AtomicU64>,
    ) -> Self {
        Self {
            file,
            tmp_path,
            final_path,
            hasher: Sha256::new(),
            expected_size,
            expected_sha256,
            written: 0,
            finished: false,
            used_bytes,
        }
    }

    /// Appends one chunk to the temp file and feeds it to the running hash.
    pub fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        let new_total = self.written + chunk.len() as u64;
        if new_total > self.expected_size {
            return Err(StorageError::SizeMismatch {
                expected: self.expected_size,
                actual: new_total,
            });
        }

        self.file.write_all(chunk)?;
        self.hasher.update(chunk);
        self.written = new_total;
        Ok(())
    }

    /// Verifies size and hash, makes the data durable and publishes the block.
    pub fn finish(mut self) -> Result<(), StorageError> {
        if self.written != self.expected_size {
            return Err(StorageError::SizeMismatch {
                expected: self.expected_size,
                actual: self.written,
            });
        }

        let digest = self.hasher.clone().finalize();
        if digest[..] != self.expected_sha256[..] {
            return Err(StorageError::HashMismatch);
        }

        // Force the bytes onto the disk before the block becomes visible.
        self.file.sync_all()?;

        let parent = self
            .final_path
            .parent()
            .expect("final path always has a parent directory");
        fs::create_dir_all(parent)?;

        // Another writer may have published the same block in the meantime.
        if self.final_path.exists() {
            return Err(StorageError::AlreadyExists);
        }

        // Atomic: the block appears complete or not at all.
        fs::rename(&self.tmp_path, &self.final_path)?;
        self.finished = true;

        // Make the new directory entry durable too.
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

impl Drop for BlockWriter {
    fn drop(&mut self) {
        // If we never finished, remove the temp file and give the space back.
        if !self.finished {
            let _ = fs::remove_file(&self.tmp_path);
            self.used_bytes
                .fetch_sub(self.expected_size, Ordering::SeqCst);
        }
    }
}
