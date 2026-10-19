use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tonic::Status;

use crate::cluster::{ClusterState, Heartbeat, WorkerInfo, WorkerState};
use crate::db::versions::VersionStatus;
use crate::gc::{
    Collector, GcReport, OLD_GRACE_MS, ORPHAN_GRACE_MS, PENDING_TIMEOUT_MS, Reason, plan_deletions,
};
use crate::namespace::Namespace;
use crate::pb;
use crate::tests::support::{TestDb, sha, test_db};
use crate::time::now_ms;
use crate::upload::{CommittedBlock, PATH_LEASE_TTL_MS, UploadCoordinator, UploadPlan};
use crate::workers::WorkerClient;

const S: u64 = 1_000; // one second, in ms
const U1: &str = "user-1";
const BLOCK: u64 = 1_000;

fn worker(id: &str, az: &str, used_bytes: u64) -> WorkerInfo {
    WorkerInfo {
        id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 1_000,
        used_bytes,
        data_dir: "/data".to_string(),
        last_heartbeat: 0,
    }
}

fn alive(w: WorkerInfo) -> (WorkerInfo, WorkerState) {
    (w, WorkerState::Alive)
}

fn ids(deletions: &[crate::gc::Deletion]) -> Vec<(&str, &str)> {
    deletions
        .iter()
        .map(|d| (d.block_id.as_str(), d.worker.id.as_str()))
        .collect()
}

// --- plan_deletions: the rules ------------------------------------------------

#[test]
fn an_orphan_is_deleted_only_after_the_grace_period() {
    let holders = vec![(
        "gone".to_string(),
        vec![alive(worker("w1", "a", 0)), alive(worker("w2", "b", 0))],
    )];
    let referenced = HashSet::new();
    let mut since = HashMap::new();

    let first = plan_deletions(holders.clone(), &referenced, &mut since, 2, 10 * S);
    assert!(first.is_empty(), "first sighting only starts the clock");

    let later = plan_deletions(
        holders,
        &referenced,
        &mut since,
        2,
        10 * S + ORPHAN_GRACE_MS,
    );
    assert_eq!(ids(&later), vec![("gone", "w1"), ("gone", "w2")]);
    assert!(later.iter().all(|d| d.reason == Reason::Orphan));
}

#[test]
fn a_block_referenced_again_restarts_the_orphan_clock() {
    let holders = vec![("b1".to_string(), vec![alive(worker("w1", "a", 0))])];
    let mut since = HashMap::new();

    plan_deletions(holders.clone(), &HashSet::new(), &mut since, 1, 0);
    // A scan that sees the row again (it was a read race, not an orphan).
    let referenced = HashSet::from(["b1".to_string()]);
    plan_deletions(holders.clone(), &referenced, &mut since, 1, 30 * S);
    assert!(since.is_empty());

    let after = plan_deletions(holders, &HashSet::new(), &mut since, 1, ORPHAN_GRACE_MS);
    assert!(
        after.is_empty(),
        "the clock started over at the last sighting"
    );
}

#[test]
fn referenced_blocks_with_r_copies_are_left_alone() {
    let holders = vec![(
        "b1".to_string(),
        vec![alive(worker("w1", "a", 0)), alive(worker("w2", "b", 0))],
    )];
    let referenced = HashSet::from(["b1".to_string()]);
    let out = plan_deletions(holders, &referenced, &mut HashMap::new(), 2, 0);
    assert!(out.is_empty());
}

#[test]
fn an_extra_copy_is_dropped_from_the_az_that_has_two() {
    // A worker came back after its blocks were re-replicated: a, a, b.
    let holders = vec![(
        "b1".to_string(),
        vec![
            alive(worker("a-empty", "a", 100)),
            alive(worker("a-full", "a", 900)),
            alive(worker("b-full", "b", 950)),
        ],
    )];
    let referenced = HashSet::from(["b1".to_string()]);
    let out = plan_deletions(holders, &referenced, &mut HashMap::new(), 2, 0);
    assert_eq!(
        ids(&out),
        vec![("b1", "a-full")],
        "keep one per AZ; in AZ a drop the fuller worker"
    );
    assert_eq!(out[0].reason, Reason::Extra);
}

