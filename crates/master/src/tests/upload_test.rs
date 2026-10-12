use crate::cluster::{ClusterState, Heartbeat};
use crate::db::versions::VersionStatus;
use crate::error::MasterError;
use crate::namespace::Namespace;
use crate::read_plan::ReadPlanner;
use crate::tests::support::{TestDb, sha, test_db};
use crate::time::now_ms;
use crate::upload::{CommittedBlock, UploadCoordinator, UploadPlan};
use std::sync::Arc;

const U1: &str = "user-1";
const BLOCK: u64 = 1_000; // tiny blocks keep the arithmetic readable

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

struct Rig {
    t: TestDb,
    cluster: Arc<ClusterState>,
    uploads: UploadCoordinator,
    reads: ReadPlanner,
    ns: Namespace,
}

/// Fresh tables plus four fake workers, two per AZ, all alive "now".
async fn rig() -> Option<Rig> {
    let t = test_db().await?;
    let cluster = Arc::new(ClusterState::new());
    let now = now_ms();
    for (id, az) in [("a1", "a"), ("a2", "a"), ("b1", "b"), ("b2", "b")] {
        cluster.heartbeat(hb(id, az), now);
    }
    let ns = Namespace::new(t.db.clone());
    Some(Rig {
        uploads: UploadCoordinator::new(ns.clone(), cluster.clone(), BLOCK, 2),
        reads: ReadPlanner::new(ns.clone(), cluster.clone()),
        ns,
        cluster,
        t,
    })
}

/// What a well-behaved gateway reports back: every block stored on the two
/// workers of its assignment, with the hash of `content`'s slice.
fn acks(plan: &UploadPlan, content: &[u8]) -> Vec<CommittedBlock> {
    plan.blocks
        .iter()
        .map(|b| {
            let start = b.idx as usize * BLOCK as usize;
            let end = start + b.size as usize;
            CommittedBlock {
                idx: b.idx,
                block_id: b.block_id.clone(),
                size: b.size,
                sha256: sha(&content[start..end]),
                worker_ids: b.workers.iter().map(|w| w.id.clone()).collect(),
            }
        })
        .collect()
}

#[tokio::test]
async fn start_upload_plans_one_block_per_16mib_with_two_azs() {
    let Some(r) = rig().await else { return };
    let now = now_ms();

    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 2_500, None, now)
        .await
        .unwrap();

    assert_eq!(plan.version_no, 1);
    assert_eq!(plan.block_size, BLOCK);
    let sizes: Vec<u64> = plan.blocks.iter().map(|b| b.size).collect();
    assert_eq!(sizes, vec![1_000, 1_000, 500]);
    for (i, b) in plan.blocks.iter().enumerate() {
        assert_eq!(b.idx as usize, i);
        assert_eq!(b.workers.len(), 2);
        assert_ne!(b.workers[0].az, b.workers[1].az);
    }

    // The metadata is already there, but the file is not visible yet.
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Pending);
    assert_eq!(
        r.t.db
            .list_blocks(&format!("{}#1", plan.inode_id))
            .await
            .unwrap()
            .len(),
        3
    );
    assert!(matches!(
        r.ns.resolve(U1, "/a.bin").await,
        Err(MasterError::NotFound)
    ));

    r.t.cleanup().await;
}

#[tokio::test]
async fn empty_file_has_no_blocks() {
    let Some(r) = rig().await else { return };

    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/empty", 0, None, now_ms())
        .await
        .unwrap();
    assert!(plan.blocks.is_empty());

    let info = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/empty",
            &plan.inode_id,
            1,
            &plan.lease_id,
            0,
            &sha(b""),
            vec![],
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!(info.size, 0);
    assert_eq!(r.ns.resolve(U1, "/empty").await.unwrap().size, 0);

    r.t.cleanup().await;
}

#[tokio::test]
async fn start_upload_needs_a_parent_and_refuses_directories() {
    let Some(r) = rig().await else { return };
    r.ns.mkdir(&r.t.fence, U1, "/docs", now_ms()).await.unwrap();

    assert!(matches!(
        r.uploads
            .start(&r.t.fence, U1, "/nope/a.bin", 10, None, now_ms())
            .await,
        Err(MasterError::NotFound)
    ));
    assert!(matches!(
        r.uploads
            .start(&r.t.fence, U1, "/docs", 10, None, now_ms())
            .await,
        Err(MasterError::AlreadyExists)
    ));

    r.t.cleanup().await;
}

