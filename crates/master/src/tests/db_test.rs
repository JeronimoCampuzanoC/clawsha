use crate::db::blocks::BlockRecord;
use crate::db::inodes::{Inode, InodeType};
use crate::db::versions::VersionStatus;
use crate::error::MasterError;
use crate::tests::support::{sha, test_db};
use crate::time::now_ms;

fn dir(parent: &str, name: &str, owner: &str) -> Inode {
    Inode {
        parent_id: parent.to_string(),
        name: name.to_string(),
        inode_id: uuid::Uuid::new_v4().to_string(),
        owner_id: owner.to_string(),
        kind: InodeType::Dir,
        current_version: 0,
        size: 0,
        modified_at: now_ms(),
    }
}

// ----- tables ---------------------------------------------------------------

#[tokio::test]
async fn create_tables_is_idempotent() {
    let Some(t) = test_db().await else { return };

    t.db.create_tables().await.unwrap();
    t.db.create_tables().await.unwrap();

    t.cleanup().await;
}

#[tokio::test]
async fn two_masters_can_create_the_tables_at_the_same_time() {
    // Both control nodes boot together after terraform apply; whoever loses
    // the race must not crash on "table already exists".
    let Ok(endpoint) = std::env::var("DFSHA_TEST_DYNAMO_ENDPOINT") else {
        return;
    };
    let prefix = format!("race{}_", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let a = crate::db::Db::connect(Some(&endpoint), &prefix)
        .await
        .unwrap();
    let b = crate::db::Db::connect(Some(&endpoint), &prefix)
        .await
        .unwrap();

    let (ra, rb) = tokio::join!(a.create_tables(), b.create_tables());

    assert!(ra.is_ok(), "{ra:?}");
    assert!(rb.is_ok(), "{rb:?}");
    let _ = a.delete_tables().await;
}

// ----- users ----------------------------------------------------------------

#[tokio::test]
async fn user_can_be_created_and_found_by_username() {
    let Some(t) = test_db().await else { return };

    t.db.create_user(&t.fence, "u1", "jero", "$argon2id$fake", now_ms())
        .await
        .unwrap();

    let found = t.db.find_user_by_username("jero").await.unwrap().unwrap();
    assert_eq!(found.user_id, "u1");
    assert_eq!(found.password_hash, "$argon2id$fake");
    assert!(
        t.db.find_user_by_username("nobody")
            .await
            .unwrap()
            .is_none()
    );

    t.cleanup().await;
}

#[tokio::test]
async fn duplicate_username_is_rejected() {
    let Some(t) = test_db().await else { return };

    t.db.create_user(&t.fence, "u1", "jero", "h1", now_ms())
        .await
        .unwrap();
    let second =
        t.db.create_user(&t.fence, "u2", "jero", "h2", now_ms())
            .await;

    assert!(matches!(second, Err(MasterError::AlreadyExists)));
    // The loser left nothing behind.
    assert!(t.db.get_user("u2").await.unwrap().is_none());

    t.cleanup().await;
}

// ----- inodes ---------------------------------------------------------------

#[tokio::test]
async fn inode_round_trip() {
    let Some(t) = test_db().await else { return };
    let docs = dir("root:u1", "docs", "u1");

    t.db.put_inode(&t.fence, &docs).await.unwrap();

    let got = t.db.get_inode("root:u1", "docs").await.unwrap().unwrap();
    assert_eq!(got, docs);
    assert!(t.db.get_inode("root:u1", "other").await.unwrap().is_none());

    t.cleanup().await;
}

#[tokio::test]
async fn putting_the_same_name_twice_fails() {
    let Some(t) = test_db().await else { return };

    t.db.put_inode(&t.fence, &dir("root:u1", "docs", "u1"))
        .await
        .unwrap();
    let again =
        t.db.put_inode(&t.fence, &dir("root:u1", "docs", "u1"))
            .await;

    assert!(matches!(again, Err(MasterError::AlreadyExists)));

    t.cleanup().await;
}

#[tokio::test]
async fn children_are_listed_sorted_by_name() {
    let Some(t) = test_db().await else { return };

    for name in ["zeta", "alpha", "mid"] {
        t.db.put_inode(&t.fence, &dir("root:u1", name, "u1"))
            .await
            .unwrap();
    }
    t.db.put_inode(&t.fence, &dir("root:u2", "other", "u2"))
        .await
        .unwrap();

    let names: Vec<String> =
        t.db.list_children("root:u1")
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.name)
            .collect();
    assert_eq!(names, vec!["alpha", "mid", "zeta"]);

    t.cleanup().await;
}

#[tokio::test]
async fn delete_checks_the_inode_id() {
    let Some(t) = test_db().await else { return };
    let docs = dir("root:u1", "docs", "u1");
    t.db.put_inode(&t.fence, &docs).await.unwrap();

    let wrong =
        t.db.delete_inode(&t.fence, "root:u1", "docs", "not-it")
            .await;
    assert!(matches!(wrong, Err(MasterError::NotFound)));

    t.db.delete_inode(&t.fence, "root:u1", "docs", &docs.inode_id)
        .await
        .unwrap();
    assert!(t.db.get_inode("root:u1", "docs").await.unwrap().is_none());

    t.cleanup().await;
}

