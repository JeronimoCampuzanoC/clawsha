use crate::block_writer::BlockWriter;
use crate::error::StorageError;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use uuid::Uuid;

/// A finished block found on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    pub block_id: Uuid,
    pub size: u64,
}

pub struct Storage {
    pub data_dir: PathBuf,
    quota_bytes: u64,
    used_bytes: Arc<AtomicU64>,
}

impl Storage {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            quota_bytes: u64::MAX,
            used_bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    // Returns <data_dir>/blocks/ab/cd/<uuid>, or an error if the id is not a valid UUID.
    pub fn block_path(&self, block_id: &str) -> Result<PathBuf, uuid::Error> {
        let id = Uuid::parse_str(block_id)?;
        let name = id.hyphenated().to_string();

        Ok(self
            .data_dir
            .join("blocks")
            .join(&name[0..2])
            .join(&name[2..4])
            .join(&name))
    }

    // Starts writing a block: validates the id, refuses duplicates, reserves quota and opens a temp file.
    pub fn begin_write(
        &self,
        block_id: &str,
        expected_size: u64,
        expected_sha256: [u8; 32],
    ) -> Result<BlockWriter, StorageError> {
        let final_path = self.block_path(block_id)?;
        if final_path.exists() {
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
        ))
    }

    // Opens a block for reading, positioned at `offset`.
    pub fn open_block(&self, block_id: &str, offset: u64) -> Result<File, StorageError> {
        let path = self.block_path(block_id)?;

        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Err(e) => return Err(e.into()),
        };

        let size = file.metadata()?.len();
        if offset > size {
            return Err(StorageError::OffsetOutOfRange { offset, size });
        }

        file.seek(SeekFrom::Start(offset))?;
        Ok(file)
    }

    pub fn delete_block(&self, block_id: &str) -> Result<u64, StorageError> {
        let path = self.block_path(block_id)?;

        let size = fs::metadata(&path).map_err(io_to_storage)?.len();
        fs::remove_file(&path).map_err(io_to_storage)?;
        self.used_bytes.fetch_sub(size, Ordering::SeqCst);

        Ok(size)
    }

    // Removes empty shard directories under blocks/ and returns how many were removed.
    // Safe to call while writes are in progress: remove_dir refuses non-empty
    // directories, and BlockWriter::finish retries if its directory disappears.
    pub fn prune_empty_dirs(&self) -> Result<usize, StorageError> {
        let blocks_dir = self.data_dir.join("blocks");

        let outer_entries = match fs::read_dir(&blocks_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
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

    /// Lists every complete block on disk (temp files are never included).
    pub fn list_blocks(&self) -> Result<Vec<BlockInfo>, StorageError> {
        let mut found = Vec::new();

        let Some(outer_entries) = read_dir_if_exists(&self.data_dir.join("blocks"))? else {
            return Ok(found);
        };

        for outer in outer_entries {
            let outer_path = outer?.path();
            if !outer_path.is_dir() {
                continue;
            }
            let Some(inner_entries) = read_dir_if_exists(&outer_path)? else {
                continue;
            };

            for inner in inner_entries {
                let inner_path = inner?.path();
                if !inner_path.is_dir() {
                    continue;
                }
                let Some(block_entries) = read_dir_if_exists(&inner_path)? else {
                    continue;
                };

                for entry in block_entries {
                    let entry = entry?;

                    // Only accept names in the exact form block_path produces.
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    let Ok(id) = Uuid::parse_str(name) else {
                        continue;
                    };
                    if id.hyphenated().to_string() != name {
                        continue;
                    }

                    // The block may be deleted while we are walking.
                    let meta = match entry.metadata() {
                        Ok(meta) => meta,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e.into()),
                    };
                    if meta.is_file() {
                        found.push(BlockInfo {
                            block_id: id,
                            size: meta.len(),
                        });
                    }
                }
            }
        }

        Ok(found)
    }

    pub fn with_quota(mut self, quota_bytes: u64) -> Self {
        self.quota_bytes = quota_bytes;
        self
    }

    /// Startup: clean leftovers, then count what is really on disk.
    pub fn open(data_dir: PathBuf, quota_bytes: u64) -> Result<Self, StorageError> {
        let storage = Self::new(data_dir).with_quota(quota_bytes);
        storage.cleanup_tmp()?;
        storage.prune_empty_dirs()?;

        let used: u64 = storage.list_blocks()?.iter().map(|b| b.size).sum();
        storage.used_bytes.store(used, Ordering::SeqCst);
        Ok(storage)
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes.load(Ordering::SeqCst)
    }

    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    /// Atomically adds `size` to the used counter, or fails if it would pass the quota.
    fn reserve(&self, size: u64) -> Result<(), StorageError> {
        self.used_bytes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(size)
                    .filter(|total| *total <= self.quota_bytes)
            })
            .map(|_| ())
            .map_err(|_| StorageError::QuotaExceeded)
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

/// read_dir that returns None when the directory does not exist.
fn read_dir_if_exists(path: &std::path::Path) -> Result<Option<fs::ReadDir>, StorageError> {
    match fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn io_to_storage(e: std::io::Error) -> StorageError {
    if e.kind() == std::io::ErrorKind::NotFound {
        StorageError::NotFound
    } else {
        StorageError::Io(e)
    }
}
