//! Background check that every block on disk still matches its checksum, so a
//! block that rotted while nobody read it is found before the other replica
//! is lost too.
//!
//! Every `WORKER_SCRUB_INTERVAL_SECS` the scrubber walks all blocks, recomputes
//! each SHA-256 and compares it with the block's `.meta`. A bad block is moved
//! to `quarantine/` (so the block report stops listing it and the master sees a
//! missing replica to repair) and then reported with `ReportCorruption`.
//! Quarantining comes first because it is the part that always works: if the
//! master is unreachable, the next block report carries the same information.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{Instant, MissedTickBehavior, interval_at};
use tracing::{error, info, warn};

use crate::cluster::master_channel;
use crate::config::Config;
use crate::error::StorageError;
use crate::pb::CorruptionReport;
use crate::pb::master_service_client::MasterServiceClient;
use crate::service::blocking;
use crate::storage::Storage;

/// A block that fails is checked once more after this pause, so a block that
/// was being replaced (a repair) is not quarantined by mistake.
const RECHECK_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScrubReport {
    /// Blocks whose hash matched.
    pub checked: usize,
    /// Blocks moved to quarantine, by id.
    pub quarantined: Vec<String>,
    /// Blocks that could not be read for another reason (I/O error). They stay
    /// in place and are tried again on the next pass.
    pub unreadable: usize,
}

/// One full pass over the blocks on disk.
pub fn scrub_once(storage: &Storage, recheck_delay: Duration) -> Result<ScrubReport, StorageError> {
    let mut report = ScrubReport::default();

    for block in storage.list_blocks()? {
        let id = block.block_id.hyphenated().to_string();
        match verify_twice(storage, &id, recheck_delay) {
            Ok(()) => report.checked += 1,
            // Deleted while we were walking: nothing to check.
            Err(StorageError::NotFound) => {}
            // The data does not match, or the checksum file is gone or damaged.
            Err(StorageError::HashMismatch | StorageError::ChecksumUnavailable) => {
                match storage.quarantine_block(&id) {
                    Ok(()) => {
                        warn!(block_id = %id, "corrupt block moved to quarantine");
                        report.quarantined.push(id);
                    }
                    Err(StorageError::NotFound) => {}
                    Err(e) => {
                        error!(block_id = %id, error = %e, "could not quarantine a corrupt block");
                        report.unreadable += 1;
                    }
                }
            }
            // An I/O error is not proof that the data is bad (it may be
            // transient), and this could be the last readable copy: leave it.
            Err(e) => {
                warn!(block_id = %id, error = %e, "could not read a block while scrubbing");
                report.unreadable += 1;
            }
        }
    }

    Ok(report)
}

fn verify_twice(storage: &Storage, id: &str, delay: Duration) -> Result<(), StorageError> {
    match storage.verify_block(id) {
        Err(StorageError::HashMismatch | StorageError::ChecksumUnavailable) => {
            std::thread::sleep(delay);
            storage.verify_block(id)
        }
        other => other,
    }
}

pub async fn run(config: Config, storage: Arc<Storage>) {
    let period = Duration::from_secs(config.scrub_interval_secs);
    let mut master = MasterServiceClient::new(master_channel(&config.master_addr));

    // First pass after one full period, so a restart does not start with a
    // read of the whole disk while the worker is still registering.
    let mut ticker = interval_at(Instant::now() + period, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    info!(every = ?period, "scrubber started");

    loop {
        ticker.tick().await;
        let started = std::time::Instant::now();

        let pass = {
            let storage = Arc::clone(&storage);
            blocking(move || scrub_once(&storage, RECHECK_DELAY)).await
        };
        let report = match pass {
            Ok(report) => report,
            Err(status) => {
                error!(%status, "scrub pass failed");
                continue;
            }
        };

        info!(
            checked = report.checked,
            quarantined = report.quarantined.len(),
            unreadable = report.unreadable,
            took = ?started.elapsed(),
            "scrub pass finished"
        );

        for block_id in report.quarantined {
            let request = CorruptionReport {
                block_id: block_id.clone(),
                worker_id: config.id.clone(),
            };
            if let Err(status) = master.report_corruption(request).await {
                warn!(%block_id, %status, "could not report corruption; the next block report will tell the master");
            }
        }
    }
}
