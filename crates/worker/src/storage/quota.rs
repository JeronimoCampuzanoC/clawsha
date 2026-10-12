use std::sync::atomic::Ordering;

use super::Storage;
use crate::error::StorageError;

impl Storage {
    pub fn with_quota(mut self, quota_bytes: u64) -> Self {
        self.quota_bytes = quota_bytes;
        self
    }

    pub fn used_bytes(&self) -> u64 {
        self.used_bytes.load(Ordering::SeqCst)
    }

    pub fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }

    /// Atomically adds `size` to the used counter, or fails if it would pass the quota.
    pub(super) fn reserve(&self, size: u64) -> Result<(), StorageError> {
        self.used_bytes
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(size)
                    .filter(|total| *total <= self.quota_bytes)
            })
            .map(|_| ())
            .map_err(|_| StorageError::QuotaExceeded)
    }
}
