//! What the leader knows about the workers, kept only in memory.
//!
//! Nothing here is persisted on purpose: the workers are the source of truth
//! for what sits on their disks, and they tell us through heartbeats (3 s,
//! incremental) and block reports (full list). After a failover the new
//! leader starts empty and gets rebuilt from those reports, like the HDFS
//! NameNode does.
//!
//! This module is the shared ground between A and B. A only needs to feed it
//! (heartbeats, block reports, commit acks) and query it (write candidates,
//! replica locations). B builds the failure reaction on top: re-replication
//! of blocks on dead workers, repairs, GC and the CloudWatch usage metric.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Silence after which a worker stops receiving new blocks.
pub const SUSPECT_AFTER_MS: u64 = 10_000;
/// Silence after which a worker is considered gone and its blocks need copies.
pub const DEAD_AFTER_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerInfo {
    pub id: String,
    pub addr: String,
    pub az: String,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub data_dir: String,
    /// Unix time in ms of the last heartbeat.
    pub last_heartbeat: u64,
}

impl WorkerInfo {
    pub fn free_bytes(&self) -> u64 {
        self.quota_bytes.saturating_sub(self.used_bytes)
    }

    /// Percentage of the quota in use, 0..=100.
    pub fn used_percent(&self) -> f64 {
        if self.quota_bytes == 0 {
            return 100.0;
        }
        self.used_bytes as f64 * 100.0 / self.quota_bytes as f64
    }

    pub fn state(&self, now_ms: u64) -> WorkerState {
        let silence = now_ms.saturating_sub(self.last_heartbeat);
        if silence >= DEAD_AFTER_MS {
            WorkerState::Dead
        } else if silence >= SUSPECT_AFTER_MS {
            WorkerState::Suspect
        } else {
            WorkerState::Alive
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Alive,
    Suspect,
    Dead,
}

/// What one heartbeat tells us. Mirrors pb::HeartbeatRequest without the
/// generated types, so this module stays easy to test.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    pub worker_id: String,
    pub addr: String,
    pub az: String,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub new_blocks: Vec<String>,
    pub removed_blocks: Vec<String>,
    pub data_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatOutcome {
    /// True when we had never heard from this worker: it should send a full
    /// block report so we learn what it has.
    pub full_report_requested: bool,
}

#[derive(Debug, Clone)]
pub struct WorkerView {
    pub info: WorkerInfo,
    pub state: WorkerState,
    pub block_count: u64,
}

#[derive(Default)]
struct Inner {
    workers: HashMap<String, WorkerInfo>,
    /// block_id -> workers that hold it
    locations: HashMap<String, HashSet<String>>,
    /// worker_id -> blocks it holds (the inverse index, used when one dies)
    blocks_by_worker: HashMap<String, HashSet<String>>,
}

pub struct ClusterState {
    inner: Mutex<Inner>,
}

impl Default for ClusterState {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusterState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn heartbeat(&self, hb: Heartbeat, now_ms: u64) -> HeartbeatOutcome {
        let mut inner = self.inner.lock().unwrap();

        let known = inner.workers.contains_key(&hb.worker_id);
        inner.workers.insert(
            hb.worker_id.clone(),
            WorkerInfo {
                id: hb.worker_id.clone(),
                addr: hb.addr,
                az: hb.az,
                quota_bytes: hb.quota_bytes,
                used_bytes: hb.used_bytes,
                data_dir: hb.data_dir,
                last_heartbeat: now_ms,
            },
        );

        for block in hb.new_blocks {
            inner.add(&block, &hb.worker_id);
        }
        for block in hb.removed_blocks {
            inner.remove(&block, &hb.worker_id);
        }

        HeartbeatOutcome {
            full_report_requested: !known,
        }
    }

    /// Full report: whatever we believed this worker had is replaced.
    pub fn block_report(&self, worker_id: &str, blocks: Vec<String>) {
        let mut inner = self.inner.lock().unwrap();

        let old = inner.blocks_by_worker.remove(worker_id).unwrap_or_default();
        for block in old {
            if let Some(holders) = inner.locations.get_mut(&block) {
                holders.remove(worker_id);
                if holders.is_empty() {
                    inner.locations.remove(&block);
                }
            }
        }
        for block in blocks {
            inner.add(&block, worker_id);
        }
    }

    /// A worker acked a write during an upload: it has the block now.
    pub fn add_location(&self, block_id: &str, worker_id: &str) {
        self.inner.lock().unwrap().add(block_id, worker_id);
    }

    /// A replica turned out bad (hash mismatch); stop handing it out.
    pub fn remove_location(&self, block_id: &str, worker_id: &str) {
        self.inner.lock().unwrap().remove(block_id, worker_id);
    }

    /// Workers that hold the block and are not dead. Suspect workers still
    /// count for reads: they may just be slow, and the gateway falls back.
    pub fn locations(&self, block_id: &str, now_ms: u64) -> Vec<WorkerInfo> {
        let inner = self.inner.lock().unwrap();
        let Some(holders) = inner.locations.get(block_id) else {
            return Vec::new();
        };
        holders
            .iter()
            .filter_map(|id| inner.workers.get(id))
            .filter(|w| w.state(now_ms) != WorkerState::Dead)
            .cloned()
            .collect()
    }

    /// Alive workers with at least `min_free_bytes` of room: the only ones
    /// that may receive new blocks.
    pub fn write_candidates(&self, now_ms: u64, min_free_bytes: u64) -> Vec<WorkerInfo> {
        let inner = self.inner.lock().unwrap();
        inner
            .workers
            .values()
            .filter(|w| w.state(now_ms) == WorkerState::Alive)
            .filter(|w| w.free_bytes() >= min_free_bytes)
            .cloned()
            .collect()
    }

    pub fn workers(&self, now_ms: u64) -> Vec<WorkerView> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<WorkerView> = inner
            .workers
            .values()
            .map(|w| WorkerView {
                state: w.state(now_ms),
                block_count: inner
                    .blocks_by_worker
                    .get(&w.id)
                    .map_or(0, |b| b.len() as u64),
                info: w.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.info.id.cmp(&b.info.id));
        out
    }

    pub fn total_blocks(&self) -> u64 {
        self.inner.lock().unwrap().locations.len() as u64
    }

    /// Blocks with fewer than `target` known copies.
    pub fn under_replicated(&self, target: usize) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner
            .locations
            .values()
            .filter(|holders| holders.len() < target)
            .count() as u64
    }
}

impl Inner {
    fn add(&mut self, block_id: &str, worker_id: &str) {
        self.locations
            .entry(block_id.to_string())
            .or_default()
            .insert(worker_id.to_string());
        self.blocks_by_worker
            .entry(worker_id.to_string())
            .or_default()
            .insert(block_id.to_string());
    }

    fn remove(&mut self, block_id: &str, worker_id: &str) {
        if let Some(holders) = self.locations.get_mut(block_id) {
            holders.remove(worker_id);
            if holders.is_empty() {
                self.locations.remove(block_id);
            }
        }
        if let Some(blocks) = self.blocks_by_worker.get_mut(worker_id) {
            blocks.remove(block_id);
        }
    }
}