#[tokio::test]
async fn concurrent_writers_on_the_same_path_are_excluded() {
    let Some(r) = rig().await else { return };
    let now = now_ms();

    let first = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 10, None, now)
        .await
        .unwrap();
    let second = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 10, None, now)
        .await;
    assert!(matches!(second, Err(MasterError::Locked)));

    r.uploads
        .abort(
            &r.t.fence,
            U1,
            "/a.bin",
            &first.inode_id,
            1,
            &first.lease_id,
            now,
        )
        .await
        .unwrap();
    // The aborted file never existed for readers, so this is a brand-new
    // file: new inode, version 1. The old version stays OLD for the GC.
    let third = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 10, None, now)
        .await
        .unwrap();
    assert_ne!(third.inode_id, first.inode_id);
    assert_eq!(third.version_no, 1);
    let old =
        r.t.db
            .get_version(&first.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(old.status, VersionStatus::Old);

    r.t.cleanup().await;
}

#[tokio::test]
async fn start_upload_without_workers_is_unavailable() {
    let Some(t) = test_db().await else { return };
    let cluster = Arc::new(ClusterState::new());
    cluster.heartbeat(hb("only", "a"), now_ms());
    let uploads = UploadCoordinator::new(Namespace::new(t.db.clone()), cluster, BLOCK, 2);

    let r = uploads
        .start(&t.fence, U1, "/a.bin", 10, None, now_ms())
        .await;
    assert!(matches!(r, Err(MasterError::NoWorkers(_))), "got {r:?}");
    // Nothing half-made is left behind.
    assert!(
        t.db.get_inode("root:user-1", "a.bin")
            .await
            .unwrap()
            .is_none()
    );

    t.cleanup().await;
}

#[tokio::test]
async fn commit_publishes_the_file_and_records_locations() {
    let Some(r) = rig().await else { return };
    let content = vec![7u8; 2_500];
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 2_500, None, now)
        .await
        .unwrap();

    let info = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            2_500,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await
        .unwrap();

    assert_eq!(info.version_no, 1);
    assert_eq!(info.size, 2_500);
    assert_eq!(info.block_count, 3);
    let stat = r.ns.resolve(U1, "/a.bin").await.unwrap();
    assert_eq!(stat.size, 2_500);
    assert_eq!(stat.current_version, 1);
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Committed);
    assert_eq!(v.sha256, sha(&content));
    let blocks =
        r.t.db
            .list_blocks(&format!("{}#1", plan.inode_id))
            .await
            .unwrap();
    assert!(blocks.iter().all(|b| b.sha256.len() == 32));
    for b in &plan.blocks {
        assert_eq!(r.cluster.locations(&b.block_id, now).len(), 2);
    }
    // The lease is free again.
    r.t.db
        .acquire_path_lease(&r.t.fence, &plan.inode_id, "someone-else", now, 1_000)
        .await
        .unwrap();

    r.t.cleanup().await;
}

#[tokio::test]
async fn replacing_a_file_retires_the_old_version() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let v1 = vec![1u8; 1_500];
    let p1 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 1_500, None, now)
        .await
        .unwrap();
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &p1.inode_id,
            1,
            &p1.lease_id,
            1_500,
            &sha(&v1),
            acks(&p1, &v1),
            now,
        )
        .await
        .unwrap();

    let v2 = vec![2u8; 500];
    let p2 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 500, None, now)
        .await
        .unwrap();
    assert_eq!(p2.inode_id, p1.inode_id, "same file, new version");
    assert_eq!(p2.version_no, 2);
    // Readers still see v1 while v2 is being written.
    assert_eq!(r.ns.resolve(U1, "/a.bin").await.unwrap().size, 1_500);

    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &p2.inode_id,
            2,
            &p2.lease_id,
            500,
            &sha(&v2),
            acks(&p2, &v2),
            now,
        )
        .await
        .unwrap();

    assert_eq!(r.ns.resolve(U1, "/a.bin").await.unwrap().size, 500);
    let old = r.t.db.get_version(&p1.inode_id, 1).await.unwrap().unwrap();
    assert_eq!(old.status, VersionStatus::Old);

    r.t.cleanup().await;
}