#[tokio::test]
async fn move_relocates_the_entry_keeping_its_inode_id() {
    let Some(t) = test_db().await else { return };
    let docs = dir("root:u1", "docs", "u1");
    t.db.put_inode(&t.fence, &docs).await.unwrap();

    t.db.move_inode(&t.fence, &docs, "root:u1", "papers", now_ms())
        .await
        .unwrap();

    assert!(t.db.get_inode("root:u1", "docs").await.unwrap().is_none());
    let moved = t.db.get_inode("root:u1", "papers").await.unwrap().unwrap();
    assert_eq!(moved.inode_id, docs.inode_id);
    assert_eq!(moved.name, "papers");

    t.cleanup().await;
}

#[tokio::test]
async fn move_onto_an_existing_name_fails() {
    let Some(t) = test_db().await else { return };
    let a = dir("root:u1", "a", "u1");
    t.db.put_inode(&t.fence, &a).await.unwrap();
    t.db.put_inode(&t.fence, &dir("root:u1", "b", "u1"))
        .await
        .unwrap();

    let result =
        t.db.move_inode(&t.fence, &a, "root:u1", "b", now_ms())
            .await;

    assert!(matches!(result, Err(MasterError::AlreadyExists)));
    assert!(t.db.get_inode("root:u1", "a").await.unwrap().is_some());

    t.cleanup().await;
}

// ----- versions and blocks --------------------------------------------------

#[tokio::test]
async fn versions_are_numbered_per_inode() {
    let Some(t) = test_db().await else { return };

    assert_eq!(t.db.latest_version_no("f1").await.unwrap(), 0);
    t.db.put_pending_version(&t.fence, "f1", 1, "u1", 100, now_ms())
        .await
        .unwrap();
    t.db.put_pending_version(&t.fence, "f1", 2, "u1", 100, now_ms())
        .await
        .unwrap();
    t.db.put_pending_version(&t.fence, "f2", 1, "u1", 100, now_ms())
        .await
        .unwrap();

    assert_eq!(t.db.latest_version_no("f1").await.unwrap(), 2);
    assert_eq!(t.db.latest_version_no("f2").await.unwrap(), 1);

    let v = t.db.get_version("f1", 2).await.unwrap().unwrap();
    assert_eq!(v.status, VersionStatus::Pending);
    assert_eq!(v.size, 100);
    assert_eq!(v.owner_id, "u1");

    t.cleanup().await;
}

#[tokio::test]
async fn blocks_are_listed_in_index_order_even_past_a_transaction_chunk() {
    let Some(t) = test_db().await else { return };
    let version_id = "f1#1";

    // 120 blocks > the 99 a single fenced transaction can carry.
    let records: Vec<BlockRecord> = (0u32..120)
        .map(|idx| BlockRecord {
            version_id: version_id.to_string(),
            idx,
            block_id: uuid::Uuid::new_v4().to_string(),
            size: 16,
            sha256: sha(&idx.to_le_bytes()),
        })
        .collect();
    t.db.put_blocks(&t.fence, &records).await.unwrap();

    let listed = t.db.list_blocks(version_id).await.unwrap();
    assert_eq!(listed.len(), 120);
    assert!(listed.windows(2).all(|w| w[0].idx < w[1].idx));
    assert_eq!(listed[7], records[7]);

    t.cleanup().await;
}

#[tokio::test]
async fn blocks_can_be_overwritten_and_deleted() {
    let Some(t) = test_db().await else { return };
    let mut rec = BlockRecord {
        version_id: "f1#1".to_string(),
        idx: 0,
        block_id: "old".to_string(),
        size: 16,
        sha256: vec![],
    };
    t.db.put_blocks(&t.fence, std::slice::from_ref(&rec))
        .await
        .unwrap();

    rec.block_id = "new".to_string();
    rec.sha256 = sha(b"data");
    t.db.put_blocks(&t.fence, std::slice::from_ref(&rec))
        .await
        .unwrap();
    assert_eq!(t.db.list_blocks("f1#1").await.unwrap(), vec![rec]);

    t.db.delete_blocks(&t.fence, "f1#1").await.unwrap();
    assert!(t.db.list_blocks("f1#1").await.unwrap().is_empty());

    t.cleanup().await;
}

// ----- path leases ----------------------------------------------------------

#[tokio::test]
async fn path_lease_excludes_a_second_holder_until_it_expires() {
    let Some(t) = test_db().await else { return };
    let now = now_ms();

    t.db.acquire_path_lease(&t.fence, "f1", "lease-a", now, 60_000)
        .await
        .unwrap();

    let other =
        t.db.acquire_path_lease(&t.fence, "f1", "lease-b", now + 1_000, 60_000)
            .await;
    assert!(matches!(other, Err(MasterError::Locked)));

    // Same holder renews freely.
    t.db.acquire_path_lease(&t.fence, "f1", "lease-a", now + 2_000, 60_000)
        .await
        .unwrap();

    // After the TTL anyone can take it.
    t.db.acquire_path_lease(&t.fence, "f1", "lease-b", now + 70_000, 60_000)
        .await
        .unwrap();

    t.cleanup().await;
}

