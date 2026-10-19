//! Garbage collection: frees what no file needs any more.
//!
//! Blocks stop being needed in four ways: a file is removed (`rm`), an
//! upload is aborted, a gateway dies mid-upload and nobody finishes it, or a
//! block is reassigned (the half-written old id is dropped). On top of that,
//! a worker that comes back after being declared dead brings copies the
//! health module already replaced, so some blocks end up with R + 1.
//!
//! Each pass of the leader does, in order:
//!
//! 1. PENDING versions older than PENDING_TIMEOUT_MS whose path has no valid
//!    lease become OLD (their writer is gone).
//! 2. OLD versions past OLD_GRACE_MS lose their block rows and their row.
//!    The grace lets a reader that got a read plan just before a replace or
//!    an `rm` finish its download.
//! 3. The sweep: any block a worker holds that no row in `blocks` points to
//!    is deleted from the workers, once it has looked like that for
//!    ORPHAN_GRACE_MS. Blocks with more than R live copies lose the extra
//!    ones, keeping one copy per AZ when possible.
//!
//! Step 3 only trusts DynamoDB, not the steps before: it also catches
//! reassigned blocks and copies on workers that were away when their file
//! was deleted. Every DynamoDB write goes through the fenced layer, and the
//! DeleteBlock orders carry the leader's fencing, so a deposed leader cannot
//! delete anything.
//!
//! Safety valve: rows in `blocks` only disappear when this GC drops them
//! (and one at a time on ReassignBlock), so the GC knows which orphans it
//! explains. A crowd of orphans it does not explain means the metadata is
//! wrong, not the workers: empty tables, a wrong MASTER_TABLE_PREFIX, a
//! DynamoDB Local that restarted in memory. Then it deletes none of them and
//! logs an error, instead of wiping every disk in the cluster.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::MissedTickBehavior;
use tracing::{error, info, warn};

use crate::cluster::{ClusterState, WorkerInfo, WorkerState};
use crate::db::versions::{VersionStatus, version_id};
use crate::db::{Db, Fence};
use crate::error::MasterError;
use crate::leader::Leadership;
use crate::pb;
use crate::workers::WorkerClient;

pub const GC_EVERY_MS: u64 = 30_000;
/// How long an OLD version is kept (the plan's 5 minutes).
pub const OLD_GRACE_MS: u64 = 5 * 60_000;
/// A PENDING upload this old with no lease on its path is abandoned. The
/// gateway renews the lease every 20 s while it writes, so the lease check is
/// what matters; the age only keeps the GC away from fresh uploads.
pub const PENDING_TIMEOUT_MS: u64 = 60 * 60_000;
/// A block nobody points to is deleted only after looking orphan this long,
/// which takes at least two passes, each with a fresh scan of `blocks`.
pub const ORPHAN_GRACE_MS: u64 = 60_000;
/// Versions handled per pass and status, so one pass stays short.
const VERSIONS_PER_PASS: i32 = 100;
/// Unexplained orphans tolerated before suspecting the metadata: at least
/// this many and more than half of the blocks the workers hold.
pub const SUSPICIOUS_ORPHANS: usize = 10;
/// How long the GC remembers the blocks whose rows it dropped, so a worker
/// that was away and comes back with them still gets them deleted.
const DROPPED_MEMORY_MS: u64 = 60 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// No version points to the block.
    Orphan,
    /// The block has more than R live copies.
    Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deletion {
    pub block_id: String,
    pub worker: WorkerInfo,
    pub reason: Reason,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    pub deletions: Vec<Deletion>,
    /// Orphans past their grace that were not deleted because there are too
    /// many unexplained ones (see the safety valve above).
    pub held_back: usize,
}

