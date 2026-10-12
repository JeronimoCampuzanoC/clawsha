use crate::cluster::{ClusterState, Heartbeat};
use crate::leader::{Leadership, Transition};
use crate::tests::support::test_db;
use crate::time::now_ms;
use std::sync::Arc;

const TTL: u64 = 10_000;

#[tokio::test]
async fn first_master_becomes_leader_and_the_second_waits() {
    let Some(t) = test_db().await else { return };
    // test_db() holds the lease as "test-master" for 60 s; start past that.
    let now = now_ms() + 61_000;
    let a = Leadership::new(
        t.db.clone(),
        "master-a",
        "a:50051",
        Arc::new(ClusterState::new()),
    );
    let b = Leadership::new(
        t.db.clone(),
        "master-b",
        "b:50051",
        Arc::new(ClusterState::new()),
    );

    assert_eq!(a.tick(now, TTL).await.unwrap(), Transition::BecameLeader);
    assert_eq!(b.tick(now, TTL).await.unwrap(), Transition::StillStandby);

    assert!(a.is_leader());
    assert!(!b.is_leader());
    let fence = a.fence().expect("leader has a fence");
    assert_eq!(fence.holder, "master-a");
    assert_eq!(fence.fencing, t.fence.fencing + 1);
    assert!(b.fence().is_none());

    t.cleanup().await;
}

#[tokio::test]
async fn leader_renews_while_alive() {
    let Some(t) = test_db().await else { return };
    let now = now_ms() + 61_000;
    let a = Leadership::new(
        t.db.clone(),
        "master-a",
        "a:50051",
        Arc::new(ClusterState::new()),
    );
    a.tick(now, TTL).await.unwrap();

    assert_eq!(
        a.tick(now + 3_000, TTL).await.unwrap(),
        Transition::StillLeader
    );
    let lease = t.db.get_leader().await.unwrap().unwrap();
    assert_eq!(lease.expires_at, now + 3_000 + TTL);

    t.cleanup().await;
}

#[tokio::test]
async fn standby_takes_over_when_the_lease_expires_and_old_leader_steps_down() {
    let Some(t) = test_db().await else { return };
    let now = now_ms() + 61_000;
    let a = Leadership::new(
        t.db.clone(),
        "master-a",
        "a:50051",
        Arc::new(ClusterState::new()),
    );
    let b = Leadership::new(
        t.db.clone(),
        "master-b",
        "b:50051",
        Arc::new(ClusterState::new()),
    );
    a.tick(now, TTL).await.unwrap();
    let a_fence = a.fence().unwrap();

    // a goes silent for longer than the TTL; b's next tick wins.
    assert_eq!(
        b.tick(now + TTL + 1, TTL).await.unwrap(),
        Transition::BecameLeader
    );
    assert_eq!(b.fence().unwrap().fencing, a_fence.fencing + 1);

    // When a wakes up, its renewal fails and it drops to standby.
    assert_eq!(
        a.tick(now + TTL + 2, TTL).await.unwrap(),
        Transition::SteppedDown
    );
    assert!(!a.is_leader());
    assert!(a.fence().is_none());

    // Any write a still tries with its old fence is refused.
    let write =
        t.db.acquire_path_lease(&a_fence, "f1", "l1", now, 1_000)
            .await;
    assert!(matches!(write, Err(crate::error::MasterError::Fenced)));

    t.cleanup().await;
}

#[tokio::test]
async fn fence_expires_when_renewals_stop_landing() {
    let Some(t) = test_db().await else { return };
    let now = now_ms() + 61_000;
    let a = Leadership::new(
        t.db.clone(),
        "master-a",
        "a:50051",
        Arc::new(ClusterState::new()),
    );
    a.tick(now, TTL).await.unwrap();

    // Within the TTL we still lead...
    assert!(a.fence_at(now + TTL - 1).is_some());
    assert!(a.is_leader_at(now + TTL - 1));
    // ...but if DynamoDB has been unreachable for a whole TTL, the standby
    // may already hold the lease: stop acting and stop answering 200.
    assert!(a.fence_at(now + TTL + 1).is_none());
    assert!(!a.is_leader_at(now + TTL + 1));

    // A successful renewal extends it again.
    a.tick(now + 3_000, TTL).await.unwrap();
    assert!(a.fence_at(now + TTL + 1).is_some());

    t.cleanup().await;
}

#[tokio::test]
async fn becoming_leader_starts_with_an_empty_cluster_view_in_safe_mode() {
    let Some(t) = test_db().await else { return };
    let now = now_ms() + 61_000;
    let cluster = Arc::new(ClusterState::new());
    // Stale knowledge from a previous term.
    cluster.heartbeat(
        Heartbeat {
            worker_id: "w1".into(),
            addr: "w1:50052".into(),
            az: "a".into(),
            quota_bytes: 10,
            used_bytes: 0,
            new_blocks: vec!["b1".into()],
            removed_blocks: vec![],
            data_dir: String::new(),
        },
        now,
    );
    let a = Leadership::new(t.db.clone(), "master-a", "a:50051", cluster.clone());

    a.tick(now, TTL).await.unwrap();

    assert!(
        cluster.workers(now).is_empty(),
        "the workers will report again"
    );
    assert!(a.safe_mode(now + 1_000));
    assert!(!a.safe_mode(now + 31_000));

    t.cleanup().await;
}
