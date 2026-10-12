use std::time::{SystemTime, UNIX_EPOCH};

/// Unix time in milliseconds. Every timestamp the master stores or compares
/// (leases, heartbeats, modified_at) uses this unit.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before 1970")
        .as_millis() as u64
}