#[tokio::test]
async fn commit_rejects_bad_reports() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let content = vec![7u8; 1_500];
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 1_500, None, now)
        .await
        .unwrap();
    let good = acks(&plan, &content);

    // Wrong lease.
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            "not-my-lease",
            1_500,
            &sha(&content),
            good.clone(),
            now,
        )
        .await;
    assert!(matches!(e, Err(MasterError::Locked)), "got {e:?}");

    // A block with a single copy.
    let mut one_copy = good.clone();
    one_copy[1].worker_ids.truncate(1);
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            one_copy,
            now,
        )
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );

    // Missing block.
    let mut short = good.clone();
    short.pop();
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            short,
            now,
        )
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );

    // Sizes that do not add up.
    let mut wrong_size = good.clone();
    wrong_size[0].size = 999;
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            wrong_size,
            now,
        )
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );

    // Still pending after all that, and a good commit works.
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Pending);
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            good,
            now,
        )
        .await
        .unwrap();

    r.t.cleanup().await;
}

#[tokio::test]
async fn absurd_sizes_are_rejected_before_any_work() {
    let Some(r) = rig().await else { return };
    let too_big = (crate::upload::MAX_BLOCKS_PER_FILE as u64 + 1) * BLOCK;

    let e = r
        .uploads
        .start(&r.t.fence, U1, "/huge", too_big, None, now_ms())
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );
    assert!(
        r.t.db
            .get_inode("root:user-1", "huge")
            .await
            .unwrap()
            .is_none()
    );

    r.t.cleanup().await;
}

#[tokio::test]
async fn a_stale_writer_cannot_overwrite_a_newer_version() {
    let Some(r) = rig().await else { return };
    let t0 = now_ms();
    let v1 = vec![1u8; 100];
    let slow = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, t0)
        .await
        .unwrap();

    // The slow writer's lease expires; a second writer uploads and commits.
    // (The fake workers heartbeat again so they are still alive at t1.)
    let t1 = t0 + 70_000;
    for (id, az) in [("a1", "a"), ("a2", "a"), ("b1", "b"), ("b2", "b")] {
        r.cluster.heartbeat(hb(id, az), t1);
    }
    let v2 = vec![2u8; 50];
    let fast = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 50, None, t1)
        .await
        .unwrap();
    assert_eq!(fast.version_no, 2);
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &fast.inode_id,
            2,
            &fast.lease_id,
            50,
            &sha(&v2),
            acks(&fast, &v2),
            t1,
        )
        .await
        .unwrap();

    // The slow writer wakes up and tries to commit version 1.
    let late = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &slow.inode_id,
            1,
            &slow.lease_id,
            100,
            &sha(&v1),
            acks(&slow, &v1),
            t1 + 1_000,
        )
        .await;
    assert!(
        matches!(late, Err(MasterError::Conflict(_))),
        "got {late:?}"
    );

    let stat = r.ns.resolve(U1, "/a.bin").await.unwrap();
    assert_eq!(stat.current_version, 2, "the newer version stays");
    assert_eq!(stat.size, 50);
    let v2_row =
        r.t.db
            .get_version(&fast.inode_id, 2)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v2_row.status, VersionStatus::Committed);

    r.t.cleanup().await;
}

#[tokio::test]
async fn another_user_cannot_touch_someone_elses_upload() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 1_500, None, now)
        .await
        .unwrap();
    let intruder = "user-2";

    let e = r
        .uploads
        .abort(
            &r.t.fence,
            intruder,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            now,
        )
        .await;
    assert!(matches!(e, Err(MasterError::NotFound)), "abort: {e:?}");

    let e = r
        .uploads
        .reassign(
            &r.t.fence,
            intruder,
            &plan.inode_id,
            1,
            &plan.lease_id,
            0,
            &[],
            now,
        )
        .await;
    assert!(matches!(e, Err(MasterError::NotFound)), "reassign: {e:?}");

    let content = vec![7u8; 1_500];
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            intruder,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await;
    assert!(matches!(e, Err(MasterError::NotFound)), "commit: {e:?}");

    // The real owner is unaffected.
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Pending);
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            1_500,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await
        .unwrap();

    r.t.cleanup().await;
}