#[test]
fn no_trim_while_a_holder_is_suspect() {
    let holders = vec![(
        "b1".to_string(),
        vec![
            alive(worker("w1", "a", 0)),
            alive(worker("w2", "a", 0)),
            (worker("w3", "b", 0), WorkerState::Suspect),
        ],
    )];
    let referenced = HashSet::from(["b1".to_string()]);
    let out = plan_deletions(holders, &referenced, &mut HashMap::new(), 2, 0);
    assert!(
        out.is_empty(),
        "w3 may die and the extra copy would be needed"
    );
}

#[test]
fn suspect_workers_get_no_delete_orders() {
    let holders = vec![(
        "gone".to_string(),
        vec![
            alive(worker("w1", "a", 0)),
            (worker("w2", "b", 0), WorkerState::Suspect),
        ],
    )];
    let mut since = HashMap::from([("gone".to_string(), 0)]);
    let out = plan_deletions(holders, &HashSet::new(), &mut since, 2, ORPHAN_GRACE_MS);
    assert_eq!(ids(&out), vec![("gone", "w1")]);
}

// --- Collector: a pass against DynamoDB Local and fake workers ----------------

#[derive(Clone, Default)]
struct FakeWorkers {
    deletes: Arc<Mutex<Vec<(String, pb::DeleteBlockRequest)>>>,
}

impl FakeWorkers {
    fn deleted(&self) -> HashSet<(String, String)> {
        self.deletes
            .lock()
            .unwrap()
            .iter()
            .map(|(addr, r)| (r.block_id.clone(), addr.clone()))
            .collect()
    }
}

impl WorkerClient for FakeWorkers {
    async fn replicate(
        &self,
        _worker_addr: &str,
        _request: pb::ReplicateBlockRequest,
    ) -> Result<u64, Status> {
        unreachable!("the GC never copies")
    }

    async fn delete(
        &self,
        worker_addr: &str,
        request: pb::DeleteBlockRequest,
    ) -> Result<u64, Status> {
        self.deletes
            .lock()
            .unwrap()
            .push((worker_addr.to_string(), request));
        Ok(BLOCK)
    }
}

struct Rig {
    t: TestDb,
    cluster: Arc<ClusterState>,
    uploads: UploadCoordinator,
    ns: Namespace,
    workers: FakeWorkers,
    gc: Collector<FakeWorkers>,
}

fn hb(id: &str, az: &str) -> Heartbeat {
    Heartbeat {
        worker_id: id.to_string(),
        addr: format!("{id}:50052"),
        az: az.to_string(),
        quota_bytes: 1_000_000,
        used_bytes: 0,
        new_blocks: vec![],
        removed_blocks: vec![],
        data_dir: "/data".to_string(),
    }
}

impl Rig {
    /// The GC runs minutes "later"; the workers must still look alive then.
    fn beat(&self, at: u64) {
        for (id, az) in [("a1", "a"), ("b1", "b")] {
            self.cluster.heartbeat(hb(id, az), at);
        }
    }

    async fn pass(&self, at: u64) -> GcReport {
        self.beat(at);
        self.gc.pass(&self.t.fence, at).await.unwrap()
    }

    async fn upload(&self, path: &str, content: &[u8], now: u64) -> UploadPlan {
        let plan = self
            .uploads
            .start(&self.t.fence, U1, path, content.len() as u64, None, now)
            .await
            .unwrap();
        let blocks = plan
            .blocks
            .iter()
            .map(|b| {
                let start = b.idx as usize * BLOCK as usize;
                CommittedBlock {
                    idx: b.idx,
                    block_id: b.block_id.clone(),
                    size: b.size,
                    sha256: sha(&content[start..start + b.size as usize]),
                    worker_ids: b.workers.iter().map(|w| w.id.clone()).collect(),
                }
            })
            .collect();
        self.uploads
            .commit(
                &self.t.fence,
                U1,
                path,
                &plan.inode_id,
                plan.version_no,
                &plan.lease_id,
                content.len() as u64,
                &sha(content),
                blocks,
                now,
            )
            .await
            .unwrap();
        plan
    }
}

