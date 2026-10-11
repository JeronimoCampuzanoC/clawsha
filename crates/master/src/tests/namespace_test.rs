use crate::db::inodes::{Inode, InodeType, root_id};
use crate::error::MasterError;
use crate::namespace::Namespace;
use crate::tests::support::{TestDb, test_db};
use crate::time::now_ms;

const U1: &str = "user-1";
const U2: &str = "user-2";

/// A file that already has a committed version, created straight in the
/// tables so these tests do not depend on the upload flow.
async fn put_file(t: &TestDb, parent_id: &str, name: &str, size: u64) -> Inode {
    let inode = Inode {
        parent_id: parent_id.to_string(),
        name: name.to_string(),
        inode_id: uuid::Uuid::new_v4().to_string(),
        owner_id: U1.to_string(),
        kind: InodeType::File,
        current_version: 1,
        size,
        modified_at: now_ms(),
    };
    t.db.put_inode(&t.fence, &inode).await.unwrap();
    t.db
        .put_pending_version(&t.fence, &inode.inode_id, 1, size, now_ms())
        .await
        .unwrap();
    inode
}

#[tokio::test]
async fn new_user_root_is_an_empty_directory() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());

    let root = ns.stat(U1, "/").await.unwrap();
    assert_eq!(root.kind, InodeType::Dir);
    assert_eq!(root.inode_id, root_id(U1));
    assert!(ns.list(U1, "/").await.unwrap().is_empty());

    t.cleanup().await;
}

#[tokio::test]
async fn mkdir_then_list_and_stat() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());

    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/docs/2026", now_ms()).await.unwrap();

    let names: Vec<String> = ns.list(U1, "/").await.unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["docs"]);
    let inner = ns.list(U1, "/docs").await.unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].name, "2026");
    assert_eq!(ns.stat(U1, "/docs/2026").await.unwrap().kind, InodeType::Dir);

    t.cleanup().await;
}

#[tokio::test]
async fn mkdir_needs_an_existing_parent_directory() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());

    let r = ns.mkdir(&t.fence, U1, "/missing/child", now_ms()).await;
    assert!(matches!(r, Err(MasterError::NotFound)));

    put_file(&t, &root_id(U1), "file.txt", 3).await;
    let r = ns.mkdir(&t.fence, U1, "/file.txt/child", now_ms()).await;
    assert!(matches!(r, Err(MasterError::NotFound)), "a file is not a directory");

    t.cleanup().await;
}

#[tokio::test]
async fn mkdir_twice_is_already_exists_and_root_is_invalid() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());

    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();
    assert!(matches!(
        ns.mkdir(&t.fence, U1, "/docs", now_ms()).await,
        Err(MasterError::AlreadyExists)
    ));
    assert!(matches!(
        ns.mkdir(&t.fence, U1, "/", now_ms()).await,
        Err(MasterError::InvalidPath(_))
    ));

    t.cleanup().await;
}

#[tokio::test]
async fn users_do_not_see_each_other() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();

    assert!(matches!(ns.stat(U2, "/docs").await, Err(MasterError::NotFound)));
    assert!(matches!(ns.list(U2, "/docs").await, Err(MasterError::NotFound)));
    assert!(ns.list(U2, "/").await.unwrap().is_empty());

    t.cleanup().await;
}

#[tokio::test]
async fn files_without_a_committed_version_are_hidden() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    let mut uploading = put_file(&t, &root_id(U1), "half.bin", 10).await;
    // Simulate a first upload still running: no committed version yet.
    t.db.delete_inode(&t.fence, &root_id(U1), "half.bin", &uploading.inode_id).await.unwrap();
    uploading.current_version = 0;
    t.db.put_inode(&t.fence, &uploading).await.unwrap();
    put_file(&t, &root_id(U1), "done.bin", 10).await;

    let names: Vec<String> = ns.list(U1, "/").await.unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, vec!["done.bin"]);
    assert!(matches!(ns.stat(U1, "/half.bin").await, Err(MasterError::NotFound)));

    t.cleanup().await;
}

