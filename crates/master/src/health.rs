//! Re-replication: how the leader gets every block back to R copies.
//!
//! A copy goes missing in three ways: its worker stops sending heartbeats and
//! turns DEAD (30 s), the scrubber quarantines a corrupted copy, or the
//! gateway reports a bad hash. In all three cases the block shows up in
//! `ClusterState::under_replicated_blocks`, and this module orders a worker
//! that still has it to copy it to another one (`ReplicateBlock`).
//!
//! Copies go to workers that are already alive, not to the replacement the
//! ASG launches: waiting for a new instance would leave the block with a
//! single copy for minutes. The replacement arrives empty and gets new blocks
//! through AvailableSpace like any other worker. HDFS does the same.
//!
//! Only the leader repairs, and not while it is in safe mode: right after a
//! takeover the map is still being rebuilt from block reports and almost
//! every block would look under-replicated.
//!
//! Not done here: deleting the extra copy when a dead worker comes back (the
//! block then has R + 1 copies, which costs space but loses nothing).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tonic::{Code, Status};
use tracing::{info, warn};

use crate::cluster::{ClusterState, WorkerInfo, WorkerState};
use crate::leader::Leadership;
use crate::pb;
use crate::placement;
use crate::workers::WorkerClient;

/// How often the leader looks for blocks to repair.
pub const PASS_EVERY_MS: u64 = 5_000;
/// A block must look under-replicated this long before we copy it. During an
/// upload the two replicas report in separate heartbeats (3 s apart), so a
/// brand-new block can look like it has one copy for a moment.
pub const REPAIR_GRACE_MS: u64 = 10_000;
/// Copies a worker may send or receive at once, so repairs do not starve the
/// uploads and downloads that share its disk and network.
pub const MAX_STREAMS_PER_WORKER: usize = 2;
/// A 16 MiB block takes about a second inside the VPC; this only cuts off a
/// worker that hangs mid-copy, so the block is retried on the next pass.
const REPLICATE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    pub block_id: String,
    pub source: WorkerInfo,
    pub target: WorkerInfo,
}

/// Decides which copies to start in one pass. Pure (no network, no clock) so
/// the rules can be tested on their own.
///
/// - `needy`: blocks below R with their live holders, already past the grace
///   period and without a copy in flight.
/// - `candidates`: alive workers with room for one more block.
/// - `busy`: copies in flight per worker, as source or as target.
///
/// Blocks with the fewest copies go first. The source is a holder that is
/// alive (a suspect one only if there is no other). The target is a worker
/// without the block, preferably in an AZ that has no copy yet, chosen with
/// AvailableSpace. One copy per block per pass.
pub fn plan_repairs<R: Rng>(
    mut needy: Vec<(String, Vec<WorkerInfo>)>,
    candidates: &[WorkerInfo],
    busy: &HashMap<String, usize>,
    now_ms: u64,
    rng: &mut R,
) -> Vec<Repair> {
    let mut busy = busy.clone();
    let has_room = |busy: &HashMap<String, usize>, id: &str| {
        busy.get(id).copied().unwrap_or(0) < MAX_STREAMS_PER_WORKER
    };
    needy.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.0.cmp(&b.0)));

    let mut repairs = Vec::new();
    for (block_id, holders) in needy {
        // false sorts before true: alive holders first.
        let Some(source) = holders
            .iter()
            .filter(|w| has_room(&busy, &w.id))
            .min_by_key(|w| w.state(now_ms) != WorkerState::Alive)
            .cloned()
        else {
            continue;
        };

        let holder_ids: HashSet<&str> = holders.iter().map(|w| w.id.as_str()).collect();
        let covered_azs: HashSet<&str> = holders.iter().map(|w| w.az.as_str()).collect();
        let pool: Vec<WorkerInfo> = candidates
            .iter()
            .filter(|w| !holder_ids.contains(w.id.as_str()) && has_room(&busy, &w.id))
            .cloned()
            .collect();
        let other_az: Vec<WorkerInfo> = pool
            .iter()
            .filter(|w| !covered_azs.contains(w.az.as_str()))
            .cloned()
            .collect();
        // Degraded: no free worker in a missing AZ. Same AZ, different host.
        let pool = if other_az.is_empty() { pool } else { other_az };
        let Ok(target) = placement::pick_one(&pool, rng) else {
            continue;
        };
        let target = target.clone();

        *busy.entry(source.id.clone()).or_default() += 1;
        *busy.entry(target.id.clone()).or_default() += 1;
        repairs.push(Repair {
            block_id,
            source,
            target,
        });
    }
    repairs
}

#[derive(Default)]
struct Progress {
    /// block_id -> (source id, target id) of the copy in flight.
    in_flight: HashMap<String, (String, String)>,
    /// block_id -> when we first saw it under-replicated (ms).
    first_seen: HashMap<String, u64>,
}

pub struct Healer<C> {
    cluster: Arc<ClusterState>,
    client: C,
    replication: usize,
    block_size: u64,
    progress: Mutex<Progress>,
    /// Rings when a copy finishes, so the loop plans the next one right away.
    wake: Notify,
}

