use std::sync::{Arc, Mutex};

use super::Storage;

/// Blocks added or removed since the last heartbeat took the list.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub new_blocks: Vec<String>,
    pub removed_blocks: Vec<String>,
}

/// Shared between Storage and every BlockWriter it hands out.
pub(crate) type SharedChanges = Arc<Mutex<Changes>>;

pub(super) fn lock(changes: &Mutex<Changes>) -> std::sync::MutexGuard<'_, Changes> {
    // The lists stay valid even if another thread panicked while holding the lock.
    changes.lock().unwrap_or_else(|e| e.into_inner())
}

impl Storage {
    /// Hands over the pending changes and leaves the lists empty.
    pub fn take_changes(&self) -> Changes {
        std::mem::take(&mut *lock(&self.changes))
    }

    /// Puts changes back after a heartbeat failed, so the next one sends them.
    pub fn restore_changes(&self, old: Changes) {
        let mut current = lock(&self.changes);
        current.new_blocks.splice(0..0, old.new_blocks);
        current.removed_blocks.splice(0..0, old.removed_blocks);
    }
}