#[tokio::test]
async fn plan_never_fills_a_worker_past_its_quota() {
    let Some(t) = test_db().await else { return };
    let cluster = Arc::new(ClusterState::new());
    let now = now_ms();
    // Each worker has room for exactly two more blocks (needs 2 blocks free to
    // be a candidate): 4 workers x 2 = 8 block copies, i.e. a 4-block file.
    for (id, az) in [("a1", "a"), ("a2", "a"), ("b1", "b"), ("b2", "b")] {
        let mut h = hb(id, az);
        h.quota_bytes = 3 * BLOCK;
        cluster.heartbeat(h, now);
    }
    let uploads = UploadCoordinator::new(Namespace::new(t.db.clone()), cluster, BLOCK, 2);

    let plan = uploads
        .start(&t.fence, U1, "/fits", 4 * BLOCK, None, now)
        .await
        .unwrap();
    let mut per_worker = std::collections::HashMap::new();
    for b in &plan.blocks {
        for w in &b.workers {
            *per_worker.entry(w.id.clone()).or_insert(0u64) += b.size;
        }
    }
    assert!(
        per_worker.values().all(|&bytes| bytes <= 2 * BLOCK),
        "{per_worker:?}"
    );

    let e = uploads
        .start(&t.fence, U1, "/too-much", 5 * BLOCK, None, now)
        .await;
    assert!(matches!(e, Err(MasterError::NoWorkers(_))), "got {e:?}");

    t.cleanup().await;
}

#[tokio::test]
async fn commit_rejects_block_ids_that_are_not_uuids() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let content = vec![7u8; 100];
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, now)
        .await
        .unwrap();
    let mut bad = acks(&plan, &content);
    bad[0].block_id = "ñ".to_string();

    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            100,
            &sha(&content),
            bad,
            now,
        )
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );

    r.t.cleanup().await;
}

#[tokio::test]
async fn commit_is_idempotent_for_the_same_content() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let content = vec![7u8; 100];
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, now)
        .await
        .unwrap();
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            100,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await
        .unwrap();

    let again = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            100,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await
        .unwrap();
    // A retry after a lost reply must succeed, not fail with 400.
    assert_eq!(again.version_no, 1);
    assert_eq!(again.size, 100);
    assert_eq!(again.block_count, 1);

    // But not for different content under the same version.
    let other = vec![8u8; 100];
    let e = r
        .uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            100,
            &sha(&other),
            acks(&plan, &other),
            now,
        )
        .await;
    assert!(
        matches!(e, Err(MasterError::InvalidArgument(_))),
        "got {e:?}"
    );

    r.t.cleanup().await;
}

#[tokio::test]
async fn abort_of_a_new_file_leaves_no_trace_for_readers() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, now)
        .await
        .unwrap();

    r.uploads
        .abort(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            now,
        )
        .await
        .unwrap();

    assert!(
        r.t.db
            .get_inode("root:user-1", "a.bin")
            .await
            .unwrap()
            .is_none()
    );
    let v =
        r.t.db
            .get_version(&plan.inode_id, 1)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(v.status, VersionStatus::Old, "the GC will free its blocks");
    // mkdir with that name works now.
    r.ns.mkdir(&r.t.fence, U1, "/a.bin", now).await.unwrap();

    r.t.cleanup().await;
}

#[tokio::test]
async fn abort_of_a_replacement_keeps_the_current_version() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let v1 = vec![1u8; 100];
    let p1 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, now)
        .await
        .unwrap();
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &p1.inode_id,
            1,
            &p1.lease_id,
            100,
            &sha(&v1),
            acks(&p1, &v1),
            now,
        )
        .await
        .unwrap();
    let p2 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 50, None, now)
        .await
        .unwrap();

    r.uploads
        .abort(&r.t.fence, U1, "/a.bin", &p2.inode_id, 2, &p2.lease_id, now)
        .await
        .unwrap();

    let stat = r.ns.resolve(U1, "/a.bin").await.unwrap();
    assert_eq!(stat.current_version, 1);
    assert_eq!(stat.size, 100);

    r.t.cleanup().await;
}