impl<C: WorkerClient> Healer<C> {
    pub fn new(cluster: Arc<ClusterState>, client: C, replication: usize, block_size: u64) -> Self {
        Self {
            cluster,
            client,
            replication,
            block_size,
            progress: Mutex::new(Progress::default()),
            wake: Notify::new(),
        }
    }

    /// One pass: plans the copies and starts them in the background. Returns
    /// their handles so the tests can wait for them; the loop lets them run.
    pub fn pass(self: &Arc<Self>, now_ms: u64, fencing: u64) -> Vec<JoinHandle<()>> {
        let needy = self
            .cluster
            .under_replicated_blocks(self.replication, now_ms);
        let candidates = self.cluster.write_candidates(now_ms, self.block_size);

        let repairs = {
            let mut progress = self.progress.lock().unwrap();

            let still_needy: HashSet<&str> = needy.iter().map(|(b, _)| b.as_str()).collect();
            progress
                .first_seen
                .retain(|block, _| still_needy.contains(block.as_str()));

            let mut ready = Vec::new();
            for (block_id, holders) in &needy {
                let first_seen = *progress.first_seen.entry(block_id.clone()).or_insert_with(|| {
                    if holders.is_empty() {
                        warn!(%block_id, "every copy of the block is on a dead worker; waiting for one to come back");
                    }
                    now_ms
                });
                let waited = now_ms.saturating_sub(first_seen) >= REPAIR_GRACE_MS;
                if waited && !holders.is_empty() && !progress.in_flight.contains_key(block_id) {
                    ready.push((block_id.clone(), holders.clone()));
                }
            }

            let mut busy: HashMap<String, usize> = HashMap::new();
            for (source, target) in progress.in_flight.values() {
                *busy.entry(source.clone()).or_default() += 1;
                *busy.entry(target.clone()).or_default() += 1;
            }

            let repairs = plan_repairs(ready, &candidates, &busy, now_ms, &mut rand::rng());
            for r in &repairs {
                progress.in_flight.insert(
                    r.block_id.clone(),
                    (r.source.id.clone(), r.target.id.clone()),
                );
            }
            repairs
        };

        if !repairs.is_empty() {
            info!(
                count = repairs.len(),
                under_replicated = needy.len(),
                "starting re-replication"
            );
        }
        repairs
            .into_iter()
            .map(|repair| {
                let healer = Arc::clone(self);
                tokio::spawn(async move { healer.copy(repair, fencing).await })
            })
            .collect()
    }

    async fn copy(&self, repair: Repair, fencing: u64) {
        let result = match self.replicate(&repair, fencing, false).await {
            // The target has a file for this block that we do not know about:
            // a copy the gateway found corrupt and nobody has quarantined yet.
            // The source sends checked data, so replacing it is safe.
            Err(status) if status.code() == Code::AlreadyExists => {
                self.replicate(&repair, fencing, true).await
            }
            other => other,
        };

        let Repair {
            block_id,
            source,
            target,
        } = &repair;
        let copied = match result {
            Ok(sent_bytes) => {
                self.cluster.add_location(block_id, &target.id);
                info!(%block_id, source = %source.id, target = %target.id, sent_bytes, "block re-replicated");
                true
            }
            Err(status) => {
                warn!(
                    %block_id,
                    source = %source.id,
                    target = %target.id,
                    code = ?status.code(),
                    message = status.message(),
                    "re-replication failed; retrying on the next pass"
                );
                false
            }
        };
        self.progress.lock().unwrap().in_flight.remove(block_id);
        // A copy takes milliseconds; waiting for the next tick to use the
        // freed slot would cap repairs at MAX_STREAMS_PER_WORKER per tick.
        // Only on success: a failing worker must not turn this into a loop.
        if copied {
            self.wake.notify_one();
        }
    }

    async fn replicate(
        &self,
        repair: &Repair,
        fencing: u64,
        overwrite: bool,
    ) -> Result<u64, Status> {
        let request = pb::ReplicateBlockRequest {
            block_id: repair.block_id.clone(),
            target: repair.target.addr.clone(),
            fencing,
            overwrite,
        };
        match tokio::time::timeout(
            REPLICATE_TIMEOUT,
            self.client.replicate(&repair.source.addr, request),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(Status::deadline_exceeded("ReplicateBlock timed out")),
        }
    }
}

/// Runs a pass every PASS_EVERY_MS, and right after each successful copy,
/// while this master leads and is out of safe mode. A standby skips the
/// pass: its block map is empty.
pub async fn run<C: WorkerClient>(healer: Arc<Healer<C>>, leadership: Arc<Leadership>) {
    let mut interval = tokio::time::interval(Duration::from_millis(PASS_EVERY_MS));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = healer.wake.notified() => {}
        }
        let now = crate::time::now_ms();
        let Some(fence) = leadership.fence_at(now) else {
            continue;
        };
        if leadership.safe_mode(now) {
            continue;
        }
        healer.pass(now, fence.fencing);
    }
}