#[tokio::test]
async fn remove_file_retires_its_version() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    let file = put_file(&t, &root_id(U1), "a.txt", 3).await;

    ns.remove(&t.fence, U1, "/a.txt", false, now_ms()).await.unwrap();

    assert!(matches!(ns.stat(U1, "/a.txt").await, Err(MasterError::NotFound)));
    let v = t.db.get_version(&file.inode_id, 1).await.unwrap().unwrap();
    assert_eq!(v.status, crate::db::versions::VersionStatus::Old);

    t.cleanup().await;
}

#[tokio::test]
async fn rmdir_refuses_a_non_empty_directory_unless_recursive() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/docs/inner", now_ms()).await.unwrap();
    let docs = ns.stat(U1, "/docs").await.unwrap();
    put_file(&t, &docs.inode_id, "a.txt", 3).await;

    assert!(matches!(
        ns.remove(&t.fence, U1, "/docs", false, now_ms()).await,
        Err(MasterError::DirNotEmpty)
    ));
    assert!(ns.stat(U1, "/docs/a.txt").await.is_ok(), "nothing was deleted");

    ns.remove(&t.fence, U1, "/docs", true, now_ms()).await.unwrap();
    assert!(matches!(ns.stat(U1, "/docs").await, Err(MasterError::NotFound)));
    assert!(t.db.list_children(&docs.inode_id).await.unwrap().is_empty());

    t.cleanup().await;
}

#[tokio::test]
async fn removing_root_or_a_missing_path_fails() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());

    assert!(matches!(
        ns.remove(&t.fence, U1, "/", true, now_ms()).await,
        Err(MasterError::InvalidPath(_))
    ));
    assert!(matches!(
        ns.remove(&t.fence, U1, "/nope", false, now_ms()).await,
        Err(MasterError::NotFound)
    ));

    t.cleanup().await;
}

#[tokio::test]
async fn move_renames_and_relocates() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/archive", now_ms()).await.unwrap();
    let file = put_file(&t, &ns.stat(U1, "/docs").await.unwrap().inode_id, "a.txt", 3).await;

    ns.mv(&t.fence, U1, "/docs/a.txt", "/docs/b.txt", now_ms()).await.unwrap();
    ns.mv(&t.fence, U1, "/docs/b.txt", "/archive/b.txt", now_ms()).await.unwrap();

    assert!(matches!(ns.stat(U1, "/docs/a.txt").await, Err(MasterError::NotFound)));
    let moved = ns.stat(U1, "/archive/b.txt").await.unwrap();
    assert_eq!(moved.inode_id, file.inode_id);
    assert_eq!(moved.size, 3);

    t.cleanup().await;
}

#[tokio::test]
async fn moving_a_directory_keeps_its_children() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    ns.mkdir(&t.fence, U1, "/docs", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/docs/inner", now_ms()).await.unwrap();

    ns.mv(&t.fence, U1, "/docs", "/papers", now_ms()).await.unwrap();

    assert_eq!(ns.stat(U1, "/papers/inner").await.unwrap().kind, InodeType::Dir);

    t.cleanup().await;
}

#[tokio::test]
async fn move_refuses_bad_destinations() {
    let Some(t) = test_db().await else { return };
    let ns = Namespace::new(t.db.clone());
    ns.mkdir(&t.fence, U1, "/a", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/a/b", now_ms()).await.unwrap();
    ns.mkdir(&t.fence, U1, "/c", now_ms()).await.unwrap();

    // Onto an existing name.
    assert!(matches!(
        ns.mv(&t.fence, U1, "/a", "/c", now_ms()).await,
        Err(MasterError::AlreadyExists)
    ));
    // Into a parent that does not exist.
    assert!(matches!(
        ns.mv(&t.fence, U1, "/a", "/nope/a", now_ms()).await,
        Err(MasterError::NotFound)
    ));
    // Into its own subtree.
    assert!(matches!(
        ns.mv(&t.fence, U1, "/a", "/a/b/a", now_ms()).await,
        Err(MasterError::InvalidArgument(_))
    ));
    // The root itself.
    assert!(matches!(
        ns.mv(&t.fence, U1, "/", "/x", now_ms()).await,
        Err(MasterError::InvalidPath(_))
    ));
    // Something that is not there.
    assert!(matches!(
        ns.mv(&t.fence, U1, "/zzz", "/x", now_ms()).await,
        Err(MasterError::NotFound)
    ));

    t.cleanup().await;
}