#[tokio::test]
async fn reassign_gives_a_fresh_block_on_other_workers() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 1_500, None, now)
        .await
        .unwrap();
    let failed = &plan.blocks[1];
    let exclude: Vec<String> = failed.workers.iter().map(|w| w.id.clone()).collect();

    let again = r
        .uploads
        .reassign(
            &r.t.fence,
            U1,
            &plan.inode_id,
            1,
            &plan.lease_id,
            1,
            &exclude,
            now,
        )
        .await
        .unwrap();

    assert_eq!(again.idx, 1);
    assert_eq!(again.size, 500);
    assert_ne!(again.block_id, failed.block_id);
    assert!(again.workers.iter().all(|w| !exclude.contains(&w.id)));
    let rows =
        r.t.db
            .list_blocks(&format!("{}#1", plan.inode_id))
            .await
            .unwrap();
    assert_eq!(rows[1].block_id, again.block_id);

    // With a stranger's lease it is refused.
    let e = r
        .uploads
        .reassign(&r.t.fence, U1, &plan.inode_id, 1, "other", 1, &[], now)
        .await;
    assert!(matches!(e, Err(MasterError::Locked)));

    r.t.cleanup().await;
}

#[tokio::test]
async fn read_plan_lists_blocks_with_live_replicas_only() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let content = vec![9u8; 2_500];
    let plan = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 2_500, None, now)
        .await
        .unwrap();

    // Not readable while pending.
    assert!(matches!(
        r.reads.plan(U1, "/a.bin", None, now).await,
        Err(MasterError::NotFound)
    ));

    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &plan.inode_id,
            1,
            &plan.lease_id,
            2_500,
            &sha(&content),
            acks(&plan, &content),
            now,
        )
        .await
        .unwrap();

    let read = r.reads.plan(U1, "/a.bin", None, now).await.unwrap();
    assert_eq!(read.version_no, 1);
    assert_eq!(read.size, 2_500);
    assert_eq!(read.sha256, sha(&content));
    assert_eq!(read.blocks.len(), 3);
    for (i, b) in read.blocks.iter().enumerate() {
        assert_eq!(b.idx as usize, i);
        assert_eq!(b.block_id, plan.blocks[i].block_id);
        assert_eq!(
            b.sha256,
            sha(&content[i * 1_000..(i * 1_000 + b.size as usize)])
        );
        assert_eq!(b.replicas.len(), 2);
    }

    // One worker of block 0 dies: the plan offers the other one only.
    let dead = &plan.blocks[0].workers[0].id;
    let later = now + 40_000;
    for w in ["a1", "a2", "b1", "b2"] {
        if w != dead {
            r.cluster
                .heartbeat(hb(w, if w.starts_with('a') { "a" } else { "b" }), later);
        }
    }
    let read = r.reads.plan(U1, "/a.bin", None, later).await.unwrap();
    assert_eq!(read.blocks[0].replicas.len(), 1);
    assert_ne!(&read.blocks[0].replicas[0].id, dead);

    // Both copies gone: the read cannot be served (503 for the gateway).
    r.cluster
        .remove_location(&plan.blocks[0].block_id, &read.blocks[0].replicas[0].id);
    assert!(matches!(
        r.reads.plan(U1, "/a.bin", None, later).await,
        Err(MasterError::NoReplica(_))
    ));

    r.t.cleanup().await;
}

#[tokio::test]
async fn read_plan_can_pin_an_old_version() {
    let Some(r) = rig().await else { return };
    let now = now_ms();
    let v1 = vec![1u8; 100];
    let p1 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 100, None, now)
        .await
        .unwrap();
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &p1.inode_id,
            1,
            &p1.lease_id,
            100,
            &sha(&v1),
            acks(&p1, &v1),
            now,
        )
        .await
        .unwrap();
    let v2 = vec![2u8; 50];
    let p2 = r
        .uploads
        .start(&r.t.fence, U1, "/a.bin", 50, None, now)
        .await
        .unwrap();
    r.uploads
        .commit(
            &r.t.fence,
            U1,
            "/a.bin",
            &p2.inode_id,
            2,
            &p2.lease_id,
            50,
            &sha(&v2),
            acks(&p2, &v2),
            now,
        )
        .await
        .unwrap();

    assert_eq!(
        r.reads.plan(U1, "/a.bin", None, now).await.unwrap().size,
        50
    );
    // An open() handle taken before the replacement keeps reading v1.
    assert_eq!(
        r.reads.plan(U1, "/a.bin", Some(1), now).await.unwrap().size,
        100
    );
    assert!(matches!(
        r.reads.plan(U1, "/a.bin", Some(9), now).await,
        Err(MasterError::NotFound)
    ));

    r.t.cleanup().await;
}
