mod block_writer;
mod changes;
mod checksum;
mod fs_util;
mod listing;
mod maintenance;
mod quarantine;
mod quota;
mod read;
mod write;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use uuid::Uuid;

use crate::error::StorageError;

pub(crate) use block_writer::BlockWriter;
pub(crate) use changes::SharedChanges;
pub(crate) use checksum::meta_path_for;
#[allow(unused_imports)]
pub use changes::Changes;
// Part of the public API; only used by tests until the BlockReport is built.
#[allow(unused_imports)]
pub use listing::BlockInfo;

pub struct Storage {
    pub data_dir: PathBuf,
    quota_bytes: u64,
    used_bytes: Arc<AtomicU64>,
    changes: SharedChanges,
}

impl Storage {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            quota_bytes: u64::MAX,
            used_bytes: Arc::new(AtomicU64::new(0)),
            changes: SharedChanges::default(),
        }
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

    // Returns <data_dir>/blocks/ab/cd/<uuid>, or an error if the id is not a valid UUID.
    pub fn block_path(&self, block_id: &str) -> Result<PathBuf, StorageError> {
        let id = Uuid::parse_str(block_id)?;
        let name = id.hyphenated().to_string();

        Ok(self
            .data_dir
            .join("blocks")
            .join(&name[0..2])
            .join(&name[2..4])
            .join(&name))
    }
}
