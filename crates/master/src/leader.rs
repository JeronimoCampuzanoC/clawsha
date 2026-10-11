//! Who is in charge. Two masters run at the same time; only the one holding
//! the lease in DynamoDB acts, the other one answers "not leader" and waits.
//!
//! The loop is simple on purpose: every 3 s the leader renews a 10 s lease
//! and the standby tries to take it. If the leader dies, the standby wins at
//! most 10 s later with fencing + 1; if the old leader comes back it fails to
//! renew, drops to standby, and every write it still attempts with the old
//! fence is rejected by DynamoDB (see db::fenced_write). The internal NLB
//! only sends traffic to the master whose /health/leader answers 200, so
//! gateways and workers never need to know who won.

use std::sync::{Arc, RwLock};

use crate::cluster::ClusterState;
use crate::db::{Db, Fence};
use crate::error::MasterError;

/// The leader renews this often...
pub const RENEW_EVERY_MS: u64 = 3_000;
/// ...a lease that lasts this long.
pub const LEASE_TTL_MS: u64 = 10_000;
/// Right after a takeover the block map is empty; until the workers have
/// reported (or this much time has passed) nothing destructive should run.
pub const SAFE_MODE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    BecameLeader,
    StillLeader,
    SteppedDown,
    StillStandby,
}

struct State {
    fence: Option<Fence>,
    /// When we last became leader (ms), for safe mode.
    since_ms: u64,
}

pub struct Leadership {
    db: Db,
    me: String,
    addr: String,
    cluster: Arc<ClusterState>,
    state: RwLock<State>,
}

impl Leadership {
    pub fn new(db: Db, me: &str, addr: &str, cluster: Arc<ClusterState>) -> Self {
        Self {
            db,
            me: me.to_string(),
            addr: addr.to_string(),
            cluster,
            state: RwLock::new(State {
                fence: None,
                since_ms: 0,
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.me
    }

    pub fn is_leader(&self) -> bool {
        self.state.read().unwrap().fence.is_some()
    }

    /// The fence to attach to writes, or None when we are standby.
    pub fn fence(&self) -> Option<Fence> {
        self.state.read().unwrap().fence.clone()
    }

    /// Like fence() but as the error every RPC returns on a standby.
    pub fn require_fence(&self) -> Result<Fence, MasterError> {
        self.fence().ok_or(MasterError::NotLeader)
    }

    pub fn fencing(&self) -> u64 {
        self.fence().map_or(0, |f| f.fencing)
    }

    pub fn safe_mode(&self, now_ms: u64) -> bool {
        let state = self.state.read().unwrap();
        state.fence.is_some() && now_ms.saturating_sub(state.since_ms) < SAFE_MODE_MS
    }

    /// One step of the loop: renew if we lead, otherwise try to take over.
    pub async fn tick(&self, now_ms: u64, ttl_ms: u64) -> Result<Transition, MasterError> {
        match self.fence() {
            Some(fence) => {
                if self.db.renew_leader(&fence, now_ms, ttl_ms).await? {
                    Ok(Transition::StillLeader)
                } else {
                    self.state.write().unwrap().fence = None;
                    tracing::warn!(master = %self.me, fencing = fence.fencing, "lost the leader lease, now standby");
                    Ok(Transition::SteppedDown)
                }
            }
            None => match self
                .db
                .try_acquire_leader(&self.me, &self.addr, now_ms, ttl_ms)
                .await?
            {
                Some(lease) => {
                    // Whatever we knew about the workers is from another term.
                    // They will see the new fencing and report everything again.
                    self.cluster.clear();
                    let mut state = self.state.write().unwrap();
                    state.fence = Some(Fence {
                        holder: lease.holder,
                        fencing: lease.fencing,
                    });
                    state.since_ms = now_ms;
                    tracing::info!(master = %self.me, fencing = lease.fencing, "became leader");
                    Ok(Transition::BecameLeader)
                }
                None => Ok(Transition::StillStandby),
            },
        }
    }

    /// Runs tick() forever. Errors talking to DynamoDB are logged and
    /// retried on the next round; if the leader cannot renew for a whole TTL
    /// the standby takes over anyway.
    pub async fn run(self: Arc<Self>) {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(RENEW_EVERY_MS));
        loop {
            interval.tick().await;
            if let Err(e) = self.tick(crate::time::now_ms(), LEASE_TTL_MS).await {
                tracing::error!(master = %self.me, error = %e, "leader lease round failed");
            }
        }
    }
}