#[tokio::test]
async fn releasing_a_path_lease_frees_it() {
    let Some(t) = test_db().await else { return };
    let now = now_ms();
    t.db.acquire_path_lease(&t.fence, "f1", "lease-a", now, 60_000)
        .await
        .unwrap();

    let wrong = t.db.release_path_lease(&t.fence, "f1", "lease-b").await;
    assert!(matches!(wrong, Err(MasterError::Locked)));

    t.db.release_path_lease(&t.fence, "f1", "lease-a")
        .await
        .unwrap();
    t.db.acquire_path_lease(&t.fence, "f1", "lease-b", now, 60_000)
        .await
        .unwrap();

    t.cleanup().await;
}

#[tokio::test]
async fn releasing_a_lease_twice_is_fine() {
    let Some(t) = test_db().await else { return };
    t.db.acquire_path_lease(&t.fence, "f1", "lease-a", now_ms(), 60_000)
        .await
        .unwrap();

    t.db.release_path_lease(&t.fence, "f1", "lease-a")
        .await
        .unwrap();
    // A gateway retrying after a lost reply must not get an error.
    t.db.release_path_lease(&t.fence, "f1", "lease-a")
        .await
        .unwrap();

    t.cleanup().await;
}

#[test]
fn transaction_cancellations_are_classified_by_what_failed() {
    use crate::db::{Cancellation, classify_cancellation};

    // Item 0 is always the fence check.
    assert_eq!(
        classify_cancellation(&[Some("ConditionalCheckFailed"), Some("None")]),
        Cancellation::Fenced
    );
    assert_eq!(
        classify_cancellation(&[Some("None"), Some("None"), Some("ConditionalCheckFailed")]),
        Cancellation::ConditionFailed(1)
    );
    // Only conflicts with other transactions: worth retrying.
    assert_eq!(
        classify_cancellation(&[Some("TransactionConflict"), Some("None")]),
        Cancellation::Conflict
    );
    assert_eq!(
        classify_cancellation(&[Some("None"), Some("TransactionConflict")]),
        Cancellation::Conflict
    );
    // A real condition failure wins over a conflict elsewhere.
    assert_eq!(
        classify_cancellation(&[Some("TransactionConflict"), Some("ConditionalCheckFailed")]),
        Cancellation::ConditionFailed(0)
    );
    assert_eq!(
        classify_cancellation(&[Some("None"), Some("ValidationError")]),
        Cancellation::Other
    );
}

// ----- leader lease and fencing --------------------------------------------

#[tokio::test]
async fn leader_lease_is_exclusive_until_it_expires() {
    let Some(t) = test_db().await else { return };
    let now = now_ms();
    // test_db() already took the lease as "test-master" for 60 s.

    let rival =
        t.db.try_acquire_leader("master-b", "b:50051", now, 10_000)
            .await
            .unwrap();
    assert!(rival.is_none());

    let later =
        t.db.try_acquire_leader("master-b", "b:50051", now + 61_000, 10_000)
            .await
            .unwrap()
            .expect("lease expired, takeover must succeed");
    assert_eq!(later.holder, "master-b");
    assert_eq!(later.fencing, t.fence.fencing + 1);

    t.cleanup().await;
}

#[tokio::test]
async fn renew_only_works_for_the_current_holder() {
    let Some(t) = test_db().await else { return };
    let now = now_ms();

    assert!(
        t.db.renew_leader(&t.fence, now, 10_000).await.unwrap(),
        "holder renews"
    );
    let lease = t.db.get_leader().await.unwrap().unwrap();
    assert_eq!(lease.expires_at, now + 10_000);

    // Pretend the lease ran out and master-b took over.
    t.db.try_acquire_leader("master-b", "b:50051", now + 20_000, 10_000)
        .await
        .unwrap()
        .expect("takeover");

    assert!(
        !t.db
            .renew_leader(&t.fence, now + 21_000, 10_000)
            .await
            .unwrap(),
        "old holder must not renew"
    );

    t.cleanup().await;
}

#[tokio::test]
async fn writes_with_a_stale_fence_are_rejected() {
    let Some(t) = test_db().await else { return };
    let now = now_ms();

    // Another master takes the lease: our fencing number is now old.
    t.db.try_acquire_leader("master-b", "b:50051", now + 61_000, 10_000)
        .await
        .unwrap()
        .expect("takeover");

    let write =
        t.db.put_inode(&t.fence, &dir("root:u1", "docs", "u1"))
            .await;

    assert!(matches!(write, Err(MasterError::Fenced)), "got {write:?}");
    assert!(t.db.get_inode("root:u1", "docs").await.unwrap().is_none());

    t.cleanup().await;
}