/// Decides which copies to delete. Pure (no network, no clock) so the rules
/// can be tested on their own.
///
/// - `holders`: every block the leader knows, with its non-dead holders.
/// - `referenced`: block ids some version points to.
/// - `explained`: block ids whose rows this GC dropped itself.
/// - `orphan_since`: when each block first looked orphan; updated here.
///
/// Only alive workers get orders. A block with a suspect holder is never
/// trimmed: that holder may be about to die, and the copy would be needed.
pub fn plan_deletions(
    holders: Vec<(String, Vec<(WorkerInfo, WorkerState)>)>,
    referenced: &HashSet<String>,
    explained: &HashSet<String>,
    orphan_since: &mut HashMap<String, u64>,
    replication: usize,
    now_ms: u64,
) -> Plan {
    let known: HashSet<&str> = holders.iter().map(|(b, _)| b.as_str()).collect();
    orphan_since.retain(|b, _| known.contains(b.as_str()) && !referenced.contains(b));

    let unexplained = holders
        .iter()
        .filter(|(b, _)| !referenced.contains(b) && !explained.contains(b))
        .count();
    let suspicious = unexplained >= SUSPICIOUS_ORPHANS && unexplained * 2 > holders.len();

    let mut plan = Plan::default();
    let deletions = &mut plan.deletions;
    for (block_id, live) in &holders {
        let alive: Vec<&WorkerInfo> = live
            .iter()
            .filter(|(_, state)| *state == WorkerState::Alive)
            .map(|(w, _)| w)
            .collect();

        if !referenced.contains(block_id) {
            let since = *orphan_since.entry(block_id.clone()).or_insert(now_ms);
            if now_ms.saturating_sub(since) < ORPHAN_GRACE_MS {
                continue;
            }
            if suspicious && !explained.contains(block_id) {
                plan.held_back += 1;
            } else {
                deletions.extend(alive.iter().map(|w| Deletion {
                    block_id: block_id.clone(),
                    worker: (*w).clone(),
                    reason: Reason::Orphan,
                }));
            }
            continue;
        }

        if alive.len() > replication && alive.len() == live.len() {
            for w in pick_extra(&alive, alive.len() - replication) {
                deletions.push(Deletion {
                    block_id: block_id.clone(),
                    worker: w.clone(),
                    reason: Reason::Extra,
                });
            }
        }
    }
    deletions.sort_by(|a, b| (&a.block_id, &a.worker.id).cmp(&(&b.block_id, &b.worker.id)));
    plan
}

/// The `count` copies to drop: always from the AZ with the most copies, so
/// the survivors stay spread; within it, the fullest worker.
fn pick_extra<'a>(holders: &[&'a WorkerInfo], count: usize) -> Vec<&'a WorkerInfo> {
    let mut keep: Vec<&WorkerInfo> = holders.to_vec();
    let mut drop = Vec::new();
    for _ in 0..count {
        let mut per_az: HashMap<&str, usize> = HashMap::new();
        for w in &keep {
            *per_az.entry(w.az.as_str()).or_default() += 1;
        }
        let victim = keep
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                per_az[a.az.as_str()]
                    .cmp(&per_az[b.az.as_str()])
                    .then(a.used_percent().total_cmp(&b.used_percent()))
                    .then(b.id.cmp(&a.id))
            })
            .map(|(i, _)| i);
        let Some(i) = victim else { break };
        drop.push(keep.remove(i));
    }
    drop
}

/// What one pass did, for the log and the tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    pub abandoned_uploads: usize,
    pub versions_dropped: usize,
    pub orphan_copies_deleted: usize,
    pub extra_copies_deleted: usize,
    pub failed_deletes: usize,
    pub orphans_held_back: usize,
}

impl GcReport {
    fn is_empty(&self) -> bool {
        *self == GcReport::default()
    }
}

pub struct Collector<C> {
    db: Db,
    cluster: Arc<ClusterState>,
    client: C,
    replication: usize,
    orphan_since: Mutex<HashMap<String, u64>>,
    /// block_id -> when this GC dropped the row that pointed to it.
    dropped: Mutex<HashMap<String, u64>>,
}

impl<C: WorkerClient> Collector<C> {
    pub fn new(db: Db, cluster: Arc<ClusterState>, client: C, replication: usize) -> Self {
        Self {
            db,
            cluster,
            client,
            replication,
            orphan_since: Mutex::new(HashMap::new()),
            dropped: Mutex::new(HashMap::new()),
        }
    }