/// Two workers (one per AZ) so every block lands on a1 and b1.
async fn rig() -> Option<Rig> {
    let t = test_db().await?;
    let cluster = Arc::new(ClusterState::new());
    let ns = Namespace::new(t.db.clone());
    let workers = FakeWorkers::default();
    let rig = Rig {
        uploads: UploadCoordinator::new(ns.clone(), cluster.clone(), BLOCK, 2),
        gc: Collector::new(t.db.clone(), cluster.clone(), workers.clone(), 2),
        workers,
        ns,
        cluster,
        t,
    };
    rig.beat(now_ms());
    Some(rig)
}

fn block_ids(plan: &UploadPlan) -> Vec<String> {
    plan.blocks.iter().map(|b| b.block_id.clone()).collect()
}

#[tokio::test]
async fn a_replaced_version_is_freed_after_the_grace_period() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let v1 = r.upload("/a.bin", &[1u8; 1_500], now).await;
    let v2 = r.upload("/a.bin", &[2u8; 500], now).await;

    // Inside the grace a reader may still be downloading v1.
    let report = r.pass(now + S).await;
    assert_eq!(report, GcReport::default());

    let later = now + OLD_GRACE_MS + S;
    let report = r.pass(later).await;
    assert_eq!(report.versions_dropped, 1);
    assert!(r.t.db.get_version(&v1.inode_id, 1).await.unwrap().is_none());
    assert!(
        r.t.db
            .list_blocks(&format!("{}#1", v1.inode_id))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        r.workers.deleted().is_empty(),
        "orphans wait for their own grace"
    );

    let report = r.pass(later + ORPHAN_GRACE_MS).await;
    assert_eq!(report.orphan_copies_deleted, 4, "2 blocks x 2 copies");
    let expected: HashSet<(String, String)> = block_ids(&v1)
        .into_iter()
        .flat_map(|b| [(b.clone(), "a1:50052".into()), (b, "b1:50052".into())])
        .collect();
    assert_eq!(r.workers.deleted(), expected);
    for b in block_ids(&v1) {
        assert!(r.cluster.locations(&b, later + ORPHAN_GRACE_MS).is_empty());
    }

    // v2 is the file now: untouched.
    assert_eq!(r.ns.resolve(U1, "/a.bin").await.unwrap().size, 500);
    for b in block_ids(&v2) {
        assert_eq!(r.cluster.locations(&b, later + ORPHAN_GRACE_MS).len(), 2);
    }
    r.t.cleanup().await;
}

#[tokio::test]
async fn an_abandoned_upload_is_retired_then_freed() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/half.bin", 1_500, None, now)
        .await
        .unwrap();

    let report = r.pass(now + 10 * S).await;
    assert_eq!(report.abandoned_uploads, 0, "too young to call abandoned");

    let gone = now + PENDING_TIMEOUT_MS + S;
    let report = r.pass(gone).await;
    assert_eq!(report.abandoned_uploads, 1);
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Old);

    let report = r.pass(gone + OLD_GRACE_MS + S).await;
    assert_eq!(report.versions_dropped, 1);
    assert!(
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .is_none()
    );
    r.t.cleanup().await;
}

#[tokio::test]
async fn an_old_upload_whose_writer_still_renews_is_kept() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/slow.bin", 1_500, None, now)
        .await
        .unwrap();

    // A very long upload: the gateway renewed the lease a moment ago.
    let late = now + PENDING_TIMEOUT_MS + S;
    r.t.db
        .acquire_path_lease(
            &r.t.fence,
            &plan.inode_id,
            &plan.lease_id,
            late,
            PATH_LEASE_TTL_MS,
        )
        .await
        .unwrap();

    let report = r.pass(late + S).await;
    assert_eq!(report.abandoned_uploads, 0);
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Pending);
    r.t.cleanup().await;
}

#[tokio::test]
async fn blocks_of_a_live_upload_are_not_orphans() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/writing.bin", 1_500, None, now)
        .await
        .unwrap();
    // The workers already have the data, the commit has not happened yet.
    for b in &plan.blocks {
        for w in &b.workers {
            r.cluster.add_location(&b.block_id, &w.id);
        }
    }

    r.pass(now + S).await;
    let report = r.pass(now + S + ORPHAN_GRACE_MS).await;
    assert_eq!(report.orphan_copies_deleted, 0);
    assert!(r.workers.deleted().is_empty());
    r.t.cleanup().await;
}
