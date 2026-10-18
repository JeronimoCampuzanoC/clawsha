use std::sync::atomic::{AtomicU64, Ordering};

use tonic::Status;

/// The highest fencing token this worker has seen.
#[derive(Default)]
pub struct Fencing {
    highest: AtomicU64,
}

impl Fencing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> u64 {
        self.highest.load(Ordering::SeqCst)
    }

    pub fn check(&self, token: u64) -> Result<(), Status> {
        let previous = self.highest.fetch_max(token, Ordering::SeqCst);

        if token < previous {
            return Err(Status::failed_precondition(format!(
                "stale fencing token {token}: the current one is {previous}"
            )));
        }
        Ok(())
    }
}
