//! End to end through the real gRPC surface: a fake gateway and fake workers
//! talk to an in-process master backed by dynamodb-local. This is the
//! "an upload and a download go through the real master" check of the plan.

use std::net::SocketAddr;
use std::sync::Arc;

use tonic::Code;
use tonic::transport::Channel;

use crate::app::App;
use crate::pb;
use crate::pb::cluster_service_client::ClusterServiceClient;
use crate::pb::master_service_client::MasterServiceClient;
use crate::tests::support::{sha, test_tables};
use crate::time::now_ms;

const BLOCK: u64 = 1_000;

struct Rig {
    app: Arc<App>,
    master: MasterServiceClient<Channel>,
    cluster: ClusterServiceClient<Channel>,
    health: SocketAddr,
}

/// Starts a master on random ports. `lead` decides whether it takes the
/// lease (leader) or stays standby.
async fn start(lead: bool) -> Option<Rig> {
    let db = test_tables().await?;
    let app = Arc::new(App::new(db, "master-test", "127.0.0.1:0", BLOCK, 2));
    if lead {
        app.leadership.tick(now_ms(), 60_000).await.unwrap();
    }

    let grpc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_addr = grpc.local_addr().unwrap();
    tokio::spawn(crate::grpc::serve(app.clone(), grpc));

    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let health = http.local_addr().unwrap();
    tokio::spawn(crate::http::serve(app.clone(), http));

    let channel = Channel::from_shared(format!("http://{grpc_addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    Some(Rig {
        app,
        master: MasterServiceClient::new(channel.clone()),
        cluster: ClusterServiceClient::new(channel),
        health,
    })
}

async fn heartbeat(rig: &mut Rig, id: &str, az: &str) -> pb::HeartbeatResponse {
    rig.cluster
        .heartbeat(pb::HeartbeatRequest {
            worker_id: id.into(),
            addr: format!("{id}:50052"),
            az: az.into(),
            quota_bytes: 1_000_000,
            used_bytes: 0,
            new_blocks: vec![],
            removed_blocks: vec![],
            data_dir: "/data".into(),
        })
        .await
        .unwrap()
        .into_inner()
}

