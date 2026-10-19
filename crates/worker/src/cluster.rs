use std::sync::Arc;
use std::time::{Duration, Instant};

use tonic::Status;
use tonic::transport::{Channel, Endpoint};
use tracing::{info, warn};

use crate::config::Config;
use crate::fencing::Fencing;
use crate::pb::cluster_service_client::ClusterServiceClient;
use crate::pb::{BlockEntry, BlockReportRequest, HeartbeatRequest};
use crate::service::blocking;
use crate::storage::Storage;

const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
const REPORT_EVERY: Duration = Duration::from_secs(5 * 60);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(30);

struct Cluster {
    client: ClusterServiceClient<Channel>,
    config: Config,
    storage: Arc<Storage>,
    fencing: Arc<Fencing>,
    reported_fencing: Option<u64>,
    last_report: Instant,
}

/// A lazy channel connects on first use and reconnects by itself.
pub(crate) fn master_channel(master_addr: &str) -> Channel {
    Endpoint::from_shared(master_addr.to_string())
        .expect("master address is not a valid URI")
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .connect_lazy()
}

pub async fn run(config: Config, storage: Arc<Storage>, fencing: Arc<Fencing>) {
    let channel = master_channel(&config.master_addr);

    let mut cluster = Cluster {
        client: ClusterServiceClient::new(channel),
        config,
        storage,
        fencing,
        reported_fencing: None,
        last_report: Instant::now(),
    };

    let mut retry_wait = FIRST_RETRY;
    loop {
        match cluster.beat().await {
            Ok(()) => {
                retry_wait = FIRST_RETRY;
                tokio::time::sleep(HEARTBEAT_EVERY).await;
            }
            Err(status) => {
                warn!(%status, retry_in = ?retry_wait, "cannot reach the master");
                tokio::time::sleep(retry_wait).await;
                retry_wait = (retry_wait * 2).min(LAST_RETRY);
            }
        }
    }
}

impl Cluster {
    async fn beat(&mut self) -> Result<(), Status> {
        let leader_fencing = self.heartbeat().await?;

        let new_leader = self.reported_fencing != Some(leader_fencing);
        if new_leader || self.last_report.elapsed() >= REPORT_EVERY {
            self.send_report(leader_fencing).await?;
        }
        Ok(())
    }

    async fn heartbeat(&mut self) -> Result<u64, Status> {
        let changes = self.storage.take_changes();
        let request = HeartbeatRequest {
            worker_id: self.config.id.clone(),
            addr: self.config.advertise_addr.clone(),
            az: self.config.az.clone(),
            quota_bytes: self.storage.quota_bytes(),
            used_bytes: self.storage.used_bytes(),
            new_blocks: changes.new_blocks.clone(),
            removed_blocks: changes.removed_blocks.clone(),
            data_dir: self.config.data_dir.display().to_string(),
        };

        let answer = match self.client.heartbeat(request).await {
            Ok(response) => response.into_inner(),
            Err(status) => {
                self.storage.restore_changes(changes);
                return Err(status);
            }
        };

        // Remember the token; a lower one means this answer came from an old leader.
        if let Err(status) = self.fencing.check(answer.fencing) {
            self.storage.restore_changes(changes);
            return Err(status);
        }

        // The master does not know us yet: make the next step send a full report.
        if answer.full_report_requested {
            self.reported_fencing = None;
        }
        Ok(answer.fencing)
    }

    async fn send_report(&mut self, leader_fencing: u64) -> Result<(), Status> {
        let storage = Arc::clone(&self.storage);
        let blocks = blocking(move || list_entries(&storage)).await?;

        info!(
            blocks = blocks.len(),
            leader_fencing, "sending block report"
        );
        self.client
            .block_report(BlockReportRequest {
                worker_id: self.config.id.clone(),
                blocks,
            })
            .await?;

        self.reported_fencing = Some(leader_fencing);
        self.last_report = Instant::now();
        Ok(())
    }
}

fn list_entries(storage: &Storage) -> Result<Vec<BlockEntry>, crate::error::StorageError> {
    let mut entries = Vec::new();
    for block in storage.list_blocks()? {
        let block_id = block.block_id.hyphenated().to_string();
        let Ok(sha256) = storage.stored_checksum(&block_id) else {
            warn!(%block_id, "block has no checksum, not reported");
            continue;
        };
        entries.push(BlockEntry {
            block_id,
            size: block.size,
            sha256: sha256.to_vec(),
        });
    }
    Ok(entries)
}
