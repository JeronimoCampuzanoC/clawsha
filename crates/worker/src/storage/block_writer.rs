use crate::error::StorageError;
use super::changes::{SharedChanges, lock};
use crate::storage::meta_path_for;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
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
    changes: SharedChanges,
    overwrite: bool,
}

impl BlockWriter {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        file: File,
        tmp_path: PathBuf,
        final_path: PathBuf,
        expected_size: u64,
        expected_sha256: [u8; 32],
        used_bytes: Arc<AtomicU64>,
        changes: SharedChanges,
        overwrite: bool,
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
            changes,
            overwrite,
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

    /// Creates the shard directory, writes the .meta first and then publishes the block.
    fn publish(&self, parent: &Path, meta_path: &Path) -> std::io::Result<()> {
        fs::create_dir_all(parent)?;
        write_meta(meta_path, &self.expected_sha256)?;
        fs::rename(&self.tmp_path, &self.final_path)
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

        self.file.sync_all()?;

        let parent = self
            .final_path
            .parent()
            .expect("final path always has a parent directory");

        // A normal write refuses to replace a block; a repair is allowed to.
        let replaced_size = if self.overwrite {
            fs::metadata(&self.final_path).map(|m| m.len()).unwrap_or(0)
        } else {
            if self.final_path.exists() {
                return Err(StorageError::AlreadyExists);
            }
            0
        };

        // The shard directory can be pruned between create_dir_all and the
        // write/rename, so retry a few times.
        let meta_path = meta_path_for(&self.final_path);
        let mut attempts = 0;
        loop {
            match self.publish(parent, &meta_path) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempts < 3 => {
                    attempts += 1;
                }
                Err(e) => {
                    let _ = fs::remove_file(&meta_path);
                    return Err(e.into());
                }
            }
        }
        self.finished = true;

        // The old copy is gone: give its space back.
        self.used_bytes.fetch_sub(replaced_size, Ordering::SeqCst);

        File::open(parent)?.sync_all()?;

        if let Some(name) = self.final_path.file_name().and_then(|n| n.to_str()) {
            lock(&self.changes).new_blocks.push(name.to_string());
        }
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

fn write_meta(path: &Path, sha256: &[u8; 32]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(sha256)?;
    file.sync_all()
}