    pub async fn pass(&self, fence: &Fence, now_ms: u64) -> Result<GcReport, MasterError> {
        let mut report = GcReport {
            abandoned_uploads: self.retire_abandoned(fence, now_ms).await?,
            versions_dropped: self.drop_old_versions(fence, now_ms).await?,
            ..GcReport::default()
        };

        // The map first, then the scan. A block that is on a worker now had
        // its row written before any worker got it, so if this scan does not
        // see the row, no version points to the block any more.
        let holders = self.cluster.holders(now_ms);
        let referenced = self.db.referenced_block_ids().await?;
        let plan = {
            let known: HashSet<&str> = holders.iter().map(|(b, _)| b.as_str()).collect();
            let mut dropped = self.dropped.lock().unwrap();
            dropped.retain(|b, at| {
                known.contains(b.as_str()) || now_ms.saturating_sub(*at) < DROPPED_MEMORY_MS
            });
            let explained: HashSet<String> = dropped.keys().cloned().collect();
            drop(dropped);

            let mut orphan_since = self.orphan_since.lock().unwrap();
            plan_deletions(
                holders,
                &referenced,
                &explained,
                &mut orphan_since,
                self.replication,
                now_ms,
            )
        };
        report.orphans_held_back = plan.held_back;
        if plan.held_back > 0 {
            error!(
                held_back = plan.held_back,
                "most blocks on the workers belong to no file and the GC did not free them: \
                 the metadata looks wrong (empty tables? wrong MASTER_TABLE_PREFIX?). \
                 Not deleting them; fix the metadata or clean the workers by hand"
            );
        }

        for d in plan.deletions {
            let request = pb::DeleteBlockRequest {
                block_id: d.block_id.clone(),
                fencing: fence.fencing,
            };
            match self.client.delete(&d.worker.addr, request).await {
                Ok(_) => {
                    self.cluster.remove_location(&d.block_id, &d.worker.id);
                    match d.reason {
                        Reason::Orphan => report.orphan_copies_deleted += 1,
                        Reason::Extra => report.extra_copies_deleted += 1,
                    }
                }
                Err(status) => {
                    report.failed_deletes += 1;
                    warn!(
                        block_id = %d.block_id,
                        worker = %d.worker.id,
                        code = ?status.code(),
                        message = status.message(),
                        "DeleteBlock failed; retrying on the next pass"
                    );
                }
            }
        }
        Ok(report)
    }

    async fn retire_abandoned(&self, fence: &Fence, now_ms: u64) -> Result<usize, MasterError> {
        let stale = self
            .db
            .versions_by_status(
                VersionStatus::Pending,
                now_ms.saturating_sub(PENDING_TIMEOUT_MS),
                VERSIONS_PER_PASS,
            )
            .await?;
        let mut retired = 0;
        for v in stale {
            if self
                .db
                .retire_abandoned_version(fence, &v.inode_id, v.version_no, now_ms)
                .await?
            {
                info!(inode_id = %v.inode_id, version_no = v.version_no, "abandoned upload retired");
                retired += 1;
            }
        }
        Ok(retired)
    }

    async fn drop_old_versions(&self, fence: &Fence, now_ms: u64) -> Result<usize, MasterError> {
        let expired = self
            .db
            .versions_by_status(
                VersionStatus::Old,
                now_ms.saturating_sub(OLD_GRACE_MS),
                VERSIONS_PER_PASS,
            )
            .await?;
        let count = expired.len();
        for v in expired {
            let vid = version_id(&v.inode_id, v.version_no);
            // Remember what we orphan, so the sweep knows these are ours.
            let blocks = self.db.list_blocks(&vid).await?;
            self.dropped
                .lock()
                .unwrap()
                .extend(blocks.into_iter().map(|b| (b.block_id, now_ms)));
            // Rows first: if we stop halfway, the version is still OLD and
            // the next pass finishes the job.
            self.db.delete_blocks(fence, &vid).await?;
            self.db
                .delete_old_version(fence, &v.inode_id, v.version_no)
                .await?;
        }
        Ok(count)
    }
}

/// Runs a pass every GC_EVERY_MS while this master leads and is out of safe
/// mode (right after a takeover the map is still incomplete).
pub async fn run<C: WorkerClient>(collector: Arc<Collector<C>>, leadership: Arc<Leadership>) {
    let mut interval = tokio::time::interval(Duration::from_millis(GC_EVERY_MS));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let now = crate::time::now_ms();
        let Some(fence) = leadership.fence_at(now) else {
            continue;
        };
        if leadership.safe_mode(now) {
            continue;
        }
        match collector.pass(&fence, now).await {
            Ok(report) if !report.is_empty() => info!(?report, "gc pass"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "gc pass failed; retrying on the next one"),
        }
    }
}