async fn http_status(addr: SocketAddr, path: &str) -> u16 {
    use http_body_util::Empty;
    use hyper_util::rt::TokioIo;
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = hyper::Request::builder()
        .uri(path)
        .header("host", "localhost")
        .body(Empty::<bytes::Bytes>::new())
        .unwrap();
    sender.send_request(req).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn standby_refuses_every_call_and_fails_its_leader_health_check() {
    let Some(mut rig) = start(false).await else {
        return;
    };

    let err = rig
        .master
        .register(pb::RegisterRequest {
            username: "jero".into(),
            password: "password123".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unavailable);

    assert_eq!(http_status(rig.health, "/health").await, 200);
    assert_eq!(http_status(rig.health, "/health/leader").await, 503);

    rig.app.db.delete_tables().await.unwrap();
}

#[tokio::test]
async fn leader_answers_its_health_check() {
    let Some(rig) = start(true).await else { return };

    assert_eq!(http_status(rig.health, "/health").await, 200);
    assert_eq!(http_status(rig.health, "/health/leader").await, 200);

    rig.app.db.delete_tables().await.unwrap();
}

#[tokio::test]
async fn full_upload_and_download_through_the_master() {
    let Some(mut rig) = start(true).await else {
        return;
    };
    let fencing = rig.app.leadership.fencing();

    // Four workers show up. The first heartbeat of each asks for a report.
    for (id, az) in [("a1", "a"), ("a2", "a"), ("b1", "b"), ("b2", "b")] {
        let answer = heartbeat(&mut rig, id, az).await;
        assert_eq!(answer.fencing, fencing);
        assert!(answer.full_report_requested);
        rig.cluster
            .block_report(pb::BlockReportRequest {
                worker_id: id.into(),
                blocks: vec![],
            })
            .await
            .unwrap();
    }
    assert!(!heartbeat(&mut rig, "a1", "a").await.full_report_requested);

    // Users.
    let user_id = rig
        .master
        .register(pb::RegisterRequest {
            username: "jero".into(),
            password: "password123".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .user_id;
    let dup = rig
        .master
        .register(pb::RegisterRequest {
            username: "jero".into(),
            password: "otherpass1".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(dup.code(), Code::AlreadyExists);
    let login = rig
        .master
        .login(pb::LoginRequest {
            username: "jero".into(),
            password: "password123".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(login.user_id, user_id);
    let bad = rig
        .master
        .login(pb::LoginRequest {
            username: "jero".into(),
            password: "wrongpass1".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code(), Code::Unauthenticated);

    // Namespace.
    let path = |p: &str| pb::PathRequest {
        user_id: user_id.clone(),
        path: p.into(),
    };
    rig.master.mkdir(path("/docs")).await.unwrap();
    let listed = rig
        .master
        .list(path("/"))
        .await
        .unwrap()
        .into_inner()
        .entries;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "docs");
    assert_eq!(listed[0].r#type, pb::EntryType::Dir as i32);

    // Upload.
    let content = vec![42u8; 2_500];
    let plan = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user_id.clone(),
            path: "/docs/a.bin".into(),
            size: 2_500,
            lease_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(plan.block_size, BLOCK);
    assert_eq!(plan.blocks.len(), 3);
    for b in &plan.blocks {
        assert_eq!(b.workers.len(), 2);
        assert_ne!(b.workers[0].az, b.workers[1].az);
        assert!(b.workers[0].addr.ends_with(":50052"));
    }
    // The file is not there yet.
    assert_eq!(
        rig.master
            .list(path("/docs"))
            .await
            .unwrap()
            .into_inner()
            .entries
            .len(),
        0
    );

    let acks: Vec<pb::CommittedBlock> = plan
        .blocks
        .iter()
        .map(|b| {
            let start = b.idx as usize * BLOCK as usize;
            pb::CommittedBlock {
                idx: b.idx,
                block_id: b.block_id.clone(),
                size: b.size,
                sha256: sha(&content[start..start + b.size as usize]),
                worker_ids: b.workers.iter().map(|w| w.worker_id.clone()).collect(),
            }
        })
        .collect();
    let info = rig
        .master
        .commit_upload(pb::CommitUploadRequest {
            user_id: user_id.clone(),
            path: "/docs/a.bin".into(),
            inode_id: plan.inode_id.clone(),
            version_no: plan.version_no,
            lease_id: plan.lease_id.clone(),
            size: 2_500,
            sha256: sha(&content),
            blocks: acks,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.version_no, 1);
    assert_eq!(info.block_count, 3);

    // Download plan.
    let read = rig
        .master
        .get_read_plan(pb::ReadPlanRequest {
            user_id: user_id.clone(),
            path: "/docs/a.bin".into(),
            version_no: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(read.size, 2_500);
    assert_eq!(read.sha256, sha(&content));
    assert_eq!(read.block_size, BLOCK);
    assert_eq!(read.blocks.len(), 3);
    assert!(read.blocks.iter().all(|b| b.replicas.len() == 2));
    assert_eq!(read.blocks[2].sha256, sha(&content[2_000..]));

    // Stat and list show the committed file.
    let stat = rig
        .master
        .stat(path("/docs/a.bin"))
        .await
        .unwrap()
        .into_inner();
    let entry = stat.entry.unwrap();
    assert_eq!(entry.size, 2_500);
    assert_eq!(entry.r#type, pb::EntryType::File as i32);
    assert_eq!(stat.version_no, 1);
    assert_eq!(stat.blocks.len(), 3);
    assert_eq!(stat.blocks[0].replicas.len(), 2);
    assert!(
        stat.blocks[0].replicas[0]
            .disk_path
            .starts_with("/data/blocks/")
    );
    let listed = rig
        .master
        .list(path("/docs"))
        .await
        .unwrap()
        .into_inner()
        .entries;
    assert_eq!(listed[0].name, "a.bin");
    assert_eq!(listed[0].size, 2_500);

    // A corrupt replica is dropped from the plan.
    rig.master
        .report_corruption(pb::CorruptionReport {
            block_id: read.blocks[0].block_id.clone(),
            worker_id: read.blocks[0].replicas[0].worker_id.clone(),
        })
        .await
        .unwrap();
    let read2 = rig
        .master
        .get_read_plan(pb::ReadPlanRequest {
            user_id: user_id.clone(),
            path: "/docs/a.bin".into(),
            version_no: 0,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(read2.blocks[0].replicas.len(), 1);

    // Dashboard numbers.
    let status = rig
        .master
        .cluster_status(pb::Empty {})
        .await
        .unwrap()
        .into_inner();
    assert!(status.is_leader);
    assert_eq!(status.fencing, fencing);
    assert_eq!(status.workers.len(), 4);
    assert!(
        status
            .workers
            .iter()
            .all(|w| w.state == pb::WorkerState::Alive as i32)
    );
    assert_eq!(status.total_blocks, 3);
    assert_eq!(status.under_replicated_blocks, 1);

    // Move, then remove.
    rig.master
        .r#move(pb::MoveRequest {
            user_id: user_id.clone(),
            from: "/docs/a.bin".into(),
            to: "/a.bin".into(),
        })
        .await
        .unwrap();
    rig.master
        .remove(pb::RemoveRequest {
            user_id: user_id.clone(),
            path: "/docs".into(),
            recursive: false,
        })
        .await
        .unwrap();
    rig.master
        .remove(pb::RemoveRequest {
            user_id: user_id.clone(),
            path: "/a.bin".into(),
            recursive: false,
        })
        .await
        .unwrap();
    let gone = rig
        .master
        .get_read_plan(pb::ReadPlanRequest {
            user_id: user_id.clone(),
            path: "/a.bin".into(),
            version_no: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(gone.code(), Code::NotFound);

    rig.app.db.delete_tables().await.unwrap();
}

#[tokio::test]
async fn another_user_gets_not_found_and_bad_input_gets_invalid_argument() {
    let Some(mut rig) = start(true).await else {
        return;
    };
    let register = |u: &str| pb::RegisterRequest {
        username: u.into(),
        password: "password123".into(),
    };
    let a = rig
        .master
        .register(register("ana"))
        .await
        .unwrap()
        .into_inner()
        .user_id;
    let b = rig
        .master
        .register(register("beto"))
        .await
        .unwrap()
        .into_inner()
        .user_id;
    rig.master
        .mkdir(pb::PathRequest {
            user_id: a.clone(),
            path: "/private".into(),
        })
        .await
        .unwrap();

    let err = rig
        .master
        .list(pb::PathRequest {
            user_id: b.clone(),
            path: "/private".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::NotFound);

    let err = rig
        .master
        .mkdir(pb::PathRequest {
            user_id: a.clone(),
            path: "relative".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    let err = rig
        .master
        .register(pb::RegisterRequest {
            username: "short".into(),
            password: "1234567".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);

    // rmdir on a non-empty directory is a precondition failure (409).
    rig.master
        .mkdir(pb::PathRequest {
            user_id: a.clone(),
            path: "/private/inner".into(),
        })
        .await
        .unwrap();
    let err = rig
        .master
        .remove(pb::RemoveRequest {
            user_id: a.clone(),
            path: "/private".into(),
            recursive: false,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);

    rig.app.db.delete_tables().await.unwrap();
}

#[tokio::test]
async fn explicit_lease_blocks_other_writers_and_can_be_reused_by_the_upload() {
    let Some(mut rig) = start(true).await else {
        return;
    };
    for (id, az) in [("a1", "a"), ("b1", "b")] {
        heartbeat(&mut rig, id, az).await;
    }
    let user = rig
        .master
        .register(pb::RegisterRequest {
            username: "jero".into(),
            password: "password123".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .user_id;

    let lease = rig
        .master
        .acquire_lease(pb::LeaseRequest {
            user_id: user.clone(),
            path: "/locked.bin".into(),
            ttl_seconds: 0,
            lease_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!lease.lease_id.is_empty());
    assert!(lease.expires_at > now_ms() as i64);

    // Somebody else cannot upload there while the lock is held...
    let err = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user.clone(),
            path: "/locked.bin".into(),
            size: 10,
            lease_id: String::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::FailedPrecondition);

    // ...but the lock holder can, passing its lease.
    let plan = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user.clone(),
            path: "/locked.bin".into(),
            size: 10,
            lease_id: lease.lease_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(plan.lease_id, lease.lease_id);

    rig.master
        .abort_upload(pb::AbortUploadRequest {
            user_id: user.clone(),
            path: "/locked.bin".into(),
            inode_id: plan.inode_id,
            version_no: plan.version_no,
            lease_id: plan.lease_id,
        })
        .await
        .unwrap();

    rig.app.db.delete_tables().await.unwrap();
}

#[tokio::test]
async fn uploading_over_a_file_makes_a_numbered_copy_unless_the_path_is_locked() {
    let Some(mut rig) = start(true).await else {
        return;
    };
    for (id, az) in [("a1", "a"), ("b1", "b")] {
        heartbeat(&mut rig, id, az).await;
    }
    let user = rig
        .master
        .register(pb::RegisterRequest {
            username: "jero".into(),
            password: "password123".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .user_id;

    // First upload of /a.bin: the path is free, so it keeps its name.
    let content = vec![7u8; 10];
    let first = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user.clone(),
            path: "/a.bin".into(),
            size: 10,
            lease_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.path, "/a.bin");
    let acks: Vec<pb::CommittedBlock> = first
        .blocks
        .iter()
        .map(|b| pb::CommittedBlock {
            idx: b.idx,
            block_id: b.block_id.clone(),
            size: b.size,
            sha256: sha(&content),
            worker_ids: b.workers.iter().map(|w| w.worker_id.clone()).collect(),
        })
        .collect();
    rig.master
        .commit_upload(pb::CommitUploadRequest {
            user_id: user.clone(),
            path: first.path.clone(),
            inode_id: first.inode_id.clone(),
            version_no: first.version_no,
            lease_id: first.lease_id.clone(),
            size: 10,
            sha256: sha(&content),
            blocks: acks,
        })
        .await
        .unwrap();

    // Same path again: a different file, the committed one is not touched.
    let second = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user.clone(),
            path: "/a.bin".into(),
            size: 10,
            lease_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(second.path, "/a (1).bin");
    assert_ne!(second.inode_id, first.inode_id);
    assert_eq!(second.version_no, 1);

    // Whoever locked the path asked for that exact path: that is a replacement.
    let lease = rig
        .master
        .acquire_lease(pb::LeaseRequest {
            user_id: user.clone(),
            path: "/a.bin".into(),
            ttl_seconds: 0,
            lease_id: String::new(),
        })
        .await
        .unwrap()
        .into_inner();
    let replacement = rig
        .master
        .start_upload(pb::StartUploadRequest {
            user_id: user.clone(),
            path: "/a.bin".into(),
            size: 10,
            lease_id: lease.lease_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(replacement.path, "/a.bin");
    assert_eq!(replacement.inode_id, first.inode_id);
    assert_eq!(replacement.version_no, 2);

    rig.app.db.delete_tables().await.unwrap();
}
